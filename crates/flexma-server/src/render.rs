//! Server-side rendering: SVG export (shared with the editor via flexma-doc) and
//! PNG rasterization through resvg, used for thumbnails, exports and the MCP
//! `get_screenshot` tool so agents can see what they built. Rasterizing takes
//! a font database so workspace fonts render (see `fonts::fontdb_for`).

use flexma_doc::svg::{export, SvgOptions};
use flexma_doc::{Color, Document, ImageCrop, NodeId, Rect};
use std::collections::HashMap;
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
pub fn thumbnail_png(doc: &Document, data_dir: &Path, fonts: Arc<FontDb>) -> Option<Vec<u8>> {
    let page = doc.pages().first()?.clone();
    if doc.children(&page).is_empty() {
        return None;
    }
    let bg = page_background(doc, &page);
    let mode = AssetMode::Inline(data_dir);
    let resolve = |id: &str| resolve_asset(&mode, id);
    let crop = |id: &str, c: &ImageCrop| crop_asset(&mode, id, c);
    let measurer = TextMeasurer::new(fonts.clone());
    let measure = |n: &flexma_doc::Node, s: &str| measurer.measure(n, s);
    let opts = SvgOptions { padding: 48.0, background: Some(bg), resolve_asset: &resolve, image_size: &image_size, measure_text: Some(&measure), resolve_crop: &crop };
    let (svg, _) = export(doc, &[page], &opts)?;
    svg_to_png(&svg, 2.0, THUMBNAIL_PX, fonts).ok().map(|(png, _, _)| png)
}

pub enum AssetMode<'a> {
    /// Reference assets by URL (for browsers).
    Url,
    /// Reference assets through a public share link (`/api/share/{token}/assets/{id}`).
    Shared(&'a str),
    /// Inline assets as data URIs (for rasterizing on the server).
    Inline(&'a Path),
}

/// Export nodes (or a whole page) to SVG. With `fonts`, text breaks into
/// lines by the real font metrics instead of estimates.
pub fn export_svg(doc: &Document, ids: &[NodeId], background: Option<Color>, assets: AssetMode, fonts: Option<&Arc<FontDb>>) -> Option<(String, Rect)> {
    let resolve = |id: &str| resolve_asset(&assets, id);
    let crop = |id: &str, c: &ImageCrop| crop_asset(&assets, id, c);
    let measurer = fonts.map(|f| TextMeasurer::new(f.clone()));
    let measure = |n: &flexma_doc::Node, s: &str| measurer.as_ref().map(|m| m.measure(n, s)).unwrap_or(0.0);
    let opts = SvgOptions {
        padding: 0.0,
        background,
        resolve_asset: &resolve,
        image_size: &image_size,
        measure_text: if measurer.is_some() { Some(&measure) } else { None },
        resolve_crop: &crop,
    };
    export(doc, ids, &opts)
}

/// Text widths from the fonts the renderer will draw with: the node's family
/// (or the SVG's fallbacks: Inter, Helvetica, Arial, sans-serif) at its
/// weight and style, by glyph advances.
pub struct TextMeasurer {
    db: Arc<FontDb>,
    faces: std::cell::RefCell<HashMap<(String, u16, bool), Option<resvg::usvg::fontdb::ID>>>,
    /// Advance per character in em.
    advances: std::cell::RefCell<HashMap<(resvg::usvg::fontdb::ID, char), f32>>,
}

impl TextMeasurer {
    pub fn new(db: Arc<FontDb>) -> TextMeasurer {
        TextMeasurer { db, faces: Default::default(), advances: Default::default() }
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

    pub fn measure(&self, n: &flexma_doc::Node, s: &str) -> f64 {
        let Some(id) = self.face(n) else {
            return flexma_doc::text::estimate_width(s, n.font_size, n.letter_spacing, n.font_weight);
        };
        let missing: Vec<char> = {
            let adv = self.advances.borrow();
            let mut m: Vec<char> = s.chars().filter(|c| !adv.contains_key(&(id, *c))).collect();
            m.sort_unstable();
            m.dedup();
            m
        };
        if !missing.is_empty() {
            let found: Vec<(char, f32)> = self
                .db
                .with_face_data(id, |data, index| {
                    let face = ttf_parser::Face::parse(data, index).ok()?;
                    let upem = face.units_per_em().max(1) as f32;
                    Some(
                        missing
                            .iter()
                            .map(|&c| {
                                let a = face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g)).map(|a| a as f32 / upem);
                                // Missing glyphs fall back to another font; guess like the estimate.
                                (c, a.unwrap_or_else(|| (flexma_doc::text::estimate_width(&c.to_string(), 1.0, 0.0, 400)) as f32))
                            })
                            .collect(),
                    )
                })
                .flatten()
                .unwrap_or_default();
            let mut adv = self.advances.borrow_mut();
            for (c, a) in found {
                adv.insert((id, c), a);
            }
        }
        let adv = self.advances.borrow();
        s.chars().map(|c| adv.get(&(id, c)).copied().unwrap_or(0.5) as f64 * n.font_size + n.letter_spacing).sum()
    }
}

