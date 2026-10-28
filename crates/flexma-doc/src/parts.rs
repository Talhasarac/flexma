//! A document split into parts: a shared part with what every page needs
//! (the root, the pages themselves, styles, and every component with its
//! layers, since instances on any page expand from them) and a part per page
//! with the rest of its layers. Rooms save only the parts that changed, and
//! editors get the page they open before the others.

use crate::doc::Document;
use crate::node::{is_derived_id, Node, NodeId, NodeType, ROOT_ID};
use serde::Deserialize;
use std::collections::HashSet;

/// The shared part's name; page parts are named by their page's id.
pub const SHARED: &str = "shared";

/// A document's stored nodes, each in exactly one part, parents before
/// children within a part.
pub struct Parts<'a> {
    pub shared: Vec<&'a Node>,
    /// Each page's other layers, in page order.
    pub pages: Vec<(NodeId, Vec<&'a Node>)>,
}

impl<'a> Parts<'a> {
    /// Every part with its name, shared first.
    pub fn named(&self) -> impl Iterator<Item = (&str, &[&'a Node])> {
        std::iter::once((SHARED, self.shared.as_slice())).chain(self.pages.iter().map(|(p, nodes)| (p.as_str(), nodes.as_slice())))
    }
}

pub fn split(doc: &Document) -> Parts<'_> {
    let mut parts = Parts { shared: Vec::new(), pages: Vec::new() };
    visit(doc, ROOT_ID, None, false, &mut parts);
    parts
}

fn visit<'a>(doc: &'a Document, id: &str, page: Option<usize>, in_component: bool, parts: &mut Parts<'a>) {
    let Some(n) = doc.get(id) else { return };
    let in_component = in_component || n.kind == NodeType::Component;
    let mut page = page;
    if n.kind == NodeType::Page {
        parts.shared.push(n);
        parts.pages.push((n.id.clone(), Vec::new()));
        page = Some(parts.pages.len() - 1);
    } else {
        match page {
            Some(i) if !in_component => parts.pages[i].1.push(n),
            _ => parts.shared.push(n),
        }
    }
    // Instance layers are derived on every replica, never stored.
    for c in doc.children(id) {
        if !is_derived_id(c) {
            visit(doc, c, page, in_component, parts);
        }
    }
}

/// A part as JSON: an array of nodes.
pub fn to_json(nodes: &[&Node]) -> String {
    serde_json::to_string(nodes).unwrap_or_else(|_| "[]".into())
}

/// A stored document's JSON from its parts' JSON arrays, in any order.
pub fn assemble<'s>(parts: impl IntoIterator<Item = &'s str>) -> String {
    let mut out = String::from(r#"{"nodes":["#);
    let mut empty = true;
    for p in parts {
        let inner = p.trim().strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or("").trim();
        if inner.is_empty() {
            continue;
        }
        if !empty {
            out.push(',');
        }
        out.push_str(inner);
        empty = false;
    }
    out.push_str("]}");
    out
}

/// What an editor needs to show `page` before the other pages arrive: the
/// shared part, the frames each component sits in (a node whose parent is
/// missing would be dropped on load) and the page's own layers. Parents
/// come before children.
pub fn first_part<'a>(doc: &'a Document, parts: &Parts<'a>, page: &str) -> Vec<&'a Node> {
    let mut keep: HashSet<&str> = parts.shared.iter().map(|n| n.id.as_str()).collect();
    for n in &parts.shared {
        if n.kind != NodeType::Component {
            continue;
        }
        let mut up = n.parent.as_deref();
        while let Some(p) = up {
            if !keep.insert(p) {
                break;
            }
            up = doc.get(p).and_then(|x| x.parent.as_deref());
        }
    }
    if let Some((_, nodes)) = parts.pages.iter().find(|(p, _)| p == page) {
        keep.extend(nodes.iter().map(|n| n.id.as_str()));
    }
    let mut out = Vec::with_capacity(keep.len());
    preorder(doc, ROOT_ID, &keep, &mut out);
    out
}

fn preorder<'a>(doc: &'a Document, id: &str, keep: &HashSet<&str>, out: &mut Vec<&'a Node>) {
    let Some(n) = doc.get(id) else { return };
    if keep.contains(id) {
        out.push(n);
    }
    for c in doc.children(id) {
        if !is_derived_id(c) {
            preorder(doc, c, keep, out);
        }
    }
}

