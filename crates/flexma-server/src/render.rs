//! Server-side rendering: SVG export (shared with the editor via flexma-doc) and
//! PNG rasterization through resvg, used for thumbnails, exports and the MCP
//! `get_screenshot` tool so agents can see what they built. Rasterizing takes
//! a font database so workspace fonts render (see `fonts::fontdb_for`).

use flexma_doc::svg::{export, SvgOptions};
use flexma_doc::{Color, Document, ImageCrop, NodeId, Rect};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub use resvg::usvg::fontdb::Database as FontDb;

/// Longest side of a dashboard thumbnail, px (cards are ~320 css px wide).
const THUMBNAIL_PX: u32 = 640;

/// Parse a stored document and derive its computed state (instances, layout).
pub fn load_document(json: &str) -> Option<Document> {
    let mut doc = Document::from_json(json).ok()?;
    flexma_doc::components::sync_all(&mut doc);
    flexma_doc::styles::sync_all(&mut doc);
    flexma_doc::layout::relayout_all(&mut doc);
    Some(doc)
}

pub fn page_background(doc: &Document, page: &str) -> Color {
    doc.get(page)
        .and_then(|p| p.background)
        .unwrap_or(Color::rgba(0.961, 0.961, 0.961, 1.0))
}

/// PNG of the first page for dashboard thumbnails and version previews.
/// `None` when the page is empty.
pub fn thumbnail_png(doc: &Document, data_dir: &Path, allowed: &HashSet<String>, fonts: Arc<FontDb>, paint: &PaintSource) -> Option<Vec<u8>> {
    let page = doc.pages().first()?.clone();
    if doc.children(&page).is_empty() {
        return None;
    }
    let bg = page_background(doc, &page);
    let mode = AssetMode::Inline(data_dir, allowed);
    let linear = linear_for(doc, std::slice::from_ref(&page));
    let resolve = |id: &str| resolve_asset(&mode, id);
    let size = |id: &str| mode.image_size(id);
    let variant = |id: &str, c: Option<&ImageCrop>, t: Option<Color>| image_variant(&mode, id, c, t, linear);
    let measurer = TextMeasurer::new(fonts.clone());
    let measure = |n: &flexma_doc::Node, s: &str| measurer.measure(n, s);
    let metrics = |n: &flexma_doc::Node| measurer.metrics(n);
    let runs = |n: &flexma_doc::Node, s: &str| measurer.svg_runs(n, s);
    let opts = SvgOptions { padding: 48.0, background: Some(bg), resolve_asset: &resolve, image_size: &size, measure_text: Some(&measure), font_metrics: Some(&metrics), font_runs: Some(&runs), resolve_variant: &variant, linear, paint_tiles: &|l| paint.tiles(l, true) };
    let (svg, _) = export(doc, &[page], &opts)?;
    svg_to_png(&svg, 2.0, THUMBNAIL_PX, fonts, linear, Some((data_dir, allowed))).ok().map(|(png, _, _)| png)
}

/// Where a file's paint-layer tiles are (see `paint.rs`).
pub struct PaintSource<'a> {
    pub data_dir: &'a Path,
    pub file_id: &'a str,
    /// Link tiles through this share link instead of the member URL.
    pub share_token: Option<&'a str>,
}

impl PaintSource<'_> {
    /// A layer's tiles as data URIs (for rasterizing here) or URLs (for browsers).
    fn tiles(&self, layer: &str, inline: bool) -> Vec<(i32, i32, String)> {
        crate::paint::tile_files(self.data_dir, self.file_id, layer)
            .into_iter()
            .filter_map(|(x, y, path)| {
                if inline {
                    use base64::Engine;
                    let bytes = std::fs::read(&path).ok()?;
                    Some((x, y, format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))))
                } else {
                    let kind = if path.to_string_lossy().ends_with(".view.png") { "view" } else { "base" };
                    let base = match self.share_token {
                        Some(t) => format!("/api/share/{t}/paint"),
                        None => format!("/api/files/{}/paint", self.file_id),
                    };
                    Some((x, y, format!("{base}/{}/{x}/{y}?kind={kind}", urlencode(layer))))
                }
            })
            .collect()
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

pub enum AssetMode<'a> {
    /// Reference assets by URL (for browsers).
    Url,
    /// Reference assets through a public share link (`/api/share/{token}/assets/{id}`).
    Shared(&'a str),
    /// Inline assets as data URIs (for rasterizing on the server): only the
    /// file's own workspace's images (`allowed`); any other id renders as
    /// missing, so a document can't pull in another workspace's pixels.
    Inline(&'a Path, &'a HashSet<String>),
}

impl AssetMode<'_> {
    /// Whether this render may use asset `id`.
    fn allows(&self, id: &str) -> bool {
        id.chars().all(|c| c.is_ascii_alphanumeric())
            && match self {
                AssetMode::Inline(_, allowed) => allowed.contains(id),
                // Browsers fetch these through routes that check access.
                AssetMode::Url | AssetMode::Shared(_) => true,
            }
    }

    /// Pixel size of uploaded image `id`, read from its header.
    fn image_size(&self, id: &str) -> Option<(f64, f64)> {
        if !self.allows(id) {
            return None;
        }
        let dir = match self {
            AssetMode::Inline(dir, _) => dir,
            AssetMode::Url | AssetMode::Shared(_) => ASSETS_DIR.get()?.as_path(),
        };
        let s = imagesize::size(asset_path(dir, id)).ok()?;
        Some((s.width as f64, s.height as f64))
    }
}

/// Export nodes (or a whole page) to SVG. With `fonts`, text breaks into
/// lines by the real font metrics instead of estimates.
pub fn export_svg(doc: &Document, ids: &[NodeId], background: Option<Color>, assets: AssetMode, fonts: Option<&Arc<FontDb>>, paint: &PaintSource) -> Option<(String, Rect)> {
    let inline = matches!(assets, AssetMode::Inline(..));
    // Linear blending only applies when the server rasterizes (inlined
    // images); browsers viewing an SVG blend in sRGB.
    let linear = inline && linear_for(doc, ids);
    let resolve = |id: &str| resolve_asset(&assets, id);
    let size = |id: &str| assets.image_size(id);
    let variant = |id: &str, c: Option<&ImageCrop>, t: Option<Color>| image_variant(&assets, id, c, t, linear);
    let measurer = fonts.map(|f| TextMeasurer::new(f.clone()));
    let measure = |n: &flexma_doc::Node, s: &str| measurer.as_ref().map(|m| m.measure(n, s)).unwrap_or(0.0);
    let metrics = |n: &flexma_doc::Node| measurer.as_ref().map(|m| m.metrics(n)).unwrap_or((n.font_size * 0.8, n.font_size * 0.2));
    let runs = |n: &flexma_doc::Node, s: &str| measurer.as_ref().map(|m| m.svg_runs(n, s)).unwrap_or_else(|| vec![(s.to_string(), None)]);
    let opts = SvgOptions {
        padding: 0.0,
        background,
        resolve_asset: &resolve,
        image_size: &size,
        measure_text: if measurer.is_some() { Some(&measure) } else { None },
        font_metrics: if measurer.is_some() { Some(&metrics) } else { None },
        font_runs: if measurer.is_some() { Some(&runs) } else { None },
        resolve_variant: &variant,
        linear,
        paint_tiles: &|l| paint.tiles(l, inline),
    };
    export(doc, ids, &opts)
}

/// Whether the page holding these nodes blends in linear light.
pub fn linear_for(doc: &Document, ids: &[NodeId]) -> bool {
    let Some(mut id) = ids.first().cloned() else { return false };
    loop {
        let Some(n) = doc.get(&id) else { return false };
        if n.kind == flexma_doc::NodeType::Page {
            return n.linear_blending;
        }
        match &n.parent {
            Some(p) => id = p.clone(),
            None => return false,
        }
    }
}

/// Encode a pixmap rendered in linear light back to sRGB.
pub fn encode_linear(pm: &mut resvg::tiny_skia::Pixmap) {
    for px in pm.pixels_mut() {
        let a = px.alpha();
        if a == 0 {
            continue;
        }
        let af = a as f32 / 255.0;
        let e = |v: u8| ((Color::linear_to_srgb((v as f32 / 255.0 / af).min(1.0)) * af * 255.0).round() as u8).min(a);
        *px = resvg::tiny_skia::PremultipliedColorU8::from_rgba(e(px.red()), e(px.green()), e(px.blue()), a).unwrap_or(*px);
    }
}

/// Text widths from the fonts the renderer will draw with: the node's family
/// (or the SVG's fallbacks: Inter, Helvetica, Arial, sans-serif) at its
/// weight and style, shaped like resvg draws them (kerning, ligatures).
pub struct TextMeasurer {
    db: Arc<FontDb>,
    faces: std::cell::RefCell<HashMap<(String, u16, bool), Option<FaceId>>>,
    /// Shaped width per string in em.
    widths: std::cell::RefCell<HashMap<(FaceId, String), f32>>,
    /// (ascent, descent) per face in em.
    metrics: std::cell::RefCell<HashMap<FaceId, (f32, f32)>>,
    /// Whether a face has a character.
    glyphs: std::cell::RefCell<HashMap<(FaceId, char), bool>>,
    /// The face that draws a character another face lacks: (face, char, as emoji).
    fallbacks: std::cell::RefCell<HashMap<(FaceId, char, bool), Option<FaceId>>>,
}

type FaceId = resvg::usvg::fontdb::ID;

/// The bundled emoji font (see `assets/fonts/twemoji/README.md`).
const EMOJI_FAMILY: &str = "Twemoji Mozilla";

/// Whether `c` (followed by `next`) shows as a color emoji in browsers:
/// pictographs, the symbols drawn as emoji by default, and anything with the
/// emoji variation selector U+FE0F.
fn is_emoji(c: char, next: Option<char>) -> bool {
    let u = c as u32;
    next == Some('\u{fe0f}')
        || (0x1F000..=0x1FAFF).contains(&u)
        || matches!(u, 0x231A..=0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD..=0x25FE | 0x2614..=0x2615 | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1
            | 0x26AA..=0x26AB | 0x26BD..=0x26BE | 0x26C4..=0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2..=0x26F3 | 0x26F5 | 0x26FA | 0x26FD | 0x2705
            | 0x270A..=0x270B | 0x2728 | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55)
}

/// Characters that belong to the one before them (variation selectors,
/// joiners, combining marks, skin tones, tags).
fn joins_previous(c: char) -> bool {
    matches!(c as u32, 0xFE00..=0xFE0F | 0x200D | 0x20E3 | 0x0300..=0x036F | 0x20D0..=0x20FF | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F)
}

impl TextMeasurer {
    pub fn new(db: Arc<FontDb>) -> TextMeasurer {
        TextMeasurer {
            db,
            faces: Default::default(),
            widths: Default::default(),
            metrics: Default::default(),
            glyphs: Default::default(),
            fallbacks: Default::default(),
        }
    }

    fn has_glyph(&self, id: FaceId, c: char) -> bool {
        if let Some(h) = self.glyphs.borrow().get(&(id, c)) {
            return *h;
        }
        let h = self.db.with_face_data(id, |data, index| ttf_parser::Face::parse(data, index).ok().and_then(|f| f.glyph_index(c)).is_some()).unwrap_or(false);
        self.glyphs.borrow_mut().insert((id, c), h);
        h
    }

    fn query(&self, family: &str, n: &flexma_doc::Node) -> Option<FaceId> {
        use resvg::usvg::fontdb::{Family, Query, Stretch, Style, Weight};
        self.db.query(&Query {
            families: &[Family::Name(family)],
            weight: Weight(n.font_weight),
            stretch: Stretch::Normal,
            style: if n.italic { Style::Italic } else { Style::Normal },
        })
    }

