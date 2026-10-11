//! SVG export. Used by the editor's "Export SVG", by the server to rasterize
//! previews (via resvg), and by MCP tools so agents can look at designs.

use crate::color::Color;
use crate::doc::Document;
use crate::geom::{fmt_num, Affine, Rect};
use crate::node::{Node, NodeId, NodeType};
use crate::paint::{Effect, FillRule, ImageCrop, ImageFit, NineSlice, Paint, StrokeCap, StrokeJoin, TextAlign, TextAlignVertical};
use crate::path;
use std::fmt::Write;

pub struct SvgOptions<'a> {
    /// Extra space around the exported bounds.
    pub padding: f64,
    /// Background fill (e.g. the page color). `None` = transparent.
    pub background: Option<Color>,
    /// Resolve an image asset id to a URL or data URI.
    pub resolve_asset: &'a dyn Fn(&str) -> Option<String>,
    /// Pixel size of an image asset, needed for 9-slice fills (which otherwise
    /// export stretched).
    pub image_size: &'a dyn Fn(&str) -> Option<(f64, f64)>,
    /// Text line breaking with real metrics if available.
    pub measure_text: Option<&'a dyn Fn(&Node, &str) -> f64>,
    /// An image with its crop cut out and/or its tint multiplied in, as its
    /// own URL or data URI, if the host can make one. Otherwise crops are
    /// viewports into the whole image (where smoothing can pick up a sliver of
    /// the pixels next to the region) and tints are color-matrix filters
    /// (which resvg can drop on large or sliced layers).
    pub resolve_variant: &'a dyn Fn(&str, Option<&ImageCrop>, Option<Color>) -> Option<String>,
}

