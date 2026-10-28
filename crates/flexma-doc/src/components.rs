//! Components and instances.
//!
//! A COMPONENT is a frame-like main component. An INSTANCE points at one with
//! `componentId` and stores `overrides`. Instance children are *derived*: every
//! replica expands them from the component locally (like auto layout), so they
//! are never stored or sent as ops. Derived ids are deterministic —
//! `instanceId~sourceId`, nested `I~N~S` — so selections survive re-expansion.
//!
//! Edits aimed at derived nodes are rewritten into override updates on the
//! owning instance by [`route_ops`] before they are applied.

use crate::doc::Document;
use crate::node::{is_derived_id, Node, NodeId, NodeType};
use crate::ops::Op;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Instance props that follow the main component unless overridden.
pub const INHERITED: &[&str] = &[
    "fills",
    "strokes",
    "strokeWeight",
    "strokeAlign",
    "dashPattern",
    "cornerRadius",
    "cornerRadii",
    "effects",
    "clipsContent",
    "clipAlpha",
    "hideFill",
    "blendMode",
    "layoutMode",
    "itemSpacing",
    "padding",
    "primaryAxisAlign",
    "counterAxisAlign",
    "layoutWrap",
];

/// Props that can be overridden on layers inside an instance.
pub fn overridable(key: &str) -> bool {
    !matches!(key, "id" | "type" | "parent" | "index" | "overrides")
}

const MAX_NESTING: usize = 8;

/// The real instance that owns a derived node, and the override key for it.
pub fn owner_of(derived_id: &str) -> Option<(&str, &str)> {
    let (inst, rest) = derived_id.split_once('~')?;
    Some((inst, rest))
}

