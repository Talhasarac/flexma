//! Auto layout (flexbox-like frames). Layout is a pure function of the document,
//! so every replica (server and each client) derives the same child positions
//! locally after applying ops — layout results never travel as ops.

use crate::doc::Document;
use crate::node::{NodeId, NodeType};
use crate::node::Node;
use crate::paint::{GridCorner, LayoutAlign, LayoutMode, SizingMode};
use std::collections::HashSet;

fn is_auto(doc: &Document, id: &str) -> bool {
    doc.get(id)
        .map(|n| n.kind.is_frame_like() && n.layout_mode != LayoutMode::None)
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
        if n.kind.is_frame_like() && n.layout_mode != LayoutMode::None {
            frames.push(n.id.clone());
        }
    });
    let ids: Vec<NodeId> = frames;
    relayout(doc, &ids)
}

fn layout_subtree(doc: &mut Document, root: &str, out: &mut Vec<NodeId>) {
    // Frame sizes before layout, top-down.
    let mut before: Vec<(NodeId, f64, f64)> = Vec::new();
    doc.walk(root, &mut |n| {
        if n.kind.is_frame_like() {
            before.push((n.id.clone(), n.width, n.height));
        }
    });
    // Two passes: FILL widths assigned in `arrange` can change wrapped text
    // heights, which feed back into HUG sizes.
    for _ in 0..2 {
        measure_hug(doc, root, out);
        arrange(doc, root, out);
    }
    // Children layout doesn't place (absolute ones, and those of frames
    // without auto layout) follow their constraints when layout resizes their
    // frame, as when it's resized by hand, and like Unity anchors. A frame
    // created in this batch isn't resized: its first size is its size.
    for (id, w0, h0) in before {
        let resized = !doc.is_fresh(&id) && doc.get(&id).is_some_and(|n| (n.width - w0).abs() > 1e-9 || (n.height - h0).abs() > 1e-9);
        if resized {
            out.extend(crate::constraints::follow_layout_resize(doc, &id, w0, h0));
        }
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
    if n.layout_mode == LayoutMode::Grid {
        let g = grid_geom(doc, &n, &flow);
        let (bw, bh) = g.block();
        let w = if n.layout_sizing_horizontal == SizingMode::Hug { bw + n.padding.left + n.padding.right } else { n.width };
        let h = if n.layout_sizing_vertical == SizingMode::Hug { bh + n.padding.top + n.padding.bottom } else { n.height };
        if doc.set_geometry(id, n.x, n.y, w.max(0.0), h.max(0.0)) {
            out.push(id.to_string());
        }
        return;
    }
    if wraps(&n) {
        // The main axis is fixed; the cross axis may hug the lines.
        let lines = wrap_lines(doc, &n, &flow);
        let cross: f64 = lines.iter().map(|l| l.cross).sum::<f64>() + cross_gap(&n) * lines.len().saturating_sub(1) as f64;
        let (mut w, mut h) = (n.width, n.height);
        if horizontal && n.layout_sizing_vertical == SizingMode::Hug {
            h = cross + n.padding.top + n.padding.bottom;
        } else if !horizontal && n.layout_sizing_horizontal == SizingMode::Hug {
            w = cross + n.padding.left + n.padding.right;
        }
        if doc.set_geometry(id, n.x, n.y, w.max(0.0), h.max(0.0)) {
            out.push(id.to_string());
        }
        return;
    }
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
    let special = doc.get(id).filter(|n| is_auto(doc, id) && (n.layout_mode == LayoutMode::Grid || wraps(n))).cloned();
    if let Some(n) = special {
        if n.layout_mode == LayoutMode::Grid {
            arrange_grid(doc, &n, out);
        } else {
            arrange_wrapped(doc, &n, out);
        }
    } else if is_auto(doc, id) {
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
            let hugs_cross = if horizontal { cn.layout_sizing_vertical } else { cn.layout_sizing_horizontal } == SizingMode::Hug;
            if fill_cross || (n.counter_axis_align == LayoutAlign::Stretch && hugs_cross && cn.kind != NodeType::Text) {
                cc = inner_cross;
            }
            sizes.push((cm, cc));
        }
        let used: f64 = sizes.iter().map(|s| s.0).sum::<f64>() + spacing_total;
        let free = (inner_main - used).max(0.0);
        let (mut cursor, gap) = match n.primary_axis_align {
            LayoutAlign::Start | LayoutAlign::Stretch => (0.0, n.item_spacing),
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
                LayoutAlign::Start | LayoutAlign::SpaceBetween | LayoutAlign::Stretch => 0.0,
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
            // Wrapped text given a width other than the one it was measured
            // at wraps differently: estimate its height and flag it, so the
            // room (for agent edits) or an editor measures it exactly. Its old
            // height would leave the box, and hugging parents, at the old line
            // count. Back at the measured width, it gets its measured height
            // back.
            if let Some(measured) = doc.measured_height_back(c, w) {
                h = measured;
            } else if doc.get(c).is_some_and(|cn| rewraps(cn, w)) {
                let mut probe = doc.get(c).unwrap().clone();
                probe.width = w;
                h = crate::text::estimate_layout(&probe).2;
                doc.note_estimate(c, h);
                doc.set_needs_measure(c);
            }
            if doc.set_geometry(c, x, y, w, h) {
                out.push(c.clone());
            }
            cursor += cm + gap;
        }
    }
    if is_auto(doc, id) {
        fill_absolute(doc, id, out);
    }
    let kids: Vec<NodeId> = doc.children(id).to_vec();
    for k in kids {
        arrange(doc, &k, out);
    }
}

