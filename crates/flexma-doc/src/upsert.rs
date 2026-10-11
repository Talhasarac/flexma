//! Upserts: a node spec with a `key` updates the node an earlier upload created
//! from it, in place, instead of adding a copy. Re-exports from Unity or other
//! tools keep node ids, so comments, links and instances survive.
//!
//! The specs are first built as if they were new, then matched against the
//! existing nodes (children by `key`, or else by name and type), and only the
//! differences become ops.

use crate::friendly::build_create_ops;
use crate::node::{is_derived_id, Node, NodeId, NodeType, ROOT_ID};
use crate::ops::Op;
use crate::paint::{LayoutMode, SizingMode, TextAutoResize};
use crate::{fractional, Document};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct Upsert {
    pub ops: Vec<Op>,
    /// The resulting top-level nodes, in spec order.
    pub top: Vec<NodeId>,
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
    /// Top-level specs that matched an existing node.
    pub matched: usize,
}

/// The scene node with this key (outside instances), if any.
pub fn find_by_key(doc: &Document, key: &str) -> Option<NodeId> {
    let mut found = None;
    doc.walk(ROOT_ID, &mut |n| {
        if found.is_none() && n.key.as_deref() == Some(key) && n.kind.is_scene() && !is_derived_id(&n.id) {
            found = Some(n.id.clone());
        }
    });
    found
}

/// Props that change the measured size of a text box.
const TEXT_LAYOUT: &[&str] = &[
    "characters",
    "fontFamily",
    "fontSize",
    "fontWeight",
    "italic",
    "letterSpacing",
    "lineHeight",
    "textAutoResize",
    "textWrap",
    "textOverflow",
];

/// Like `friendly::build_create_ops`, but top-level specs whose `key` matches
/// an existing node update that node and its subtree instead.
pub fn build_upsert_ops(
    doc: &Document,
    parent: &str,
    specs: &[Value],
    insert_at: Option<usize>,
    new_id: &mut dyn FnMut() -> NodeId,
) -> Result<Upsert, String> {
    let (create_ops, fresh_top) = build_create_ops(doc, parent, specs, insert_at, new_id)?;
    let mut fresh = Fresh::default();
    for op in create_ops {
        if let Op::Create { node } = op {
            if let Some(p) = &node.parent {
                fresh.kids.entry(p.clone()).or_default().push(node.id.clone());
            }
            fresh.order.push(node.id.clone());
            fresh.nodes.insert(node.id.clone(), node);
        }
    }
    for list in fresh.kids.values_mut() {
        list.sort_by(|a, b| fresh.nodes[a].index.cmp(&fresh.nodes[b].index).then(a.cmp(b)));
    }

    let mut w = Work { doc, fresh: &fresh, deletes: Vec::new(), creates: HashSet::new(), parent_of: HashMap::new(), updates: Vec::new(), out: Upsert::default() };
    for (spec, t) in specs.iter().zip(&fresh_top) {
        let tn = &fresh.nodes[t];
        let existing = tn.key.as_deref().and_then(|k| find_by_key(doc, k)).and_then(|id| doc.get(&id));
        match existing {
            Some(e) if e.kind == tn.kind => {
                w.out.matched += 1;
                let mut skip: HashSet<&str> = HashSet::new();
                // Keep the node where it is unless the spec places it.
                let has_pos = spec.get("x").is_some() || spec.get("y").is_some() || spec.get("x1").is_some();
                if !has_pos {
                    skip.extend(["x", "y"]);
                }
                if e.parent.as_deref() == Some(parent) {
                    skip.insert("index");
                } else {
                    // Found elsewhere (another page or frame): move it here.
                    let mut props = Map::new();
                    props.insert("parent".into(), Value::String(parent.to_string()));
                    props.insert("index".into(), Value::String(tn.index.clone()));
                    w.updates.push(Op::Update { id: e.id.clone(), props });
                    skip.insert("index");
                }
                w.reconcile(e, t, &skip);
                w.out.top.push(e.id.clone());
            }
            Some(e) => {
                // Same key, different kind: replace it.
                w.delete(&e.id);
                w.create(t, None);
                w.out.top.push(t.clone());
            }
            None => {
                w.create(t, None);
                w.out.top.push(t.clone());
            }
        }
    }

    // Deletes first, then creates (parents before children), then updates.
    let Work { deletes, creates, parent_of, updates, mut out, .. } = w;
    for id in deletes {
        out.ops.push(Op::Delete { id });
    }
    for id in &fresh.order {
        if creates.contains(id) {
            let mut node = fresh.nodes[id].clone();
            if let Some((p, index)) = parent_of.get(id) {
                node.parent = Some(p.clone());
                if let Some(index) = index {
                    node.index = index.clone();
                }
            }
            out.ops.push(Op::Create { node });
        }
    }
    out.ops.extend(updates);
    Ok(out)
}