impl Default for SvgOptions<'_> {
    fn default() -> Self {
        SvgOptions { padding: 0.0, background: None, resolve_asset: &|_| None, image_size: &|_| None, measure_text: None, resolve_variant: &|_, _, _| None }
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
        // Nodes write their local transform; a nested root also needs its
        // parent's, to land in page space like the bounds.
        let nested = parent_world != Affine::IDENTITY;
        if nested {
            let _ = write!(w.body, r#"<g transform="{}">"#, parent_world.to_svg());
        }
        w.node(n, &parent_world);
        if nested {
            w.body.push_str("</g>");
        }
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
    // Text outlines reach past the text box (plus a little for glyph overhang).
    if n.kind == NodeType::Text && n.has_visible_stroke() {
        let reach = match n.stroke_align {
            crate::paint::StrokeAlign::Outside => n.stroke_weight,
            crate::paint::StrokeAlign::Center => n.stroke_weight / 2.0,
            crate::paint::StrokeAlign::Inside => 0.0,
        };
        grow = grow.max(reach + n.font_size * 0.1);
    }
    if grow > 0.0 { b.inflate(grow.ceil()) } else { b }
}

/// The region a filled image shows (Unity's Filled type), as path data in a
/// `w`×`h` box. Matches the engine's `filledCoverage`.
fn filled_path(f: &crate::paint::ImageFilled, w: f64, h: f64) -> String {
    use crate::paint::FillMethod;
    let a = f.amount.clamp(0.0, 1.0) as f64;
    let o = f.origin_index();
    let rect = |x: f64, y: f64, rw: f64, rh: f64| format!("M{} {}H{}V{}H{}Z", fmt_num(x), fmt_num(y), fmt_num(x + rw), fmt_num(y + rh), fmt_num(x));
    let (pivot, start, span) = match f.method {
        FillMethod::Horizontal => return if o == 0 { rect(0.0, 0.0, w * a, h) } else { rect(w * (1.0 - a), 0.0, w * a, h) },
        FillMethod::Vertical => return if o == 0 { rect(0.0, h * (1.0 - a), w, h * a) } else { rect(0.0, 0.0, w, h * a) },
        FillMethod::Radial90 => ([(0.0, h), (0.0, 0.0), (w, 0.0), (w, h)][o], o as f64 * 90.0, 90.0),
        FillMethod::Radial180 => ([(w / 2.0, h), (0.0, h / 2.0), (w / 2.0, 0.0), (w, h / 2.0)][o], [270.0, 0.0, 90.0, 180.0][o], 180.0),
        FillMethod::Radial360 => ((w / 2.0, h / 2.0), [180.0, 90.0, 0.0, 270.0][o], 360.0),
    };
    let (lo, hi) = if f.clockwise { (0.0, span * a) } else { (span * (1.0 - a), span) };
    // A wedge from the pivot, far enough out to cover the box.
    let r = 2.0 * w.hypot(h) + 1.0;
    let steps = ((hi - lo) / 5.0).ceil().max(1.0) as usize;
    let mut d = format!("M{} {}", fmt_num(pivot.0), fmt_num(pivot.1));
    for i in 0..=steps {
        let t = (start + lo + (hi - lo) * i as f64 / steps as f64).to_radians();
        let _ = write!(d, "L{} {}", fmt_num(pivot.0 + r * t.sin()), fmt_num(pivot.1 - r * t.cos()));
    }
    d.push('Z');
    d
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
            NodeType::Rectangle | NodeType::Frame | NodeType::Component | NodeType::Instance => {
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
            Paint::Image { .. } if attr == "fill" => {
                let content = self.image_content(p, w, h)?;
                let id = self.id("p");
                let _ = write!(
                    self.defs,
                    r#"<pattern id="{id}" patternUnits="userSpaceOnUse" width="{w}" height="{h}">{content}</pattern>"#,
                    w = fmt_num(w),
                    h = fmt_num(h),
                );
                Some(format!(r#" {attr}="url(#{id})""#))
            }
            _ => None,
        }
    }

    /// An image paint drawn in a w×h box at the origin (opacity and tint
    /// applied). Shapes clip it directly; patterns are only used where a fill
    /// attribute is unavoidable (text), since renderers rasterize a pattern
    /// tile first and cut off very large ones.
    fn image_content(&mut self, p: &Paint, w: f64, h: f64) -> Option<String> {
        match p {
            Paint::Image { asset, fit, opacity, tint, slice, crop, tile_scale, .. } => {
                let mut href = escape(&(self.opts.resolve_asset)(asset)?);
                // A crop makes the region the image. Viewports below point
                // their viewBox at the region inside the full image (offset by
                // `origin`), so nothing else of an atlas shows.
                let mut full = (self.opts.image_size)(asset);
                let mut region = full.and_then(|(fw, fh)| crop.and_then(|c| c.within(fw, fh)));
                let mut tint = tint.filter(|t| *t != Color::WHITE);
                // Better: the region cut out and the tint multiplied in, as an
                // image of its own.
                if region.is_some() || tint.is_some() {
                    if let Some(variant) = (self.opts.resolve_variant)(asset, region.as_ref(), tint) {
                        href = escape(&variant);
                        if let Some(c) = region.take() {
                            full = Some((c.width, c.height));
                        }
                        tint = None;
                    }
                }
                let (size, origin) = match region {
                    Some(c) => (Some((c.width, c.height)), (c.x, c.y)),
                    None => (full, (0.0, 0.0)),
                };
                let full_image = full.map(|(fw, fh)| format!(r#"<image href="{href}" width="{}" height="{}"/>"#, fmt_num(fw), fmt_num(fh)));
                let filter = match tint {
                    Some(t) => {
                        // Multiply each channel, like a GPU tint.
                        let fid = self.id("t");
                        let _ = write!(
                            self.defs,
                            // An explicit region: resvg misjudges the bounds of sliced
                            // content (nested viewports) and would draw nothing.
                            r#"<filter id="{fid}" filterUnits="userSpaceOnUse" x="0" y="0" width="{}" height="{}" color-interpolation-filters="sRGB"><feColorMatrix type="matrix" values="{} 0 0 0 0 0 {} 0 0 0 0 0 {} 0 0 0 0 0 {} 0"/></filter>"#,
                            fmt_num(w.max(1.0)),
                            fmt_num(h.max(1.0)),
                            fmt_num(t.r as f64),
                            fmt_num(t.g as f64),
                            fmt_num(t.b as f64),
                            fmt_num(t.a as f64)
                        );
                        format!(r#" filter="url(#{fid})""#)
                    }
                    _ => String::new(),
                };
                let sliced = slice.and_then(|s| size.zip(full).map(|(size, full)| (s, size, full)));
                let content = match sliced {
                    Some((s, (iw, ih), full)) => self.nine_slice(&href, &s, origin, full, iw, ih, w, h),
                    None => match (fit, slice, size) {
                        // Tiles from the bottom-left corner, like Unity.
                        (ImageFit::Tile, None, Some((iw, ih))) if iw > 0.0 && ih > 0.0 => {
                            let k = tile_scale.max(0.01) as f64;
                            let (tw, th) = (iw * k, ih * k);
                            let pid = self.id("p");
                            let _ = write!(
                                self.defs,
                                r#"<pattern id="{pid}" patternUnits="userSpaceOnUse" x="0" y="{}" width="{}" height="{}" viewBox="{} {} {} {}" preserveAspectRatio="none">{}</pattern>"#,
                                fmt_num(h),
                                fmt_num(tw),
                                fmt_num(th),
                                fmt_num(origin.0),
                                fmt_num(origin.1),
                                fmt_num(iw),
                                fmt_num(ih),
                                full_image.clone().unwrap_or_default()
                            );
                            format!(r#"<rect width="{}" height="{}" fill="url(#{pid})"/>"#, fmt_num(w), fmt_num(h))
                        }
                        // Cover: show just the centered crop through a viewport, so
                        // nothing spills outside the box (and no clip is needed).
                        (ImageFit::Fill, None, Some((iw, ih))) if iw > 0.0 && ih > 0.0 && w > 0.0 && h > 0.0 => {
                            let (cw, ch) = if iw / ih > w / h { (ih * w / h, ih) } else { (iw, iw * h / w) };
                            format!(
                                r#"<svg width="{}" height="{}" viewBox="{} {} {} {}" preserveAspectRatio="none">{}</svg>"#,
                                fmt_num(w),
                                fmt_num(h),
                                fmt_num(origin.0 + (iw - cw) / 2.0),
                                fmt_num(origin.1 + (ih - ch) / 2.0),
                                fmt_num(cw),
                                fmt_num(ch),
                                full_image.clone().unwrap_or_default()
                            )
                        }
                        _ => {
                            let par = match (fit, slice) {
                                (_, Some(_)) | (ImageFit::Stretch | ImageFit::Tile, _) => "none",
                                (ImageFit::Fill, _) => "xMidYMid slice",
                                (ImageFit::Fit, _) => "xMidYMid meet",
                            };
                            match (region, &full_image) {
                                (Some(c), Some(img)) => {
                                    // The viewport is exactly the drawn region.
                                    let (dw, dh) = match fit {
                                        ImageFit::Fit if slice.is_none() => {
                                            let k = (w / c.width).min(h / c.height);
                                            (c.width * k, c.height * k)
                                        }
                                        _ => (w, h),
                                    };
                                    format!(
                                        r#"<svg x="{}" y="{}" width="{}" height="{}" viewBox="{} {} {} {}" preserveAspectRatio="none">{img}</svg>"#,
                                        fmt_num((w - dw) / 2.0),
                                        fmt_num((h - dh) / 2.0),
                                        fmt_num(dw),
                                        fmt_num(dh),
                                        fmt_num(c.x),
                                        fmt_num(c.y),
                                        fmt_num(c.width),
                                        fmt_num(c.height)
                                    )
                                }
                                _ => format!(r#"<image href="{href}" width="{}" height="{}" preserveAspectRatio="{par}"/>"#, fmt_num(w), fmt_num(h)),
                            }
                        }
                    },
                };
                Some(format!(r#"<g opacity="{}"{filter}>{content}</g>"#, fmt_num(*opacity as f64)))
            }
            _ => None,
        }
    }

    /// A 9-slice image as nine cells, each a viewport onto one part of the image.
    /// Slices an `iw`×`ih` image, or region of one at `origin` in a `full`-sized image.
    #[allow(clippy::too_many_arguments)]
    fn nine_slice(&mut self, href: &str, s: &NineSlice, origin: (f64, f64), full: (f64, f64), iw: f64, ih: f64, w: f64, h: f64) -> String {
        let src = s.source_insets(iw, ih);
        let dst = s.dest_insets(src, w, h);
        let img = self.id("i");
        let _ = write!(self.defs, r#"<image id="{img}" href="{href}" width="{}" height="{}"/>"#, fmt_num(full.0), fmt_num(full.1));
        let sx = [0.0, src[3], iw - src[1], iw];
        let sy = [0.0, src[0], ih - src[2], ih];
        let dx = [0.0, dst[3], w - dst[1], w];
        let dy = [0.0, dst[0], h - dst[2], h];
        let mut out = String::new();
        for row in 0..3 {
            for col in 0..3 {
                if row == 1 && col == 1 && !s.fill_center {
                    continue;
                }
                let (mut x0, mut y0) = (sx[col], sy[row]);
                let (mut sw, mut sh) = (sx[col + 1] - x0, sy[row + 1] - y0);
                let (dw, dh) = (dx[col + 1] - dx[col], dy[row + 1] - dy[row]);
                if dw <= 0.0 || dh <= 0.0 {
                    continue;
                }
                // Borders that meet leave no middle to stretch: like Unity, stretch
                // the 1 px seam where they meet.
                if sw <= 0.0 {
                    if col != 1 || iw < 1.0 {
                        continue;
                    }
                    (x0, sw) = ((x0 - 0.5).clamp(0.0, iw - 1.0), 1.0);
                }
                if sh <= 0.0 {
                    if row != 1 || ih < 1.0 {
                        continue;
                    }
                    (y0, sh) = ((y0 - 0.5).clamp(0.0, ih - 1.0), 1.0);
                }
                let _ = write!(
                    out,
                    r##"<svg x="{}" y="{}" width="{}" height="{}" viewBox="{} {} {} {}" preserveAspectRatio="none"><use href="#{img}"/></svg>"##,
                    fmt_num(dx[col]),
                    fmt_num(dy[row]),
                    fmt_num(dw),
                    fmt_num(dh),
                    fmt_num(origin.0 + x0),
                    fmt_num(origin.1 + y0),
                    fmt_num(sw),
                    fmt_num(sh)
                );
            }
        }
        out
    }

    /// The fills of a closed shape, in its local coordinates.
    fn fills_markup(&mut self, n: &Node, shape: &str) -> String {
        let mut out = String::new();
        let fills: Vec<Paint> = n.fills.clone();
        for p in fills.iter().filter(|p| p.is_visible()) {
            let rule = if n.fill_rule == FillRule::Evenodd { r#" fill-rule="evenodd""# } else { "" };
            if matches!(p, Paint::Image { .. }) {
                let filled = match p {
                    Paint::Image { filled: Some(f), .. } if f.amount < 1.0 => Some(*f),
                    _ => None,
                };
                if filled.is_some_and(|f| f.amount <= 0.0) {
                    continue;
                }
                if let Some(content) = self.image_content(p, n.width, n.height) {
                    let content = match filled {
                        Some(f) => {
                            let clip = self.id("c");
                            let _ = write!(self.defs, r#"<clipPath id="{clip}"><path d="{}"/></clipPath>"#, filled_path(&f, n.width, n.height));
                            format!(r#"<g clip-path="url(#{clip})">{content}</g>"#)
                        }
                        None => content,
                    };
                    // 9-slice, stretch and fit never leave the box, so a plain
                    // rectangle needs no clip. (resvg cuts off very large content
                    // under several nested clips, e.g. a full-screen sliced overlay
                    // inside a clipped frame.)
                    let spills = match p {
                        Paint::Image { asset, fit: ImageFit::Fill, slice: None, .. } => (self.opts.image_size)(asset).is_none(),
                        _ => false,
                    };
                    let plain = shape.starts_with("<rect") && !shape.contains("rx=");
                    if plain && !spills {
                        out.push_str(&content);
                    } else {
                        let clip = self.id("c");
                        let clip_rule = if rule.is_empty() { "" } else { r#" clip-rule="evenodd""# };
                        let _ = write!(self.defs, r#"<clipPath id="{clip}">{shape}{clip_rule}/></clipPath>"#);
                        let _ = write!(out, r#"<g clip-path="url(#{clip})">{content}</g>"#);
                    }
                }
            } else if let Some(attr) = self.paint_attr(p, n.width, n.height, "fill") {
                let _ = write!(out, "{shape}{attr}{rule}/>");
            }
        }
        out
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
        // The region covers the box, half its size again for overflowing
        // content, and how far the effects reach (3σ of blur).
        let reach = effects
            .iter()
            .map(|e| match e {
                Effect::DropShadow { x, y, blur, spread, .. } => x.abs().max(y.abs()) as f64 + *blur as f64 * 1.5 + spread.max(0.0) as f64,
                Effect::LayerBlur { radius, .. } => *radius as f64 * 1.5,
            })
            .fold(0.0, f64::max);
        let m = (reach + n.width.abs().max(n.height.abs()) * 0.5 + 2.0).ceil();
        // sRGB blending, like the editor.
        let _ = write!(
            self.defs,
            r#"<filter id="{id}" filterUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" color-interpolation-filters="sRGB">"#,
            fmt_num(-m),
            fmt_num(-m),
            fmt_num(n.width.abs() + 2.0 * m),
            fmt_num(n.height.abs() + 2.0 * m)
        );
        // Each shadow is built from the source's alpha (grown or shrunk by
        // `spread`, blurred, offset, colored); all of them go under the source,
        // the first one lowest. A layer blur then blurs the result.
        let mut layers = Vec::new();
        let mut blur = None;
        for (i, e) in effects.iter().enumerate() {
            match e {
                Effect::DropShadow { color, x, y, blur: b, spread, .. } => {
                    let mut input = "SourceAlpha".to_string();
                    if spread.abs() > 0.01 {
                        let op = if *spread > 0.0 { "dilate" } else { "erode" };
                        let _ = write!(self.defs, r#"<feMorphology in="SourceAlpha" operator="{op}" radius="{}" result="s{i}m"/>"#, fmt_num(spread.abs() as f64));
                        input = format!("s{i}m");
                    }
                    // Blur 0 is a hard copy (Unity's Shadow); some viewers
                    // treat stdDeviation="0" as "draw nothing".
                    if *b > 0.0 {
                        let _ = write!(self.defs, r#"<feGaussianBlur in="{input}" stdDeviation="{}" result="s{i}b"/>"#, fmt_num(*b as f64 / 2.0));
                        input = format!("s{i}b");
                    }
                    let _ = write!(
                        self.defs,
                        r#"<feOffset in="{input}" dx="{}" dy="{}" result="s{i}o"/><feFlood flood-color="{}" flood-opacity="{}"/><feComposite in2="s{i}o" operator="in" result="s{i}"/>"#,
                        fmt_num(*x as f64),
                        fmt_num(*y as f64),
                        color.to_hex_rgb(),
                        fmt_num(color.a as f64)
                    );
                    layers.push(format!("s{i}"));
                }
                Effect::LayerBlur { radius, .. } => blur = Some(*radius),
            }
        }
        if !layers.is_empty() {
            self.defs.push_str("<feMerge>");
            for l in &layers {
                let _ = write!(self.defs, r#"<feMergeNode in="{l}"/>"#);
            }
            self.defs.push_str(r#"<feMergeNode in="SourceGraphic"/></feMerge>"#);
        }
        if let Some(r) = blur {
            let _ = write!(self.defs, r#"<feGaussianBlur stdDeviation="{}"/>"#, fmt_num(r as f64 / 2.0));
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
                    let closed = n.kind != NodeType::Line;
                    if closed && !n.hide_fill {
                        let fills = self.fills_markup(n, &shape);
                        self.body.push_str(&fills);
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
                                let cap = match n.stroke_cap {
                                    Some(StrokeCap::Round) => r#" stroke-linecap="round""#,
                                    Some(StrokeCap::Square) => r#" stroke-linecap="square""#,
                                    None if n.kind == NodeType::Line => r#" stroke-linecap="round""#,
                                    _ => "",
                                };
                                let join = match n.stroke_join {
                                    StrokeJoin::Round => r#" stroke-linejoin="round""#,
                                    StrokeJoin::Bevel => r#" stroke-linejoin="bevel""#,
                                    StrokeJoin::Miter => "",
                                };
                                let _ = write!(
                                    self.body,
                                    r#"{shape} fill="none"{attr} stroke-width="{}"{dash}{cap}{join}/>"#,
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
            let clip = n.kind.is_frame_like() && n.clips_content;
            if clip && n.clip_alpha {
                // Alpha mask from the fills. Any alpha above Unity's cutoff
                // (0.001) shows the children fully, like the editor's stencil.
                // Nearest sampling: resvg's smooth filters ring to zero next to
                // hard alpha edges, which would cut holes into the mask.
                let shape = self.shape(n).unwrap_or_default();
                let fills = self.fills_markup(n, &shape);
                let (id, fid) = (self.id("m"), self.id("f"));
                let (w, h) = (fmt_num(n.width), fmt_num(n.height));
                let _ = write!(
                    self.defs,
                    r#"<filter id="{fid}" filterUnits="userSpaceOnUse" x="0" y="0" width="{w}" height="{h}"><feComponentTransfer><feFuncA type="linear" slope="1000" intercept="0"/></feComponentTransfer></filter><mask id="{id}" mask-type="alpha" maskUnits="userSpaceOnUse" x="0" y="0" width="{w}" height="{h}"><g filter="url(#{fid})" image-rendering="optimizeSpeed">{fills}</g></mask>"#
                );
                let _ = write!(self.body, r#"<g mask="url(#{id})">"#);
            } else if clip {
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
        let fitted = match self.opts.measure_text {
            Some(m) => crate::text::fitted(n, &mut |s| m(n, s)),
            None => crate::text::fitted(n, &mut |s| crate::text::estimate_width(s, n.font_size, n.letter_spacing, n.font_weight)),
        };
        let n: &Node = &fitted;
        let lines = match self.opts.measure_text {
            Some(m) => crate::text::layout_lines(n, &mut |s| m(n, s)),
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
        let font = format!(
            r#"font-family="'{}', Inter, Helvetica, Arial, sans-serif" font-size="{}" font-weight="{}" text-anchor="{anchor}"{style}{deco}{ls}"#,
            // Quoted: names like "Exo 2" aren't valid unquoted CSS identifiers.
            escape(&n.font_family.replace(['\'', '"', '\\'], "")),
            fmt_num(n.font_size),
            n.font_weight,
        );
        let mut spans = String::new();
        for (i, line) in lines.iter().enumerate() {
            // Baseline: center the em box in the line box, ascent ≈ 0.8em.
            let baseline = y0 + lh * i as f64 + (lh - n.font_size) / 2.0 + n.font_size * 0.8;
            let _ = write!(
                spans,
                r#"<tspan x="{}" y="{}" xml:space="preserve">{}</tspan>"#,
                fmt_num(x),
                fmt_num(baseline),
                escape(line)
            );
        }
        // Text outline (stroke): drawn from separate copies so it renders the
        // same everywhere. OUTSIDE sits behind the fill, CENTER on top of it,
        // INSIDE on top but clipped to the letters.
        let stroke = if n.stroke_weight > 0.0 {
            n.strokes.iter().rev().find(|p| p.is_visible()).cloned().and_then(|p| self.paint_attr(&p, n.width, n.height, "stroke"))
        } else {
            None
        };
        let outline = |width: f64, attr: &str| {
            format!(
                r#"<text {font} fill="none"{attr} stroke-width="{}" stroke-linejoin="round" stroke-linecap="round">{spans}</text>"#,
                fmt_num(width)
            )
        };
        let filled = format!(r#"<text {font}{fill_attr}>{spans}</text>"#);
        let markup = match (stroke, n.stroke_align) {
            (None, _) => filled,
            (Some(attr), crate::paint::StrokeAlign::Outside) => outline(n.stroke_weight * 2.0, &attr) + &filled,
            (Some(attr), crate::paint::StrokeAlign::Center) => filled + &outline(n.stroke_weight, &attr),
            (Some(attr), crate::paint::StrokeAlign::Inside) => {
                let clip = self.id("tc");
                let _ = write!(self.defs, r#"<clipPath id="{clip}"><text {font}>{spans}</text></clipPath>"#);
                format!(r#"{filled}<g clip-path="url(#{clip})">{}</g>"#, outline(n.stroke_weight * 2.0, &attr))
            }
        };
        if n.text_overflow == crate::paint::TextOverflow::Clip {
            let clip = self.id("c");
            let _ = write!(self.defs, r#"<clipPath id="{clip}"><rect width="{}" height="{}"/></clipPath>"#, fmt_num(n.width), fmt_num(n.height));
            let _ = write!(self.body, r#"<g clip-path="url(#{clip})">{markup}</g>"#);
        } else {
            self.body.push_str(&markup);
        }
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

    #[test]
    fn exports_nine_slice_and_tint() {
        let mut doc = Document::new_empty();
        let mut r = Node::new("r:1", NodeType::Rectangle);
        r.parent = Some("0:1".into());
        r.width = 200.0;
        r.height = 60.0;
        r.set_prop(
            "fills",
            &serde_json::json!([{ "type": "IMAGE", "asset": "a1", "tint": "#ff000080", "slice": { "top": 8, "right": 12, "bottom": 8, "left": 12, "fillCenter": false } }]),
        )
        .unwrap();
        doc.apply(&Op::Create { node: r }).unwrap();
        let resolve = |id: &str| Some(format!("/api/assets/{id}"));
        let size = |_: &str| Some((48.0, 32.0));
        let opts = SvgOptions { resolve_asset: &resolve, image_size: &size, ..SvgOptions::default() };
        let (svg, _) = export(&doc, &["0:1".into()], &opts).unwrap();
        // Eight cells (no center): corners keep their size, the right column starts at 200 - 12.
        assert_eq!(svg.matches("<use href=").count(), 8);
        assert!(svg.contains(r#"<svg x="188" y="0" width="12" height="8" viewBox="36 0 12 8""#), "{svg}");
        assert!(svg.contains(r#"values="1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0.502 0""#), "{svg}");
        // Without the image size the image is stretched instead.
        let (plain, _) = export(&doc, &["0:1".into()], &SvgOptions { resolve_asset: &resolve, ..SvgOptions::default() }).unwrap();
        assert!(plain.contains(r#"preserveAspectRatio="none""#) && !plain.contains("<use"));
    }
}