    /// The face for a character `primary` lacks (or an emoji): like browsers
    /// with the editor's font list, Inter first, then the emoji font, then
    /// any font that has it.
    fn fallback(&self, primary: FaceId, n: &flexma_doc::Node, c: char, emoji: bool) -> Option<FaceId> {
        let key = (primary, c, emoji);
        if let Some(f) = self.fallbacks.borrow().get(&key) {
            return *f;
        }
        let inter = self.query("Inter", n);
        let color = self.query(EMOJI_FAMILY, n);
        let order = if emoji { [color, inter] } else { [inter, color] };
        let found = order
            .into_iter()
            .flatten()
            .find(|id| self.has_glyph(*id, c))
            .or_else(|| self.db.faces().map(|f| f.id).find(|id| *id != primary && self.has_glyph(*id, c)));
        self.fallbacks.borrow_mut().insert(key, found);
        found
    }

    /// `s` in runs by the face that draws them: the node's face, except for
    /// characters it lacks and emoji, which come from a fallback face per
    /// character (not the whole text, as resvg would do on its own).
    pub fn runs(&self, n: &flexma_doc::Node, s: &str) -> Vec<(String, Option<FaceId>)> {
        let primary = self.face(n);
        let chars: Vec<char> = s.chars().collect();
        let mut out: Vec<(String, Option<FaceId>)> = Vec::new();
        for (i, &c) in chars.iter().enumerate() {
            let face = match out.last() {
                Some((_, f)) if joins_previous(c) => *f,
                _ => match primary {
                    Some(p) => {
                        let emoji = is_emoji(c, chars.get(i + 1).copied());
                        if !emoji && (c.is_whitespace() || self.has_glyph(p, c)) {
                            Some(p)
                        } else {
                            self.fallback(p, n, c, emoji).or(Some(p))
                        }
                    }
                    None => None,
                },
            };
            match out.last_mut() {
                Some((t, f)) if *f == face => t.push(c),
                _ => out.push((c.to_string(), face)),
            }
        }
        out
    }

    /// The family name a face goes by.
    pub fn family(&self, id: FaceId) -> Option<String> {
        self.db.face(id).and_then(|f| f.families.first().map(|(name, _)| name.clone()))
    }

    /// Runs for an SVG: text with the family of each fallback run (`None`
    /// for the node's own font).
    pub fn svg_runs(&self, n: &flexma_doc::Node, s: &str) -> Vec<(String, Option<String>)> {
        let primary = self.face(n);
        self.runs(n, s).into_iter().map(|(t, f)| (t, f.filter(|f| Some(*f) != primary).and_then(|f| self.family(f)))).collect()
    }

    /// The face's (ascent, descent) in px at the node's size, from the table
    /// browsers use: OS/2 typo when USE_TYPO_METRICS is set, otherwise hhea
    /// (ttf-parser falls back to typo, then win, when hhea is empty). Rounded
    /// to whole px like Chrome, whose values the editor places text with.
    pub fn metrics(&self, n: &flexma_doc::Node) -> (f64, f64) {
        let em = self.face(n).and_then(|id| {
            if let Some(m) = self.metrics.borrow().get(&id) {
                return Some(*m);
            }
            let m = self
                .db
                .with_face_data(id, |data, index| {
                    let face = ttf_parser::Face::parse(data, index).ok()?;
                    let upem = face.units_per_em().max(1) as f32;
                    let (asc, desc) = (face.ascender() as f32 / upem, -(face.descender() as f32) / upem);
                    (asc > 0.0 && asc + desc > 0.0).then_some((asc, desc))
                })
                .flatten()?;
            self.metrics.borrow_mut().insert(id, m);
            Some(m)
        });
        match em {
            Some((asc, desc)) => ((asc as f64 * n.font_size).round(), (desc as f64 * n.font_size).round()),
            None => (n.font_size * 0.8, n.font_size * 0.2),
        }
    }

    fn face(&self, n: &flexma_doc::Node) -> Option<resvg::usvg::fontdb::ID> {
        use resvg::usvg::fontdb::{Family, Query, Stretch, Style, Weight};
        let key = (n.font_family.clone(), n.font_weight, n.italic);
        if let Some(id) = self.faces.borrow().get(&key) {
            return *id;
        }
        let families = [Family::Name(&n.font_family), Family::Name("Inter"), Family::Name("Helvetica"), Family::Name("Arial"), Family::SansSerif];
        let id = self.db.query(&Query {
            families: &families,
            weight: Weight(n.font_weight),
            stretch: Stretch::Normal,
            style: if n.italic { Style::Italic } else { Style::Normal },
        });
        self.faces.borrow_mut().insert(key, id);
        id
    }

    /// Whether the node's own font family is here (not a fallback).
    pub fn has_font(&self, n: &flexma_doc::Node) -> bool {
        self.face(n).and_then(|id| self.db.face(id)).is_some_and(|f| f.families.iter().any(|(name, _)| name.eq_ignore_ascii_case(&n.font_family)))
    }

    /// The box a text node should have for its auto-resize mode, like the
    /// editor's `measure_box` (flexma-engine text.rs).
    pub fn measure_box(&self, n: &flexma_doc::Node) -> (f64, f64) {
        if flexma_doc::text::has_runs(n) {
            let lines = flexma_doc::text::rich_layout(n, &mut |st, s| self.measure(st, s));
            let h = (lines.len().max(1) as f64 * n.effective_line_height()).ceil();
            return match n.text_auto_resize {
                flexma_doc::TextAutoResize::WidthAndHeight => (lines.iter().map(|l| l.width).fold(0.0, f64::max).ceil().max(1.0), h),
                flexma_doc::TextAutoResize::Height => (n.width, h),
                flexma_doc::TextAutoResize::None => (n.width, n.height),
            };
        }
        let lines = flexma_doc::text::layout_lines(n, &mut |s| self.measure(n, s));
        let h = (lines.len().max(1) as f64 * n.effective_line_height()).ceil();
        match n.text_auto_resize {
            flexma_doc::TextAutoResize::WidthAndHeight => {
                let w = lines.iter().map(|l| self.measure(n, l)).fold(0.0, f64::max);
                (w.ceil().max(1.0), h)
            }
            flexma_doc::TextAutoResize::Height => (n.width, h),
            flexma_doc::TextAutoResize::None => (n.width, n.height),
        }
    }

    pub fn measure(&self, n: &flexma_doc::Node, s: &str) -> f64 {
        if self.face(n).is_none() {
            return flexma_doc::text::estimate_width(s, n.font_size, n.letter_spacing, n.font_weight);
        }
        let mut em = 0.0;
        for (run, face) in self.runs(n, s) {
            let Some(id) = face else { continue };
            let key = (id, run);
            let cached = self.widths.borrow().get(&key).copied();
            em += match cached {
                Some(w) => w,
                None => {
                    let w = self.db.with_face_data(id, |data, index| shaped_width(data, index, &key.1)).flatten();
                    let w = w.unwrap_or_else(|| flexma_doc::text::estimate_width(&key.1, 1.0, 0.0, n.font_weight) as f32);
                    let mut widths = self.widths.borrow_mut();
                    if widths.len() > 20_000 {
                        widths.clear();
                    }
                    widths.insert(key, w);
                    w
                }
            };
        }
        em as f64 * n.font_size + n.letter_spacing * s.chars().count() as f64
    }
}

/// Width of `s` in em, shaped with the face like resvg (default features, so
/// kerning and ligatures). Characters the face lacks are drawn from another
/// font; they're guessed like the estimate.
fn shaped_width(data: &[u8], index: u32, s: &str) -> Option<f32> {
    let face = rustybuzz::Face::from_slice(data, index)?;
    let upem = face.units_per_em().max(1) as f32;
    let mut buf = rustybuzz::UnicodeBuffer::new();
    buf.push_str(s);
    let out = rustybuzz::shape(&face, &[], buf);
    let mut w = 0.0;
    for (info, pos) in out.glyph_infos().iter().zip(out.glyph_positions()) {
        w += if info.glyph_id == 0 {
            let c = s.get(info.cluster as usize..).and_then(|t| t.chars().next()).unwrap_or(' ');
            flexma_doc::text::estimate_width(c.encode_utf8(&mut [0; 4]), 1.0, 0.0, 400) as f32
        } else {
            pos.x_advance as f32 / upem
        };
    }
    Some(w)
}

/// An inlined image with its crop cut out (whole pixels) and/or its tint
/// multiplied in. Server renders use these instead of viewports and filters:
/// smoothing never reaches past a sprite, and resvg can't drop a tint on a
/// large or sliced layer. Returns a `flexma-variant:` href that names all it
/// takes to make the PNG, which the resolver does when it rasterizes (see
/// `variant_png`).
fn image_variant(assets: &AssetMode, id: &str, crop: Option<&ImageCrop>, tint: Option<Color>, linear: bool) -> Option<String> {
    let AssetMode::Inline(..) = assets else { return None };
    // Checked before the cache, which every workspace shares.
    let (fw, fh) = assets.image_size(id)?;
    let (x, y, w, h) = match crop {
        Some(c) => (c.x.round(), c.y.round(), c.width.round().max(1.0), c.height.round().max(1.0)),
        None => (0.0, 0.0, fw, fh),
    };
    let tint = tint.map(|t| t.to_hex().trim_start_matches('#').to_string()).unwrap_or_default();
    Some(format!("flexma-variant:{id}:{x}:{y}:{w}:{h}:{fw}:{fh}:{tint}:{}", u8::from(linear)))
}

/// The PNG a `flexma-variant:` href names, from the cache or made now. It
/// is made from the href alone, so a variant the size-bounded cache dropped
/// between writing an SVG and rasterizing it (other renders filled it) is
/// made again instead of going missing.
fn variant_png(dir: &Path, allowed: &HashSet<String>, href: &str) -> Option<Arc<Vec<u8>>> {
    if let Some(png) = variants().lock().ok()?.get(href) {
        return Some(png);
    }
    let parts: Vec<&str> = href.strip_prefix("flexma-variant:")?.split(':').collect();
    let [id, x, y, w, h, fw, fh, tint, linear] = parts[..] else { return None };
    let num = |v: &str| v.parse::<f64>().ok().filter(|n| n.is_finite());
    let (x, y, w, h, fw, fh) = (num(x)?, num(y)?, num(w)?, num(h)?, num(fw)?, num(fh)?);
    let tint = if tint.is_empty() { None } else { Some(Color::parse(&format!("#{tint}"))?) };
    let linear = linear == "1";
    // Render the region 1:1 with nearest sampling: an exact pixel copy.
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}"><image href="flexma-asset:{id}" width="{fw}" height="{fh}" image-rendering="optimizeSpeed"/></svg>"#
    );
    let mut opt = svg_options();
    opt.image_href_resolver.resolve_string = image_resolver(Some((dir, allowed)));
    let tree = resvg::usvg::Tree::from_str(&svg, &opt).ok()?;
    let mut pm = resvg::tiny_skia::Pixmap::new(w as u32, h as u32)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
    if linear {
        // Linear light, for a page that blends that way.
        for px in pm.pixels_mut() {
            let a = px.alpha();
            if a == 0 {
                continue;
            }
            let af = a as f32 / 255.0;
            let l = |v: u8| ((Color::srgb_to_linear((v as f32 / 255.0 / af).min(1.0)) * af * 255.0).round() as u8).min(a);
            *px = resvg::tiny_skia::PremultipliedColorU8::from_rgba(l(px.red()), l(px.green()), l(px.blue()), a).unwrap_or(*px);
        }
    }
    if let Some(t) = tint {
        // Premultiplied: color × tint color × tint alpha, alpha × tint alpha.
        let t = if linear { t.to_linear() } else { t };
        let (r, g, b, a) = (t.r * t.a, t.g * t.a, t.b * t.a, t.a);
        for px in pm.pixels_mut() {
            let c = |v: u8, k: f32| (v as f32 * k).round().clamp(0.0, 255.0) as u8;
            let alpha = c(px.alpha(), a);
            *px = resvg::tiny_skia::PremultipliedColorU8::from_rgba(c(px.red(), r).min(alpha), c(px.green(), g).min(alpha), c(px.blue(), b).min(alpha), alpha)
                .unwrap_or(resvg::tiny_skia::PremultipliedColorU8::TRANSPARENT);
        }
    }
    let png = Arc::new(pm.encode_png().ok()?);
    if let Ok(mut cache) = variants().lock() {
        cache.insert(href.to_string(), png.clone());
    }
    Some(png)
}