#[derive(Default)]
struct Fresh {
    nodes: HashMap<NodeId, Node>,
    kids: HashMap<NodeId, Vec<NodeId>>,
    /// Creation order (parents before children).
    order: Vec<NodeId>,
}

struct Work<'a> {
    doc: &'a Document,
    fresh: &'a Fresh,
    deletes: Vec<NodeId>,
    /// Fresh nodes to create.
    creates: HashSet<NodeId>,
    /// New parent (and index) for fresh nodes placed under existing ones.
    parent_of: HashMap<NodeId, (NodeId, Option<String>)>,
    updates: Vec<Op>,
    out: Upsert,
}

impl Work<'_> {
    fn delete(&mut self, id: &str) {
        let mut n = 0;
        self.doc.walk(id, &mut |c| {
            if !is_derived_id(&c.id) {
                n += 1;
            }
        });
        self.out.deleted += n;
        self.deletes.push(id.to_string());
    }

    /// Create the fresh subtree at `t`, optionally under an existing parent.
    fn create(&mut self, t: &str, under: Option<(NodeId, Option<String>)>) {
        if let Some(u) = under {
            self.parent_of.insert(t.to_string(), u);
        }
        let mut stack = vec![t.to_string()];
        while let Some(id) = stack.pop() {
            self.out.created += 1;
            if let Some(k) = self.fresh.kids.get(&id) {
                stack.extend(k.iter().cloned());
            }
            self.creates.insert(id);
        }
    }

    /// Make existing node `e` match fresh node `t`, children included.
    fn reconcile(&mut self, e: &Node, t: &str, skip: &HashSet<&str>) {
        let tn = &self.fresh.nodes[t];
        let parent = e.parent.as_deref().and_then(|p| self.doc.get(p));
        let mut skip: HashSet<&str> = skip.clone();
        skip.extend(["parent", "needsMeasure"]);
        if !skip.contains("index") && e.parent == tn.parent {
            skip.insert("index");
        }
        // Sizes and positions that auto layout derives.
        if e.layout_sizing_horizontal != SizingMode::Fixed && e.layout_sizing_horizontal == tn.layout_sizing_horizontal {
            skip.insert("width");
        }
        if e.layout_sizing_vertical != SizingMode::Fixed && e.layout_sizing_vertical == tn.layout_sizing_vertical {
            skip.insert("height");
        }
        if parent.is_some_and(|p| p.layout_mode != LayoutMode::None) && !e.layout_absolute && !tn.layout_absolute {
            skip.extend(["x", "y"]);
        }
        // Values a shared style supplies (applied on every replica).
        if e.fill_style_id.is_some() && e.fill_style_id == tn.fill_style_id {
            skip.insert("fills");
        }
        if e.stroke_style_id.is_some() && e.stroke_style_id == tn.stroke_style_id {
            skip.insert("strokes");
        }
        if e.text_style_id.is_some() && e.text_style_id == tn.text_style_id {
            skip.extend(crate::styles::TEXT_PROPS.iter().copied());
        }
        let mut props = Map::new();
        for &k in Node::PROPS {
            if skip.contains(k) {
                continue;
            }
            let (a, b) = (e.get_prop(k), tn.get_prop(k));
            if a != b {
                props.insert(k.to_string(), b.unwrap_or(Value::Null));
            }
        }
        if e.kind == NodeType::Text && !TEXT_LAYOUT.iter().any(|k| props.contains_key(*k)) {
            // Same text: keep the size the browser measured instead of the estimate.
            match e.text_auto_resize {
                TextAutoResize::WidthAndHeight => {
                    props.remove("width");
                    props.remove("height");
                }
                TextAutoResize::Height => {
                    props.remove("height");
                }
                TextAutoResize::None => {}
            }
        } else if e.kind == NodeType::Text && !props.is_empty() {
            props.insert("needsMeasure".into(), Value::Bool(true));
        }
        if !props.is_empty() {
            self.out.updated += 1;
            self.updates.push(Op::Update { id: e.id.clone(), props });
        }
        // Instance children are derived from the component.
        if e.kind == NodeType::Instance {
            return;
        }
        self.reconcile_children(e, t);
    }

    fn reconcile_children(&mut self, e: &Node, t: &str) {
        let empty = Vec::new();
        let want = self.fresh.kids.get(t).unwrap_or(&empty);
        let have: Vec<&Node> = self.doc.children(&e.id).iter().filter(|id| !is_derived_id(id)).filter_map(|id| self.doc.get(id)).collect();
        let mut taken: HashSet<&str> = HashSet::new();
        // Match by key, or else by name and type among unkeyed children.
        let mut pairs: Vec<(&NodeId, Option<&Node>)> = Vec::new();
        for c in want {
            let cn = &self.fresh.nodes[c];
            let m = match &cn.key {
                Some(k) => have.iter().find(|h| h.key.as_ref() == Some(k) && !taken.contains(h.id.as_str())),
                None => have.iter().find(|h| h.key.is_none() && h.name == cn.name && h.kind == cn.kind && !taken.contains(h.id.as_str())),
            }
            .copied();
            if let Some(h) = m {
                taken.insert(&h.id);
            }
            pairs.push((c, m));
        }
        // Children that left the spec: keyed ones came from an upload and go;
        // unkeyed ones were added in Flexma and stay, on top.
        let mut extras: Vec<&Node> = Vec::new();
        for h in &have {
            if taken.contains(h.id.as_str()) {
                continue;
            }
            if h.key.is_some() {
                self.delete(&h.id);
            } else {
                extras.push(h);
            }
        }
        // Final order: the spec's, then the extras.
        let mut final_ids: Vec<NodeId> = Vec::new();
        for (c, m) in &pairs {
            match m {
                Some(h) if h.kind == self.fresh.nodes[*c].kind => final_ids.push(h.id.clone()),
                Some(h) => {
                    self.delete(&h.id);
                    final_ids.push((*c).clone());
                }
                None => final_ids.push((*c).clone()),
            }
        }
        final_ids.extend(extras.iter().map(|h| h.id.clone()));
        let current: Vec<&str> = have.iter().filter(|h| final_ids.contains(&h.id)).map(|h| h.id.as_str()).collect();
        let reorder = current.len() != final_ids.len() || current.iter().zip(&final_ids).any(|(a, b)| *a != b.as_str());
        let keys = if reorder { fractional::keys_between(None, None, final_ids.len()) } else { Vec::new() };
        let index_of = |i: usize| keys.get(i).cloned();
        for (i, (c, m)) in pairs.iter().enumerate() {
            match m {
                Some(h) if h.kind == self.fresh.nodes[*c].kind => {
                    let mut skip: HashSet<&str> = HashSet::new();
                    skip.insert("index");
                    self.reconcile(h, c, &skip);
                    if let Some(k) = index_of(i) {
                        if h.index != k {
                            self.updates.push(Op::update1(h.id.clone(), "index", Value::String(k)));
                        }
                    }
                }
                _ => {
                    let index = index_of(i).or_else(|| Some(self.fresh.nodes[*c].index.clone()));
                    self.create(c, Some((e.id.clone(), index)));
                }
            }
        }
        for (i, h) in extras.iter().enumerate() {
            if let Some(k) = index_of(pairs.len() + i) {
                if h.index != k {
                    self.updates.push(Op::update1(h.id.clone(), "index", Value::String(k)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(doc: &mut Document, specs: Value, n: &mut u32) -> Upsert {
        let mut gen = || {
            *n += 1;
            format!("u:{n}")
        };
        let up = build_upsert_ops(doc, "0:1", specs.as_array().unwrap(), None, &mut gen).unwrap();
        let (_, errs) = doc.apply_all(&up.ops);
        assert!(errs.is_empty(), "{errs:?}");
        crate::layout::relayout_all(doc);
        up
    }

    fn screen(title: &str, with_badge: bool) -> Value {
        let mut kids = vec![
            json!({ "type": "text", "key": "title", "text": title, "size": 24 }),
            json!({ "type": "rect", "key": "panel", "x": 0, "y": 40, "w": 300, "h": 200, "fill": "#223344" }),
        ];
        if with_badge {
            kids.push(json!({ "type": "ellipse", "key": "badge", "x": 280, "y": 0, "w": 20, "h": 20, "fill": "#ff0000" }));
        }
        json!([{ "type": "frame", "key": "screen/main", "name": "Main", "x": 0, "y": 0, "width": 400, "height": 300, "children": kids }])
    }

    #[test]
    fn reuploads_update_in_place() {
        let mut doc = Document::new_empty();
        let mut n = 0;
        let first = run(&mut doc, screen("Hello", true), &mut n);
        assert_eq!((first.created, first.matched), (4, 0));
        let frame = first.top[0].clone();
        let kids: Vec<NodeId> = doc.children(&frame).to_vec();
        // Someone adds a sticky note in Flexma.
        let mut note = Node::new("note", NodeType::Rectangle);
        note.parent = Some(frame.clone());
        note.index = doc.index_at_end(&frame);
        doc.apply(&Op::Create { node: note }).unwrap();

        // Same export again: nothing changes.
        let again = run(&mut doc, screen("Hello", true), &mut n);
        assert_eq!((again.created, again.updated, again.deleted, again.matched), (0, 0, 0, 1), "{:?}", again.ops);
        assert_eq!(again.top, vec![frame.clone()]);

        // Changed title, badge removed: same ids, badge deleted, note kept.
        let changed = run(&mut doc, screen("Goodbye", false), &mut n);
        assert_eq!((changed.created, changed.deleted), (0, 1));
        let title = doc.get(&kids[0]).unwrap();
        assert_eq!(title.characters, "Goodbye");
        assert!(doc.get(&kids[2]).is_none(), "the badge left the export");
        assert!(doc.get("note").is_some(), "nodes added in Flexma stay");
        assert_eq!(doc.children(&frame).last().map(String::as_str), Some("note"), "and stay on top");

        // A new child appears in the middle of the spec order.
        let mut spec = screen("Goodbye", false);
        spec[0]["children"].as_array_mut().unwrap().insert(1, json!({ "type": "rect", "key": "divider", "w": 300, "h": 1 }));
        let grown = run(&mut doc, spec, &mut n);
        assert_eq!(grown.created, 1);
        let names: Vec<Option<String>> = doc.children(&frame).iter().map(|c| doc.get(c).unwrap().key.clone()).collect();
        assert_eq!(names, vec![Some("title".into()), Some("divider".into()), Some("panel".into()), None]);
    }

    #[test]
    fn kind_changes_replace_and_unkeyed_specs_create() {
        let mut doc = Document::new_empty();
        let mut n = 0;
        run(&mut doc, json!([{ "type": "rect", "key": "k", "w": 10, "h": 10 }]), &mut n);
        let up = run(&mut doc, json!([{ "type": "ellipse", "key": "k", "w": 10, "h": 10 }, { "type": "rect", "w": 5, "h": 5 }]), &mut n);
        assert_eq!((up.created, up.deleted, up.matched), (2, 1, 0));
        assert_eq!(doc.get(&up.top[0]).unwrap().kind, NodeType::Ellipse);
    }
}
