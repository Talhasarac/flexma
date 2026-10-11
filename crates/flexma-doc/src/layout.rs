//! Auto layout (flexbox-like frames). Layout is a pure function of the document,
//! so every replica (server and each client) derives the same child positions
//! locally after applying ops — layout results never travel as ops.

use crate::doc::Document;
use crate::node::{NodeId, NodeType};
use crate::paint::{LayoutAlign, LayoutMode, SizingMode};
use std::collections::HashSet;

fn is_auto(doc: &Document, id: &str) -> bool {
    doc.get(id)
        .map(|n| n.kind == NodeType::Frame && n.layout_mode != LayoutMode::None)
        .unwrap_or(false)
}

/// Re-run auto layout for everything affected by changes to `changed` nodes.
/// Returns ids of nodes whose geometry was changed by layout.
pub fn relayout(doc: &mut Document, changed: &[NodeId]) -> Vec<NodeId> {
    let mut roots: Vec<NodeId> = Vec::new();
    let mut seen = HashSet::new();
    for id in changed {
        if !doc.contains(id) {
            continue;
        }
        // Climb while the parent is an auto-layout frame: its hug size and the
        // positions of our siblings depend on us.
        let mut top: Option<NodeId> = if is_auto(doc, id) { Some(id.clone()) } else { None };
        let mut cur = id.clone();
        while let Some(p) = doc.parent_of(&cur).map(str::to_string) {
            if is_auto(doc, &p) {
                top = Some(p.clone());
                cur = p;
            } else {
                break;
            }
        }
        if let Some(t) = top {
            if seen.insert(t.clone()) {
                roots.push(t);
            }
        }
    }
    let mut out = Vec::new();
    for r in roots {
        layout_subtree(doc, &r, &mut out);
    }
    out
}

/// Lay out every auto-layout frame in the document.
pub fn relayout_all(doc: &mut Document) -> Vec<NodeId> {
    let mut frames = Vec::new();
    doc.walk(crate::node::ROOT_ID, &mut |n| {
        if n.kind == NodeType::Frame && n.layout_mode != LayoutMode::None {
            frames.push(n.id.clone());
        }
    });
    let ids: Vec<NodeId> = frames;
    relayout(doc, &ids)
}

fn layout_subtree(doc: &mut Document, root: &str, out: &mut Vec<NodeId>) {
    // Two passes: FILL widths assigned in `arrange` can change wrapped text
    // heights, which feed back into HUG sizes.
    for _ in 0..2 {
        measure_hug(doc, root, out);
        arrange(doc, root, out);
    }
    out.sort();
    out.dedup();
}

fn flow_children(doc: &Document, id: &str) -> Vec<NodeId> {
    doc.children(id)
        .iter()
        .filter(|c| doc.get(c).map(|n| n.visible && !n.layout_absolute).unwrap_or(false))
        .cloned()
        .collect()
}

/// Bottom-up: compute HUG sizes.
fn measure_hug(doc: &mut Document, id: &str, out: &mut Vec<NodeId>) {
    let kids: Vec<NodeId> = doc.children(id).to_vec();
    for k in &kids {
        measure_hug(doc, k, out);
    }
    if !is_auto(doc, id) {
        return;
    }
    let n = doc.get(id).unwrap().clone();
    let horizontal = n.layout_mode == LayoutMode::Horizontal;
    let flow = flow_children(doc, id);
    let mut main = 0.0f64;
    let mut cross = 0.0f64;
    let mut cross_all = 0.0f64;
    for (i, c) in flow.iter().enumerate() {
        let cn = doc.get(c).unwrap();
        let (cm, cc) = if horizontal { (cn.width, cn.height) } else { (cn.height, cn.width) };
        main += cm;
        if i > 0 {
            main += n.item_spacing;
        }
        // Children that FILL the cross axis take the hugged size instead of driving it.
        if !child_fill(cn, horizontal).1 {
            cross = cross.max(cc);
        }
        cross_all = cross_all.max(cc);
    }
    if cross == 0.0 {
        cross = cross_all;
    }
    let pad_main = if horizontal { n.padding.left + n.padding.right } else { n.padding.top + n.padding.bottom };
    let pad_cross = if horizontal { n.padding.top + n.padding.bottom } else { n.padding.left + n.padding.right };
    let (mut w, mut h) = (n.width, n.height);
    let hug_w = n.layout_sizing_horizontal == SizingMode::Hug;
    let hug_h = n.layout_sizing_vertical == SizingMode::Hug;
    if horizontal {
        if hug_w {
            w = main + pad_main;
        }
        if hug_h {
            h = cross + pad_cross;
        }
    } else {
        if hug_h {
            h = main + pad_main;
        }
        if hug_w {
            w = cross + pad_cross;
        }
    }
    if doc.set_geometry(id, n.x, n.y, w.max(0.0), h.max(0.0)) {
        out.push(id.to_string());
    }
}