/// Rendered image variants (PNG), by href. Bounded by total size: when full
/// it starts over (a variant is cheap to remake; memory on a small server isn't).
struct Variants {
    by_href: HashMap<String, Arc<Vec<u8>>>,
    bytes: usize,
}

const VARIANT_CACHE_BYTES: usize = 24 << 20;

impl Variants {
    fn get(&self, href: &str) -> Option<Arc<Vec<u8>>> {
        self.by_href.get(href).cloned()
    }

    fn insert(&mut self, href: String, png: Arc<Vec<u8>>) {
        if self.bytes + png.len() > VARIANT_CACHE_BYTES {
            self.by_href.clear();
            self.bytes = 0;
        }
        self.bytes += png.len();
        if let Some(old) = self.by_href.insert(href, png) {
            self.bytes -= old.len();
        }
    }
}

fn variants() -> &'static std::sync::Mutex<Variants> {
    static V: OnceLock<std::sync::Mutex<Variants>> = OnceLock::new();
    V.get_or_init(|| std::sync::Mutex::new(Variants { by_href: HashMap::new(), bytes: 0 }))
}

/// The uploaded images a render may load: the data dir and the ids allowed.
pub type Images<'a> = Option<(&'a Path, &'a HashSet<String>)>;

/// Resolves the image hrefs server renders write: `flexma-asset:<id>` reads
/// the uploaded file, `flexma-variant:<id>:<params>` a variant of it (see `variant_png`),
/// both only for allowed ids. Nothing else loads (no paths from disk, no
/// other schemes).
fn image_resolver(images: Images<'_>) -> resvg::usvg::ImageHrefStringResolverFn<'_> {
    // A sprite used many times is read (and held) once.
    let read: std::sync::Mutex<HashMap<String, Arc<Vec<u8>>>> = Default::default();
    Box::new(move |href: &str, _: &resvg::usvg::Options| {
        let (dir, allowed) = images?;
        let allows = |id: &str| id.chars().all(|c| c.is_ascii_alphanumeric()) && allowed.contains(id);
        let bytes = if let Some(id) = href.strip_prefix("flexma-asset:") {
            if !allows(id) {
                return None;
            }
            let mut read = read.lock().ok()?;
            match read.get(id) {
                Some(b) => b.clone(),
                None => {
                    let b = Arc::new(std::fs::read(asset_path(dir, id)).ok()?);
                    read.insert(id.to_string(), b.clone());
                    b
                }
            }
        } else if let Some((id, _)) = href.strip_prefix("flexma-variant:").and_then(|v| v.split_once(':')) {
            // The cache is shared by every workspace.
            if !allows(id) {
                return None;
            }
            variant_png(dir, allowed, href)?
        } else {
            return None;
        };
        use resvg::usvg::ImageKind;
        match sniff_mime(&bytes)? {
            "image/png" => Some(ImageKind::PNG(bytes)),
            "image/jpeg" => Some(ImageKind::JPEG(bytes)),
            "image/gif" => Some(ImageKind::GIF(bytes)),
            "image/webp" => Some(ImageKind::WEBP(bytes)),
            _ => None,
        }
    })
}

/// Rasterizing is memory-hungry, so one render runs at a time (exports,
/// thumbnails, version previews and agent screenshots alike).
pub static RENDERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Give memory freed by a render back to the system (glibc keeps it
/// otherwise, and a long-running server creeps up to its memory limit).
pub fn release_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: malloc_trim only hands free heap pages back to the OS.
        unsafe {
            malloc_trim(0);
        }
    }
}

/// usvg options for every SVG the server parses: an `<image href>` that names
/// a path is ignored instead of read from the server's disk (usvg's default),
/// so only images embedded as data URIs load.
pub fn svg_options() -> resvg::usvg::Options<'static> {
    resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: resvg::usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..resvg::usvg::Options::default()
    }
}

static ASSETS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where uploaded images live, so exports can read their pixel size (9-slice
/// fills need it). Set once at startup.
pub fn set_data_dir(data_dir: &Path) {
    let _ = ASSETS_DIR.set(data_dir.to_path_buf());
}


fn resolve_asset(assets: &AssetMode, id: &str) -> Option<String> {
    if !assets.allows(id) {
        return None;
    }
    match assets {
        AssetMode::Url => Some(format!("/api/assets/{id}")),
        AssetMode::Shared(token) => Some(format!("/api/share/{token}/assets/{id}")),
        // Read when rasterized (`image_resolver`), never copied into the SVG.
        AssetMode::Inline(dir, _) => asset_path(dir, id).exists().then(|| format!("flexma-asset:{id}")),
    }
}

pub fn asset_path(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("assets").join(id)
}

pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The editor's Inter (see `assets/fonts/inter/README.md`).
static INTER: [&[u8]; 18] = [
    include_bytes!("../assets/fonts/inter/Inter-100.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-100-Italic.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-200.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-200-Italic.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-300.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-300-Italic.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-400.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-400-Italic.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-500.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-500-Italic.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-600.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-600-Italic.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-700.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-700-Italic.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-800.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-800-Italic.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-900.ttf"),
    include_bytes!("../assets/fonts/inter/Inter-900-Italic.ttf"),
];

/// Color emoji for server renders (see `assets/fonts/twemoji/README.md`).
static TWEMOJI: &[u8] = include_bytes!("../assets/fonts/twemoji/Twemoji.Mozilla.ttf");

/// The bundled Inter and emoji font, then system fonts (plus
/// `FLEXMA_FONTS_DIR`), shared by every workspace. Inter goes first so it wins
/// over an installed Inter of another version, whose widths would wrap lines
/// differently from the editor.
pub fn base_fontdb() -> Arc<FontDb> {
    static DB: OnceLock<Arc<FontDb>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = FontDb::new();
        for data in INTER {
            db.load_font_source(resvg::usvg::fontdb::Source::Binary(Arc::new(data)));
        }
        db.load_font_source(resvg::usvg::fontdb::Source::Binary(Arc::new(TWEMOJI)));
        db.load_system_fonts();
        if let Ok(dir) = std::env::var("FLEXMA_FONTS_DIR") {
            db.load_fonts_dir(dir);
        }
        db.set_sans_serif_family("Inter");
        Arc::new(db)
    })
    .clone()
}

/// Rasterize SVG to PNG at `scale`, clamped so the longest side is ≤ `max_dim`.
/// Put a render over a grey checkerboard, so transparent areas (and white
/// content on them) show, like design tools draw transparency.
/// `y0`: the row `pm` starts at in the whole image, for strips.
pub fn over_checkerboard(pm: &resvg::tiny_skia::Pixmap, y0: u32) -> resvg::tiny_skia::Pixmap {
    const CELL: u32 = 8;
    let (w, h) = (pm.width(), pm.height());
    let mut out = resvg::tiny_skia::Pixmap::new(w, h).unwrap_or_else(|| pm.clone());
    // Mid greys, so both white and dark content stand out.
    let (light, dark) = (resvg::tiny_skia::PremultipliedColorU8::from_rgba(196, 196, 202, 255).unwrap(), resvg::tiny_skia::PremultipliedColorU8::from_rgba(150, 150, 158, 255).unwrap());
    for (i, px) in out.pixels_mut().iter_mut().enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w + y0);
        *px = if (x / CELL + y / CELL) % 2 == 0 { light } else { dark };
    }
    out.draw_pixmap(0, 0, pm.as_ref(), &resvg::tiny_skia::PixmapPaint::default(), resvg::tiny_skia::Transform::identity(), None);
    out
}

/// Whether a screenshot of this node should show transparency as a
/// checkerboard: it isn't a page and has no opaque solid fill of its own.
pub fn wants_checkerboard(doc: &Document, ids: &[NodeId]) -> bool {
    let [id] = ids else { return false };
    let Some(n) = doc.get(id) else { return false };
    if matches!(n.kind, flexma_doc::NodeType::Page | flexma_doc::NodeType::Document) {
        return false;
    }
    let opaque = !n.hide_fill
        && n.opacity >= 0.999
        && n.fills.iter().any(|p| matches!(p, flexma_doc::Paint::Solid { color, opacity, visible: true } if color.a * opacity >= 0.999));
    !opaque
}

/// An uploaded SVG as a PNG to use as an image: sharp when drawn larger than
/// its own size (2× to 8×, aiming at 2048 px), 4096 px at most. Returns the
/// PNG and the SVG's own size in px.
pub fn rasterize_svg(svg: &str, fonts: Arc<FontDb>) -> Result<(Vec<u8>, f64, f64), String> {
    let mut opt = svg_options();
    opt.fontdb = fonts;
    opt.font_family = "Inter".into();
    let tree = resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    let (w, h) = (tree.size().width(), tree.size().height());
    let longest = w.max(h).max(1.0);
    let scale = (2048.0 / longest).clamp(2.0, 8.0);
    let (png, _, _) = tree_to_png(&tree, scale, 4096, false, false)?;
    Ok((png, w as f64, h as f64))
}

pub fn svg_to_png(svg: &str, scale: f32, max_dim: u32, fonts: Arc<FontDb>, linear: bool, images: Images) -> Result<(Vec<u8>, u32, u32), String> {
    svg_to_png_on(svg, scale, max_dim, fonts, linear, false, images)
}

/// Bigger images render in horizontal strips that stream into the PNG, so
/// memory stays bounded however large the export. resvg gives a clipped or
/// translucent group a layer up to 5× the canvas around it (clipped to the
/// group), so a strip is short enough for such a layer to stay this size.
const LAYER_PIXELS: u32 = 4 << 20;

/// Pixel size (and the scale used) of a `width` × `height` render at `scale`,
/// shrunk so the longest side is at most `max_dim`.
pub fn png_size(width: f32, height: f32, scale: f32, max_dim: u32) -> (u32, u32, f32) {
    let mut s = scale.clamp(0.01, 8.0);
    let longest = width.max(height) * s;
    if longest > max_dim as f32 {
        s *= max_dim as f32 / longest;
    }
    (((width * s).ceil() as u32).max(1), ((height * s).ceil() as u32).max(1), s)
}

/// Rows per strip for an image `w` × `h` (all of it when it's small).
fn strip_rows(w: u32, h: u32) -> u32 {
    if w.saturating_mul(h) <= LAYER_PIXELS {
        return h;
    }
    (LAYER_PIXELS / (5 * w.max(1))).max(16).min(h)
}