/// A page's part as an editor receives it after the first part.
#[derive(Deserialize)]
pub struct PagePart {
    pub page: NodeId,
    pub nodes: Vec<Node>,
}

/// JSON for a page's part: `{ "page": id, "nodes": [...] }`.
pub fn page_part_json(page: &str, nodes: &[&Node]) -> String {
    format!(r#"{{"page":{},"nodes":{}}}"#, serde_json::to_string(page).unwrap_or_default(), to_json(nodes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Op;

    fn add(doc: &mut Document, id: &str, kind: NodeType, parent: &str) {
        let mut n = Node::new(id, kind);
        n.parent = Some(parent.into());
        n.index = doc.index_at_end(parent);
        doc.apply(&Op::Create { node: n }).unwrap();
    }

    /// Page 1 has a screen with a card component; page 2 an instance of it
    /// and a style; the root a color style.
    fn sample() -> Document {
        let mut doc = Document::new_empty();
        add(&mut doc, "p:2", NodeType::Page, ROOT_ID);
        add(&mut doc, "s:1", NodeType::Style, ROOT_ID);
        add(&mut doc, "f:1", NodeType::Frame, "0:1");
        add(&mut doc, "c:1", NodeType::Component, "f:1");
        add(&mut doc, "t:1", NodeType::Text, "c:1");
        add(&mut doc, "r:1", NodeType::Rectangle, "f:1");
        add(&mut doc, "f:2", NodeType::Frame, "p:2");
        let mut i = Node::new("i:1", NodeType::Instance);
        i.parent = Some("f:2".into());
        i.component_id = Some("c:1".into());
        doc.apply(&Op::Create { node: i }).unwrap();
        crate::components::sync_all(&mut doc);
        doc
    }

    fn ids(nodes: &[&Node]) -> Vec<String> {
        nodes.iter().map(|n| n.id.clone()).collect()
    }

    #[test]
    fn every_stored_node_is_in_exactly_one_part() {
        let doc = sample();
        let parts = split(&doc);
        assert_eq!(ids(&parts.shared), ["0:0", "0:1", "c:1", "t:1", "p:2", "s:1"]);
        assert_eq!(parts.pages.iter().map(|(p, n)| (p.as_str(), ids(n))).collect::<Vec<_>>(), [("0:1", vec!["f:1".to_string(), "r:1".into()]), ("p:2", vec!["f:2".into(), "i:1".into()])]);
        // The parts assemble into the same document.
        let json = assemble(parts.named().map(|(_, n)| to_json(n)).collect::<Vec<_>>().iter().map(String::as_str));
        let back = Document::from_json(&json).unwrap();
        assert_eq!(back.snapshot().nodes.len(), doc.snapshot().nodes.len());
        for n in doc.snapshot().nodes {
            assert_eq!(back.get(&n.id), Some(&n), "{}", n.id);
        }
    }

    #[test]
    fn the_first_part_shows_a_page_with_every_component() {
        let doc = sample();
        // Page 2 first: its instance's component sits on page 1, so its frame comes along.
        let first = first_part(&doc, &split(&doc), "p:2");
        assert_eq!(ids(&first), ["0:0", "0:1", "f:1", "c:1", "t:1", "p:2", "f:2", "i:1", "s:1"]);
        let mut partial = Document::from_json(&format!(r#"{{"nodes":{}}}"#, to_json(&first))).unwrap();
        crate::components::sync_all(&mut partial);
        assert!(partial.contains("i:1~t:1"), "the instance expands from the shared component");
        assert!(!partial.contains("r:1"));
        // Page 1 streams in afterwards.
        let parts = split(&doc);
        let (page, nodes) = &parts.pages[0];
        let part: PagePart = serde_json::from_str(&page_part_json(page, nodes)).unwrap();
        assert_eq!(part.page, "0:1");
        partial.merge_nodes(part.nodes);
        assert_eq!(partial.snapshot().nodes.len(), doc.snapshot().nodes.len());
        assert_eq!(partial.children("f:1"), doc.children("f:1"));
    }
}
