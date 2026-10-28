//! The document: a flat map of nodes plus a derived, sorted child index.

use crate::fractional;
use crate::geom::{local_transform, Affine, Rect};
use crate::node::{Node, NodeId, NodeType, ROOT_ID};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Document {
    nodes: HashMap<NodeId, Node>,
    children: HashMap<NodeId, Vec<NodeId>>,
    /// Nodes created by ops in the batch being applied. Their first layout
    /// isn't a resize: children were placed for the laid-out size. Not
    /// stored; every replica clears it after a batch (`clear_fresh`).
    fresh: std::collections::HashSet<NodeId>,
}

/// Wire/storage form of a document: nodes in tree pre-order (parents first).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DocSnapshot {
    pub nodes: Vec<Node>,
}

impl Document {
    /// A fresh document with one empty page.
    pub fn new_empty() -> Document {
        let mut doc = Document::default();
        let root = Node::new(ROOT_ID, NodeType::Document);
        doc.nodes.insert(ROOT_ID.into(), root);
        let mut page = Node::new("0:1", NodeType::Page);
        page.name = "Page 1".into();
        page.parent = Some(ROOT_ID.into());
        page.index = fractional::key_between(None, None);
        doc.insert_unchecked(page);
        doc
    }

    /// Build from a snapshot. Nodes whose parent is missing are dropped.
    pub fn from_snapshot(snap: DocSnapshot) -> Document {
        let mut doc = Document::default();
        let mut pending: Vec<Node> = Vec::new();
        for n in snap.nodes {
            if n.id == ROOT_ID {
                let mut root = n;
                root.kind = NodeType::Document;
                root.parent = None;
                doc.nodes.insert(ROOT_ID.into(), root);
            } else {
                pending.push(n);
            }
        }
        if !doc.nodes.contains_key(ROOT_ID) {
            doc.nodes.insert(ROOT_ID.into(), Node::new(ROOT_ID, NodeType::Document));
        }
        // Insert in passes so out-of-order snapshots still load.
        loop {
            let before = pending.len();
            let mut rest = Vec::new();
            for n in pending {
                let ok = n
                    .parent
                    .as_ref()
                    .map(|p| doc.nodes.get(p).map(|pn| pn.kind.is_container()).unwrap_or(false))
                    .unwrap_or(false);
                if ok && !doc.nodes.contains_key(&n.id) {
                    doc.insert_unchecked(n);
                } else if !doc.nodes.contains_key(&n.id) {
                    rest.push(n);
                }
            }
            pending = rest;
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
        if doc.pages().is_empty() {
            let mut page = Node::new("0:1", NodeType::Page);
            page.name = "Page 1".into();
            page.parent = Some(ROOT_ID.into());
            page.index = fractional::key_between(None, None);
            doc.insert_unchecked(page);
        }
        doc
    }

    pub fn from_json(json: &str) -> Result<Document, String> {
        let snap: DocSnapshot = serde_json::from_str(json).map_err(|e| e.to_string())?;
        Ok(Document::from_snapshot(snap))
    }

    pub fn snapshot(&self) -> DocSnapshot {
        let mut nodes = Vec::with_capacity(self.nodes.len());
        self.walk(ROOT_ID, &mut |n| nodes.push(n.clone()));
        DocSnapshot { nodes }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.snapshot()).unwrap_or_else(|_| "{\"nodes\":[]}".into())
    }