/// `svg_to_png`, optionally over a checkerboard (agent screenshots of
/// transparent layers). `images`: the uploaded images it may load.
pub fn svg_to_png_on(svg: &str, scale: f32, max_dim: u32, fonts: Arc<FontDb>, linear: bool, checker: bool, images: Images) -> Result<(Vec<u8>, u32, u32), String> {
    let mut opt = svg_options();
    opt.fontdb = fonts;
    opt.font_family = "Inter".into();
    // Exports already give each character a font that has it
    // (`TextMeasurer::svg_runs`). resvg's own fallback shapes a whole line
    // with each span's font and, when one font has every character, swaps
    // the entire span to it (the first face with the character, often a
    // thin weight), so it stays off.
    opt.font_resolver.select_fallback = Box::new(|_, _, _| None);
    opt.image_href_resolver.resolve_string = image_resolver(images);
    let tree = resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    tree_to_png(&tree, scale, max_dim, linear, checker)
}

/// Draw a parsed SVG to PNG in strips (see `svg_to_png_on`).
fn tree_to_png(tree: &resvg::usvg::Tree, scale: f32, max_dim: u32, linear: bool, checker: bool) -> Result<(Vec<u8>, u32, u32), String> {
    let (w, h, s) = png_size(tree.size().width(), tree.size().height(), scale, max_dim);
    let strip = strip_rows(w, h);
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        let mut out = wr.stream_writer().map_err(|e| e.to_string())?;
        let mut y0 = 0;
        while y0 < h {
            let sh = strip.min(h - y0);
            let mut pm = resvg::tiny_skia::Pixmap::new(w, sh).ok_or("image too large")?;
            resvg::render(tree, resvg::tiny_skia::Transform::from_row(s, 0.0, 0.0, s, 0.0, -(y0 as f32)), &mut pm.as_mut());
            if linear {
                encode_linear(&mut pm);
            }
            if checker {
                pm = over_checkerboard(&pm, y0);
            }
            use std::io::Write;
            out.write_all(&straight_rgba(pm)).map_err(|e| e.to_string())?;
            y0 += sh;
        }
        out.finish().map_err(|e| e.to_string())?;
        wr.finish().map_err(|e| e.to_string())?;
    }
    Ok((png, w, h))
}

/// A pixmap's pixels as straight (not premultiplied) RGBA, for PNG.
fn straight_rgba(pm: resvg::tiny_skia::Pixmap) -> Vec<u8> {
    let mut data = pm.take();
    for px in data.chunks_exact_mut(4) {
        let c = resvg::tiny_skia::PremultipliedColorU8::from_rgba(px[0], px[1], px[2], px[3]).map(|c| c.demultiply());
        if let Some(c) = c {
            px.copy_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
    }
    data
}

/// One image of several renders in a labelled grid (a contact sheet), so an
/// agent can compare screens at a glance. `tiles` are (label, svg). Returns
/// the PNG, its size and each tile's rect in it.
pub fn contact_sheet(tiles: &[(String, String, bool, bool)], scale: f32, max_dim: u32, fonts: Arc<FontDb>, images: Images) -> Result<(Vec<u8>, u32, u32, Vec<[u32; 4]>), String> {
    const PAD: f32 = 16.0;
    const LABEL: f32 = 22.0;
    let mut opt = svg_options();
    opt.fontdb = fonts;
    opt.font_family = "Inter".into();
    opt.image_href_resolver.resolve_string = image_resolver(images);
    let trees: Vec<resvg::usvg::Tree> = tiles.iter().map(|(_, svg, _, _)| resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    if trees.is_empty() {
        return Err("nothing to render".into());
    }
    // Shelf packing: tiles keep their own sizes, left to right in rows about
    // as wide as a square sheet would be, all shrunk together to fit max_dim.
    let place = |s: f32| -> (Vec<(f32, f32)>, f32, f32) {
        // A slot is at least as wide as its label (13 px text ≈ 7.2 px a character).
        let sizes: Vec<(f32, f32)> = trees
            .iter()
            .zip(tiles)
            .map(|(t, (label, _, _, _))| ((t.size().width() * s).max(label.chars().take(80).count() as f32 * 7.2 + 8.0), t.size().height() * s))
            .collect();
        let area: f32 = sizes.iter().map(|(w, h)| (w + PAD) * (h + LABEL + PAD)).sum();
        let widest = sizes.iter().map(|z| z.0).fold(0.0, f32::max);
        let row_w = widest.max(area.sqrt() * 1.2);
        let (mut x, mut y, mut row_h, mut sheet_w) = (PAD, PAD, 0.0f32, 0.0f32);
        let mut pos = Vec::new();
        for (w, h) in &sizes {
            if x > PAD && x + w > row_w + PAD {
                x = PAD;
                y += row_h + LABEL + PAD;
                row_h = 0.0;
            }
            pos.push((x, y));
            x += w + PAD;
            sheet_w = sheet_w.max(x);
            row_h = row_h.max(*h);
        }
        (pos, sheet_w, y + row_h + LABEL + PAD)
    };
    let mut s = scale.clamp(0.01, 8.0);
    for _ in 0..4 {
        let (_, sw, sh) = place(s);
        let longest = sw.max(sh);
        if longest <= max_dim as f32 {
            break;
        }
        s *= (max_dim as f32 / longest) * 0.98;
    }
    let (pos, sw, sh) = place(s);
    let (w, h) = ((sw.ceil() as u32).min(max_dim.max(1) * 2), (sh.ceil() as u32).min(max_dim.max(1) * 2));
    let mut pm = resvg::tiny_skia::Pixmap::new(w, h).ok_or("image too large")?;
    pm.fill(resvg::tiny_skia::Color::from_rgba8(232, 233, 237, 255));
    let mut rects = Vec::new();
    let mut labels = String::new();
    for ((tree, (label, _, linear, checker)), (x, y)) in trees.iter().zip(tiles).zip(pos) {
        let tw = ((tree.size().width() * s).ceil() as u32).max(1);
        let th = ((tree.size().height() * s).ceil() as u32).max(1);
        let mut tile = resvg::tiny_skia::Pixmap::new(tw, th).ok_or("image too large")?;
        resvg::render(tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut tile.as_mut());
        if *linear {
            encode_linear(&mut tile);
        }
        if *checker {
            tile = over_checkerboard(&tile, 0);
        }
        let ty = (y + LABEL).round() as i32;
        pm.draw_pixmap(x.round() as i32, ty, tile.as_ref(), &resvg::tiny_skia::PixmapPaint::default(), resvg::tiny_skia::Transform::identity(), None);
        rects.push([x.round() as u32, ty as u32, tw, th]);
        let text: String = label.chars().take(80).collect();
        labels.push_str(&format!(
            r##"<text x="{}" y="{}" font-size="13" font-family="Inter, Helvetica, Arial, sans-serif" fill="#3a3d46">{}</text>"##,
            x,
            y + 15.0,
            flexma_doc::svg::escape(&text)
        ));
    }
    let overlay = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">{labels}</svg>"#);
    if let Ok(t) = resvg::usvg::Tree::from_str(&overlay, &opt) {
        resvg::render(&t, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
    }
    let png = pm.encode_png().map_err(|e| e.to_string())?;
    Ok((png, w, h, rects))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flexma_doc::{Node, NodeType, Op};
    use resvg::tiny_skia::{Color as SkColor, Paint, Pixmap, Rect as SkRect, Transform};

    /// A 30×30 image: red 10px corners, green edges, blue center.
    fn sprite() -> String {
        let mut p = Pixmap::new(30, 30).unwrap();
        let mut fill = |x: f32, y: f32, w: f32, h: f32, c: SkColor| {
            let mut paint = Paint::default();
            paint.set_color(c);
            p.fill_rect(SkRect::from_xywh(x, y, w, h).unwrap(), &paint, Transform::identity(), None);
        };
        fill(0.0, 0.0, 30.0, 30.0, SkColor::from_rgba8(0, 255, 0, 255));
        fill(10.0, 10.0, 10.0, 10.0, SkColor::from_rgba8(0, 0, 255, 255));
        for (x, y) in [(0.0, 0.0), (20.0, 0.0), (0.0, 20.0), (20.0, 20.0)] {
            fill(x, y, 10.0, 10.0, SkColor::from_rgba8(255, 0, 0, 255));
        }
        use base64::Engine;
        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(p.encode_png().unwrap()))
    }

    fn render(fill: serde_json::Value) -> Pixmap {
        let mut doc = Document::new_empty();
        let mut r = Node::new("r:1", NodeType::Rectangle);
        r.parent = Some("0:1".into());
        r.width = 100.0;
        r.height = 50.0;
        r.set_prop("fills", &serde_json::json!([fill])).unwrap();
        doc.apply(&Op::Create { node: r }).unwrap();
        let data = sprite();
        let resolve = |_: &str| Some(data.clone());
        let size = |_: &str| Some((30.0, 30.0));
        let opts = SvgOptions { resolve_asset: &resolve, image_size: &size, ..SvgOptions::default() };
        let (svg, _) = export(&doc, &["r:1".into()], &opts).unwrap();
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        let mut pixmap = Pixmap::new(100, 50).unwrap();
        resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
        pixmap
    }

    /// Pixel color, rounded to the nearest of 0 / 255 per channel (image
    /// filtering blends a little across slice seams).
    fn rgb(p: &Pixmap, x: u32, y: u32) -> [u8; 3] {
        let c = p.pixel(x, y).unwrap();
        [c.red(), c.green(), c.blue()].map(|v| if v > 127 { 255 } else { 0 })
    }

    #[test]
    fn rasterizes_nine_slice_and_tint() {
        let p = render(serde_json::json!({ "type": "IMAGE", "asset": "a", "slice": 10 }));
        // Corners keep 10px, edges and center stretch.
        assert_eq!(rgb(&p, 5, 5), [255, 0, 0]);
        assert_eq!(rgb(&p, 94, 44), [255, 0, 0]);
        assert_eq!(rgb(&p, 50, 5), [0, 255, 0]);
        assert_eq!(rgb(&p, 5, 25), [0, 255, 0]);
        assert_eq!(rgb(&p, 50, 25), [0, 0, 255]);
        assert_eq!(rgb(&p, 85, 25), [0, 0, 255]);
        // A red tint multiplies: red stays, green and blue go black.
        let p = render(serde_json::json!({ "type": "IMAGE", "asset": "a", "slice": 10, "tint": "#ff0000" }));
        assert_eq!(rgb(&p, 5, 5), [255, 0, 0]);
        assert_eq!(rgb(&p, 50, 25), [0, 0, 0]);
        // No fill center leaves a hole.
        let p = render(serde_json::json!({ "type": "IMAGE", "asset": "a", "slice": { "top": 10, "right": 10, "bottom": 10, "left": 10, "fillCenter": false } }));
        assert_eq!(p.pixel(50, 25).unwrap().alpha(), 0);
    }
}

#[cfg(test)]
mod large_fill_tests {
    use super::*;
    use flexma_doc::{Color, ImageFit, NineSlice, Node, NodeType, Op, Paint};

    /// Share of the rendered pixels that aren't the frame's #222 background.
    fn image_coverage(svg: &str) -> f32 {
        let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).unwrap();
        let mut pm = resvg::tiny_skia::Pixmap::new(480, 270).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(0.25, 0.25), &mut pm.as_mut());
        let shown = pm.pixels().iter().filter(|p| p.red().abs_diff(0x22) > 4 || p.green().abs_diff(0x22) > 4).count();
        shown as f32 / (480.0 * 270.0)
    }

    /// A Unity-style full-screen dim overlay: a huge 9-sliced rect inside a
    /// clipped 1920×1080 frame used to come out cut off in screenshots.
    #[test]
    fn huge_image_fills_render_fully_in_clipped_frames() {
        let mut pm = resvg::tiny_skia::Pixmap::new(64, 40).unwrap();
        pm.fill(resvg::tiny_skia::Color::from_rgba8(220, 120, 120, 255));
        use base64::Engine;
        let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(pm.encode_png().unwrap()));
        let slice = NineSlice { top: 2.0, right: 2.0, bottom: 3.0, left: 2.0, scale: 1.0, fill_center: true };
        let tint = Some(flexma_doc::Color::parse("#1a1a1ac7").unwrap());
        for (slice, fit, side, tint) in [
            (Some(slice), ImageFit::Stretch, 6000.0, None),
            (None, ImageFit::Stretch, 6000.0, None),
            (None, ImageFit::Fill, 6000.0, None),
            // The pause-menu dim: 8192 px, sliced and tinted.
            (Some(slice), ImageFit::Stretch, 8192.0, tint),
            (None, ImageFit::Stretch, 8192.0, tint),
        ] {
            let mut doc = Document::new_empty();
            let mut frame = Node::new("f:1", NodeType::Frame);
            frame.parent = Some("0:1".into());
            frame.width = 1920.0;
            frame.height = 1080.0;
            frame.clips_content = true;
            frame.fills = vec![Paint::solid(Color::parse("#222222").unwrap())];
            doc.apply(&Op::Create { node: frame }).unwrap();
            let mut dim = Node::new("r:1", NodeType::Rectangle);
            dim.parent = Some("f:1".into());
            dim.x = (1920.0 - side) / 2.0;
            dim.y = (1080.0 - side) / 2.0;
            dim.width = side;
            dim.height = side;
            dim.fills = vec![Paint::Image { asset: "a".into(), fit, tint, slice, filled: None, crop: None, tile_scale: 1.0, opacity: 1.0, visible: true }];
            doc.apply(&Op::Create { node: dim }).unwrap();
            let resolve = |_: &str| Some(uri.clone());
            let size = |_: &str| Some((64.0, 40.0));
            // Like the server's image_variant: the tint multiplied into the pixels.
            let variant = |_: &str, _: Option<&flexma_doc::ImageCrop>, t: Option<Color>| {
                let t = t?;
                let mut p = pm.clone();
                for px in p.pixels_mut() {
                    let k = |v: u8, f: f32| (v as f32 * f).round() as u8;
                    let a = k(px.alpha(), t.a);
                    *px = resvg::tiny_skia::PremultipliedColorU8::from_rgba(k(px.red(), t.r * t.a).min(a), k(px.green(), t.g * t.a).min(a), k(px.blue(), t.b * t.a).min(a), a).unwrap();
                }
                Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(p.encode_png().unwrap())))
            };
            let opts = SvgOptions { resolve_asset: &resolve, image_size: &size, resolve_variant: &variant, ..SvgOptions::default() };
            let (svg, _) = export(&doc, &["f:1".to_string()], &opts).unwrap();
            let cov = image_coverage(&svg);
            assert!(cov > 0.99, "{side} {slice:?} {fit:?} {tint:?}: only {:.0}% of the frame shows the image", cov * 100.0);
        }
    }
}

