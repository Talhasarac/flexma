//! Design audit: what a design uses (colors, type styles, spacing, radii,
//! fonts) and where it drifts (near-duplicate colors, sizes off a type scale,
//! spacing off the 4-pt grid, low text contrast, tiny text). Used by the MCP
//! `audit_design` tool so agents can match and tidy an existing design.

use crate::color::Color;
use crate::doc::Document;
use crate::geom::Rect;
use crate::node::{is_derived_id, Node, NodeId, NodeType};
use crate::paint::Paint;
use serde::Serialize;
use std::collections::BTreeMap;

/// A common type scale (px). Sizes outside it are reported.
pub const TYPE_SCALE: &[f64] = &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 18.0, 20.0, 22.0, 24.0, 28.0, 30.0, 32.0, 36.0, 40.0, 44.0, 48.0, 56.0, 60.0, 64.0, 72.0, 80.0, 96.0, 128.0];

#[derive(Debug, Serialize, Default)]
pub struct Audit {
    pub layers: usize,
    /// (hex, uses, example ids), most used first.
    pub colors: Vec<(String, usize, Vec<NodeId>)>,
    /// Pairs of colors that look almost the same.
    pub near_duplicates: Vec<(String, String)>,
    pub text_styles: Vec<TextStyleUse>,
    /// (size, example ids) for sizes outside `TYPE_SCALE`.
    pub off_scale_sizes: Vec<(f64, Vec<NodeId>)>,
    pub gaps: Vec<(f64, usize)>,
    pub paddings: Vec<(f64, usize)>,
    pub radii: Vec<(f64, usize)>,
    /// Spacing or radius values that aren't multiples of 4 (2 below 8).
    pub off_grid: Vec<OffGrid>,
    /// (family, text layers).
    pub fonts: Vec<(String, usize)>,
    pub contrast: Vec<ContrastIssue>,
    /// Text layers under 12 px.
    pub small_text: Vec<NodeId>,
    pub components: usize,
    pub instances: usize,
}

#[derive(Debug, Serialize)]
pub struct TextStyleUse {
    pub family: String,
    pub size: f64,
    pub weight: u16,
    pub line_height: Option<f64>,
    pub uses: usize,
    pub example: NodeId,
}

