//! Shared color and text styles. A STYLE node (a child of the document root)
//! holds the values; layers point at it with `fillStyleId`, `strokeStyleId`
//! or `textStyleId`. Like instance children, the applied values are derived
//! state: every replica copies them in locally after applying ops, so changing
//! a style updates everything that uses it without sending per-layer ops.
//!
//! Editing a styled property directly (say, a layer's fills) detaches that
//! style, like in Figma; `route_ops` adds the detach to the edit.

use crate::doc::Document;
use crate::node::{Node, NodeId, NodeType};
use crate::ops::Op;
use crate::paint::StyleType;
use serde_json::Value;
use std::collections::HashSet;

/// Props a TEXT style controls.
pub const TEXT_PROPS: &[&str] = &["fontFamily", "fontSize", "fontWeight", "italic", "lineHeight", "letterSpacing", "textDecoration"];

/// Which style ref a prop belongs to (editing the prop detaches the style).
fn ref_for(prop: &str) -> Option<&'static str> {
    match prop {
        "fills" => Some("fillStyleId"),
        "strokes" => Some("strokeStyleId"),
        p if TEXT_PROPS.contains(&p) => Some("textStyleId"),
        _ => None,
    }
}

fn uses_styles(n: &Node) -> bool {
    n.fill_style_id.is_some() || n.stroke_style_id.is_some() || n.text_style_id.is_some()
}

/// Add style detaches to direct edits of styled props.
pub fn route_ops(doc: &Document, ops: Vec<Op>) -> Vec<Op> {
    ops.into_iter()
        .map(|op| match op {
            Op::Update { id, mut props } => {
                if let Some(n) = doc.get(&id).filter(|n| uses_styles(n) && n.kind != NodeType::Style) {
                    let edited: Vec<&'static str> = props.keys().filter_map(|k| ref_for(k)).collect();
                    for r in edited {
                        let attached = match r {
                            "fillStyleId" => n.fill_style_id.is_some(),
                            "strokeStyleId" => n.stroke_style_id.is_some(),
                            _ => n.text_style_id.is_some(),
                        };
                        if attached && !props.contains_key(r) {
                            props.insert(r.to_string(), Value::Null);
                        }
                    }
                }
                Op::Update { id, props }
            }
            other => other,
        })
        .collect()
}

fn style<'a>(doc: &'a Document, id: &Option<NodeId>, ty: StyleType) -> Option<&'a Node> {
    id.as_ref().and_then(|s| doc.get(s)).filter(|s| s.kind == NodeType::Style && s.style_type == ty)
}

/// Copy a layer's style values in. Returns true if anything changed.
fn resolve(doc: &mut Document, id: &str) -> bool {
    let Some(node) = doc.get(id).cloned() else { return false };
    let mut next = node.clone();
    if let Some(s) = style(doc, &node.fill_style_id, StyleType::Fill) {
        next.fills = s.fills.clone();
    }
    if let Some(s) = style(doc, &node.stroke_style_id, StyleType::Fill) {
        next.strokes = s.fills.clone();
    }
    if node.kind == NodeType::Text {
        if let Some(s) = style(doc, &node.text_style_id, StyleType::Text) {
            for key in TEXT_PROPS {
                if let Some(v) = s.get_prop(key) {
                    let _ = next.set_prop(key, &v);
                }
            }
            // A new font or size needs the box re-measured: estimate it now
            // (browsers then measure exactly, see `needs_measure`).
            let text_changed = TEXT_PROPS.iter().any(|k| node.get_prop(k) != next.get_prop(k));
            if text_changed {
                crate::text::fit_box(&mut next);
            }
        }
    }
    if serde_json::to_value(&next).ok() == serde_json::to_value(&node).ok() {
        return false;
    }
    doc.replace_unchecked(next);
    true
}

/// Re-apply styles after ops changed `changed` (layers or styles). Returns
/// the layers whose values changed.
pub fn sync_styles(doc: &mut Document, changed: &[NodeId]) -> Vec<NodeId> {
    let changed_styles: HashSet<&str> = changed
        .iter()
        .filter(|id| doc.get(id).map(|n| n.kind == NodeType::Style).unwrap_or(false))
        .map(String::as_str)
        .collect();
    let mut targets: Vec<NodeId> = changed.iter().filter(|id| doc.get(id).map(uses_styles).unwrap_or(false)).cloned().collect();
    if !changed_styles.is_empty() {
        let refers = |r: &Option<NodeId>| r.as_deref().map(|s| changed_styles.contains(s)).unwrap_or(false);
        targets.extend(
            doc.nodes()
                .filter(|n| refers(&n.fill_style_id) || refers(&n.stroke_style_id) || refers(&n.text_style_id))
                .map(|n| n.id.clone()),
        );
    }
    targets.sort();
    targets.dedup();
    targets.into_iter().filter(|id| resolve(doc, id)).collect()
}

/// Apply every style (after loading a document).
pub fn sync_all(doc: &mut Document) {
    let ids: Vec<NodeId> = doc.nodes().filter(|n| uses_styles(n)).map(|n| n.id.clone()).collect();
    for id in ids {
        resolve(doc, &id);
    }
}