#[cfg(test)]
mod alpha_mask_tests {
    use super::*;
    use flexma_doc::{Color, ImageFit, Node, NodeType, Op, Paint};

    /// A Unity-style Mask: a frame whose image's alpha clips its children,
    /// with the mask graphic shown or hidden.
    #[test]
    fn clip_alpha_masks_children_by_image_alpha() {
        // 10×10 blue image: columns 0-2 opaque, 3-6 nearly transparent (above
        // Unity's 0.001 cutoff), 7-9 fully transparent.
        let mut img = resvg::tiny_skia::Pixmap::new(10, 10).unwrap();
        for (i, px) in img.pixels_mut().iter_mut().enumerate() {
            let a = match i % 10 {
                0..=2 => 255,
                3..=6 => 3,
                _ => 0,
            };
            *px = resvg::tiny_skia::ColorU8::from_rgba(0, 0, 255, a).premultiply();
        }
        use base64::Engine;
        let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(img.encode_png().unwrap()));
        for hide in [false, true] {
            let mut doc = Document::new_empty();
            let mut bg = Node::new("f:0", NodeType::Frame);
            bg.parent = Some("0:1".into());
            bg.width = 100.0;
            bg.height = 100.0;
            bg.fills = vec![Paint::solid(Color::parse("#222222").unwrap())];
            doc.apply(&Op::Create { node: bg }).unwrap();
            let mut mask = Node::new("f:1", NodeType::Frame);
            mask.parent = Some("f:0".into());
            mask.width = 100.0;
            mask.height = 100.0;
            mask.clips_content = true;
            mask.clip_alpha = true;
            mask.hide_fill = hide;
            mask.fills = vec![Paint::Image { asset: "a".into(), fit: ImageFit::Stretch, tint: None, slice: None, filled: None, crop: None, tile_scale: 1.0, opacity: 1.0, visible: true }];
            doc.apply(&Op::Create { node: mask }).unwrap();
            let mut red = Node::new("r:1", NodeType::Rectangle);
            red.parent = Some("f:1".into());
            red.width = 100.0;
            red.height = 50.0;
            red.fills = vec![Paint::solid(Color::parse("#ff0000").unwrap())];
            doc.apply(&Op::Create { node: red }).unwrap();
            let resolve = |_: &str| Some(uri.clone());
            let size = |_: &str| Some((10.0, 10.0));
            let opts = SvgOptions { resolve_asset: &resolve, image_size: &size, ..SvgOptions::default() };
            let (svg, _) = export(&doc, &["f:0".to_string()], &opts).unwrap();
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
            let mut pm = resvg::tiny_skia::Pixmap::new(100, 100).unwrap();
            resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
            let at = |x: u32, y: u32| {
                let p = pm.pixel(x, y).unwrap().demultiply();
                (p.red(), p.green(), p.blue())
            };
            assert_eq!(at(10, 25), (255, 0, 0), "opaque mask pixels show the child (hide={hide})");
            assert_eq!(at(50, 25), (255, 0, 0), "faint mask pixels show the child fully (hide={hide})");
            assert_eq!(at(90, 25), (0x22, 0x22, 0x22), "transparent mask pixels hide the child (hide={hide})");
            let graphic = at(10, 75);
            if hide {
                assert_eq!(graphic, (0x22, 0x22, 0x22), "hideFill doesn't draw the mask image");
            } else {
                assert_eq!(graphic, (0, 0, 255), "the mask image is drawn");
            }
        }
    }
}

#[cfg(test)]
mod filled_image_tests {
    use super::*;
    use flexma_doc::{ImageFilled, ImageFit, Node, NodeType, Op, Paint};

    /// Unity's Filled images in exports: which part of a red image shows.
    #[test]
    fn filled_images_show_their_part() {
        let mut img = resvg::tiny_skia::Pixmap::new(10, 10).unwrap();
        img.fill(resvg::tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        use base64::Engine;
        let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(img.encode_png().unwrap()));
        let cases: &[(serde_json::Value, &[(u32, u32)], &[(u32, u32)])] = &[
            (serde_json::json!({"method": "RADIAL_360", "amount": 0.25, "origin": "top"}), &[(75, 25)], &[(25, 25), (75, 75), (25, 75)]),
            (serde_json::json!({"method": "radial360", "amount": 0.25, "origin": 0, "clockwise": false}), &[(75, 75)], &[(25, 75), (25, 25), (75, 25)]),
            (serde_json::json!({"method": "HORIZONTAL", "amount": 0.3, "origin": 1}), &[(90, 50)], &[(50, 50), (10, 50)]),
            (serde_json::json!({"method": "VERTICAL", "amount": 0.5}), &[(50, 75)], &[(50, 25)]),
            (serde_json::json!({"method": "RADIAL_90", "amount": 0.5, "origin": "BOTTOM_LEFT"}), &[(10, 50)], &[(90, 90)]),
            (serde_json::json!({"method": "Radial_180", "amount": 0.5, "origin": "bottom"}), &[(25, 50)], &[(75, 50)]),
        ];
        for (spec, shown, hidden) in cases {
            let filled: ImageFilled = serde_json::from_value(spec.clone()).unwrap();
            let mut doc = Document::new_empty();
            let mut bg = Node::new("f:0", NodeType::Frame);
            bg.parent = Some("0:1".into());
            (bg.width, bg.height) = (100.0, 100.0);
            doc.apply(&Op::Create { node: bg }).unwrap();
            let mut r = Node::new("r:1", NodeType::Rectangle);
            r.parent = Some("f:0".into());
            (r.width, r.height) = (100.0, 100.0);
            r.fills = vec![Paint::Image { asset: "a".into(), fit: ImageFit::Stretch, tint: None, slice: None, filled: Some(filled), crop: None, tile_scale: 1.0, opacity: 1.0, visible: true }];
            doc.apply(&Op::Create { node: r }).unwrap();
            let resolve = |_: &str| Some(uri.clone());
            let size = |_: &str| Some((10.0, 10.0));
            let opts = SvgOptions { resolve_asset: &resolve, image_size: &size, ..SvgOptions::default() };
            let (svg, _) = export(&doc, &["f:0".to_string()], &opts).unwrap();
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
            let mut pm = resvg::tiny_skia::Pixmap::new(100, 100).unwrap();
            resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
            let red = |x: u32, y: u32| pm.pixel(x, y).unwrap().green() < 50;
            for &(x, y) in *shown {
                assert!(red(x, y), "{spec}: ({x},{y}) should show the image");
            }
            for &(x, y) in *hidden {
                assert!(!red(x, y), "{spec}: ({x},{y}) should be cut away");
            }
        }
        for bad in [serde_json::json!({"method": "RADIAL_90", "origin": "TOP"}), serde_json::json!({"method": "HORIZONTAL", "origin": 2}), serde_json::json!({"method": "SPIRAL"})] {
            assert!(serde_json::from_value::<ImageFilled>(bad.clone()).is_err(), "{bad} should be rejected");
        }
    }
}

#[cfg(test)]
mod shadow_tests {
    use super::*;
    use flexma_doc::{Color, Effect, Node, NodeType, Op, Paint};

    /// Drop shadows in exports: spread grows the shape, blur 0 is a hard
    /// copy (Unity's Shadow), and several shadows don't shadow each other.
    #[test]
    fn shadow_spread_and_hard_shadows() {
        let shadow = |x: f32, spread: f32, color: &str| Effect::DropShadow { color: Color::parse(color).unwrap(), x, y: 0.0, blur: 0.0, spread, visible: true };
        let mut doc = Document::new_empty();
        let mut bg = Node::new("f:0", NodeType::Frame);
        bg.parent = Some("0:1".into());
        (bg.width, bg.height) = (200.0, 100.0);
        doc.apply(&Op::Create { node: bg }).unwrap();
        let mut r = Node::new("r:1", NodeType::Rectangle);
        r.parent = Some("f:0".into());
        (r.x, r.y, r.width, r.height) = (60.0, 30.0, 40.0, 40.0);
        r.fills = vec![Paint::solid(Color::parse("#000000").unwrap())];
        r.effects = vec![shadow(0.0, 10.0, "#ff0000"), shadow(60.0, 0.0, "#0000ff")];
        doc.apply(&Op::Create { node: r }).unwrap();
        let (svg, _) = export(&doc, &["f:0".to_string()], &SvgOptions::default()).unwrap();
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        let mut pm = resvg::tiny_skia::Pixmap::new(200, 100).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
        let at = |x: u32, y: u32| {
            let p = pm.pixel(x, y).unwrap().demultiply();
            (p.red(), p.green(), p.blue())
        };
        assert_eq!(at(55, 50), (255, 0, 0), "spread 10 reaches 5px out");
        assert_eq!(at(45, 50), (255, 255, 255), "but not 15px out");
        assert_eq!(at(80, 50), (0, 0, 0), "the shape stays on top");
        assert_eq!(at(150, 50), (0, 0, 255), "a blur-0 shadow is a hard copy");
        assert_eq!(at(115, 50), (255, 255, 255), "shadows don't shadow each other");
    }
}