/// A sprite cut out of an inlined image as a PNG data URI (whole pixels,
/// cached), so smoothing at its edges never reaches the pixels next to it.
fn crop_asset(assets: &AssetMode, id: &str, c: &ImageCrop) -> Option<String> {
    let AssetMode::Inline(_) = assets else { return None };
    let (x, y, w, h) = (c.x.round(), c.y.round(), c.width.round().max(1.0), c.height.round().max(1.0));
    let key = format!("{id}/{x}/{y}/{w}/{h}");
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok()?.get(&key) {
        return Some(hit.clone());
    }
    let (fw, fh) = image_size(id)?;
    let src = resolve_asset(assets, id)?;
    // Render the region 1:1 with nearest sampling: an exact pixel copy.
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}"><image href="{src}" width="{fw}" height="{fh}" image-rendering="optimizeSpeed"/></svg>"#
    );
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).ok()?;
    let mut pm = resvg::tiny_skia::Pixmap::new(w as u32, h as u32)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
    use base64::Engine;
    let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(pm.encode_png().ok()?));
    if let Ok(mut m) = cache.lock() {
        if m.len() > 512 {
            m.clear();
        }
        m.insert(key, uri.clone());
    }
    Some(uri)
}

static ASSETS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where uploaded images live, so exports can read their pixel size (9-slice
/// fills need it). Set once at startup.
pub fn set_data_dir(data_dir: &Path) {
    let _ = ASSETS_DIR.set(data_dir.to_path_buf());
}

/// Pixel size of an uploaded image, read from its header.
fn image_size(id: &str) -> Option<(f64, f64)> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let s = imagesize::size(asset_path(ASSETS_DIR.get()?, id)).ok()?;
    Some((s.width as f64, s.height as f64))
}

fn resolve_asset(assets: &AssetMode, id: &str) -> Option<String> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    match assets {
        AssetMode::Url => Some(format!("/api/assets/{id}")),
        AssetMode::Shared(token) => Some(format!("/api/share/{token}/assets/{id}")),
        AssetMode::Inline(dir) => {
            let bytes = std::fs::read(asset_path(dir, id)).ok()?;
            let mime = sniff_mime(&bytes).unwrap_or("image/png");
            use base64::Engine;
            Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
        }
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

/// System fonts (plus `FLEXMA_FONTS_DIR`), shared by every workspace.
pub fn base_fontdb() -> Arc<FontDb> {
    static DB: OnceLock<Arc<FontDb>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = FontDb::new();
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
pub fn svg_to_png(svg: &str, scale: f32, max_dim: u32, fonts: Arc<FontDb>) -> Result<(Vec<u8>, u32, u32), String> {
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb = fonts;
    opt.font_family = "Inter".into();
    let tree = resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    let size = tree.size();
    let mut s = scale.clamp(0.01, 8.0);
    let longest = size.width().max(size.height()) * s;
    if longest > max_dim as f32 {
        s *= max_dim as f32 / longest;
    }
    let w = ((size.width() * s).ceil() as u32).max(1);
    let h = ((size.height() * s).ceil() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("image too large")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut pixmap.as_mut());
    let png = pixmap.encode_png().map_err(|e| e.to_string())?;
    Ok((png, w, h))
}

/// One image of several renders in a labelled grid (a contact sheet), so an
/// agent can compare screens at a glance. `tiles` are (label, svg). Returns
/// the PNG, its size and each tile's rect in it.
pub fn contact_sheet(tiles: &[(String, String)], scale: f32, max_dim: u32, fonts: Arc<FontDb>) -> Result<(Vec<u8>, u32, u32, Vec<[u32; 4]>), String> {
    const PAD: f32 = 16.0;
    const LABEL: f32 = 22.0;
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb = fonts;
    opt.font_family = "Inter".into();
    let trees: Vec<resvg::usvg::Tree> = tiles.iter().map(|(_, svg)| resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
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
            .map(|(t, (label, _))| ((t.size().width() * s).max(label.chars().take(80).count() as f32 * 7.2 + 8.0), t.size().height() * s))
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
    for ((tree, (label, _)), (x, y)) in trees.iter().zip(tiles).zip(pos) {
        let tw = ((tree.size().width() * s).ceil() as u32).max(1);
        let th = ((tree.size().height() * s).ceil() as u32).max(1);
        let mut tile = resvg::tiny_skia::Pixmap::new(tw, th).ok_or("image too large")?;
        resvg::render(tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut tile.as_mut());
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
        for (slice, fit) in [(Some(slice), ImageFit::Stretch), (None, ImageFit::Stretch), (None, ImageFit::Fill)] {
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
            dim.x = -2500.0;
            dim.y = -2500.0;
            dim.width = 6000.0;
            dim.height = 6000.0;
            dim.fills = vec![Paint::Image { asset: "a".into(), fit, tint: None, slice, filled: None, crop: None, tile_scale: 1.0, opacity: 1.0, visible: true }];
            doc.apply(&Op::Create { node: dim }).unwrap();
            let resolve = |_: &str| Some(uri.clone());
            let size = |_: &str| Some((64.0, 40.0));
            let opts = SvgOptions { resolve_asset: &resolve, image_size: &size, ..SvgOptions::default() };
            let (svg, _) = export(&doc, &["f:1".to_string()], &opts).unwrap();
            let cov = image_coverage(&svg);
            assert!(cov > 0.99, "{slice:?} {fit:?}: only {:.0}% of the frame shows the image", cov * 100.0);
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
}
