//! Operations: the only way documents change. The server applies ops in arrival
//! order and rebroadcasts them; clients apply optimistically and use the echo as
//! an acknowledgement. Conflicts resolve last-writer-wins per property.

use crate::doc::Document;
use crate::node::{Node, NodeId, NodeType, ROOT_ID};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    /// Insert a new node. Its `parent` must exist.
    Create { node: Node },
    /// Set one or more properties of a node.
    Update { id: NodeId, props: Map<String, Value> },
    /// Delete a node and its whole subtree.
    Delete { id: NodeId },
}

impl Op {
    pub fn update(id: impl Into<NodeId>, props: Map<String, Value>) -> Op {
        Op::Update { id: id.into(), props }
    }

    pub fn update1(id: impl Into<NodeId>, key: &str, value: Value) -> Op {
        let mut props = Map::new();
        props.insert(key.to_string(), value);
        Op::Update { id: id.into(), props }
    }

    pub fn target(&self) -> &str {
        match self {
            Op::Create { node } => &node.id,
            Op::Update { id, .. } | Op::Delete { id } => id,
        }
    }
}

/// What an applied op changed, so views can invalidate precisely.
#[derive(Clone, Debug, Default)]
pub struct OpEffect {
    /// Tree shape changed (create/delete/reparent/reorder).
    pub structure: bool,
    /// Nodes whose own properties changed.
    pub changed: Vec<NodeId>,
    /// Nodes removed from the document.
    pub removed: Vec<NodeId>,
}

impl OpEffect {
    pub fn merge(&mut self, o: OpEffect) {
        self.structure |= o.structure;
        self.changed.extend(o.changed);
        self.removed.extend(o.removed);
    }
}

fn check_parent(doc: &Document, node: &Node) -> Result<(), String> {
    let parent_id = node.parent.as_deref().ok_or("node needs a parent")?;
    let parent = doc.get(parent_id).ok_or_else(|| format!("parent '{parent_id}' does not exist"))?;
    match node.kind {
        NodeType::Document => Err("cannot create another document root".into()),
        NodeType::Page => {
            if parent_id == ROOT_ID {
                Ok(())
            } else {
                Err("pages must be children of the document root".into())
            }
        }
        NodeType::Style => {
            if parent_id == ROOT_ID {
                Ok(())
            } else {
                Err("styles must be children of the document root".into())
            }
        }
        _ => {
            if parent.kind == NodeType::Document {
                Err("canvas nodes must live inside a page, frame or group".into())
            } else if parent.kind == NodeType::Instance || crate::node::is_derived_id(parent_id) {
                Err("instances can't contain other layers; edit the main component instead".into())
            } else if !parent.kind.is_container() {
                Err(format!("parent '{parent_id}' ({}) cannot have children", parent.kind.as_str()))
            } else {
                Ok(())
            }
        }
    }
}