#[cfg(test)]
mod crop_tests {
    use super::*;
    use flexma_doc::{ImageCrop, ImageFit, NineSlice, Node, NodeType, Op, Paint};

    /// A sprite cut out of an atlas: only its region shows, in every mode.
    #[test]
    fn cropped_sprites_show_only_their_region() {
        // 20×10 atlas: red sprite on the left, blue on the right.
        let mut img = resvg::tiny_skia::Pixmap::new(20, 10).unwrap();
        for (i, px) in img.pixels_mut().iter_mut().enumerate() {
            let c = if i % 20 < 10 { (255, 0, 0) } else { (0, 0, 255) };
            *px = resvg::tiny_skia::ColorU8::from_rgba(c.0, c.1, c.2, 255).premultiply();
        }
        use base64::Engine;
        let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(img.encode_png().unwrap()));
        let crop = Some(ImageCrop { x: 10.0, y: 0.0, width: 10.0, height: 10.0 });
        let slice = Some(NineSlice::uniform(3.0));
        for (fit, slice, (w, h), blue, white) in [
            (ImageFit::Stretch, None, (100.0, 100.0), vec![(5, 5), (95, 95)], vec![]),
            (ImageFit::Fit, None, (200.0, 100.0), vec![(100, 50)], vec![(20, 50), (180, 50)]),
            (ImageFit::Fill, None, (100.0, 50.0), vec![(5, 25), (95, 25)], vec![]),
            (ImageFit::Stretch, slice, (120.0, 60.0), vec![(2, 2), (60, 30), (118, 58)], vec![]),
        ] {
            let mut doc = Document::new_empty();
            let mut bg = Node::new("f:0", NodeType::Frame);
            bg.parent = Some("0:1".into());
            (bg.width, bg.height) = (w, h);
            doc.apply(&Op::Create { node: bg }).unwrap();
            let mut r = Node::new("r:1", NodeType::Rectangle);
            r.parent = Some("f:0".into());
            (r.width, r.height) = (w, h);
            r.fills = vec![Paint::Image { asset: "a".into(), fit, tint: None, slice, filled: None, crop, tile_scale: 1.0, opacity: 1.0, visible: true }];
            doc.apply(&Op::Create { node: r }).unwrap();
            let resolve = |_: &str| Some(uri.clone());
            let size = |_: &str| Some((20.0, 10.0));
            let opts = SvgOptions { resolve_asset: &resolve, image_size: &size, ..SvgOptions::default() };
            let (svg, _) = export(&doc, &["f:0".to_string()], &opts).unwrap();
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
            let mut pm = resvg::tiny_skia::Pixmap::new(w as u32, h as u32).unwrap();
            resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
            let at = |x: u32, y: u32| {
                let p = pm.pixel(x, y).unwrap().demultiply();
                (p.red(), p.green(), p.blue())
            };
            for (x, y) in blue {
                let c = at(x, y);
                assert!(c.2 > 200 && c.0 < 60, "{fit:?} {slice:?}: ({x},{y}) should be the blue sprite, got {c:?}");
            }
            for (x, y) in white {
                assert_eq!(at(x, y), (255, 255, 255), "{fit:?}: ({x},{y}) is letterbox, not the atlas");
            }
        }
    }
}

#[cfg(test)]
mod tile_tests {
    use super::*;
    use flexma_doc::{ImageCrop, ImageFit, Node, NodeType, Op, Paint};

    fn render(img: &resvg::tiny_skia::Pixmap, size: (f64, f64), crop: Option<ImageCrop>, scale: f32) -> resvg::tiny_skia::Pixmap {
        use base64::Engine;
        let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(img.encode_png().unwrap()));
        let (iw, ih) = (img.width() as f64, img.height() as f64);
        let mut doc = Document::new_empty();
        let mut bg = Node::new("f:0", NodeType::Frame);
        bg.parent = Some("0:1".into());
        (bg.width, bg.height) = size;
        doc.apply(&Op::Create { node: bg }).unwrap();
        let mut r = Node::new("r:1", NodeType::Rectangle);
        r.parent = Some("f:0".into());
        (r.width, r.height) = size;
        r.fills = vec![Paint::Image { asset: "a".into(), fit: ImageFit::Tile, tint: None, slice: None, filled: None, crop, tile_scale: scale, opacity: 1.0, visible: true }];
        doc.apply(&Op::Create { node: r }).unwrap();
        let resolve = |_: &str| Some(uri.clone());
        let isize = |_: &str| Some((iw, ih));
        let opts = SvgOptions { resolve_asset: &resolve, image_size: &isize, ..SvgOptions::default() };
        let (svg, _) = export(&doc, &["f:0".to_string()], &opts).unwrap();
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        let mut pm = resvg::tiny_skia::Pixmap::new(size.0 as u32, size.1 as u32).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
        pm
    }

    fn two_tone(w: u32, h: u32, split: impl Fn(u32, u32) -> bool) -> resvg::tiny_skia::Pixmap {
        let mut img = resvg::tiny_skia::Pixmap::new(w, h).unwrap();
        for (i, px) in img.pixels_mut().iter_mut().enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let c = if split(x, y) { (255, 0, 0) } else { (0, 0, 255) };
            *px = resvg::tiny_skia::ColorU8::from_rgba(c.0, c.1, c.2, 255).premultiply();
        }
        img
    }

    fn red(pm: &resvg::tiny_skia::Pixmap, x: u32, y: u32) -> bool {
        let p = pm.pixel(x, y).unwrap().demultiply();
        p.red() > 200 && p.blue() < 60
    }

    /// Tiled images repeat from the bottom-left corner, like Unity.
    #[test]
    fn tiles_repeat_from_the_bottom_left() {
        // Red top half, blue bottom half, in a 10×30 box: from the bottom,
        // a full tile at 10..30 and the lower (blue) half of one above it.
        let img = two_tone(10, 20, |_, y| y < 10);
        let pm = render(&img, (10.0, 30.0), None, 1.0);
        assert!(!red(&pm, 5, 5) && red(&pm, 5, 15) && !red(&pm, 5, 25), "anchored at the bottom");
        // Columns: the red|blue atlas cropped to its red half, tiled at 2×:
        // 20 px red tiles all the way across.
        let atlas = two_tone(20, 10, |x, _| x < 10);
        let pm = render(&atlas, (50.0, 20.0), Some(ImageCrop { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }), 2.0);
        assert!(red(&pm, 5, 10) && red(&pm, 25, 10) && red(&pm, 45, 10), "only the cropped sprite repeats");
        // Uncropped at 1×: red, blue, red, blue, red every 10 px.
        let pm = render(&atlas, (50.0, 10.0), None, 1.0);
        assert!(red(&pm, 5, 5) && !red(&pm, 15, 5) && red(&pm, 25, 5) && !red(&pm, 35, 5) && red(&pm, 45, 5));
    }
}

#[cfg(test)]
mod nested_export_tests {
    use super::*;
    use flexma_doc::{Node, NodeType, Op, Paint};

    /// Exporting a layer inside a moved frame used to come out blank: the
    /// viewBox was in page space but the layer kept only its local offset.
    #[test]
    fn nested_layers_export_in_place() {
        let mut doc = Document::new_empty();
        let mut f = Node::new("f:1", NodeType::Frame);
        f.parent = Some("0:1".into());
        (f.x, f.y, f.width, f.height) = (300.0, 200.0, 100.0, 100.0);
        doc.apply(&Op::Create { node: f }).unwrap();
        let mut r = Node::new("r:1", NodeType::Rectangle);
        r.parent = Some("f:1".into());
        (r.x, r.y, r.width, r.height) = (10.0, 20.0, 40.0, 30.0);
        r.fills = vec![Paint::solid(flexma_doc::Color::parse("#ff0000").unwrap())];
        doc.apply(&Op::Create { node: r }).unwrap();
        let (svg, bounds) = export(&doc, &["r:1".to_string()], &SvgOptions::default()).unwrap();
        assert_eq!((bounds.x, bounds.y), (310.0, 220.0));
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        let mut pm = resvg::tiny_skia::Pixmap::new(40, 30).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
        assert_eq!(pm.pixel(20, 15).unwrap().red(), 255, "the layer fills its own export");
    }
}

#[cfg(test)]
mod measure_tests {
    use super::*;
    use flexma_doc::{Node, NodeType};

    /// Server renders break lines by real font metrics: "front std rear drag"
    /// in Arial / Liberation Sans 24px is ~190px, which the Inter-like
    /// estimate put just over a 200px box.
    #[test]
    fn real_metrics_keep_fitting_text_on_one_line() {
        let db = base_fontdb();
        let mut n = Node::new("t", NodeType::Text);
        (n.font_family, n.font_size, n.width, n.height) = ("Arial".into(), 24.0, 200.0, 30.0);
        n.text_auto_resize = flexma_doc::TextAutoResize::None;
        n.characters = "front std rear drag".into();
        let m = TextMeasurer::new(db.clone());
        let Some(id) = m.face(&n) else { return };
        let is_arial = db.face(id).is_some_and(|f| f.families.iter().any(|(name, _)| name.contains("Arial") || name.contains("Liberation Sans")));
        if !is_arial {
            return; // No Arial-metric font on this machine.
        }
        let w = m.measure(&n, &n.characters);
        assert!((185.0..196.0).contains(&w), "{w}");
        let lines = flexma_doc::text::layout_lines(&n, &mut |s| m.measure(&n, s));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(flexma_doc::text::estimate_width(&n.characters, 24.0, 0.0, 400) > 200.0, "the estimate alone would wrap");
    }