/// Layers using a style.
pub fn uses_of(doc: &Document, style_id: &str) -> usize {
    let is = |r: &Option<NodeId>| r.as_deref() == Some(style_id);
    doc.nodes()
        .filter(|n| !crate::node::is_derived_id(&n.id) && (is(&n.fill_style_id) || is(&n.stroke_style_id) || is(&n.text_style_id)))
        .count()
}

/// Find a style by id or name (case-insensitive).
pub fn find(doc: &Document, key: &str) -> Option<NodeId> {
    let key = key.trim();
    doc.styles().into_iter().find(|id| {
        id == key || doc.get(id).map(|s| s.name.eq_ignore_ascii_case(key)).unwrap_or(false)
    })
}

/// Turn friendly `fillStyle` / `strokeStyle` / `textStyle` (a style's name
/// or id, or null to detach) into style ref props.
pub fn resolve_refs(doc: &Document, props: &mut serde_json::Map<String, Value>) -> Result<(), String> {
    for (friendly, canonical, ty) in [
        ("fillStyle", "fillStyleId", StyleType::Fill),
        ("strokeStyle", "strokeStyleId", StyleType::Fill),
        ("textStyle", "textStyleId", StyleType::Text),
    ] {
        let Some(v) = props.remove(friendly) else { continue };
        let id = match &v {
            Value::Null => Value::Null,
            Value::String(key) => {
                let id = find(doc, key).ok_or_else(|| format!("no style named '{key}' in this file (see list_styles)"))?;
                let st = doc.get(&id).map(|n| n.style_type).unwrap_or_default();
                if st != ty {
                    return Err(format!("'{key}' is a {} style; {friendly} needs a {} style", kind_name(st), kind_name(ty)));
                }
                Value::String(id)
            }
            other => return Err(format!("{friendly} must be a style name or id, not {other}")),
        };
        props.insert(canonical.to_string(), id);
    }
    Ok(())
}

pub fn kind_name(t: StyleType) -> &'static str {
    match t {
        StyleType::Fill => "color",
        StyleType::Text => "text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::paint::Paint;
    use serde_json::json;

    fn setup() -> Document {
        let mut doc = Document::new_empty();
        let mut brand = Node::new("s:1", NodeType::Style);
        brand.parent = Some(crate::ROOT_ID.into());
        brand.name = "Brand/Primary".into();
        brand.fills = vec![Paint::solid(Color::parse("#6d5dfc").unwrap())];
        doc.apply(&Op::Create { node: brand }).unwrap();
        let mut h1 = Node::new("s:2", NodeType::Style);
        h1.parent = Some(crate::ROOT_ID.into());
        h1.style_type = StyleType::Text;
        h1.font_size = 40.0;
        h1.font_weight = 800;
        doc.apply(&Op::Create { node: h1 }).unwrap();
        let mut card = Node::new("r:1", NodeType::Rectangle);
        card.parent = Some("0:1".into());
        card.fill_style_id = Some("s:1".into());
        doc.apply(&Op::Create { node: card }).unwrap();
        let mut title = Node::new("t:1", NodeType::Text);
        title.parent = Some("0:1".into());
        title.characters = "Hi".into();
        title.text_style_id = Some("s:2".into());
        doc.apply(&Op::Create { node: title }).unwrap();
        sync_all(&mut doc);
        doc
    }

    #[test]
    fn styles_apply_and_update_everywhere() {
        let mut doc = setup();
        assert_eq!(doc.pages(), vec!["0:1".to_string()], "styles aren't pages");
        assert_eq!(doc.get("r:1").unwrap().fills, doc.get("s:1").unwrap().fills);
        assert_eq!((doc.get("t:1").unwrap().font_size, doc.get("t:1").unwrap().font_weight), (40.0, 800));
        // Change the style: every user follows.
        let op = Op::update1("s:1", "fills", json!([{ "type": "SOLID", "color": "#ff0000" }]));
        let eff = doc.apply(&op).unwrap();
        let synced = sync_styles(&mut doc, &eff.changed);
        assert_eq!(synced, vec!["r:1".to_string()]);
        assert_eq!(doc.get("r:1").unwrap().fills, doc.get("s:1").unwrap().fills);
        let eff = doc.apply(&Op::update1("s:2", "fontSize", json!(32))).unwrap();
        sync_styles(&mut doc, &eff.changed);
        assert!(doc.get("t:1").unwrap().needs_measure, "text re-measures after a style change");
        assert_eq!(uses_of(&doc, "s:1"), 1);
        assert_eq!(find(&doc, "brand/primary").as_deref(), Some("s:1"));
    }

    #[test]
    fn editing_a_styled_prop_detaches_it() {
        let doc = setup();
        let ops = route_ops(&doc, vec![Op::update1("r:1", "fills", json!([])), Op::update1("t:1", "fontSize", json!(12))]);
        match &ops[..] {
            [Op::Update { props: a, .. }, Op::Update { props: b, .. }] => {
                assert_eq!(a.get("fillStyleId"), Some(&Value::Null));
                assert_eq!(b.get("textStyleId"), Some(&Value::Null));
            }
            other => panic!("{other:?}"),
        }
        // Setting the style ref itself isn't a detach.
        let ops = route_ops(&doc, vec![Op::update1("r:1", "fillStyleId", json!("s:1"))]);
        assert!(matches!(&ops[0], Op::Update { props, .. } if props.len() == 1));
    }
}
