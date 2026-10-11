//! Server-side rendering: SVG export (shared with the editor via flexma-doc) and
//! PNG rasterization through resvg, used for thumbnails, exports and the MCP
//! `get_screenshot` tool so agents can see what they built. Rasterizing takes
//! a font database so workspace fonts render (see `fonts::fontdb_for`).

use flexma_doc::svg::{export, SvgOptions};
use flexma_doc::{Color, Document, NodeId, Rect};
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
    let opts = SvgOptions { padding: 48.0, background: Some(bg), resolve_asset: &resolve, image_size: &image_size, measure_text: None };
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

/// Export nodes (or a whole page) to SVG.
pub fn export_svg(doc: &Document, ids: &[NodeId], background: Option<Color>, assets: AssetMode) -> Option<(String, Rect)> {
    let resolve = |id: &str| resolve_asset(&assets, id);
    let opts = SvgOptions { padding: 0.0, background, resolve_asset: &resolve, image_size: &image_size, measure_text: None };
    export(doc, ids, &opts)
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
            dim.fills = vec![Paint::Image { asset: "a".into(), fit, tint: None, slice, opacity: 1.0, visible: true }];
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
            mask.fills = vec![Paint::Image { asset: "a".into(), fit: ImageFit::Stretch, tint: None, slice: None, opacity: 1.0, visible: true }];
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
