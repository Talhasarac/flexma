//! SVG export. Used by the editor's "Export SVG", by the server to rasterize
//! previews (via resvg), and by MCP tools so agents can look at designs.

use crate::color::Color;
use crate::doc::Document;
use crate::geom::{fmt_num, Affine, Rect};
use crate::node::{Node, NodeId, NodeType};
use crate::paint::{Effect, ImageFit, Paint, TextAlign, TextAlignVertical};
use crate::path;
use std::fmt::Write;

pub struct SvgOptions<'a> {
    /// Extra space around the exported bounds.
    pub padding: f64,
    /// Background fill (e.g. the page color). `None` = transparent.
    pub background: Option<Color>,
    /// Resolve an image asset id to a URL or data URI.
    pub resolve_asset: &'a dyn Fn(&str) -> Option<String>,
    /// Text line breaking with real metrics if available.
    pub measure_text: Option<&'a dyn Fn(&Node, &str) -> f64>,
}

impl Default for SvgOptions<'_> {
    fn default() -> Self {
        SvgOptions { padding: 0.0, background: None, resolve_asset: &|_| None, measure_text: None }
    }
}

/// Export the given nodes (or a page: pass the page id) to an SVG document.
/// Returns the SVG and the page-space bounds it covers.
pub fn export(doc: &Document, ids: &[NodeId], opts: &SvgOptions) -> Option<(String, Rect)> {
    let mut bounds = Rect::empty();
    let mut roots: Vec<&Node> = Vec::new();
    for id in ids {
        let n = doc.get(id)?;
        if n.kind == NodeType::Page || n.kind == NodeType::Document {
            for c in doc.children(id) {
                if let Some(cn) = doc.get(c) {
                    if cn.visible {
                        bounds = bounds.union(&visual_bounds(doc, cn));
                        roots.push(cn);
                    }
                }
            }
        } else {
            bounds = bounds.union(&visual_bounds(doc, n));
            roots.push(n);
        }
    }
    if bounds.is_empty() {
        bounds = Rect::new(0.0, 0.0, 1.0, 1.0);
    }
    let b = bounds.inflate(opts.padding);
    let mut w = Writer { doc, opts, defs: String::new(), body: String::new(), next_id: 0 };
    for n in roots {
        let parent_world = n.parent.as_deref().map(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY);
        w.node(n, &parent_world);
    }
    let mut out = String::new();
    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}">"#,
        x = fmt_num(b.x),
        y = fmt_num(b.y),
        w = fmt_num(b.width.max(1.0)),
        h = fmt_num(b.height.max(1.0)),
    );
    if !w.defs.is_empty() {
        out.push_str("<defs>");
        out.push_str(&w.defs);
        out.push_str("</defs>");
    }
    if let Some(bg) = opts.background {
        let _ = write!(
            out,
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
            fmt_num(b.x),
            fmt_num(b.y),
            fmt_num(b.width),
            fmt_num(b.height),
            bg.to_hex()
        );
    }
    out.push_str(&w.body);
    out.push_str("</svg>");
    Some((out, b))
}

/// Subtree bounds grown by the node's own shadows/blur so exports don't clip them.
fn visual_bounds(doc: &Document, n: &Node) -> Rect {
    let b = doc.subtree_bounds(&n.id);
    let mut grow = 0.0f64;
    for e in &n.effects {
        match e {
            Effect::DropShadow { x, y, blur, spread, visible, .. } if *visible => {
                grow = grow.max((*blur as f64) + (*spread as f64).max(0.0) + (*x as f64).abs().max((*y as f64).abs()));
            }
            Effect::LayerBlur { radius, visible } if *visible => grow = grow.max(*radius as f64),
            _ => {}
        }
    }
    if grow > 0.0 { b.inflate(grow.ceil()) } else { b }
}

struct Writer<'a> {
    doc: &'a Document,
    opts: &'a SvgOptions<'a>,
    defs: String,
    body: String,
    next_id: usize,
}

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => o.push(c),
        }
    }
    o
}