/// Wrapping text laid out at a width other than the one its height was
/// measured at. Text measured before `measuredWidth` existed counts its
/// current width.
fn rewraps(n: &Node, w: f64) -> bool {
    n.kind == NodeType::Text && n.text_auto_resize == crate::paint::TextAutoResize::Height && (n.measured_width.unwrap_or(n.width) - w).abs() > 0.01
}

/// Absolute children that FILL an axis span the whole frame along it, like
/// CSS `inset-x-0` (a badge across a card, an overlay).
fn fill_absolute(doc: &mut Document, id: &str, out: &mut Vec<NodeId>) {
    let Some((pw, ph)) = doc.get(id).map(|n| (n.width, n.height)) else { return };
    for c in doc.children(id).to_vec() {
        let Some(cn) = doc.get(&c) else { continue };
        if !cn.layout_absolute {
            continue;
        }
        let (mut x, mut y, mut w, mut h) = (cn.x, cn.y, cn.width, cn.height);
        if cn.layout_sizing_horizontal == SizingMode::Fill {
            (x, w) = (0.0, pw);
        }
        if cn.layout_sizing_vertical == SizingMode::Fill {
            (y, h) = (0.0, ph);
        }
        let probe = rewraps(cn, w).then(|| Node { width: w, ..cn.clone() });
        if let Some(measured) = doc.measured_height_back(&c, w) {
            h = measured;
        } else if let Some(probe) = probe {
            h = crate::text::estimate_layout(&probe).2;
            doc.note_estimate(&c, h);
            doc.set_needs_measure(&c);
        }
        if doc.set_geometry(&c, x, y, w, h) {
            out.push(c);
        }
    }
}

/// Wrapping needs a fixed main axis to wrap against.
fn wraps(n: &Node) -> bool {
    n.layout_wrap
        && match n.layout_mode {
            LayoutMode::Horizontal => n.layout_sizing_horizontal != SizingMode::Hug,
            LayoutMode::Vertical => n.layout_sizing_vertical != SizingMode::Hug,
            _ => false,
        }
}

fn cross_gap(n: &Node) -> f64 {
    n.counter_axis_spacing.unwrap_or(n.item_spacing)
}

struct Line {
    items: Vec<NodeId>,
    /// Main-axis length (with gaps) and cross-axis thickness.
    main: f64,
    cross: f64,
}

/// Break flow children into lines along the main axis.
fn wrap_lines(doc: &Document, n: &Node, flow: &[NodeId]) -> Vec<Line> {
    let horizontal = n.layout_mode == LayoutMode::Horizontal;
    let inner = if horizontal { n.width - n.padding.left - n.padding.right } else { n.height - n.padding.top - n.padding.bottom };
    let mut lines: Vec<Line> = Vec::new();
    for c in flow {
        let Some(cn) = doc.get(c) else { continue };
        let (cm, cc) = if horizontal { (cn.width, cn.height) } else { (cn.height, cn.width) };
        let cc = if child_fill(cn, horizontal).1 { 0.0 } else { cc };
        match lines.last_mut() {
            Some(l) if l.main + n.item_spacing + cm <= inner + 0.01 => {
                l.main += n.item_spacing + cm;
                l.cross = l.cross.max(cc);
                l.items.push(c.clone());
            }
            _ => lines.push(Line { items: vec![c.clone()], main: cm, cross: cc }),
        }
    }
    lines
}