impl Document {
    /// Validate and apply one op.
    pub fn apply(&mut self, op: &Op) -> Result<OpEffect, String> {
        match op {
            Op::Create { node } => {
                if node.id.is_empty() || node.id.len() > 128 || crate::node::is_derived_id(&node.id) {
                    return Err("invalid node id".into());
                }
                if self.contains(&node.id) {
                    return Err(format!("node '{}' already exists", node.id));
                }
                check_parent(self, node)?;
                let mut node = node.clone();
                if !crate::fractional::is_valid_key(&node.index) {
                    node.index = self.index_at_end(node.parent.as_deref().unwrap_or(ROOT_ID));
                }
                let id = node.id.clone();
                let parent = node.parent.clone().unwrap_or_default();
                self.insert_unchecked(node);
                self.mark_fresh(&id);
                Ok(OpEffect { structure: true, changed: vec![id, parent], removed: vec![] })
            }
            Op::Update { id, props } => {
                if crate::node::is_derived_id(id) {
                    return Err("layers inside an instance are changed through the instance's overrides".into());
                }
                let current = self.get(id).ok_or_else(|| format!("node '{id}' does not exist"))?;
                if id == ROOT_ID && (props.contains_key("parent") || props.contains_key("index")) {
                    return Err("cannot move the document root".into());
                }
                let mut next = current.clone();
                for (k, v) in props {
                    next.set_prop(k, v)?;
                }
                let structure = next.parent != current.parent || next.index != current.index;
                if next.parent != current.parent {
                    check_parent(self, &next)?;
                    let new_parent = next.parent.as_deref().unwrap_or("");
                    if new_parent == id || self.is_ancestor(id, new_parent) {
                        return Err("cannot move a node into itself".into());
                    }
                }
                if structure && !crate::fractional::is_valid_key(&next.index) {
                    next.index = self.index_at_end(next.parent.as_deref().unwrap_or(ROOT_ID));
                }
                // Setting values a node already has changes nothing: no re-sync,
                // relayout or save (a measurement that agrees, sent again).
                if next == *current {
                    return Ok(OpEffect::default());
                }
                let mut changed = vec![id.clone()];
                if structure {
                    changed.extend(current.parent.clone());
                    changed.extend(next.parent.clone());
                }
                self.replace_unchecked(next);
                Ok(OpEffect { structure, changed, removed: vec![] })
            }
            Op::Delete { id } => {
                if crate::node::is_derived_id(id) {
                    return Err("layers inside an instance can't be deleted; hide them instead".into());
                }
                let node = self.get(id).ok_or_else(|| format!("node '{id}' does not exist"))?;
                if id == ROOT_ID {
                    return Err("cannot delete the document root".into());
                }
                if node.kind == NodeType::Page && self.pages().len() <= 1 {
                    return Err("cannot delete the last page".into());
                }
                let parent = node.parent.clone();
                let removed = self.remove_unchecked(id).into_iter().map(|n| n.id).collect();
                Ok(OpEffect { structure: true, changed: parent.into_iter().collect(), removed })
            }
        }
    }

    /// Ops that undo `op` if applied right now (call *before* applying `op`).
    pub fn inverse_of(&self, op: &Op) -> Vec<Op> {
        match op {
            Op::Create { node } => vec![Op::Delete { id: node.id.clone() }],
            Op::Update { id, props } => match self.get(id) {
                Some(n) => {
                    let mut old = Map::new();
                    for k in props.keys() {
                        if let Some(v) = n.get_prop(k) {
                            old.insert(k.clone(), v);
                        }
                    }
                    vec![Op::Update { id: id.clone(), props: old }]
                }
                None => vec![],
            },
            Op::Delete { id } => {
                let mut out = Vec::new();
                self.walk(id, &mut |n| out.push(Op::Create { node: n.clone() }));
                out
            }
        }
    }

    /// Ops that bring a client that optimistically applied a rejected `op` back
    /// in line with this (authoritative) document.
    pub fn correction_for(&self, op: &Op) -> Vec<Op> {
        match op {
            Op::Create { node } => match self.get(&node.id) {
                // Create was rejected because the id exists: resend our version.
                Some(existing) => {
                    let mut out = Vec::new();
                    let _ = existing;
                    self.walk(&node.id, &mut |n| out.push(Op::Create { node: n.clone() }));
                    out
                }
                None => vec![Op::Delete { id: node.id.clone() }],
            },
            Op::Update { id, props } => match self.get(id) {
                Some(n) => {
                    let mut cur = Map::new();
                    for k in props.keys() {
                        if let Some(v) = n.get_prop(k) {
                            cur.insert(k.clone(), v);
                        }
                    }
                    vec![Op::Update { id: id.clone(), props: cur }]
                }
                None => vec![Op::Delete { id: id.clone() }],
            },
            Op::Delete { id } => {
                let mut out = Vec::new();
                self.walk(id, &mut |n| out.push(Op::Create { node: n.clone() }));
                out
            }
        }
    }