    /// The baseline comes from the font's own ascent and descent, centred in
    /// the line box like the editor, not a 0.8em guess shared by every font.
    #[test]
    fn baseline_follows_the_font_metrics() {
        let db = base_fontdb();
        let mut doc = Document::new_empty();
        let mut n = Node::new("t:1", NodeType::Text);
        n.parent = Some("0:1".into());
        (n.font_family, n.font_size, n.line_height, n.width, n.height) = ("Arial".into(), 20.0, Some(24.0), 200.0, 24.0);
        n.text_auto_resize = flexma_doc::TextAutoResize::None;
        n.characters = "H".into();
        let m = TextMeasurer::new(db.clone());
        let Some(id) = m.face(&n) else { return };
        let is_arial = db.face(id).is_some_and(|f| f.families.iter().any(|(name, _)| name.contains("Arial") || name.contains("Liberation Sans")));
        if !is_arial {
            return; // No Arial-metric font on this machine.
        }
        // Arial's hhea: 1854 / 434 of 2048 units, 18.1 / 4.2 px at 20px.
        let (asc, desc) = m.metrics(&n);
        assert_eq!((asc, desc), (18.0, 4.0));
        doc.apply(&flexma_doc::Op::Create { node: n }).unwrap();
        let paint = PaintSource { data_dir: Path::new("/nonexistent"), file_id: "f", share_token: None };
        let (svg, _) = export_svg(&doc, &["t:1".to_string()], None, AssetMode::Url, Some(&db), &paint).unwrap();
        let expected = (24.0 - (asc + desc)) / 2.0 + asc;
        assert!(svg.contains(&format!(r#"y="{}""#, flexma_doc::geom::fmt_num(expected))), "baseline {expected} in {svg}");
    }

    /// Text is shaped (kerning) with the editor's Inter, so lines break where
    /// the editor breaks them. Chrome measures this at 308.92 px.
    #[test]
    fn inter_widths_match_the_editor() {
        let m = TextMeasurer::new(base_fontdb());
        let mut n = Node::new("t", NodeType::Text);
        (n.font_size, n.font_weight) = (14.0, 400);
        let w = m.measure(&n, "AVATAR Toyota Wave. Tuesday, LTA yield 7.4%");
        assert!((w - 308.92).abs() < 0.01, "{w}");
    }
}

#[cfg(test)]
mod seam_tests {
    use super::*;
    use flexma_doc::{ImageFit, NineSlice, Node, NodeType, Op, Paint};

    /// A bar sprite sliced 64 + 64 on a 128 px image has no middle: Unity
    /// stretches the seam, so the bar stays solid instead of two end caps.
    #[test]
    fn zero_width_slice_centers_stretch_the_seam() {
        let mut img = resvg::tiny_skia::Pixmap::new(8, 4).unwrap();
        img.fill(resvg::tiny_skia::Color::from_rgba8(20, 30, 40, 255));
        use base64::Engine;
        let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(img.encode_png().unwrap()));
        let mut doc = Document::new_empty();
        let mut bg = Node::new("f:0", NodeType::Frame);
        bg.parent = Some("0:1".into());
        (bg.width, bg.height) = (200.0, 20.0);
        doc.apply(&Op::Create { node: bg }).unwrap();
        let mut r = Node::new("r:1", NodeType::Rectangle);
        r.parent = Some("f:0".into());
        (r.width, r.height) = (200.0, 20.0);
        let slice = NineSlice { top: 0.0, right: 4.0, bottom: 0.0, left: 4.0, scale: 1.0, fill_center: true };
        r.fills = vec![Paint::Image { asset: "a".into(), fit: ImageFit::Stretch, tint: None, slice: Some(slice), filled: None, crop: None, tile_scale: 1.0, opacity: 1.0, visible: true }];
        doc.apply(&Op::Create { node: r }).unwrap();
        let resolve = |_: &str| Some(uri.clone());
        let size = |_: &str| Some((8.0, 4.0));
        let opts = SvgOptions { resolve_asset: &resolve, image_size: &size, ..SvgOptions::default() };
        let (svg, _) = export(&doc, &["f:0".to_string()], &opts).unwrap();
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        let mut pm = resvg::tiny_skia::Pixmap::new(200, 20).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
        let p = pm.pixel(100, 10).unwrap();
        assert_eq!((p.red(), p.green(), p.blue()), (20, 30, 40), "the middle is the stretched seam, not the white frame");
    }
}

#[cfg(test)]
mod linear_blend_tests {
    use super::*;
    use flexma_doc::{Node, NodeType, Op, Paint};

    /// Unity projects in Linear color space blend in linear light: a black
    /// panel at 85% over white is 107 there, 38 in sRGB (measured in the game).
    #[test]
    fn pages_with_linear_blending_match_unity() {
        for (linear, want) in [(true, 107u8), (false, 38u8)] {
            let mut doc = Document::new_empty();
            doc.apply(&Op::update1("0:1", "linearBlending", serde_json::json!(linear))).unwrap();
            let mut f = Node::new("f:1", NodeType::Frame);
            f.parent = Some("0:1".into());
            (f.width, f.height) = (20.0, 20.0);
            doc.apply(&Op::Create { node: f }).unwrap();
            let mut r = Node::new("r:1", NodeType::Rectangle);
            r.parent = Some("f:1".into());
            (r.width, r.height) = (20.0, 20.0);
            r.fills = vec![Paint::solid(flexma_doc::Color::parse("#000000d9").unwrap())];
            doc.apply(&Op::Create { node: r }).unwrap();
            assert_eq!(linear_for(&doc, &["f:1".to_string()]), linear);
            let dir = std::env::temp_dir();
            let paint = PaintSource { data_dir: &dir, file_id: "linear-test", share_token: None };
            let (svg, _) = export_svg(&doc, &["f:1".to_string()], None, AssetMode::Inline(&dir, &HashSet::new()), None, &paint).unwrap();
            let (png, _, _) = svg_to_png(&svg, 1.0, 100, base_fontdb(), linear, None).unwrap();
            let pm = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
            let v = pm.pixel(10, 10).unwrap().red();
            assert!(v.abs_diff(want) <= 2, "linear={linear}: got {v}, Unity/sRGB math says {want}");
        }
    }
}

#[cfg(test)]
mod nested_clip_tests {
    use super::*;
    use flexma_doc::{ImageFit, NineSlice, Node, NodeType, Op, Paint};

    /// Canvas_map / MaxMap: a clipped screen holding a clipped frame with a
    /// 2048 px map, a sliced bar whose middle is the stretched seam, then more
    /// layers. Rendering the outer frame lost every layer after the bar.
    #[test]
    fn layers_after_a_stretched_seam_still_draw_in_nested_clips() {
        let png = |w: u32, h: u32, c: (u8, u8, u8)| {
            let mut p = resvg::tiny_skia::Pixmap::new(w, h).unwrap();
            p.fill(resvg::tiny_skia::Color::from_rgba8(c.0, c.1, c.2, 255));
            use base64::Engine;
            format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(p.encode_png().unwrap()))
        };
        let (map, bar) = (png(64, 64, (0, 120, 0)), png(128, 128, (200, 0, 0)));
        let mut doc = Document::new_empty();
        let frame = |id: &str, parent: &str, x: f64, y: f64, w: f64, h: f64, clip: bool| {
            let mut f = Node::new(id, NodeType::Frame);
            f.parent = Some(parent.into());
            (f.x, f.y, f.width, f.height, f.clips_content) = (x, y, w, h, clip);
            f
        };
        doc.apply(&Op::Create { node: frame("outer", "0:1", 0.0, 0.0, 1920.0, 1080.0, true) }).unwrap();
        doc.apply(&Op::Create { node: frame("inner", "outer", 0.0, 0.0, 1920.0, 1080.0, true) }).unwrap();
        let mut big = frame("map", "inner", -64.0, -484.0, 2048.0, 2048.0, false);
        big.fills = vec![Paint::Image { asset: "map".into(), fit: ImageFit::Stretch, tint: None, slice: None, filled: None, crop: None, tile_scale: 1.0, opacity: 1.0, visible: true }];
        doc.apply(&Op::Create { node: big }).unwrap();
        let mut bottom = frame("bar", "inner", 48.0, 992.0, 1824.0, 88.0, false);
        bottom.index = "b".into();
        let slice = NineSlice { top: 64.0, right: 64.0, bottom: 8.0, left: 64.0, scale: 1.0, fill_center: true };
        bottom.fills = vec![Paint::Image { asset: "bar".into(), fit: ImageFit::Stretch, tint: None, slice: Some(slice), filled: None, crop: None, tile_scale: 1.0, opacity: 1.0, visible: true }];
        doc.apply(&Op::Create { node: bottom }).unwrap();
        let mut panel = Node::new("panel", NodeType::Rectangle);
        panel.parent = Some("inner".into());
        panel.index = "c".into();
        (panel.x, panel.y, panel.width, panel.height) = (1556.0, 110.0, 328.0, 772.0);
        panel.fills = vec![Paint::solid(flexma_doc::Color::parse("#ffffff").unwrap())];
        doc.apply(&Op::Create { node: panel }).unwrap();
        let resolve = |id: &str| Some(if id == "map" { map.clone() } else { bar.clone() });
        let size = |id: &str| Some(if id == "map" { (64.0, 64.0) } else { (128.0, 128.0) });
        // Like the server's image_variant: a region cut out (here, solid colors).
        let variant = |id: &str, c: Option<&flexma_doc::ImageCrop>, _: Option<flexma_doc::Color>| {
            let c = c?;
            Some(png(c.width.max(1.0) as u32, c.height.max(1.0) as u32, if id == "map" { (0, 120, 0) } else { (200, 0, 0) }))
        };
        let opts = SvgOptions { resolve_asset: &resolve, image_size: &size, resolve_variant: &variant, ..SvgOptions::default() };
        for root in ["outer", "inner"] {
            let (svg, _) = export(&doc, &[root.to_string()], &opts).unwrap();
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
            let mut pm = resvg::tiny_skia::Pixmap::new(480, 270).unwrap();
            resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(0.25, 0.25), &mut pm.as_mut());
            let at = |x: f32, y: f32| {
                let p = pm.pixel((x * 0.25) as u32, (y * 0.25) as u32).unwrap();
                (p.red(), p.green(), p.blue())
            };
            assert_eq!(at(1700.0, 500.0), (255, 255, 255), "{root}: the panel after the bar draws");
            assert_eq!(at(80.0, 1010.0), (200, 0, 0), "{root}: the bar's caps draw");
            assert_eq!(at(960.0, 1010.0), (200, 0, 0), "{root}: the bar's stretched middle draws");
        }
    }
}

#[cfg(test)]
mod checker_tests {
    use super::*;
    use flexma_doc::{Node, NodeType, Op, Paint};

    #[test]
    fn transparent_layers_get_a_checkerboard() {
        let mut doc = Document::new_empty();
        let mut comp = Node::new("c:1", NodeType::Component);
        comp.parent = Some("0:1".into());
        comp.fills = vec![];
        doc.apply(&Op::Create { node: comp }).unwrap();
        let mut card = Node::new("f:1", NodeType::Frame);
        card.parent = Some("0:1".into());
        card.fills = vec![Paint::solid(flexma_doc::Color::parse("#223344").unwrap())];
        doc.apply(&Op::Create { node: card }).unwrap();
        assert!(wants_checkerboard(&doc, &["c:1".to_string()]), "no fill: checkerboard");
        assert!(!wants_checkerboard(&doc, &["f:1".to_string()]), "opaque fill: plain");
        assert!(!wants_checkerboard(&doc, &["0:1".to_string()]), "pages keep their color");
        // Transparent pixels become the grid; opaque ones stay.
        let mut pm = resvg::tiny_skia::Pixmap::new(32, 8).unwrap();
        for x in 16..32 {
            for y in 0..8 {
                let i = (y * 32 + x) as usize;
                pm.pixels_mut()[i] = resvg::tiny_skia::PremultipliedColorU8::from_rgba(255, 0, 0, 255).unwrap();
            }
        }
        let out = over_checkerboard(&pm, 0);
        let px = |x: u32, y: u32| out.pixel(x, y).unwrap();
        assert_eq!((px(1, 1).red(), px(9, 1).red()), (196, 150), "alternating cells");
        assert_eq!((px(20, 1).red(), px(20, 1).green()), (255, 0), "content on top");
    }
}

#[cfg(test)]
mod paint_render_tests {
    use super::*;
    use flexma_doc::{Node, NodeType, Op};