fn rounded_rect_path(w: f64, h: f64, r: [f64; 4]) -> String {
    let [tl, tr, br, bl] = r;
    let k = 0.5523;
    format!(
        "M{} 0 H{} C{} 0 {} {} {} {} V{} C{} {} {} {} {} {} H{} C{} {} 0 {} 0 {} V{} C0 {} {} 0 {} 0 Z",
        fmt_num(tl),
        fmt_num(w - tr),
        fmt_num(w - tr + tr * k),
        fmt_num(w),
        fmt_num(tr - tr * k),
        fmt_num(w),
        fmt_num(tr),
        fmt_num(h - br),
        fmt_num(w),
        fmt_num(h - br + br * k),
        fmt_num(w - br + br * k),
        fmt_num(h),
        fmt_num(w - br),
        fmt_num(h),
        fmt_num(bl),
        fmt_num(bl - bl * k),
        fmt_num(h),
        fmt_num(h - bl + bl * k),
        fmt_num(h - bl),
        fmt_num(tl),
        fmt_num(tl - tl * k),
        fmt_num(tl - tl * k),
        fmt_num(tl),
    )
}

impl Writer<'_> {
    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    /// SVG geometry element (without paint attributes) for a node's own shape.
    fn shape(&self, n: &Node) -> Option<String> {
        let w = n.width;
        let h = n.height;
        Some(match n.kind {
            NodeType::Rectangle | NodeType::Frame => {
                let r = n.radii();
                if r.iter().all(|v| *v == r[0]) {
                    let rx = if r[0] > 0.0 { format!(r#" rx="{}""#, fmt_num(r[0])) } else { String::new() };
                    format!(r#"<rect width="{}" height="{}"{rx}"#, fmt_num(w), fmt_num(h))
                } else {
                    format!(r#"<path d="{}""#, rounded_rect_path(w, h, r))
                }
            }
            NodeType::Ellipse => format!(
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}""#,
                fmt_num(w / 2.0),
                fmt_num(h / 2.0),
                fmt_num(w / 2.0),
                fmt_num(h / 2.0)
            ),
            NodeType::Vector | NodeType::Polygon | NodeType::Star | NodeType::Line => {
                let segs = path::node_path(n);
                if segs.is_empty() {
                    return None;
                }
                format!(r#"<path d="{}""#, path::to_svg_d(&segs))
            }
            _ => return None,
        })
    }

    fn paint_attr(&mut self, p: &Paint, w: f64, h: f64, attr: &str) -> Option<String> {
        if !p.is_visible() {
            return None;
        }
        match p {
            Paint::Solid { color, opacity, .. } => {
                let a = color.a * opacity;
                let op = if a < 1.0 { format!(r#" {attr}-opacity="{}""#, fmt_num(a as f64)) } else { String::new() };
                Some(format!(r#" {attr}="{}"{op}"#, color.to_hex_rgb()))
            }
            Paint::LinearGradient { stops, angle, opacity, .. } => {
                let id = self.id("g");
                let (s, c) = (angle.to_radians() as f64).sin_cos();
                let (x1, y1, x2, y2) = (0.5 - c / 2.0, 0.5 - s / 2.0, 0.5 + c / 2.0, 0.5 + s / 2.0);
                let _ = write!(
                    self.defs,
                    r#"<linearGradient id="{id}" x1="{}" y1="{}" x2="{}" y2="{}">"#,
                    fmt_num(x1),
                    fmt_num(y1),
                    fmt_num(x2),
                    fmt_num(y2)
                );
                for st in stops {
                    let _ = write!(
                        self.defs,
                        r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
                        fmt_num(st.position as f64),
                        st.color.to_hex_rgb(),
                        fmt_num((st.color.a * opacity) as f64)
                    );
                }
                self.defs.push_str("</linearGradient>");
                Some(format!(r#" {attr}="url(#{id})""#))
            }
            Paint::Image { asset, fit, opacity, .. } if attr == "fill" => {
                let href = (self.opts.resolve_asset)(asset)?;
                let id = self.id("p");
                let par = match fit {
                    ImageFit::Fill => "xMidYMid slice",
                    ImageFit::Fit => "xMidYMid meet",
                    ImageFit::Stretch => "none",
                };
                let _ = write!(
                    self.defs,
                    r#"<pattern id="{id}" patternUnits="userSpaceOnUse" width="{w}" height="{h}"><image href="{href}" width="{w}" height="{h}" preserveAspectRatio="{par}" opacity="{op}"/></pattern>"#,
                    w = fmt_num(w),
                    h = fmt_num(h),
                    href = escape(&href),
                    op = fmt_num(*opacity as f64),
                );
                Some(format!(r#" {attr}="url(#{id})""#))
            }
            _ => None,
        }
    }

    fn effects_filter(&mut self, n: &Node) -> Option<String> {
        let effects: Vec<&Effect> = n
            .effects
            .iter()
            .filter(|e| match e {
                Effect::DropShadow { visible, .. } | Effect::LayerBlur { visible, .. } => *visible,
            })
            .collect();
        if effects.is_empty() {
            return None;
        }
        let id = self.id("f");
        let _ = write!(self.defs, r#"<filter id="{id}" x="-50%" y="-50%" width="200%" height="200%">"#);
        for e in effects {
            match e {
                Effect::DropShadow { color, x, y, blur, .. } => {
                    let _ = write!(
                        self.defs,
                        r#"<feDropShadow dx="{}" dy="{}" stdDeviation="{}" flood-color="{}" flood-opacity="{}"/>"#,
                        fmt_num(*x as f64),
                        fmt_num(*y as f64),
                        fmt_num(*blur as f64 / 2.0),
                        color.to_hex_rgb(),
                        fmt_num(color.a as f64)
                    );
                }
                Effect::LayerBlur { radius, .. } => {
                    let _ = write!(self.defs, r#"<feGaussianBlur stdDeviation="{}"/>"#, fmt_num(*radius as f64 / 2.0));
                }
            }
        }
        self.defs.push_str("</filter>");
        Some(id)
    }

    fn node(&mut self, n: &Node, _parent_world: &Affine) {
        if !n.visible || !n.kind.is_scene() {
            return;
        }
        let local = self.doc.local_transform(&n.id);
        let mut g = String::from("<g");
        let _ = write!(g, r#" transform="{}""#, local.to_svg());
        if n.opacity < 1.0 {
            let _ = write!(g, r#" opacity="{}""#, fmt_num(n.opacity));
        }
        if let Some(f) = self.effects_filter(n) {
            let _ = write!(g, r#" filter="url(#{f})""#);
        }
        let _ = write!(g, r#" data-name="{}">"#, escape(&n.name));
        self.body.push_str(&g);

        match n.kind {
            NodeType::Text => self.text(n),
            _ => {
                if let Some(shape) = self.shape(n) {
                    let fills: Vec<Paint> = n.fills.clone();
                    let closed = n.kind != NodeType::Line;
                    if closed {
                        for p in fills.iter() {
                            if let Some(attr) = self.paint_attr(p, n.width, n.height, "fill") {
                                let _ = write!(self.body, "{shape}{attr}/>");
                            }
                        }
                    }
                    if n.has_visible_stroke() {
                        let strokes = n.strokes.clone();
                        for p in strokes.iter() {
                            if let Some(attr) = self.paint_attr(p, n.width, n.height, "stroke") {
                                let dash = if n.dash_pattern.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        r#" stroke-dasharray="{}""#,
                                        n.dash_pattern.iter().map(|v| fmt_num(*v)).collect::<Vec<_>>().join(" ")
                                    )
                                };
                                let _ = write!(
                                    self.body,
                                    r#"{shape} fill="none"{attr} stroke-width="{}"{dash}/>"#,
                                    fmt_num(n.stroke_weight)
                                );
                            }
                        }
                    }
                }
            }
        }

        let kids: Vec<NodeId> = self.doc.children(&n.id).to_vec();
        if !kids.is_empty() {
            let clip = n.kind == NodeType::Frame && n.clips_content;
            if clip {
                let id = self.id("c");
                let shape = self.shape(n).unwrap_or_default();
                let _ = write!(self.defs, r#"<clipPath id="{id}">{shape}/></clipPath>"#);
                let _ = write!(self.body, r#"<g clip-path="url(#{id})">"#);
            }
            let world = self.doc.world_transform(&n.id);
            for k in kids {
                if let Some(kn) = self.doc.get(&k) {
                    let kn = kn.clone();
                    self.node(&kn, &world);
                }
            }
            if clip {
                self.body.push_str("</g>");
            }
        }
        self.body.push_str("</g>");
    }

    fn text(&mut self, n: &Node) {
        let lines = match self.opts.measure_text {
            Some(m) => {
                let max_w = match n.text_auto_resize {
                    crate::paint::TextAutoResize::WidthAndHeight => None,
                    _ => Some(n.width),
                };
                crate::text::wrap_lines(&n.characters, max_w, &mut |s| m(n, s))
            }
            None => crate::text::estimate_layout(n).0,
        };
        let lh = n.effective_line_height();
        let total_h = lh * lines.len() as f64;
        let y0 = match n.text_align_vertical {
            TextAlignVertical::Top => 0.0,
            TextAlignVertical::Center => (n.height - total_h) / 2.0,
            TextAlignVertical::Bottom => n.height - total_h,
        };
        let (anchor, x) = match n.text_align {
            TextAlign::Left => ("start", 0.0),
            TextAlign::Center => ("middle", n.width / 2.0),
            TextAlign::Right => ("end", n.width),
        };
        let fill = n
            .fills
            .iter()
            .find(|p| p.is_visible())
            .cloned()
            .unwrap_or(Paint::solid(Color::BLACK));
        let fill_attr = self.paint_attr(&fill, n.width, n.height, "fill").unwrap_or_default();
        let style = if n.italic { r#" font-style="italic""# } else { "" };
        let deco = match n.text_decoration.as_deref() {
            Some("UNDERLINE") => r#" text-decoration="underline""#,
            Some("STRIKETHROUGH") => r#" text-decoration="line-through""#,
            _ => "",
        };
        let ls = if n.letter_spacing != 0.0 {
            format!(r#" letter-spacing="{}""#, fmt_num(n.letter_spacing))
        } else {
            String::new()
        };
        let _ = write!(
            self.body,
            r#"<text font-family="{}, Inter, Helvetica, Arial, sans-serif" font-size="{}" font-weight="{}" text-anchor="{anchor}"{style}{deco}{ls}{fill_attr}>"#,
            escape(&n.font_family),
            fmt_num(n.font_size),
            n.font_weight,
        );
        for (i, line) in lines.iter().enumerate() {
            // Baseline: center the em box in the line box, ascent ≈ 0.8em.
            let baseline = y0 + lh * i as f64 + (lh - n.font_size) / 2.0 + n.font_size * 0.8;
            let _ = write!(
                self.body,
                r#"<tspan x="{}" y="{}" xml:space="preserve">{}</tspan>"#,
                fmt_num(x),
                fmt_num(baseline),
                escape(line)
            );
        }
        self.body.push_str("</text>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Op;

    #[test]
    fn exports_basic_shapes() {
        let mut doc = Document::new_empty();
        let mut f = Node::new("f:1", NodeType::Frame);
        f.parent = Some("0:1".into());
        f.width = 100.0;
        f.height = 80.0;
        f.corner_radius = 8.0;
        doc.apply(&Op::Create { node: f }).unwrap();
        let mut t = Node::new("t:1", NodeType::Text);
        t.parent = Some("f:1".into());
        t.characters = "Hello <world>".into();
        doc.apply(&Op::Create { node: t }).unwrap();
        let (svg, b) = export(&doc, &["0:1".into()], &SvgOptions::default()).unwrap();
        assert!(svg.contains("<rect"));
        assert!(svg.contains("Hello &lt;world&gt;"));
        assert!(svg.contains("clipPath"));
        assert_eq!(b.width, 100.0);
    }
}