fn arrange_wrapped(doc: &mut Document, n: &Node, out: &mut Vec<NodeId>) {
    let horizontal = n.layout_mode == LayoutMode::Horizontal;
    let flow = flow_children(doc, &n.id);
    let lines = wrap_lines(doc, n, &flow);
    let inner_main = if horizontal { n.width - n.padding.left - n.padding.right } else { n.height - n.padding.top - n.padding.bottom };
    let (start_main, start_cross) = if horizontal { (n.padding.left, n.padding.top) } else { (n.padding.top, n.padding.left) };
    let mut cross_cursor = 0.0;
    for line in &lines {
        let free = (inner_main - line.main).max(0.0);
        let (mut cursor, gap) = match n.primary_axis_align {
            LayoutAlign::Start | LayoutAlign::Stretch => (0.0, n.item_spacing),
            LayoutAlign::Center => (free / 2.0, n.item_spacing),
            LayoutAlign::End => (free, n.item_spacing),
            LayoutAlign::SpaceBetween if line.items.len() > 1 => (0.0, n.item_spacing + free / (line.items.len() - 1) as f64),
            LayoutAlign::SpaceBetween => (0.0, n.item_spacing),
        };
        for c in &line.items {
            let Some(cn) = doc.get(c) else { continue };
            let (cm, mut cc) = if horizontal { (cn.width, cn.height) } else { (cn.height, cn.width) };
            let hugs_cross = if horizontal { cn.layout_sizing_vertical } else { cn.layout_sizing_horizontal } == SizingMode::Hug;
            if child_fill(cn, horizontal).1 || (n.counter_axis_align == LayoutAlign::Stretch && hugs_cross && cn.kind != NodeType::Text) {
                cc = line.cross;
            }
            let off = match n.counter_axis_align {
                LayoutAlign::Start | LayoutAlign::SpaceBetween | LayoutAlign::Stretch => 0.0,
                LayoutAlign::Center => (line.cross - cc) / 2.0,
                LayoutAlign::End => line.cross - cc,
            };
            let (m, x) = (start_main + cursor, start_cross + cross_cursor + off);
            let (px, py, w, h) = if horizontal { (m, x, cm, cc) } else { (x, m, cc, cm) };
            if doc.set_geometry(c, px, py, w.max(0.0), h.max(0.0)) {
                out.push(c.clone());
            }
            cursor += cm + gap;
        }
        cross_cursor += line.cross + cross_gap(n);
    }
}

struct GridGeom {
    cols: usize,
    rows: usize,
    cell: (f64, f64),
    gap: (f64, f64),
}

impl GridGeom {
    fn block(&self) -> (f64, f64) {
        let span = |k: usize, size: f64, gap: f64| if k == 0 { 0.0 } else { k as f64 * size + (k - 1) as f64 * gap };
        (span(self.cols, self.cell.0, self.gap.0), span(self.rows, self.cell.1, self.gap.1))
    }
}

/// Columns, rows and cell size of a GRID frame, like Unity's GridLayoutGroup:
/// fixed columns, fixed rows, or as many columns as fit the width.
fn grid_geom(doc: &Document, n: &Node, flow: &[NodeId]) -> GridGeom {
    let count = flow.len();
    let cell = n.grid_cell_size.map(|[w, h]| (w.max(0.0), h.max(0.0))).unwrap_or_else(|| {
        flow.iter().filter_map(|c| doc.get(c)).fold((0.0f64, 0.0f64), |(w, h), c| (w.max(c.width), h.max(c.height)))
    });
    let gap = (n.item_spacing, cross_gap(n));
    let cols = match (n.grid_columns, n.grid_rows) {
        (Some(c), _) => c.max(1) as usize,
        (None, Some(r)) => count.div_ceil(r.max(1) as usize).max(1),
        (None, None) if n.layout_sizing_horizontal == SizingMode::Hug => ((count as f64).sqrt().ceil() as usize).max(1),
        (None, None) => {
            let inner = n.width - n.padding.left - n.padding.right;
            (((inner + gap.0) / (cell.0 + gap.0).max(1e-6)).floor() as usize).max(1)
        }
    };
    let rows = match (n.grid_columns, n.grid_rows) {
        (None, Some(r)) => (r.max(1) as usize).max(count.div_ceil(cols)),
        _ => count.div_ceil(cols),
    };
    GridGeom { cols, rows, cell, gap }
}

