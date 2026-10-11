//! Constraints: how children follow when their parent is resized (Figma's
//! constraints, Unity's anchors).
//!
//! Resizing a frame turns into explicit child updates ([`resize_ops`]), so the
//! results travel as ordinary ops and undo with the resize. Inside instances,
//! expansion derives child geometry from the main component with
//! [`constrain_rect`] instead (see `components`), so an instance can be any
//! size and its contents re-flow on every replica.

use crate::doc::Document;
use crate::node::{is_derived_id, Node, NodeId, NodeType};
use crate::ops::Op;
use crate::paint::{ConstraintType, Constraints, LayoutMode, TextAutoResize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// One axis: a child at `pos` with `size` inside a parent going from `p0` to `p1`.
pub fn axis(c: ConstraintType, pos: f64, size: f64, p0: f64, p1: f64) -> (f64, f64) {
    let d = p1 - p0;
    match c {
        ConstraintType::Min => (pos, size),
        ConstraintType::Max => (pos + d, size),
        ConstraintType::Center => (pos + d / 2.0, size),
        ConstraintType::Stretch => (pos, (size + d).max(0.0)),
        ConstraintType::Scale => {
            let s = if p0.abs() > 1e-9 { p1 / p0 } else { 1.0 };
            (pos * s, size * s)
        }
    }
}

/// `[x, y, w, h]` of a child after its parent went from `from` to `to`
/// (`(width, height)`). Group children always scale.
pub fn constrain_rect(r: [f64; 4], c: Constraints, from: (f64, f64), to: (f64, f64), group: bool) -> [f64; 4] {
    let (h, v) = if group { (ConstraintType::Scale, ConstraintType::Scale) } else { (c.horizontal, c.vertical) };
    let (x, w) = axis(h, r[0], r[2], from.0, to.0);
    let (y, hh) = axis(v, r[1], r[3], from.1, to.1);
    [x, y, w, hh]
}

/// Do children of `n` follow its resizes through ops? Instance contents are
/// derived and re-expand on their own.
fn follows(n: &Node) -> bool {
    (n.kind.is_frame_like() || n.kind == NodeType::Group) && n.kind != NodeType::Instance && !is_derived_id(&n.id)
}

/// Is `child` placed by its parent's auto layout (so constraints don't apply)?
fn placed_by_layout(parent: &Node, child: &Node) -> bool {
    parent.kind.is_frame_like() && parent.layout_mode != LayoutMode::None && !child.layout_absolute
}

/// The part of a container's subtree a resize can move, as it was before the
/// resize. Recomputing from it on every pointer move keeps drags exact.
#[derive(Clone, Debug, Default)]
pub struct Subtree {
    root: NodeId,
    nodes: HashMap<NodeId, Node>,
    children: HashMap<NodeId, Vec<NodeId>>,
}

impl Subtree {
    /// Capture `root` and the descendants whose geometry can follow it: its
    /// children, and recursively the children of those that can change size.
    pub fn capture(doc: &Document, root: &str) -> Subtree {
        let mut t = Subtree { root: root.to_string(), ..Subtree::default() };
        if let Some(n) = doc.get(root) {
            t.add(doc, n);
        }
        t
    }

    fn add(&mut self, doc: &Document, n: &Node) {
        self.nodes.insert(n.id.clone(), n.clone());
        if !follows(n) {
            return;
        }
        let group = n.kind == NodeType::Group;
        let kids: Vec<NodeId> = doc.children(&n.id).to_vec();
        for k in &kids {
            let Some(c) = doc.get(k) else { continue };
            if placed_by_layout(n, c) {
                continue;
            }
            let resizes = group
                || matches!(c.constraints.horizontal, ConstraintType::Stretch | ConstraintType::Scale)
                || matches!(c.constraints.vertical, ConstraintType::Stretch | ConstraintType::Scale);
            if resizes {
                self.add(doc, c);
            } else {
                self.nodes.insert(c.id.clone(), c.clone());
            }
        }
        self.children.insert(n.id.clone(), kids.into_iter().filter(|k| self.nodes.contains_key(k)).collect());
    }

    pub fn get(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// Updates that put captured descendants back as they were, for the props
    /// `ops` don't set. During a drag, children an earlier pointer move changed
    /// must return when the frame goes back to its size or constraints are
    /// switched off.
    pub fn restore_ops(&self, doc: &Document, ops: &[Op]) -> Vec<Op> {
        const KEYS: &[&str] = &["x", "y", "width", "height", "textAutoResize", "pathData", "needsMeasure"];
        let mut set: HashMap<&str, Vec<&str>> = HashMap::new();
        for op in ops {
            if let Op::Update { id, props } = op {
                set.entry(id.as_str()).or_default().extend(props.keys().map(String::as_str));
            }
        }
        let mut out = Vec::new();
        for (id, n0) in &self.nodes {
            if *id == self.root {
                continue;
            }
            let Some(cur) = doc.get(id) else { continue };
            let skip = set.get(id.as_str());
            let mut props = Map::new();
            for k in KEYS {
                if skip.map(|s| s.contains(k)).unwrap_or(false) {
                    continue;
                }
                let v0 = n0.get_prop(k);
                if v0 != cur.get_prop(k) {
                    props.insert((*k).to_string(), v0.unwrap_or(Value::Null));
                }
            }
            if !props.is_empty() {
                out.push(Op::Update { id: id.clone(), props });
            }
        }
        out
    }
}

/// Updates that move and resize the captured root's descendants by their
/// constraints when the root becomes `w`×`h`. The root's own update is not
/// included. Text whose width changes gets an estimated height and
/// `needsMeasure`, so an editor measures it properly.
pub fn resize_ops(t: &Subtree, w: f64, h: f64) -> Vec<Op> {
    let mut out = Vec::new();
    if let Some(root) = t.nodes.get(&t.root) {
        follow(t, root, w, h, &mut out);
    }
    out
}

fn follow(t: &Subtree, parent: &Node, w: f64, h: f64, out: &mut Vec<Op>) {
    if !follows(parent) || ((w - parent.width).abs() < 1e-9 && (h - parent.height).abs() < 1e-9) {
        return;
    }
    let group = parent.kind == NodeType::Group;
    let Some(kids) = t.children.get(&parent.id) else { return };
    for k in kids {
        let Some(c) = t.nodes.get(k) else { continue };
        let [x, y, cw, mut ch] = constrain_rect([c.x, c.y, c.width, c.height], c.constraints, (parent.width, parent.height), (w, h), group);
        if c.kind == NodeType::Line {
            ch = c.height;
        }
        let mut props = Map::new();
        let mut put = |key: &str, old: f64, new: f64| {
            if (old - new).abs() > 1e-9 {
                props.insert(key.into(), json!(new));
            }
        };
        put("x", c.x, x);
        put("y", c.y, y);
        put("width", c.width, cw);
        put("height", c.height, ch);
        resized_extras(c, cw, ch, &mut props);
        if !props.is_empty() {
            out.push(Op::Update { id: c.id.clone(), props });
        }
        if (cw - c.width).abs() > 1e-9 || (ch - c.height).abs() > 1e-9 {
            follow(t, c, cw, ch, out);
        }
    }
}

/// Side effects of a node being resized to `w`×`h` by its parent: text boxes
/// become fixed along the axes that changed, vector paths scale.
fn resized_extras(n: &Node, w: f64, h: f64, props: &mut Map<String, Value>) {
    let dw = (w - n.width).abs() > 1e-9;
    let dh = (h - n.height).abs() > 1e-9;
    match n.kind {
        NodeType::Text if dw || dh => {
            let mode = if dh {
                TextAutoResize::None
            } else if n.text_auto_resize == TextAutoResize::WidthAndHeight {
                TextAutoResize::Height
            } else {
                n.text_auto_resize
            };
            if mode != n.text_auto_resize {
                props.insert("textAutoResize".into(), json!(mode));
            }
            if mode == TextAutoResize::Height {
                let mut probe = n.clone();
                probe.width = w;
                probe.text_auto_resize = mode;
                props.insert("height".into(), json!(crate::text::estimate_layout(&probe).2));
                props.insert("needsMeasure".into(), json!(true));
            }
        }
        NodeType::Vector if (dw || dh) && n.width > 0.0 && n.height > 0.0 => {
            let segs = crate::path::scale(&crate::path::parse(&n.path_data), w / n.width, h / n.height.max(1e-6));
            props.insert("pathData".into(), json!(crate::path::to_svg_d(&segs)));
        }
        _ => {}
    }
}

/// Child updates for an update `props` (new `width`/`height`) aimed at `id`,
/// computed against the document before the update applies.
pub fn follow_resize(doc: &Document, id: &str, props: &Map<String, Value>) -> Vec<Op> {
    let Some(n) = doc.get(id) else { return Vec::new() };
    let w = props.get("width").and_then(Value::as_f64).unwrap_or(n.width);
    let h = props.get("height").and_then(Value::as_f64).unwrap_or(n.height);
    if !follows(n) || ((w - n.width).abs() < 1e-9 && (h - n.height).abs() < 1e-9) {
        return Vec::new();
    }
    resize_ops(&Subtree::capture(doc, id), w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components;
    use crate::paint::ConstraintType::*;

    fn node(id: &str, kind: NodeType, parent: &str, r: [f64; 4]) -> Node {
        let mut n = Node::new(id, kind);
        n.parent = Some(parent.into());
        [n.x, n.y, n.width, n.height] = r;
        n
    }

    fn pinned(id: &str, parent: &str, r: [f64; 4], h: ConstraintType, v: ConstraintType) -> Node {
        let mut n = node(id, NodeType::Rectangle, parent, r);
        n.constraints = Constraints { horizontal: h, vertical: v };
        n
    }

    fn rect(doc: &Document, id: &str) -> [f64; 4] {
        let n = doc.get(id).unwrap();
        [n.x, n.y, n.width, n.height]
    }

    fn resize(doc: &mut Document, id: &str, w: f64, h: f64) {
        let mut props = Map::new();
        props.insert("width".into(), json!(w));
        props.insert("height".into(), json!(h));
        let mut ops = follow_resize(doc, id, &props);
        ops.push(Op::Update { id: id.into(), props });
        let (eff, errs) = doc.apply_all(&components::route_ops(doc, ops));
        assert!(errs.is_empty(), "{errs:?}");
        components::sync_instances(doc, &eff.changed, &eff.removed);
    }

    fn setup() -> Document {
        let mut doc = Document::new_empty();
        doc.apply(&Op::Create { node: node("f", NodeType::Frame, "0:1", [0.0, 0.0, 200.0, 100.0]) }).unwrap();
        for n in [
            pinned("min", "f", [10.0, 10.0, 20.0, 20.0], Min, Min),
            pinned("max", "f", [170.0, 70.0, 20.0, 20.0], Max, Max),
            pinned("center", "f", [90.0, 40.0, 20.0, 20.0], Center, Center),
            pinned("stretch", "f", [10.0, 10.0, 180.0, 80.0], Stretch, Stretch),
            pinned("scale", "f", [50.0, 25.0, 100.0, 50.0], Scale, Scale),
        ] {
            doc.apply(&Op::Create { node: n }).unwrap();
        }
        doc
    }

    #[test]
    fn children_follow_their_constraints() {
        let mut doc = setup();
        resize(&mut doc, "f", 300.0, 200.0);
        assert_eq!(rect(&doc, "min"), [10.0, 10.0, 20.0, 20.0]);
        assert_eq!(rect(&doc, "max"), [270.0, 170.0, 20.0, 20.0]);
        assert_eq!(rect(&doc, "center"), [140.0, 90.0, 20.0, 20.0]);
        assert_eq!(rect(&doc, "stretch"), [10.0, 10.0, 280.0, 180.0]);
        assert_eq!(rect(&doc, "scale"), [75.0, 50.0, 150.0, 100.0]);
    }

    #[test]
    fn nested_frames_and_groups_follow() {
        let mut doc = setup();
        let mut inner = node("inner", NodeType::Frame, "f", [10.0, 10.0, 180.0, 80.0]);
        inner.constraints = Constraints { horizontal: Stretch, vertical: Min };
        doc.apply(&Op::Create { node: inner }).unwrap();
        doc.apply(&Op::Create { node: pinned("pin", "inner", [150.0, 0.0, 20.0, 20.0], Max, Min) }).unwrap();
        let mut g = node("g", NodeType::Group, "inner", [0.0, 40.0, 180.0, 40.0]);
        g.constraints = Constraints { horizontal: Stretch, vertical: Min };
        doc.apply(&Op::Create { node: g }).unwrap();
        doc.apply(&Op::Create { node: node("gk", NodeType::Rectangle, "g", [90.0, 0.0, 90.0, 40.0]) }).unwrap();
        resize(&mut doc, "f", 380.0, 100.0);
        assert_eq!(rect(&doc, "inner"), [10.0, 10.0, 360.0, 80.0]);
        assert_eq!(rect(&doc, "pin"), [330.0, 0.0, 20.0, 20.0]);
        assert_eq!(rect(&doc, "g"), [0.0, 40.0, 360.0, 40.0]);
        assert_eq!(rect(&doc, "gk"), [180.0, 0.0, 180.0, 40.0]);
    }

    #[test]
    fn auto_layout_children_and_text() {
        let mut doc = setup();
        doc.apply(&Op::update1("f", "layoutMode", json!("VERTICAL"))).unwrap();
        doc.apply(&Op::update1("max", "layoutAbsolute", json!(true))).unwrap();
        let mut t = node("t", NodeType::Text, "0:1", [0.0, 0.0, 0.0, 0.0]);
        t.characters = "Hello".into();
        t.constraints = Constraints { horizontal: Stretch, vertical: Min };
        let ops = follow_resize(&doc, "f", &Map::from_iter([("width".to_string(), json!(260))]));
        // Only the absolute child is moved; the flow children are auto layout's.
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].target(), "max");
        // Stretched text wraps at its new width and asks to be measured.
        doc.apply(&Op::update1("f", "layoutMode", json!("NONE"))).unwrap();
        doc.apply(&Op::Create { node: Node { parent: Some("f".into()), ..t } }).unwrap();
        resize(&mut doc, "f", 260.0, 100.0);
        let t = doc.get("t").unwrap();
        assert_eq!(t.width, 60.0);
        assert_eq!(t.text_auto_resize, TextAutoResize::Height);
        assert!(t.needs_measure);
    }

    #[test]
    fn instances_reflow_from_their_component() {
        let mut doc = Document::new_empty();
        doc.apply(&Op::Create { node: node("c", NodeType::Component, "0:1", [0.0, 0.0, 100.0, 40.0]) }).unwrap();
        doc.apply(&Op::Create { node: pinned("bg", "c", [0.0, 0.0, 100.0, 40.0], Stretch, Stretch) }).unwrap();
        doc.apply(&Op::Create { node: pinned("label", "c", [30.0, 10.0, 40.0, 20.0], Center, Center) }).unwrap();
        doc.apply(&Op::Create { node: pinned("icon", "c", [80.0, 10.0, 10.0, 20.0], Max, Center) }).unwrap();
        let mut i = node("i", NodeType::Instance, "0:1", [300.0, 0.0, 100.0, 40.0]);
        i.component_id = Some("c".into());
        doc.apply(&Op::Create { node: i }).unwrap();
        components::sync_all(&mut doc);
        resize(&mut doc, "i", 200.0, 60.0);
        assert_eq!(rect(&doc, "i~bg"), [0.0, 0.0, 200.0, 60.0]);
        assert_eq!(rect(&doc, "i~label"), [80.0, 20.0, 40.0, 20.0]);
        assert_eq!(rect(&doc, "i~icon"), [180.0, 20.0, 10.0, 20.0]);
        // The instance produced no child ops: its contents are derived.
        assert!(follow_resize(&doc, "i", &Map::from_iter([("width".to_string(), json!(10))])).is_empty());
        // An override on a layer's geometry wins over its constraints.
        let ops = components::route_ops(&doc, vec![Op::update1("i~label", "x", json!(5))]);
        let (eff, _) = doc.apply_all(&ops);
        components::sync_instances(&mut doc, &eff.changed, &eff.removed);
        assert_eq!(rect(&doc, "i~label"), [5.0, 20.0, 40.0, 20.0]);
        // Resizing the main component moves its children by ops, and instances keep their own size.
        resize(&mut doc, "c", 120.0, 40.0);
        assert_eq!(rect(&doc, "icon"), [100.0, 10.0, 10.0, 20.0]);
        assert_eq!(rect(&doc, "i~icon"), [180.0, 20.0, 10.0, 20.0]);
        assert_eq!(rect(&doc, "i~bg"), [0.0, 0.0, 200.0, 60.0]);
        doc.check_consistency().unwrap();
    }

    #[test]
    fn nested_instances_follow_their_own_size() {
        let mut doc = Document::new_empty();
        doc.apply(&Op::Create { node: node("b", NodeType::Component, "0:1", [0.0, 0.0, 100.0, 40.0]) }).unwrap();
        doc.apply(&Op::Create { node: pinned("bbg", "b", [0.0, 0.0, 100.0, 40.0], Stretch, Stretch) }).unwrap();
        doc.apply(&Op::Create { node: node("card", NodeType::Component, "0:1", [0.0, 100.0, 300.0, 200.0]) }).unwrap();
        let mut nb = pinned("nb", "card", [20.0, 140.0, 260.0, 40.0], Stretch, Max);
        nb.kind = NodeType::Instance;
        nb.component_id = Some("b".into());
        doc.apply(&Op::Create { node: nb }).unwrap();
        let mut i = node("i", NodeType::Instance, "0:1", [400.0, 0.0, 300.0, 200.0]);
        i.component_id = Some("card".into());
        doc.apply(&Op::Create { node: i }).unwrap();
        components::sync_all(&mut doc);
        // The nested button is wider than its component, so its background stretches.
        assert_eq!(rect(&doc, "i~nb~bbg"), [0.0, 0.0, 260.0, 40.0]);
        resize(&mut doc, "i", 400.0, 260.0);
        assert_eq!(rect(&doc, "i~nb"), [20.0, 200.0, 360.0, 40.0]);
        assert_eq!(rect(&doc, "i~nb~bbg"), [0.0, 0.0, 360.0, 40.0]);
    }

    #[test]
    fn parses_names_and_unity_anchors() {
        let p = |v: Value| serde_json::from_value::<Constraints>(v).unwrap();
        assert_eq!(p(json!({ "horizontal": "left & right", "vertical": "BOTTOM" })), Constraints { horizontal: Stretch, vertical: Max });
        assert_eq!(p(json!("scale")), Constraints { horizontal: Scale, vertical: Scale });
        assert_eq!(p(json!({ "vertical": "center" })), Constraints { horizontal: Min, vertical: Center });
        // Unity: y grows upward, so a bottom anchor is MAX here.
        assert_eq!(p(json!({ "anchorMin": [1, 0], "anchorMax": [1, 0] })), Constraints { horizontal: Max, vertical: Max });
        assert_eq!(p(json!({ "anchorMin": { "x": 0, "y": 1 }, "anchorMax": { "x": 1, "y": 1 } })), Constraints { horizontal: Stretch, vertical: Min });
        assert_eq!(p(json!({ "anchorMin": [0.5, 0.5], "anchorMax": [0.5, 0.5] })), Constraints { horizontal: Center, vertical: Center });
        assert_eq!(p(json!({ "anchorMin": [0.5, 0], "anchorMax": [1, 1] })), Constraints { horizontal: Scale, vertical: Stretch });
        assert!(serde_json::from_value::<Constraints>(json!({ "horizontal": "sideways" })).is_err());
        // Defaults stay out of the stored document.
        let mut n = Node::new("1:1", NodeType::Rectangle);
        assert!(n.to_json().get("constraints").is_none());
        n.set_prop("constraints", &json!({ "horizontal": "STRETCH" })).unwrap();
        assert_eq!(n.to_json()["constraints"], json!({ "horizontal": "STRETCH", "vertical": "MIN" }));
    }
}