    /// Server renders show a paint layer's tile snapshots, clipped to the layer.
    #[test]
    fn paint_layers_render_their_tiles() {
        let dir = std::env::temp_dir().join(format!("flexma-paint-render-{}", std::process::id()));
        let mut doc = Document::new_empty();
        let mut f = Node::new("f:1", NodeType::Frame);
        f.parent = Some("0:1".into());
        (f.width, f.height) = (300.0, 100.0);
        doc.apply(&Op::Create { node: f }).unwrap();
        let mut p = Node::new("p:1", NodeType::Paint);
        p.parent = Some("f:1".into());
        (p.x, p.width, p.height) = (20.0, 260.0, 60.0);
        p.fills = vec![];
        doc.apply(&Op::Create { node: p }).unwrap();
        // Tile (0, 0) red, tile (1, 0) blue.
        for (tx, rgb) in [(0, (255u8, 0u8, 0u8)), (1, (0, 0, 255))] {
            let mut pm = resvg::tiny_skia::Pixmap::new(256, 256).unwrap();
            pm.fill(resvg::tiny_skia::Color::from_rgba8(rgb.0, rgb.1, rgb.2, 255));
            let path = crate::paint::tile_path(&dir, "f", "p:1", tx, 0, crate::paint::Kind::Base);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, pm.encode_png().unwrap()).unwrap();
        }
        let paint = PaintSource { data_dir: &dir, file_id: "f", share_token: None };
        let (svg, _) = export_svg(&doc, &["f:1".to_string()], None, AssetMode::Inline(&dir, &HashSet::new()), None, &paint).unwrap();
        let (png, _, _) = svg_to_png(&svg, 1.0, 400, base_fontdb(), false, None).unwrap();
        let pm = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
        let px = |x, y| {
            let c = pm.pixel(x, y).unwrap();
            (c.red(), c.green(), c.blue())
        };
        assert_eq!(px(100, 30), (255, 0, 0), "tile 0 at layer x 80");
        assert_eq!(px(278, 30), (0, 0, 255), "tile 1 starts at layer x 256 = frame x 276");
        assert_eq!(px(290, 30), (255, 255, 255), "clipped at the layer's right edge (frame x 280)");
        assert_eq!(px(100, 80), (255, 255, 255), "clipped at the layer's bottom (y 60)");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod asset_scope_tests {
    use super::*;
    use flexma_doc::{ImageFit, Node, NodeType, Op, Paint};

    /// A server render inlines the file's own workspace's images and nothing
    /// else, even when a document names another workspace's asset.
    #[test]
    fn renders_only_inline_the_workspaces_own_images() {
        let dir = std::env::temp_dir().join(format!("flexma-asset-scope-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        let mut pm = resvg::tiny_skia::Pixmap::new(4, 4).unwrap();
        pm.fill(resvg::tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        std::fs::write(dir.join("assets").join("ours123"), pm.encode_png().unwrap()).unwrap();
        std::fs::write(dir.join("assets").join("theirs456"), pm.encode_png().unwrap()).unwrap();
        let mut doc = Document::new_empty();
        let mut f = Node::new("f:1", NodeType::Frame);
        f.parent = Some("0:1".into());
        (f.width, f.height) = (20.0, 10.0);
        doc.apply(&Op::Create { node: f }).unwrap();
        for (i, asset) in ["ours123", "theirs456"].into_iter().enumerate() {
            let mut r = Node::new(&format!("r:{i}"), NodeType::Rectangle);
            r.parent = Some("f:1".into());
            (r.x, r.width, r.height) = (i as f64 * 10.0, 10.0, 10.0);
            // The second one cropped, so it renders through a cached variant.
            let crop = (i == 1).then_some(ImageCrop { x: 0.0, y: 0.0, width: 2.0, height: 2.0 });
            r.fills = vec![Paint::Image { asset: asset.into(), fit: ImageFit::Stretch, tint: None, slice: None, filled: None, crop, tile_scale: 1.0, opacity: 1.0, visible: true }];
            doc.apply(&Op::Create { node: r }).unwrap();
        }
        let allowed: HashSet<String> = ["ours123".to_string()].into_iter().collect();
        let paint = PaintSource { data_dir: &dir, file_id: "f", share_token: None };
        let render = |allowed: &HashSet<String>| export_svg(&doc, &["f:1".to_string()], None, AssetMode::Inline(&dir, allowed), None, &paint).unwrap().0;
        let images = |svg: &str| svg.matches("flexma-asset:").count() + svg.matches("flexma-variant:").count();
        assert_eq!(images(&render(&allowed)), 1, "only our image is referenced");
        let both: HashSet<String> = ["ours123".to_string(), "theirs456".to_string()].into_iter().collect();
        assert_eq!(images(&render(&both)), 2, "the test document really references both");
        assert_eq!(images(&render(&HashSet::new())), 0);
        // Rasterizing loads only allowed ids, even when the SVG names another
        // (here, the other workspace's variant is already in the cache).
        let svg = render(&both);
        assert!(svg.contains("flexma-variant:theirs456:"));
        let red = |images: &HashSet<String>| {
            let (png, w, h) = svg_to_png(&svg, 1.0, 100, base_fontdb(), false, Some((&dir, images))).unwrap();
            let pm = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
            [5, 15].map(|x| pm.pixel(x * w / 20, h / 2).map(|p| p.red() > 200 && p.green() < 50).unwrap())
        };
        assert_eq!(red(&both), [true, true]);
        assert_eq!(red(&allowed), [true, false]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A variant the shared cache dropped between writing an SVG and
    /// rasterizing it (other renders filled the cache meanwhile) is made
    /// again. Big contact sheets used to lose their first screens' tinted
    /// and linear-blended images this way.
    #[test]
    fn variants_survive_a_cache_reset() {
        let dir = std::env::temp_dir().join(format!("flexma-variant-reset-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        let mut pm = resvg::tiny_skia::Pixmap::new(4, 4).unwrap();
        pm.fill(resvg::tiny_skia::Color::WHITE);
        std::fs::write(dir.join("assets").join("white123"), pm.encode_png().unwrap()).unwrap();
        let mut doc = Document::new_empty();
        let mut f = Node::new("f:1", NodeType::Frame);
        f.parent = Some("0:1".into());
        (f.width, f.height) = (10.0, 10.0);
        doc.apply(&Op::Create { node: f }).unwrap();
        let mut r = Node::new("r:1", NodeType::Rectangle);
        r.parent = Some("f:1".into());
        (r.width, r.height) = (10.0, 10.0);
        let tint = Some(Color::rgba(0.0, 0.0, 1.0, 1.0));
        r.fills = vec![Paint::Image { asset: "white123".into(), fit: ImageFit::Stretch, tint, slice: None, filled: None, crop: None, tile_scale: 1.0, opacity: 1.0, visible: true }];
        doc.apply(&Op::Create { node: r }).unwrap();
        let allowed: HashSet<String> = ["white123".to_string()].into_iter().collect();
        let paint = PaintSource { data_dir: &dir, file_id: "f", share_token: None };
        let (svg, _) = export_svg(&doc, &["f:1".to_string()], None, AssetMode::Inline(&dir, &allowed), None, &paint).unwrap();
        assert!(svg.contains("flexma-variant:white123:"));
        if let Ok(mut cache) = variants().lock() {
            cache.by_href.clear();
            cache.bytes = 0;
        }
        let (png, w, h) = svg_to_png(&svg, 1.0, 100, base_fontdb(), false, Some((&dir, &allowed))).unwrap();
        let pm = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
        let p = pm.pixel(w / 2, h / 2).unwrap();
        assert_eq!((p.red(), p.green(), p.blue()), (0, 0, 255), "the tinted image is drawn");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A render taller than one strip comes out the same as a single pass:
    /// clipped, translucent and blurred groups across strip edges, and the
    /// checkerboard.
    #[test]
    fn strips_match_a_single_render() {
        // `y`: the first strip boundary.
        let y = strip_rows(1000, u32::MAX);
        let (w, h) = (1000, y * 2 + 37);
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"><defs><linearGradient id="g" x2="0" y2="1"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f" stop-opacity="0.3"/></linearGradient><clipPath id="c"><rect x="100" y="10" width="800" height="{y}" rx="40"/></clipPath><filter id="b" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="6"/></filter></defs><rect width="{w}" height="{h}" fill="url(#g)"/><g clip-path="url(#c)" opacity="0.6"><circle cx="500" cy="{y}" r="300" fill="#0f0"/></g><rect x="200" y="{}" width="600" height="80" fill="#ff0" filter="url(#b)"/></svg>"##,
            y * 2 - 40
        );
        let (png, pw, ph) = svg_to_png_on(&svg, 1.0, 100_000, base_fontdb(), false, true, None).unwrap();
        assert_eq!((pw, ph), (w, h));
        let got = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
        let tree = resvg::usvg::Tree::from_str(&svg, &svg_options()).unwrap();
        let mut whole = resvg::tiny_skia::Pixmap::new(w, h).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut whole.as_mut());
        let want = over_checkerboard(&whole, 0);
        let off = got.pixels().iter().zip(want.pixels()).filter(|(a, b)| a.red().abs_diff(b.red()).max(a.green().abs_diff(b.green())).max(a.blue().abs_diff(b.blue())) > 1).count();
        assert_eq!(off, 0, "pixels differ from a single render");
    }
}


#[cfg(test)]
mod svg_upload_tests {
    use super::*;

    /// An uploaded SVG becomes a sharper PNG and keeps its own size.
    #[test]
    fn svg_uploads_rasterize() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="50" height="50" fill="#f00"/></svg>"##;
        let (png, w, h) = rasterize_svg(svg, base_fontdb()).unwrap();
        assert_eq!((w, h), (100.0, 50.0));
        let size = imagesize::blob_size(&png).unwrap();
        assert_eq!((size.width, size.height), (800, 400), "8× for a small drawing");
        let pm = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
        assert_eq!(pm.pixel(10, 10).unwrap().red(), 255);
        assert_eq!(pm.pixel(700, 10).unwrap().alpha(), 0, "transparent stays transparent");
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use flexma_doc::{Node, NodeType};

    /// Only the characters a font lacks, and emoji, come from fallback fonts;
    /// the rest of the line keeps its font.
    #[test]
    fn fallback_per_character_and_color_emoji() {
        let m = TextMeasurer::new(base_fontdb());
        let mut n = Node::new("t:1", NodeType::Text);
        (n.font_size, n.font_weight) = (32.0, 600);
        let runs = m.svg_runs(&n, "Gift ⚡🎁 ok");
        assert_eq!(runs, vec![("Gift ".into(), None), ("⚡🎁".into(), Some(EMOJI_FAMILY.into())), (" ok".into(), None)]);
        // A face without Latin letters gets them from Inter, not the whole line.
        let mut emoji_font = n.clone();
        emoji_font.font_family = EMOJI_FAMILY.into();
        let runs = m.svg_runs(&emoji_font, "🎁Hi");
        assert_eq!(runs, vec![("🎁".into(), None), ("Hi".into(), Some("Inter".into()))]);
        // Drawn in color.
        let mut doc = Document::new_empty();
        n.parent = Some("0:1".into());
        n.characters = "🎁".into();
        (n.width, n.height) = (40.0, 40.0);
        doc.apply(&flexma_doc::Op::Create { node: n }).unwrap();
        let paint = PaintSource { data_dir: Path::new("/nonexistent"), file_id: "f", share_token: None };
        let (svg, _) = export_svg(&doc, &["t:1".to_string()], None, AssetMode::Url, Some(&base_fontdb()), &paint).unwrap();
        assert!(svg.contains("Twemoji Mozilla"), "{svg}");
        let (png, _, _) = svg_to_png(&svg, 1.0, 512, base_fontdb(), false, None).unwrap();
        let pm = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
        let colored = pm.pixels().iter().filter(|p| p.alpha() > 200 && (p.red() as i32 - p.blue() as i32).abs() > 60).count();
        assert!(colored > 50, "emoji drawn in color ({colored} colored pixels)");
    }
}