#[derive(Debug, Serialize)]
pub struct OffGrid {
    pub what: &'static str,
    pub value: f64,
    pub id: NodeId,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ContrastIssue {
    pub id: NodeId,
    pub name: String,
    pub ratio: f64,
    pub required: f64,
    pub fg: String,
    pub bg: String,
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn on_grid(v: f64) -> bool {
    let step = if v < 8.0 { 2.0 } else { 4.0 };
    v <= 0.0 || ((v / step) - (v / step).round()).abs() < 1e-6
}

/// The colors a node shows: visible solid fills and strokes (with paint opacity).
fn solid_colors(n: &Node) -> Vec<Color> {
    let mut out = Vec::new();
    let strokes = if n.stroke_weight > 0.0 { n.strokes.as_slice() } else { &[] };
    for p in n.fills.iter().chain(strokes) {
        if let Paint::Solid { color, opacity, visible: true } = p {
            out.push(Color { a: color.a * opacity, ..*color });
        }
    }
    out
}

fn blend(top: Color, under: Color) -> Color {
    let a = top.a;
    Color { r: top.r * a + under.r * (1.0 - a), g: top.g * a + under.g * (1.0 - a), b: top.b * a + under.b * (1.0 - a), a: 1.0 }
}

fn luminance(c: Color) -> f64 {
    let ch = |v: f32| {
        let v = v as f64;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
}

/// WCAG contrast ratio of two opaque colors.
pub fn contrast_ratio(a: Color, b: Color) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The opaque fill of a node if it's a plain solid (no images or gradients).
fn opaque_fill(n: &Node, under: Color) -> Option<Option<Color>> {
    let mut result = None;
    for p in n.fills.iter().filter(|p| p.is_visible()) {
        match p {
            Paint::Solid { color, opacity, .. } => {
                let c = Color { a: color.a * opacity, ..*color };
                result = Some(blend(c, result.unwrap_or(under)));
            }
            // Can't judge contrast over images or gradients.
            _ => return None,
        }
    }
    Some(result)
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    outer.x <= inner.x + 0.5 && outer.y <= inner.y + 0.5 && outer.right() + 0.5 >= inner.right() && outer.bottom() + 0.5 >= inner.bottom()
}

/// What's drawn behind a text layer: ancestors' fills, plus lower siblings
/// (at each level) that cover the text. `None` when it can't be judged.
fn background_of(doc: &Document, text: &Node, page_bg: Color) -> Option<Color> {
    let bounds = doc.world_bounds(&text.id);
    let mut chain = doc.ancestors(&text.id);
    chain.reverse(); // root → parent
    let mut bg = page_bg;
    let mut path: Vec<NodeId> = chain.iter().skip(1).cloned().collect();
    path.push(text.id.clone());
    for (i, id) in chain.iter().enumerate() {
        let Some(n) = doc.get(id) else { continue };
        if !matches!(n.kind, NodeType::Document | NodeType::Page) {
            if !n.visible || n.opacity < 0.999 {
                return None;
            }
            match opaque_fill(n, bg)? {
                Some(c) => bg = c,
                None => {}
            }
        }
        // Siblings painted below the next node on the path.
        let next = path.get(i);
        for sib in doc.children(id) {
            if Some(sib) == next {
                break;
            }
            let Some(s) = doc.get(sib) else { continue };
            if s.visible && s.kind != NodeType::Text && contains(&doc.world_bounds(sib), &bounds) {
                if let Some(Some(c)) = opaque_fill(s, bg) {
                    bg = c;
                }
            }
        }
    }
    Some(bg)
}

fn tally<K: Ord>(map: &mut BTreeMap<K, usize>, k: K) {
    *map.entry(k).or_default() += 1;
}

fn sorted_counts(map: BTreeMap<i64, usize>) -> Vec<(f64, usize)> {
    let mut v: Vec<(f64, usize)> = map.into_iter().map(|(k, c)| (k as f64 / 100.0, c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.total_cmp(&b.0)));
    v
}

/// Audit `root` (a page or any layer) and everything inside it.
pub fn audit(doc: &Document, root: &str, page_bg: Color) -> Audit {
    let mut a = Audit::default();
    let mut colors: BTreeMap<String, (usize, Vec<NodeId>, Color)> = BTreeMap::new();
    let mut styles: BTreeMap<(String, i64, u16, Option<i64>), (usize, NodeId)> = BTreeMap::new();
    let mut off_scale: BTreeMap<i64, Vec<NodeId>> = BTreeMap::new();
    let (mut gaps, mut paddings, mut radii) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let mut fonts: BTreeMap<String, usize> = BTreeMap::new();
    let key = |v: f64| (round2(v) * 100.0).round() as i64;

    let mut nodes: Vec<&Node> = Vec::new();
    doc.walk(root, &mut |n| {
        if let Some(n) = doc.get(&n.id) {
            nodes.push(n);
        }
    });
    for n in nodes {
        if matches!(n.kind, NodeType::Document | NodeType::Page) || !n.visible {
            continue;
        }
        a.layers += 1;
        let derived = is_derived_id(&n.id);
        match n.kind {
            NodeType::Component => a.components += 1,
            NodeType::Instance => a.instances += 1,
            _ => {}
        }
        for c in solid_colors(n) {
            let e = colors.entry(c.to_hex()).or_insert((0, Vec::new(), c));
            e.0 += 1;
            if e.1.len() < 3 {
                e.1.push(n.id.clone());
            }
        }
        // Spacing and radii come from real layers (instances repeat them).
        if !derived {
            if n.kind.is_frame_like() && n.layout_mode != crate::paint::LayoutMode::None {
                tally(&mut gaps, key(n.item_spacing));
                if !on_grid(n.item_spacing) {
                    a.off_grid.push(OffGrid { what: "gap", value: round2(n.item_spacing), id: n.id.clone(), name: n.name.clone() });
                }
                let p = &n.padding;
                for v in [p.top, p.right, p.bottom, p.left] {
                    tally(&mut paddings, key(v));
                }
                if let Some(v) = [p.top, p.right, p.bottom, p.left].into_iter().find(|v| !on_grid(*v)) {
                    a.off_grid.push(OffGrid { what: "padding", value: round2(v), id: n.id.clone(), name: n.name.clone() });
                }
            }
            if n.corner_radius > 0.0 && n.kind != NodeType::Text {
                tally(&mut radii, key(n.corner_radius));
                if !on_grid(n.corner_radius) && n.corner_radius < 500.0 {
                    a.off_grid.push(OffGrid { what: "radius", value: round2(n.corner_radius), id: n.id.clone(), name: n.name.clone() });
                }
            }
        }
        if n.kind == NodeType::Text && !n.characters.trim().is_empty() {
            tally(&mut fonts, n.font_family.clone());
            let e = styles.entry((n.font_family.clone(), key(n.font_size), n.font_weight, n.line_height.map(key))).or_insert((0, n.id.clone()));
            e.0 += 1;
            if !TYPE_SCALE.iter().any(|s| (s - n.font_size).abs() < 0.01) {
                let list = off_scale.entry(key(n.font_size)).or_default();
                if list.len() < 3 {
                    list.push(n.id.clone());
                }
            }
            if n.font_size < 12.0 {
                a.small_text.push(n.id.clone());
            }
            // Contrast (WCAG AA): 3:1 for large text, 4.5:1 otherwise.
            let fg = n.fills.iter().find_map(|p| match p {
                Paint::Solid { color, opacity, visible: true } => Some(Color { a: color.a * opacity * n.opacity as f32, ..*color }),
                _ => None,
            });
            if let (Some(fg), Some(bg)) = (fg, background_of(doc, n, page_bg)) {
                let shown = blend(fg, bg);
                let ratio = contrast_ratio(shown, bg);
                let large = n.font_size >= 24.0 || (n.font_size >= 18.66 && n.font_weight >= 700);
                let required = if large { 3.0 } else { 4.5 };
                if ratio + 1e-9 < required {
                    a.contrast.push(ContrastIssue {
                        id: n.id.clone(),
                        name: n.name.clone(),
                        ratio: round2(ratio),
                        required,
                        fg: fg.to_hex(),
                        bg: bg.to_hex_rgb(),
                    });
                }
            }
        }
    }

    let mut color_list: Vec<(String, usize, Vec<NodeId>, Color)> = colors.into_iter().map(|(h, (c, ex, col))| (h, c, ex, col)).collect();
    color_list.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    for i in 0..color_list.len() {
        for j in (i + 1)..color_list.len() {
            let (p, q) = (color_list[i].3, color_list[j].3);
            let d = ((p.r - q.r).abs()).max((p.g - q.g).abs()).max((p.b - q.b).abs()).max((p.a - q.a).abs());
            if d > 0.0 && d <= 6.0 / 255.0 {
                a.near_duplicates.push((color_list[i].0.clone(), color_list[j].0.clone()));
            }
        }
    }
    a.colors = color_list.into_iter().map(|(h, c, ex, _)| (h, c, ex)).collect();
    let mut style_list: Vec<TextStyleUse> = styles
        .into_iter()
        .map(|((family, size, weight, lh), (uses, example))| TextStyleUse {
            family,
            size: size as f64 / 100.0,
            weight,
            line_height: lh.map(|v| v as f64 / 100.0),
            uses,
            example,
        })
        .collect();
    style_list.sort_by(|x, y| y.uses.cmp(&x.uses).then(x.size.total_cmp(&y.size)));
    a.text_styles = style_list;
    a.off_scale_sizes = off_scale.into_iter().map(|(k, ids)| (k as f64 / 100.0, ids)).collect();
    a.gaps = sorted_counts(gaps);
    a.paddings = sorted_counts(paddings);
    a.radii = sorted_counts(radii);
    let mut font_list: Vec<(String, usize)> = fonts.into_iter().collect();
    font_list.sort_by(|x, y| y.1.cmp(&x.1));
    a.fonts = font_list;
    a.contrast.sort_by(|x, y| x.ratio.total_cmp(&y.ratio));
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Op;

    fn node(id: &str, kind: NodeType, parent: &str, f: impl FnOnce(&mut Node)) -> Op {
        let mut n = Node::new(id, kind);
        n.parent = Some(parent.into());
        n.width = 100.0;
        n.height = 40.0;
        f(&mut n);
        Op::Create { node: n }
    }

    #[test]
    fn finds_drift_and_contrast_problems() {
        let mut doc = Document::new_empty();
        let solid = |hex: &str| Paint::solid(Color::parse(hex).unwrap());
        doc.apply(&node("f:1", NodeType::Frame, "0:1", |n| {
            n.fills = vec![solid("#ffffff")];
            n.layout_mode = crate::paint::LayoutMode::Vertical;
            n.item_spacing = 15.0;
            n.width = 300.0;
            n.height = 300.0;
        }))
        .unwrap();
        doc.apply(&node("t:1", NodeType::Text, "f:1", |n| {
            n.characters = "Faint".into();
            n.fills = vec![solid("#bbbbbb")];
            n.font_size = 17.0;
        }))
        .unwrap();
        doc.apply(&node("t:2", NodeType::Text, "f:1", |n| {
            n.characters = "Fine".into();
            n.fills = vec![solid("#111111")];
        }))
        .unwrap();
        doc.apply(&node("r:1", NodeType::Rectangle, "f:1", |n| n.fills = vec![solid("#121212")])).unwrap();
        crate::layout::relayout_all(&mut doc);
        let a = audit(&doc, "0:1", Color::WHITE);
        assert_eq!(a.contrast.len(), 1, "{:?}", a.contrast);
        assert_eq!(a.contrast[0].id, "t:1");
        assert!(a.contrast[0].ratio < 2.0);
        assert_eq!(a.off_scale_sizes[0].0, 17.0);
        assert!(a.off_grid.iter().any(|o| o.what == "gap" && o.value == 15.0));
        assert!(a.near_duplicates.iter().any(|(x, y)| (x == "#111111" && y == "#121212") || (x == "#121212" && y == "#111111")));
        assert_eq!(a.fonts, vec![("Inter".to_string(), 2)]);
    }
}