/// Top-down: size FILL children and position all flow children.
fn arrange(doc: &mut Document, id: &str, out: &mut Vec<NodeId>) {
    if is_auto(doc, id) {
        let n = doc.get(id).unwrap().clone();
        let horizontal = n.layout_mode == LayoutMode::Horizontal;
        let flow = flow_children(doc, id);
        let inner_w = (n.width - n.padding.left - n.padding.right).max(0.0);
        let inner_h = (n.height - n.padding.top - n.padding.bottom).max(0.0);
        let (inner_main, inner_cross) = if horizontal { (inner_w, inner_h) } else { (inner_h, inner_w) };
        let parent_hugs_main = if horizontal {
            n.layout_sizing_horizontal == SizingMode::Hug
        } else {
            n.layout_sizing_vertical == SizingMode::Hug
        };

        // Sizes along both axes after FILL resolution.
        let mut sizes: Vec<(f64, f64)> = Vec::with_capacity(flow.len());
        let mut fixed_main = 0.0;
        let mut fill_count = 0usize;
        for c in &flow {
            let cn = doc.get(c).unwrap();
            let (fill_main, _) = child_fill(cn, horizontal);
            let cm = if horizontal { cn.width } else { cn.height };
            if fill_main && !parent_hugs_main {
                fill_count += 1;
            } else {
                fixed_main += cm;
            }
        }
        let spacing_total = if flow.len() > 1 { n.item_spacing * (flow.len() - 1) as f64 } else { 0.0 };
        let fill_each = if fill_count > 0 {
            ((inner_main - fixed_main - spacing_total) / fill_count as f64).max(0.0)
        } else {
            0.0
        };
        for c in &flow {
            let cn = doc.get(c).unwrap();
            let (fill_main, fill_cross) = child_fill(cn, horizontal);
            let mut cm = if horizontal { cn.width } else { cn.height };
            let mut cc = if horizontal { cn.height } else { cn.width };
            if fill_main && !parent_hugs_main {
                cm = fill_each;
            }
            if fill_cross {
                cc = inner_cross;
            }
            sizes.push((cm, cc));
        }
        let used: f64 = sizes.iter().map(|s| s.0).sum::<f64>() + spacing_total;
        let free = (inner_main - used).max(0.0);
        let (mut cursor, gap) = match n.primary_axis_align {
            LayoutAlign::Start => (0.0, n.item_spacing),
            LayoutAlign::Center => (free / 2.0, n.item_spacing),
            LayoutAlign::End => (free, n.item_spacing),
            LayoutAlign::SpaceBetween => {
                if flow.len() > 1 {
                    (0.0, n.item_spacing + free / (flow.len() - 1) as f64)
                } else {
                    (0.0, n.item_spacing)
                }
            }
        };
        let start_main = if horizontal { n.padding.left } else { n.padding.top };
        let start_cross = if horizontal { n.padding.top } else { n.padding.left };
        for (c, (cm, cc)) in flow.iter().zip(sizes) {
            let cross_off = match n.counter_axis_align {
                LayoutAlign::Start | LayoutAlign::SpaceBetween => 0.0,
                LayoutAlign::Center => (inner_cross - cc) / 2.0,
                LayoutAlign::End => inner_cross - cc,
            };
            let (x, y, w, h) = if horizontal {
                (start_main + cursor, start_cross + cross_off, cm, cc)
            } else {
                (start_cross + cross_off, start_main + cursor, cc, cm)
            };
            let mut h = h.max(0.0);
            let w = w.max(0.0);
            if let Some(cn) = doc.get(c) {
                // Text whose box is still an estimate gets re-estimated at its new width.
                if cn.kind == NodeType::Text
                    && cn.needs_measure
                    && cn.text_auto_resize == crate::paint::TextAutoResize::Height
                    && cn.width != w
                {
                    let mut probe = cn.clone();
                    probe.width = w;
                    h = crate::text::estimate_layout(&probe).2;
                }
            }
            if doc.set_geometry(c, x, y, w, h) {
                out.push(c.clone());
            }
            cursor += cm + gap;
        }
    }
    let kids: Vec<NodeId> = doc.children(id).to_vec();
    for k in kids {
        arrange(doc, &k, out);
    }
}

fn child_fill(cn: &crate::node::Node, parent_horizontal: bool) -> (bool, bool) {
    let fw = cn.layout_sizing_horizontal == SizingMode::Fill;
    let fh = cn.layout_sizing_vertical == SizingMode::Fill;
    if parent_horizontal {
        (fw, fh)
    } else {
        (fh, fw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Node;
    use crate::ops::Op;
    use crate::paint::Padding;

    #[test]
    fn horizontal_hug_and_fill() {
        let mut doc = Document::new_empty();
        let mut f = Node::new("f:1", NodeType::Frame);
        f.parent = Some("0:1".into());
        f.layout_mode = LayoutMode::Horizontal;
        f.item_spacing = 10.0;
        f.padding = Padding::uniform(5.0);
        f.layout_sizing_horizontal = SizingMode::Hug;
        f.layout_sizing_vertical = SizingMode::Hug;
        doc.apply(&Op::Create { node: f }).unwrap();
        for (i, w) in [20.0, 30.0].iter().enumerate() {
            let mut r = Node::new(format!("r:{i}"), NodeType::Rectangle);
            r.parent = Some("f:1".into());
            r.index = doc.index_at_end("f:1");
            r.width = *w;
            r.height = 10.0 + i as f64 * 10.0;
            doc.apply(&Op::Create { node: r }).unwrap();
        }
        relayout_all(&mut doc);
        let f = doc.get("f:1").unwrap();
        assert_eq!(f.width, 5.0 + 20.0 + 10.0 + 30.0 + 5.0);
        assert_eq!(f.height, 5.0 + 20.0 + 5.0);
        let r1 = doc.get("r:1").unwrap();
        assert_eq!(r1.x, 5.0 + 20.0 + 10.0);
        assert_eq!(r1.y, 5.0);

        // Fixed width frame with a FILL child.
        doc.apply(&Op::update1("f:1", "layoutSizingHorizontal", serde_json::json!("FIXED"))).unwrap();
        doc.apply(&Op::update1("f:1", "width", serde_json::json!(200))).unwrap();
        doc.apply(&Op::update1("r:0", "layoutSizingHorizontal", serde_json::json!("FILL"))).unwrap();
        relayout(&mut doc, &["r:0".to_string()]);
        let r0 = doc.get("r:0").unwrap();
        assert_eq!(r0.width, 200.0 - 10.0 - 10.0 - 30.0);
    }
}