    /// Apply a batch, collecting effects; stops at nothing — invalid ops are
    /// skipped and reported.
    pub fn apply_all(&mut self, ops: &[Op]) -> (OpEffect, Vec<(usize, String)>) {
        let mut eff = OpEffect::default();
        let mut errors = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            match self.apply(op) {
                Ok(e) => eff.merge(e),
                Err(err) => errors.push((i, err)),
            }
        }
        (eff, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rect(id: &str, parent: &str) -> Node {
        let mut n = Node::new(id, NodeType::Rectangle);
        n.parent = Some(parent.into());
        n.width = 10.0;
        n.height = 10.0;
        n
    }

    #[test]
    fn create_update_delete_with_undo() {
        let mut doc = Document::new_empty();
        let op = Op::Create { node: rect("a:1", "0:1") };
        let inv = doc.inverse_of(&op);
        doc.apply(&op).unwrap();
        assert_eq!(doc.children("0:1").len(), 1);

        let up = Op::update1("a:1", "x", json!(50));
        let inv_up = doc.inverse_of(&up);
        doc.apply(&up).unwrap();
        assert_eq!(doc.get("a:1").unwrap().x, 50.0);
        doc.apply_all(&inv_up);
        assert_eq!(doc.get("a:1").unwrap().x, 0.0);

        doc.apply_all(&inv);
        assert!(doc.get("a:1").is_none());
        doc.check_consistency().unwrap();
    }

    #[test]
    fn reparent_cycle_rejected() {
        let mut doc = Document::new_empty();
        let mut f1 = Node::new("f:1", NodeType::Frame);
        f1.parent = Some("0:1".into());
        let mut f2 = Node::new("f:2", NodeType::Frame);
        f2.parent = Some("f:1".into());
        doc.apply(&Op::Create { node: f1 }).unwrap();
        doc.apply(&Op::Create { node: f2 }).unwrap();
        let bad = Op::update1("f:1", "parent", json!("f:2"));
        assert!(doc.apply(&bad).is_err());
        let fix = doc.correction_for(&bad);
        assert_eq!(fix.len(), 1);
    }

    #[test]
    fn delete_subtree_and_restore() {
        let mut doc = Document::new_empty();
        let mut f1 = Node::new("f:1", NodeType::Frame);
        f1.parent = Some("0:1".into());
        doc.apply(&Op::Create { node: f1 }).unwrap();
        doc.apply(&Op::Create { node: rect("r:1", "f:1") }).unwrap();
        doc.apply(&Op::Create { node: rect("r:2", "f:1") }).unwrap();
        let del = Op::Delete { id: "f:1".into() };
        let inv = doc.inverse_of(&del);
        let eff = doc.apply(&del).unwrap();
        assert_eq!(eff.removed.len(), 3);
        doc.apply_all(&inv);
        assert_eq!(doc.children("f:1").len(), 2);
        doc.check_consistency().unwrap();
    }

    #[test]
    fn reorder_by_index() {
        let mut doc = Document::new_empty();
        doc.apply(&Op::Create { node: rect("r:1", "0:1") }).unwrap();
        let mut r2 = rect("r:2", "0:1");
        r2.index = doc.index_at_end("0:1");
        doc.apply(&Op::Create { node: r2 }).unwrap();
        assert_eq!(doc.children("0:1"), &["r:1".to_string(), "r:2".to_string()]);
        let idx = doc.index_at("0:1", 0, &[]);
        doc.apply(&Op::update1("r:2", "index", json!(idx))).unwrap();
        assert_eq!(doc.children("0:1"), &["r:2".to_string(), "r:1".to_string()]);
        doc.check_consistency().unwrap();
    }

    #[test]
    fn snapshot_roundtrip() {
        let mut doc = Document::new_empty();
        doc.apply(&Op::Create { node: rect("r:1", "0:1") }).unwrap();
        let json = doc.to_json();
        let doc2 = Document::from_json(&json).unwrap();
        assert_eq!(doc2.len(), doc.len());
        assert_eq!(doc2.get("r:1"), doc.get("r:1"));
    }
}
