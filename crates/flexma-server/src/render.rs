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
    let opts = SvgOptions { padding: 48.0, background: Some(bg), resolve_asset: &resolve, measure_text: None };
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
    let opts = SvgOptions { padding: 0.0, background, resolve_asset: &resolve, measure_text: None };
    export(doc, ids, &opts)
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