fn merge_into(target: &mut Map<String, Value>, key: &str, props: &Map<String, Value>) {
    let entry = target.entry(key.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    let obj = entry.as_object_mut().unwrap();
    for (k, v) in props {
        if overridable(k) {
            obj.insert(k.clone(), v.clone());
        }
    }
}

/// Rewrite ops so they never target derived nodes:
/// - updates of derived nodes become `overrides` updates on the owning instance;
/// - deleting a derived node hides it (an override);
/// - setting inherited props on an instance also records them as self-overrides
///   so they survive re-expansion.
pub fn route_ops(doc: &Document, ops: Vec<Op>) -> Vec<Op> {
    let mut pending: HashMap<NodeId, Map<String, Value>> = HashMap::new();
    let mut out = Vec::with_capacity(ops.len());
    let current = |pending: &HashMap<NodeId, Map<String, Value>>, inst: &str| -> Option<Map<String, Value>> {
        if let Some(m) = pending.get(inst) {
            return Some(m.clone());
        }
        doc.get(inst).filter(|n| n.kind == NodeType::Instance).map(|n| n.overrides.clone())
    };
    for op in ops {
        match op {
            Op::Update { id, props } if is_derived_id(&id) => {
                let Some((inst, key)) = owner_of(&id) else { continue };
                let Some(mut ov) = current(&pending, inst) else { continue };
                merge_into(&mut ov, key, &props);
                pending.insert(inst.to_string(), ov.clone());
                let mut p = Map::new();
                p.insert("overrides".into(), Value::Object(ov));
                out.push(Op::Update { id: inst.to_string(), props: p });
            }
            Op::Delete { id } if is_derived_id(&id) => {
                let Some((inst, key)) = owner_of(&id) else { continue };
                let Some(mut ov) = current(&pending, inst) else { continue };
                let mut hide = Map::new();
                hide.insert("visible".into(), Value::Bool(false));
                merge_into(&mut ov, key, &hide);
                pending.insert(inst.to_string(), ov.clone());
                let mut p = Map::new();
                p.insert("overrides".into(), Value::Object(ov));
                out.push(Op::Update { id: inst.to_string(), props: p });
            }
            Op::Update { id, mut props } => {
                let is_instance = doc.get(&id).map(|n| n.kind == NodeType::Instance).unwrap_or(false);
                let inherited: Map<String, Value> =
                    props.iter().filter(|(k, _)| INHERITED.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
                if is_instance && !inherited.is_empty() && !props.contains_key("overrides") {
                    if let Some(mut ov) = current(&pending, &id) {
                        merge_into(&mut ov, "", &inherited);
                        pending.insert(id.clone(), ov.clone());
                        props.insert("overrides".into(), Value::Object(ov));
                    }
                }
                out.push(Op::Update { id, props });
            }
            other => out.push(other),
        }
    }
    out
}

/// Re-expand instances affected by changes. Returns ids of derived nodes that
/// were (re)created plus the instances themselves, for layout and redraw.
pub fn sync_instances(doc: &mut Document, changed: &[NodeId], removed: &[NodeId]) -> Vec<NodeId> {
    // Cheap exit for documents without components.
    let all_instances: Vec<(NodeId, Option<NodeId>)> = doc
        .nodes()
        .filter(|n| n.kind == NodeType::Instance && !is_derived_id(&n.id))
        .map(|n| (n.id.clone(), n.component_id.clone()))
        .collect();
    if all_instances.is_empty() {
        return Vec::new();
    }
    // Components swapped in by overrides (instance swap) are used too.
    let swapped: Vec<(NodeId, Vec<NodeId>)> = all_instances
        .iter()
        .filter_map(|(iid, _)| {
            let used = swapped_components(doc.get(iid)?);
            (!used.is_empty()).then(|| (iid.clone(), used))
        })
        .collect();
    let mut comps: HashSet<NodeId> = HashSet::new();
    let mut targets: Vec<NodeId> = Vec::new();
    for id in changed {
        if is_derived_id(id) {
            continue;
        }
        let Some(n) = doc.get(id) else { continue };
        if n.kind == NodeType::Instance {
            targets.push(id.clone());
        }
        if n.kind == NodeType::Component {
            comps.insert(id.clone());
        }
        for a in doc.ancestors(id) {
            if doc.get(&a).map(|a| a.kind == NodeType::Component).unwrap_or(false) {
                comps.insert(a);
            }
        }
    }
    let removed: HashSet<&NodeId> = removed.iter().collect();
    // Components that contain instances of affected components are affected too.
    loop {
        let mut grew = false;
        for (iid, cid) in &all_instances {
            let hit = cid.as_ref().map(|c| comps.contains(c) || removed.contains(c)).unwrap_or(false);
            if hit {
                for a in doc.ancestors(iid) {
                    if doc.get(&a).map(|a| a.kind == NodeType::Component).unwrap_or(false) && comps.insert(a) {
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }
    for (iid, cid) in &all_instances {
        if cid.as_ref().map(|c| comps.contains(c) || removed.contains(c)).unwrap_or(false) {
            targets.push(iid.clone());
        }
    }
    for (iid, used) in &swapped {
        if used.iter().any(|c| comps.contains(c) || removed.contains(c)) {
            targets.push(iid.clone());
        }
    }
    if targets.is_empty() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    targets.retain(|t| seen.insert(t.clone()));
    // Instances nested inside components first, so outer expansions copy fresh state.
    let depth = |doc: &Document, id: &str| doc.ancestors(id).iter().filter(|a| doc.get(a).map(|n| n.kind == NodeType::Component).unwrap_or(false)).count();
    targets.sort_by_key(|t| std::cmp::Reverse(depth(doc, t)));
    let mut out = Vec::new();
    for t in targets {
        expand_instance(doc, &t, &mut out);
    }
    out
}

/// Components an instance's overrides swap in for nested instances.
fn swapped_components(inst: &Node) -> Vec<NodeId> {
    inst.overrides.values().filter_map(|o| o.get("componentId").and_then(Value::as_str).map(str::to_string)).collect()
}

/// Expand every instance (after loading a document).
pub fn sync_all(doc: &mut Document) -> Vec<NodeId> {
    let ids: Vec<NodeId> = doc
        .nodes()
        .filter(|n| n.kind == NodeType::Instance && !is_derived_id(&n.id))
        .map(|n| n.id.clone())
        .collect();
    sync_instances(doc, &ids, &[])
}

fn expand_instance(doc: &mut Document, iid: &str, out: &mut Vec<NodeId>) {
    let Some(inst) = doc.get(iid).cloned() else { return };
    for c in doc.children(iid).to_vec() {
        if is_derived_id(&c) {
            doc.remove_unchecked(&c);
        }
    }
    out.push(iid.to_string());
    let Some(cid) = inst.component_id.clone() else { return };
    let Some(comp) = doc.get(&cid).cloned() else { return };
    if comp.kind != NodeType::Component || doc.is_ancestor(&cid, iid) || cid == iid {
        return;
    }
    // Inherited visual/layout props follow the component unless overridden on the instance.
    let self_ov = inst.overrides.get("").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut next = inst.clone();
    for k in INHERITED {
        let v = self_ov.get(*k).cloned().or_else(|| comp.get_prop(k));
        if let Some(v) = v {
            let _ = next.set_prop(k, &v);
        }
    }
    let fit = Fit::new(&comp, &next);
    if next != inst {
        doc.replace_unchecked(next);
    }
    let mut stack = vec![cid.clone()];
    expand_children(doc, iid, &cid, &inst.overrides, fit, &mut stack, out);
}

/// How a derived container differs in size from its source, so its children
/// follow their constraints (see `constraints`).
#[derive(Clone, Copy)]
struct Fit {
    from: (f64, f64),
    to: (f64, f64),
    group: bool,
    auto: bool,
}

impl Fit {
    fn new(src: &Node, derived: &Node) -> Fit {
        Fit {
            from: (src.width, src.height),
            to: (derived.width, derived.height),
            group: derived.kind == NodeType::Group,
            auto: derived.kind.is_frame_like() && derived.layout_mode != crate::paint::LayoutMode::None,
        }
    }

    fn resized(&self) -> bool {
        (self.from.0 - self.to.0).abs() > 1e-9 || (self.from.1 - self.to.1).abs() > 1e-9
    }
}

fn expand_children(
    doc: &mut Document,
    base: &str,
    comp_id: &str,
    ov: &Map<String, Value>,
    fit: Fit,
    stack: &mut Vec<NodeId>,
    out: &mut Vec<NodeId>,
) {
    let kids: Vec<NodeId> = doc.children(comp_id).iter().filter(|c| !is_derived_id(c)).cloned().collect();
    for k in kids {
        clone_into(doc, &k, base, base, ov, fit, stack, out);
    }
}

/// Move and resize a derived node by its constraints when its parent is a
/// different size than in the component. Geometry overrides win.
fn follow_parent(n: &mut Node, src: &Node, o: Option<&Map<String, Value>>, fit: Fit) {
    if !fit.resized() || (fit.auto && !n.layout_absolute) {
        return;
    }
    let [x, y, w, h] = crate::constraints::constrain_rect([src.x, src.y, src.width, src.height], n.constraints, fit.from, fit.to, fit.group);
    let set = |k: &str| !o.map(|o| o.contains_key(k)).unwrap_or(false);
    if set("x") {
        n.x = x;
    }
    if set("y") {
        n.y = y;
    }
    if set("width") {
        n.width = w;
    }
    if set("height") && n.kind != NodeType::Line {
        n.height = h;
    }
    // Side effects only for the axes the constraints changed; overrides keep theirs.
    let dw = set("width") && (w - src.width).abs() > 1e-9;
    let dh = set("height") && n.kind != NodeType::Line && (h - src.height).abs() > 1e-9;
    match n.kind {
        // Same rules as resizing by ops, with the estimated height kept as derived state.
        NodeType::Text if !set("textAutoResize") => {}
        NodeType::Text if dh => n.text_auto_resize = crate::paint::TextAutoResize::None,
        NodeType::Text if dw && n.text_auto_resize != crate::paint::TextAutoResize::None => {
            n.text_auto_resize = crate::paint::TextAutoResize::Height;
            if set("height") {
                n.height = crate::text::estimate_layout(n).2;
            }
        }
        NodeType::Vector if (dw || dh) && set("pathData") && src.width > 0.0 && src.height > 0.0 => {
            let segs = crate::path::scale(&crate::path::parse(&n.path_data), n.width / src.width, n.height / src.height);
            n.path_data = crate::path::to_svg_d(&segs);
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn clone_into(
    doc: &mut Document,
    src_id: &str,
    parent: &str,
    base: &str,
    ov: &Map<String, Value>,
    fit: Fit,
    stack: &mut Vec<NodeId>,
    out: &mut Vec<NodeId>,
) {
    let Some(src) = doc.get(src_id).cloned() else { return };
    let new_id = format!("{base}~{src_id}");
    let mut n: Node = src.clone();
    n.id = new_id.clone();
    n.parent = Some(parent.to_string());
    let own = ov.get(src_id).and_then(Value::as_object);
    if let Some(o) = own {
        for (k, v) in o {
            if overridable(k) {
                let _ = n.set_prop(k, v);
            }
        }
    }
    follow_parent(&mut n, &src, own, fit);
    if src.kind == NodeType::Instance {
        // Nested instance: its own overrides, overlaid with the outer ones aimed inside it.
        let mut nested = src.overrides.clone();
        let prefix = format!("{src_id}~");
        for (k, v) in ov {
            if let (Some(rest), Some(obj)) = (k.strip_prefix(&prefix), v.as_object()) {
                merge_into(&mut nested, rest, obj);
            }
        }
        n.overrides = nested.clone();
        let derived = n.clone();
        doc.insert_unchecked(n);
        out.push(new_id.clone());
        if let Some(ncid) = derived.component_id.clone() {
            let comp = doc.get(&ncid).filter(|c| c.kind == NodeType::Component).cloned();
            if let Some(comp) = comp {
                if !stack.contains(&ncid) && stack.len() < MAX_NESTING {
                    stack.push(ncid.clone());
                    expand_children(doc, &new_id, &ncid, &nested, Fit::new(&comp, &derived), stack, out);
                    stack.pop();
                }
            }
        }
    } else {
        let fit = Fit::new(&src, &n);
        doc.insert_unchecked(n);
        out.push(new_id.clone());
        let kids: Vec<NodeId> = doc.children(src_id).iter().filter(|c| !is_derived_id(c)).cloned().collect();
        for k in kids {
            clone_into(doc, &k, &new_id, base, ov, fit, stack, out);
        }
    }
}

/// Ops that turn a FRAME (or GROUP) into a COMPONENT in place: a new component
/// node takes its props, position and children, and the original is deleted.
pub fn convert_to_component_ops(doc: &Document, id: &str, new_id: &str) -> Result<Vec<Op>, String> {
    let n = doc.get(id).ok_or_else(|| format!("node '{id}' not found"))?;
    if is_derived_id(id) {
        return Err("layers inside an instance can't become components".into());
    }
    if !matches!(n.kind, NodeType::Frame | NodeType::Group) {
        return Err(format!("only frames and groups can become components (got {})", n.kind.as_str()));
    }
    let mut comp = n.clone();
    comp.id = new_id.to_string();
    comp.kind = NodeType::Component;
    if n.kind == NodeType::Group {
        comp.fills = Vec::new();
        comp.clips_content = false;
    }
    let mut ops = vec![Op::Create { node: comp }];
    for c in doc.children(id) {
        if is_derived_id(c) {
            continue;
        }
        let mut p = Map::new();
        p.insert("parent".into(), Value::String(new_id.to_string()));
        ops.push(Op::Update { id: c.clone(), props: p });
    }
    ops.push(Op::Delete { id: id.to_string() });
    Ok(ops)
}

/// A new INSTANCE node of `comp_id` (sized like the component).
pub fn new_instance(doc: &Document, comp_id: &str, id: &str, parent: &str, index: String, x: f64, y: f64) -> Result<Node, String> {
    let comp = doc.get(comp_id).filter(|c| c.kind == NodeType::Component).ok_or("not a main component")?;
    let mut n = Node::new(id, NodeType::Instance);
    n.parent = Some(parent.to_string());
    n.index = index;
    n.name = comp.name.clone();
    n.component_id = Some(comp_id.to_string());
    n.width = comp.width;
    n.height = comp.height;
    n.x = x;
    n.y = y;
    n.layout_sizing_horizontal = comp.layout_sizing_horizontal;
    n.layout_sizing_vertical = comp.layout_sizing_vertical;
    for k in INHERITED {
        if let Some(v) = comp.get_prop(k) {
            let _ = n.set_prop(k, &v);
        }
    }
    Ok(n)
}

/// Ops that detach an instance: a FRAME with the instance's current look and
/// real copies of its (derived) contents replaces it. Returns (ops, frame id).
pub fn detach_ops(doc: &Document, inst_id: &str, new_id: &mut dyn FnMut() -> NodeId) -> Result<(Vec<Op>, NodeId), String> {
    let inst = doc.get(inst_id).filter(|n| n.kind == NodeType::Instance).ok_or("not an instance")?;
    if is_derived_id(inst_id) {
        return Err("detach the outer instance first".into());
    }
    let fid = new_id();
    let mut frame = inst.clone();
    frame.id = fid.clone();
    frame.kind = NodeType::Frame;
    frame.component_id = None;
    frame.overrides = Map::new();
    let mut ops = vec![Op::Create { node: frame }];
    fn copy(doc: &Document, src: &str, parent: &str, ops: &mut Vec<Op>, new_id: &mut dyn FnMut() -> NodeId) {
        for c in doc.children(src).to_vec() {
            let Some(n) = doc.get(&c) else { continue };
            let id = new_id();
            let mut m = n.clone();
            m.id = id.clone();
            m.parent = Some(parent.to_string());
            ops.push(Op::Create { node: m });
            // Nested instances stay instances (their contents are derived again).
            if n.kind != NodeType::Instance {
                copy(doc, &c, &id, ops, new_id);
            }
        }
    }
    copy(doc, inst_id, &fid, &mut ops, new_id);
    ops.push(Op::Delete { id: inst_id.to_string() });
    Ok((ops, fid))
}

/// Instances of a component (real ones).
pub fn instances_of(doc: &Document, comp_id: &str) -> Vec<NodeId> {
    doc.nodes()
        .filter(|n| n.kind == NodeType::Instance && !is_derived_id(&n.id) && n.component_id.as_deref() == Some(comp_id))
        .map(|n| n.id.clone())
        .collect()
}

/// The variant set a component belongs to: components with its name that
/// have variant values (just itself if it has none).
pub fn variant_set(doc: &Document, comp_id: &str) -> Vec<NodeId> {
    let Some(c) = doc.get(comp_id) else { return Vec::new() };
    if c.variant.is_empty() {
        return vec![comp_id.to_string()];
    }
    let mut set: Vec<NodeId> = Vec::new();
    doc.walk(crate::node::ROOT_ID, &mut |n| {
        if n.kind == NodeType::Component && n.name == c.name && !n.variant.is_empty() {
            set.push(n.id.clone());
        }
    });
    set
}

/// Each variant property of a component's set and the values it takes.
pub fn variant_options(doc: &Document, comp_id: &str) -> Map<String, Value> {
    let mut out: Map<String, Value> = Map::new();
    for id in variant_set(doc, comp_id) {
        let Some(c) = doc.get(&id) else { continue };
        for (k, v) in &c.variant {
            let list = out.entry(k.clone()).or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(a) = list {
                if !a.contains(v) {
                    a.push(v.clone());
                }
            }
        }
    }
    out
}

/// A variant's values as strings (`true`, `3`, `"Hover"` all compare as text).
pub fn variant_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The component in `comp_id`'s variant set with its values changed by
/// `wanted`: an exact match, or else the one matching every wanted value and
/// most of the others.
pub fn find_variant(doc: &Document, comp_id: &str, wanted: &Map<String, Value>) -> Option<NodeId> {
    let current = doc.get(comp_id)?.variant.clone();
    let mut target = current.clone();
    for (k, v) in wanted {
        target.insert(k.clone(), Value::String(variant_text(v)));
    }
    let same = |a: &Value, b: &Value| variant_text(a) == variant_text(b);
    variant_set(doc, comp_id)
        .into_iter()
        .filter_map(|id| {
            let c = doc.get(&id)?;
            if !wanted.iter().all(|(k, v)| c.variant.get(k).is_some_and(|cv| same(cv, v))) {
                return None;
            }
            let score = target.iter().filter(|(k, v)| c.variant.get(*k).is_some_and(|cv| same(cv, v))).count();
            Some((score, id))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, id)| id)
}

/// A layer's path of names from its component, to find "the same" layer in
/// another variant.
fn name_path(doc: &Document, comp_id: &str, id: &str) -> Vec<String> {
    let mut path = vec![doc.get(id).map(|n| n.name.clone()).unwrap_or_default()];
    for a in doc.ancestors(id) {
        if a == comp_id {
            break;
        }
        path.push(doc.get(&a).map(|n| n.name.clone()).unwrap_or_default());
    }
    path.reverse();
    path
}

/// The layer in component `to` matching layer `id` of another variant: same
/// `key`, else same path of names.
fn matching_layer(doc: &Document, from: &str, id: &str, to: &str) -> Option<NodeId> {
    let src = doc.get(id)?;
    let path = name_path(doc, from, id);
    let mut by_path = None;
    let mut by_key = None;
    doc.walk(to, &mut |n| {
        if n.id == to {
            return;
        }
        if by_key.is_none() && src.key.is_some() && n.key == src.key {
            by_key = Some(n.id.clone());
        }
        if by_path.is_none() && n.name == src.name && name_path(doc, to, &n.id) == path {
            by_path = Some(n.id.clone());
        }
    });
    by_key.or(by_path)
}

/// Overrides of an instance of `from` re-keyed for `to` (another variant):
/// each layer's overrides move to its match by key or name path; ones without
/// a match are dropped.
pub fn carry_overrides(doc: &Document, from: &str, to: &str, ov: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in ov {
        if k.is_empty() {
            out.insert(k.clone(), v.clone());
            continue;
        }
        let (head, rest) = match k.split_once('~') {
            Some((h, r)) => (h, Some(r)),
            None => (k.as_str(), None),
        };
        if let Some(new) = matching_layer(doc, from, head, to) {
            let key = match rest {
                Some(r) => format!("{new}~{r}"),
                None => new,
            };
            out.insert(key, v.clone());
        }
    }
    out
}

/// The update that switches an instance to another variant of its set (the
/// values in `wanted` changed), carrying its overrides over.
pub fn switch_variant_op(doc: &Document, inst_id: &str, wanted: &Map<String, Value>) -> Result<Op, String> {
    let inst = doc.get(inst_id).filter(|n| n.kind == NodeType::Instance).ok_or("not an instance")?;
    let from = inst.component_id.clone().ok_or("this instance has no main component")?;
    if doc.get(&from).is_some_and(|c| c.variant.is_empty()) {
        return Err(format!("'{}' has no variants", doc.get(&from).map(|c| c.name.as_str()).unwrap_or_default()));
    }
    let to = find_variant(doc, &from, wanted).ok_or_else(|| {
        let opts: Vec<String> = variant_options(doc, &from).iter().map(|(k, v)| format!("{k}: {v}")).collect();
        format!("no variant with those values (it has {})", opts.join(", "))
    })?;
    let mut props = Map::new();
    props.insert("componentId".into(), Value::String(to.clone()));
    props.insert("overrides".into(), Value::Object(carry_overrides(doc, &from, &to, &inst.overrides)));
    Ok(Op::Update { id: inst_id.to_string(), props })
}

/// The overrides that set component properties (`{"Show icon": false,
/// "Label": "Buy"}`) on an instance of `comp_id`, by layer.
pub fn property_overrides(doc: &Document, comp_id: &str, values: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let comp = doc.get(comp_id).ok_or("no such component")?;
    let mut out: Map<String, Value> = Map::new();
    for (name, value) in values {
        let def = comp.component_properties.get(name).and_then(Value::as_object).ok_or_else(|| {
            let names: Vec<&String> = comp.component_properties.keys().collect();
            format!("'{}' has no property '{name}' (it has {names:?})", comp.name)
        })?;
        let layer = def.get("layer").and_then(Value::as_str).unwrap_or_default().to_string();
        let ty = def.get("type").and_then(Value::as_str).unwrap_or("BOOLEAN").to_ascii_uppercase();
        let mut o = Map::new();
        match ty.as_str() {
            "BOOLEAN" => {
                o.insert("visible".into(), Value::Bool(value.as_bool().unwrap_or_else(|| variant_text(value) == "true")));
            }
            "TEXT" => {
                o.insert("characters".into(), Value::String(variant_text(value)));
                o.insert("needsMeasure".into(), Value::Bool(true));
            }
            "INSTANCE_SWAP" => {
                o.insert("componentId".into(), value.clone());
            }
            other => return Err(format!("property '{name}' has an unknown type {other}")),
        }
        let entry = out.entry(layer).or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(e) = entry {
            e.extend(o);
        }
    }
    Ok(out)
}

/// An instance's component properties with their current values.
pub fn property_values(doc: &Document, inst: &Node) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(comp) = inst.component_id.as_deref().and_then(|c| doc.get(c)) else { return out };
    for (name, def) in &comp.component_properties {
        let Some(def) = def.as_object() else { continue };
        let layer = def.get("layer").and_then(Value::as_str).unwrap_or_default();
        let ty = def.get("type").and_then(Value::as_str).unwrap_or("BOOLEAN").to_ascii_uppercase();
        let key = match ty.as_str() {
            "BOOLEAN" => "visible",
            "TEXT" => "characters",
            _ => "componentId",
        };
        let current = inst
            .overrides
            .get(layer)
            .and_then(|o| o.get(key))
            .cloned()
            .or_else(|| doc.get(layer).and_then(|n| n.get_prop(key)))
            .unwrap_or(Value::Null);
        let mut d = def.clone();
        d.insert("value".into(), current);
        out.insert(name.clone(), Value::Object(d));
    }
    out
}

/// Find a component descendant by id or (unique-ish) name, for friendly override keys.
pub fn resolve_override_key(doc: &Document, component_id: &str, key: &str) -> Option<String> {
    if key.is_empty() {
        return Some(String::new());
    }
    if doc.get(key).is_some() && doc.is_ancestor(component_id, key) {
        return Some(key.to_string());
    }
    // A layer's `key` (stable ids from an export) is unique where names repeat,
    // including layers inside nested instances (their override keys are `N~S`).
    let mut stack: Vec<NodeId> = doc.children(component_id).to_vec();
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(&id) else { continue };
        if n.key.as_deref() == Some(key) {
            return Some(id);
        }
        stack.extend(doc.children(&id).iter().cloned());
    }
    let mut found = None;
    doc.walk(component_id, &mut |n| {
        if found.is_none() && n.id != component_id && n.name == key {
            found = Some(n.id.clone());
        }
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;
    use serde_json::json;

    fn node(id: &str, kind: NodeType, parent: &str) -> Node {
        let mut n = Node::new(id, kind);
        n.parent = Some(parent.into());
        n.width = 100.0;
        n.height = 40.0;
        n
    }

    fn setup() -> Document {
        let mut doc = Document::new_empty();
        let mut c = node("c:1", NodeType::Component, "0:1");
        c.name = "Button".into();
        c.fills = vec![crate::Paint::solid(crate::Color::rgba(0.0, 0.0, 1.0, 1.0))];
        doc.apply(&Op::Create { node: c }).unwrap();
        let mut t = node("c:2", NodeType::Text, "c:1");
        t.name = "Label".into();
        t.characters = "Buy".into();
        doc.apply(&Op::Create { node: t }).unwrap();
        let mut i = node("i:1", NodeType::Instance, "0:1");
        i.component_id = Some("c:1".into());
        i.x = 300.0;
        doc.apply(&Op::Create { node: i }).unwrap();
        sync_all(&mut doc);
        doc
    }

    #[test]
    fn expands_and_inherits() {
        let doc = setup();
        let d = doc.get("i:1~c:2").expect("derived label");
        assert_eq!(d.characters, "Buy");
        assert_eq!(d.parent.as_deref(), Some("i:1"));
        assert_eq!(doc.get("i:1").unwrap().fills, doc.get("c:1").unwrap().fills);
        // Derived nodes are never serialized.
        assert!(!doc.to_json().contains("i:1~c:2"));
    }

    #[test]
    fn edits_inside_instances_become_overrides() {
        let mut doc = setup();
        let ops = route_ops(&doc, vec![Op::update1("i:1~c:2", "characters", json!("Sell"))]);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].target(), "i:1");
        let (eff, errs) = doc.apply_all(&ops);
        assert!(errs.is_empty());
        sync_instances(&mut doc, &eff.changed, &eff.removed);
        assert_eq!(doc.get("i:1~c:2").unwrap().characters, "Sell");
        // Main component changes flow to instances, overrides win.
        let (eff, _) = doc.apply_all(&[Op::update1("c:2", "fontSize", json!(20)), Op::update1("c:2", "characters", json!("Pay"))]);
        sync_instances(&mut doc, &eff.changed, &eff.removed);
        let d = doc.get("i:1~c:2").unwrap();
        assert_eq!(d.font_size, 20.0);
        assert_eq!(d.characters, "Sell");
    }

    #[test]
    fn instance_self_overrides_survive() {
        let mut doc = setup();
        let ops = route_ops(&doc, vec![Op::update1("i:1", "fills", json!([{ "type": "SOLID", "color": "#ff0000" }]))]);
        let (eff, _) = doc.apply_all(&ops);
        sync_instances(&mut doc, &eff.changed, &eff.removed);
        let (eff, _) = doc.apply_all(&[Op::update1("c:1", "cornerRadius", json!(8))]);
        sync_instances(&mut doc, &eff.changed, &eff.removed);
        let i = doc.get("i:1").unwrap();
        assert_eq!(i.corner_radius, 8.0);
        assert_eq!(i.fills[0].approx_color().to_hex(), "#ff0000");
    }

    #[test]
    fn nested_instances_and_deletes() {
        let mut doc = setup();
        // Card component containing a Button instance.
        doc.apply(&Op::Create { node: node("k:1", NodeType::Component, "0:1") }).unwrap();
        let mut nb = node("k:2", NodeType::Instance, "k:1");
        nb.component_id = Some("c:1".into());
        doc.apply(&Op::Create { node: nb }).unwrap();
        let mut card = node("j:1", NodeType::Instance, "0:1");
        card.component_id = Some("k:1".into());
        doc.apply(&Op::Create { node: card }).unwrap();
        sync_all(&mut doc);
        assert_eq!(doc.get("j:1~k:2~c:2").unwrap().characters, "Buy");
        // Override deep inside, then hide it via delete.
        let ops = route_ops(&doc, vec![Op::update1("j:1~k:2~c:2", "characters", json!("Go"))]);
        let (eff, _) = doc.apply_all(&ops);
        sync_instances(&mut doc, &eff.changed, &eff.removed);
        assert_eq!(doc.get("j:1~k:2~c:2").unwrap().characters, "Go");
        let ops = route_ops(&doc, vec![Op::Delete { id: "j:1~k:2~c:2".into() }]);
        let (eff, _) = doc.apply_all(&ops);
        sync_instances(&mut doc, &eff.changed, &eff.removed);
        assert!(!doc.get("j:1~k:2~c:2").unwrap().visible);
        // Changing the innermost component reaches through both levels.
        let (eff, _) = doc.apply_all(&[Op::update1("c:2", "fontWeight", json!(700))]);
        sync_instances(&mut doc, &eff.changed, &eff.removed);
        assert_eq!(doc.get("j:1~k:2~c:2").unwrap().font_weight, 700);
        doc.check_consistency().unwrap();
    }

    #[test]
    fn rejects_direct_derived_ops_and_children() {
        let mut doc = setup();
        assert!(doc.apply(&Op::update1("i:1~c:2", "x", json!(1))).is_err());
        assert!(doc.apply(&Op::Create { node: node("z:1", NodeType::Rectangle, "i:1") }).is_err());
        let mut bad = node("z~1", NodeType::Rectangle, "0:1");
        bad.id = "z~1".into();
        assert!(doc.apply(&Op::Create { node: bad }).is_err());
    }

    #[test]
    fn convert_and_detach() {
        let mut doc = Document::new_empty();
        doc.apply(&Op::Create { node: node("f:1", NodeType::Frame, "0:1") }).unwrap();
        doc.apply(&Op::Create { node: node("f:2", NodeType::Text, "f:1") }).unwrap();
        let ops = convert_to_component_ops(&doc, "f:1", "c:9").unwrap();
        let (_, errs) = doc.apply_all(&ops);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(doc.get("c:9").unwrap().kind, NodeType::Component);
        assert_eq!(doc.parent_of("f:2"), Some("c:9"));
        let inst = new_instance(&doc, "c:9", "i:9", "0:1", doc.index_at_end("0:1"), 500.0, 0.0).unwrap();
        doc.apply(&Op::Create { node: inst }).unwrap();
        sync_all(&mut doc);
        assert!(doc.get("i:9~f:2").is_some());
        let mut n = 0;
        let mut gen = || {
            n += 1;
            format!("d:{n}")
        };
        let (ops, fid) = detach_ops(&doc, "i:9", &mut gen).unwrap();
        let (eff, errs) = doc.apply_all(&ops);
        assert!(errs.is_empty(), "{errs:?}");
        sync_instances(&mut doc, &eff.changed, &eff.removed);
        assert_eq!(doc.get(&fid).unwrap().kind, NodeType::Frame);
        assert_eq!(doc.children(&fid).len(), 1);
        assert!(doc.get("i:9").is_none());
        doc.check_consistency().unwrap();
    }

    #[test]
    fn self_reference_is_safe() {
        let mut doc = setup();
        // Put an instance of the Button inside the Button component itself.
        let mut loop_i = node("l:1", NodeType::Instance, "c:1");
        loop_i.component_id = Some("c:1".into());
        doc.apply(&Op::Create { node: loop_i }).unwrap();
        sync_all(&mut doc);
        layout::relayout_all(&mut doc);
        assert!(doc.len() < 50);
    }
}