fn arrange_grid(doc: &mut Document, n: &Node, out: &mut Vec<NodeId>) {
    let flow = flow_children(doc, &n.id);
    let g = grid_geom(doc, n, &flow);
    let (bw, bh) = g.block();
    let inner_w = n.width - n.padding.left - n.padding.right;
    let inner_h = n.height - n.padding.top - n.padding.bottom;
    // The block of cells is aligned inside the frame.
    let align = |a: LayoutAlign, free: f64| match a {
        LayoutAlign::Center => free / 2.0,
        LayoutAlign::End => free,
        _ => 0.0,
    };
    let x0 = n.padding.left + align(n.primary_axis_align, inner_w - bw);
    let y0 = n.padding.top + align(n.counter_axis_align, inner_h - bh);
    for (i, c) in flow.iter().enumerate() {
        let (mut col, mut row) = if n.grid_vertical { (i / g.rows.max(1), i % g.rows.max(1)) } else { (i % g.cols, i / g.cols) };
        if matches!(n.grid_start_corner, GridCorner::TopRight | GridCorner::BottomRight) {
            col = g.cols.saturating_sub(1 + col);
        }
        if matches!(n.grid_start_corner, GridCorner::BottomLeft | GridCorner::BottomRight) {
            row = g.rows.saturating_sub(1 + row);
        }
        let Some(cn) = doc.get(c) else { continue };
        // An explicit cell size sizes every child, as in Unity.
        let (w, h) = if n.grid_cell_size.is_some() { g.cell } else { (cn.width, cn.height) };
        let x = x0 + col as f64 * (g.cell.0 + g.gap.0);
        let y = y0 + row as f64 * (g.cell.1 + g.gap.1);
        if doc.set_geometry(c, x, y, w, h) {
            out.push(c.clone());
        }
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

    /// An absolute badge with `width: "fill"` spans its card, and children
    /// layout doesn't place follow their constraints when layout resizes the
    /// card (as when resizing it by hand).
    #[test]
    fn absolute_children_fill_and_follow() {
        let (mut doc, top) = build(serde_json::json!([{
            "type": "frame", "name": "Card", "layout": "vertical", "padding": 20, "children": [
                { "type": "rect", "name": "Body", "w": 200, "h": 100 },
                { "type": "rect", "name": "Badge", "absolute": true, "y": -12, "width": "fill", "h": 24 },
                { "type": "rect", "name": "Corner", "absolute": true, "x": 200, "y": 10, "w": 20, "h": 20, "constraints": { "horizontal": "MAX" } }
            ] }]));
        let card = top[0].clone();
        let get = |doc: &Document, name: &str| doc.nodes().find(|n| n.name == name).cloned().unwrap();
        assert_eq!(get(&doc, "Card").width, 240.0);
        assert_eq!(get(&doc, "Corner").x, 200.0, "placed for the laid-out card");
        let badge = get(&doc, "Badge");
        assert_eq!((badge.x, badge.y, badge.width), (0.0, -12.0, 240.0));
        // The card grows by 60: the badge still spans it, the corner keeps its right margin.
        doc.clear_fresh();
        let body = get(&doc, "Body").id;
        doc.apply(&Op::update1(body.clone(), "width", serde_json::json!(260))).unwrap();
        relayout(&mut doc, &[body]);
        assert_eq!(get(&doc, "Card").width, 300.0, "{card}");
        assert_eq!(get(&doc, "Badge").width, 300.0);
        assert_eq!(get(&doc, "Corner").x, 260.0);
    }

    /// Measured wrapping text that layout narrows is re-estimated and flagged
    /// for measuring, so it and its hugging parent grow to the new line count.
    #[test]
    fn measured_instance_text_settles() {
        // A component's wrapping text, measured at 400 px, in a 150 px
        // instance: once in the flow and once as an absolute layer spanning
        // the frame.
        let mut doc = Document::new_empty();
        let mut c = Node::new("c:1", NodeType::Component);
        c.parent = Some("0:1".into());
        (c.layout_mode, c.width, c.layout_sizing_vertical) = (LayoutMode::Vertical, 400.0, SizingMode::Hug);
        doc.apply(&Op::Create { node: c }).unwrap();
        for (id, absolute) in [("t:1", false), ("t:2", true)] {
            let mut t = Node::new(id, NodeType::Text);
            t.parent = Some("c:1".into());
            t.index = id.into();
            t.characters = "A paragraph that wraps onto more lines when its column gets narrower".into();
            (t.font_size, t.line_height, t.width, t.height) = (14.0, Some(20.0), 400.0, 40.0);
            (t.text_auto_resize, t.layout_sizing_horizontal, t.layout_absolute) = (crate::paint::TextAutoResize::Height, SizingMode::Fill, absolute);
            doc.apply(&Op::Create { node: t }).unwrap();
        }
        let mut i = Node::new("i:1", NodeType::Instance);
        i.parent = Some("0:1".into());
        (i.component_id, i.width, i.y) = (Some("c:1".into()), 150.0, 200.0);
        doc.apply(&Op::Create { node: i }).unwrap();
        crate::components::sync_all(&mut doc);
        relayout_all(&mut doc);
        for id in ["i:1~t:1", "i:1~t:2"] {
            let t = doc.get(id).unwrap();
            assert!(t.width == 150.0 && t.height > 40.0 && t.needs_measure, "{id}: narrowed text is re-estimated and flagged ({} × {})", t.width, t.height);
        }
        // An editor measures it; the measurement lands in the instance's overrides.
        let measure = |doc: &mut Document| {
            let ops = crate::components::route_ops(
                doc,
                ["i:1~t:1", "i:1~t:2"].map(|id| Op::Update { id: id.into(), props: serde_json::json!({ "height": 100.0, "measuredWidth": 150.0, "needsMeasure": false }).as_object().unwrap().clone() }).to_vec(),
            );
            let mut changed = Vec::new();
            for op in ops {
                changed.extend(doc.apply(&op).unwrap().changed);
            }
            changed.extend(crate::components::sync_instances(doc, &changed, &[]));
            relayout(doc, &changed);
            changed
        };
        measure(&mut doc);
        for id in ["i:1~t:1", "i:1~t:2"] {
            let t = doc.get(id).unwrap();
            assert!(!t.needs_measure && t.height == 100.0, "{id}: settled after the instance re-synced ({} × {})", t.width, t.height);
        }
        assert!(measure(&mut doc).is_empty(), "the same measurement again changes nothing");
    }

    #[test]
    fn narrowed_text_rewraps() {
        let mut doc = Document::new_empty();
        let mut f = Node::new("f:1", NodeType::Frame);
        f.parent = Some("0:1".into());
        (f.layout_mode, f.width, f.layout_sizing_vertical) = (LayoutMode::Vertical, 400.0, SizingMode::Hug);
        doc.apply(&Op::Create { node: f }).unwrap();
        let mut t = Node::new("t:1", NodeType::Text);
        t.parent = Some("f:1".into());
        t.characters = "A paragraph that wraps onto more lines when its column gets narrower".into();
        (t.font_size, t.line_height, t.width, t.height) = (14.0, Some(20.0), 400.0, 40.0);
        (t.text_auto_resize, t.layout_sizing_horizontal) = (crate::paint::TextAutoResize::Height, SizingMode::Fill);
        doc.apply(&Op::Create { node: t }).unwrap();
        relayout_all(&mut doc);
        assert!(!doc.get("t:1").unwrap().needs_measure, "same width: kept as measured");
        doc.apply(&Op::update1("f:1", "width", serde_json::json!(150))).unwrap();
        relayout(&mut doc, &["f:1".to_string()]);
        let t = doc.get("t:1").unwrap();
        assert!(t.needs_measure && t.height > 60.0, "{} {}", t.needs_measure, t.height);
        assert_eq!(doc.get("f:1").unwrap().height, t.height);
    }

    /// A width that comes and goes (a parent briefly 0 wide while layers
    /// arrive one by one) leaves the height measured at the real width, not
    /// the estimate made for the passing one.
    #[test]
    fn passing_width_keeps_measured_height() {
        let mut doc = Document::new_empty();
        let mut f = Node::new("f:1", NodeType::Frame);
        f.parent = Some("0:1".into());
        (f.layout_mode, f.width, f.layout_sizing_vertical) = (LayoutMode::Vertical, 416.0, SizingMode::Hug);
        doc.apply(&Op::Create { node: f }).unwrap();
        let mut t = Node::new("t:1", NodeType::Text);
        t.parent = Some("f:1".into());
        t.characters = "Upgrades for your own chopping. Your crew always uses your axe material.".into();
        (t.font_size, t.line_height, t.width, t.height, t.measured_width) = (17.0, Some(20.5), 416.0, 41.0, Some(416.0));
        (t.text_auto_resize, t.layout_sizing_horizontal) = (crate::paint::TextAutoResize::Height, SizingMode::Fill);
        doc.apply(&Op::Create { node: t }).unwrap();
        relayout_all(&mut doc);
        assert_eq!(doc.get("t:1").unwrap().height, 41.0);
        doc.apply(&Op::update1("f:1", "width", serde_json::json!(0))).unwrap();
        relayout(&mut doc, &["f:1".to_string()]);
        assert!(doc.get("t:1").unwrap().height > 200.0, "0 wide: a letter a line");
        doc.apply(&Op::update1("f:1", "width", serde_json::json!(416))).unwrap();
        relayout(&mut doc, &["f:1".to_string()]);
        let t = doc.get("t:1").unwrap();
        assert_eq!((t.width, t.height), (416.0, 41.0), "back at the measured width: the measured height");
        assert_eq!(doc.get("f:1").unwrap().height, 41.0, "and the hugging frame with it");
    }

    fn build(spec: serde_json::Value) -> (Document, Vec<NodeId>) {
        let mut doc = Document::new_empty();
        let mut n = 0;
        let mut gen = || {
            n += 1;
            format!("g:{n}")
        };
        let (ops, top) = crate::friendly::build_create_ops(&doc, "0:1", spec.as_array().unwrap(), None, &mut gen).unwrap();
        let (_, errs) = doc.apply_all(&ops);
        assert!(errs.is_empty(), "{errs:?}");
        relayout_all(&mut doc);
        let kids = doc.children(&top[0]).to_vec();
        let mut all = top;
        all.extend(kids);
        (doc, all)
    }

    fn rect_of(doc: &Document, id: &str) -> (f64, f64, f64, f64) {
        let n = doc.get(id).unwrap();
        (n.x, n.y, n.width, n.height)
    }

    #[test]
    fn horizontal_wrap_breaks_lines() {
        let item = serde_json::json!({ "type": "rect", "w": 40, "h": 20 });
        let (doc, ids) = build(serde_json::json!([{ "type": "frame", "layout": "horizontal", "wrap": true, "width": 110, "gap": 10, "rowGap": 5, "padding": 5,
            "children": [item, item, item, { "type": "rect", "w": 40, "h": 30 }, item] }]));
        // Two fit per 100 px line (40 + 10 + 40): rows at y 5, 30, 65.
        let pos: Vec<(f64, f64)> = ids[1..].iter().map(|c| (rect_of(&doc, c).0, rect_of(&doc, c).1)).collect();
        assert_eq!(pos, vec![(5.0, 5.0), (55.0, 5.0), (5.0, 30.0), (55.0, 30.0), (5.0, 65.0)]);
        // The height hugs the three lines: 20 + 5 + 30 + 5 + 20 + padding.
        assert_eq!(rect_of(&doc, &ids[0]).3, 90.0);
    }

    #[test]
    fn grid_like_unity() {
        let item = serde_json::json!({ "type": "rect", "w": 10, "h": 10 });
        // Fixed 3 columns, 50×40 cells, filled from the top-right.
        let (doc, ids) = build(serde_json::json!([{ "type": "frame", "layout": "grid", "columns": 3, "cellSize": [50, 40], "gap": 10, "rowGap": 20,
            "startCorner": "UPPER_RIGHT", "children": [item, item, item, item] }]));
        assert_eq!(rect_of(&doc, &ids[0]).2, 170.0, "hugs 3 cells and 2 gaps");
        assert_eq!(rect_of(&doc, &ids[0]).3, 100.0, "and 2 rows");
        assert_eq!(rect_of(&doc, &ids[1]), (120.0, 0.0, 50.0, 40.0), "first child top-right, sized to the cell");
        assert_eq!(rect_of(&doc, &ids[3]), (0.0, 0.0, 50.0, 40.0));
        assert_eq!(rect_of(&doc, &ids[4]), (120.0, 60.0, 50.0, 40.0));
        // Flexible: as many 30 px columns as fit 100 px with 5 px gaps (3).
        let (doc, ids) = build(serde_json::json!([{ "type": "frame", "layout": "grid", "width": 100, "cellSize": [30, 30], "gap": 5, "startAxis": "vertical", "rows": 2,
            "children": [item, item, item] }]));
        // Rows fixed at 2, filling columns first: (0,0), (0,1), (1,0).
        let pos: Vec<(f64, f64)> = ids[1..].iter().map(|c| (rect_of(&doc, c).0, rect_of(&doc, c).1)).collect();
        assert_eq!(pos, vec![(0.0, 0.0), (0.0, 35.0), (35.0, 0.0)]);
    }
}