    /// Pre-order traversal of stored nodes. The derived children of instances
    /// are skipped (unless the walk starts inside a derived subtree).
    pub fn walk(&self, id: &str, f: &mut dyn FnMut(&Node)) {
        if let Some(n) = self.nodes.get(id) {
            f(n);
            let derived_parent = crate::node::is_derived_id(id);
            for c in self.children(id) {
                if derived_parent || !crate::node::is_derived_id(c) {
                    self.walk(c, f);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    pub fn get(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn root(&self) -> &Node {
        &self.nodes[ROOT_ID]
    }

    pub fn children(&self, id: &str) -> &[NodeId] {
        self.children.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Pages, in order (the root also holds shared styles).
    pub fn pages(&self) -> Vec<NodeId> {
        self.children(ROOT_ID).iter().filter(|id| self.get(id).map(|n| n.kind == NodeType::Page).unwrap_or(false)).cloned().collect()
    }

    /// Shared color and text styles, in order.
    pub fn styles(&self) -> Vec<NodeId> {
        self.children(ROOT_ID).iter().filter(|id| self.get(id).map(|n| n.kind == NodeType::Style).unwrap_or(false)).cloned().collect()
    }

    pub fn parent_of(&self, id: &str) -> Option<&str> {
        self.nodes.get(id).and_then(|n| n.parent.as_deref())
    }

    /// Ancestors from the parent up to the root.
    pub fn ancestors(&self, id: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = self.parent_of(id);
        while let Some(p) = cur {
            out.push(p.to_string());
            cur = self.parent_of(p);
            if out.len() > 10_000 {
                break; // corrupted (cyclic) data guard
            }
        }
        out
    }

    /// Is `ancestor` a (strict) ancestor of `id`?
    pub fn is_ancestor(&self, ancestor: &str, id: &str) -> bool {
        let mut cur = self.parent_of(id);
        let mut guard = 0;
        while let Some(p) = cur {
            if p == ancestor {
                return true;
            }
            cur = self.parent_of(p);
            guard += 1;
            if guard > 10_000 {
                return false;
            }
        }
        false
    }

    pub fn page_of(&self, id: &str) -> Option<NodeId> {
        let mut cur = id.to_string();
        let mut guard = 0;
        loop {
            let n = self.nodes.get(&cur)?;
            if n.kind == NodeType::Page {
                return Some(cur);
            }
            cur = n.parent.clone()?;
            guard += 1;
            if guard > 10_000 {
                return None;
            }
        }
    }

    /// All descendants in pre-order (excluding `id` itself).
    pub fn descendants(&self, id: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        fn rec(doc: &Document, id: &str, out: &mut Vec<NodeId>) {
            for c in doc.children(id) {
                out.push(c.clone());
                rec(doc, c, out);
            }
        }
        rec(self, id, &mut out);
        out
    }

    pub fn local_transform(&self, id: &str) -> Affine {
        match self.nodes.get(id) {
            Some(n) if n.kind.is_scene() => {
                local_transform(n.x, n.y, n.width, n.height, n.rotation, n.flip_x, n.flip_y)
            }
            _ => Affine::IDENTITY,
        }
    }

    /// Transform from node-local space to page (world) space.
    pub fn world_transform(&self, id: &str) -> Affine {
        let mut chain = vec![id.to_string()];
        chain.extend(self.ancestors(id));
        let mut t = Affine::IDENTITY;
        for nid in chain.iter().rev() {
            t = t.mul(&self.local_transform(nid));
        }
        t
    }

    /// Axis-aligned bounds of the node box in page space.
    pub fn world_bounds(&self, id: &str) -> Rect {
        match self.nodes.get(id) {
            Some(n) => self.world_transform(id).map_rect(&Rect::new(0.0, 0.0, n.width, n.height)),
            None => Rect::empty(),
        }
    }

    /// Bounds of a node including all visible descendants (page space).
    pub fn subtree_bounds(&self, id: &str) -> Rect {
        let mut r = Rect::empty();
        if let Some(n) = self.nodes.get(id) {
            if n.kind.is_scene() {
                r = self.world_bounds(id);
                if n.kind.is_frame_like() && n.clips_content {
                    return r;
                }
            }
            for c in self.children(id) {
                if self.nodes.get(c).map(|c| c.visible).unwrap_or(false) {
                    r = r.union(&self.subtree_bounds(c));
                }
            }
        }
        r
    }

    /// Index key that appends after the last child of `parent`.
    pub fn index_at_end(&self, parent: &str) -> String {
        let last = self.children(parent).last().and_then(|id| self.nodes.get(id)).map(|n| n.index.as_str());
        fractional::key_between(last, None)
    }

    /// Index key placing a node at position `pos` among `parent`'s children
    /// (0 = back-most). `exclude` is ignored when computing neighbours (the node being moved).
    pub fn index_at(&self, parent: &str, pos: usize, exclude: &[NodeId]) -> String {
        let sibs: Vec<&Node> = self
            .children(parent)
            .iter()
            .filter(|id| !exclude.contains(id))
            .filter_map(|id| self.nodes.get(id))
            .collect();
        let pos = pos.min(sibs.len());
        let before = if pos == 0 { None } else { Some(sibs[pos - 1].index.as_str()) };
        let after = sibs.get(pos).map(|n| n.index.as_str());
        fractional::key_between(before, after)
    }

    /// Set derived geometry in place (auto layout, text measurement). This is a
    /// local mutation, not an op. Returns true when something changed.
    pub fn set_geometry(&mut self, id: &str, x: f64, y: f64, w: f64, h: f64) -> bool {
        match self.nodes.get_mut(id) {
            Some(n) if n.x != x || n.y != y || n.width != w || n.height != h => {
                n.x = x;
                n.y = y;
                n.width = w;
                n.height = h;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn mark_fresh(&mut self, id: &str) {
        self.fresh.insert(id.to_string());
    }

    pub fn is_fresh(&self, id: &str) -> bool {
        self.fresh.contains(id)
    }

    /// End of a batch of ops (after its layout): nothing is new any more.
    pub fn clear_fresh(&mut self) {
        self.fresh.clear();
    }

    /// Flag a text box for exact measuring (derived state, like layout).
    pub fn set_needs_measure(&mut self, id: &str) {
        if let Some(n) = self.nodes.get_mut(id) {
            n.needs_measure = true;
        }
    }

    // ---- low-level mutation (no validation). Used by ops.rs. ----

    pub(crate) fn insert_unchecked(&mut self, node: Node) {
        let id = node.id.clone();
        let parent = node.parent.clone();
        self.nodes.insert(id.clone(), node);
        if let Some(p) = parent {
            self.attach_child(&p, &id);
        }
    }

    fn sort_key<'a>(&'a self, id: &'a str) -> (&'a str, &'a str) {
        (self.nodes.get(id).map(|n| n.index.as_str()).unwrap_or(""), id)
    }

    fn attach_child(&mut self, parent: &str, id: &str) {
        let key = {
            let n = &self.nodes[id];
            (n.index.clone(), id.to_string())
        };
        let list = self.children.entry(parent.to_string()).or_default();
        let nodes = &self.nodes;
        let pos = list
            .binary_search_by(|c| {
                let cn = nodes.get(c).map(|n| n.index.as_str()).unwrap_or("");
                (cn, c.as_str()).cmp(&(key.0.as_str(), key.1.as_str()))
            })
            .unwrap_or_else(|p| p);
        list.insert(pos, id.to_string());
    }

    fn detach_child(&mut self, parent: &str, id: &str) {
        if let Some(list) = self.children.get_mut(parent) {
            if let Some(pos) = list.iter().position(|c| c == id) {
                list.remove(pos);
            }
            if list.is_empty() {
                self.children.remove(parent);
            }
        }
    }

    /// Replace a node wholesale, keeping the child index consistent.
    pub(crate) fn replace_unchecked(&mut self, node: Node) {
        let id = node.id.clone();
        let old = self.nodes.get(&id);
        let relink = match old {
            Some(o) => o.parent != node.parent || o.index != node.index,
            None => true,
        };
        if relink {
            if let Some(Some(p)) = old.map(|o| o.parent.clone()) {
                self.detach_child(&p, &id);
            }
            self.nodes.insert(id.clone(), node);
            if let Some(p) = self.nodes[&id].parent.clone() {
                self.attach_child(&p, &id);
            }
        } else {
            self.nodes.insert(id, node);
        }
    }

    /// Remove a node and its subtree; returns removed nodes in pre-order.
    pub(crate) fn remove_unchecked(&mut self, id: &str) -> Vec<Node> {
        let mut ids = vec![id.to_string()];
        ids.extend(self.descendants(id));
        if let Some(p) = self.parent_of(id).map(str::to_string) {
            self.detach_child(&p, id);
        }
        let mut out = Vec::with_capacity(ids.len());
        for nid in ids {
            self.children.remove(&nid);
            if let Some(n) = self.nodes.remove(&nid) {
                out.push(n);
            }
        }
        out
    }

    /// Debug check that the child index matches the node map.
    pub fn check_consistency(&self) -> Result<(), String> {
        for (id, n) in &self.nodes {
            if let Some(p) = &n.parent {
                if !self.children(p).contains(id) {
                    return Err(format!("{id} missing from children of {p}"));
                }
            }
        }
        for (p, list) in &self.children {
            for w in list.windows(2) {
                if self.sort_key(&w[0]) > self.sort_key(&w[1]) {
                    return Err(format!("children of {p} out of order"));
                }
            }
        }
        Ok(())
    }
}
