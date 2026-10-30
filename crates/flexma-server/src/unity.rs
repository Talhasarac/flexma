//! Unity export: frames as a zip a Unity editor tool rebuilds as uGUI
//! (`skills/flexma-to-unity`), the same as in the design.
//!
//! Every layer that paints something comes as its own image, drawn by the
//! renderer exactly as the design shows it (cut by the frames that clip it),
//! with the stretchable middle cut down to a few pixels so Unity draws it as
//! a 9-slice at any size. Text comes as text, so TextMeshPro keeps it live,
//! with the font files it draws with and the metrics that place its
//! baseline. Layout comes as constraints (Unity anchors) and auto layout
//! settings, so the screen resizes and reflows like the design.
//!
//! `manifest.json` lists the frames as a layer tree; `sprites/`, `fonts/` and
//! `refs/` (each frame as Flexma draws it, to diff Unity's render against)
//! hold the files.

use crate::render::{self, AssetMode, FontDb, Layers, PaintSource, TextMeasurer};
use flexma_doc::{Document, Effect, Node, NodeType, Paint, Rect};
use resvg::tiny_skia::{ColorU8, Pixmap, PixmapPaint, PremultipliedColorU8, Transform};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

/// Layer images at most this many pixels on a side (bigger ones are drawn smaller).
const MAX_SPRITE_PX: u32 = 4096;
/// A run of identical columns (or rows) at least this long becomes a 9-slice middle...
const MIN_RUN: u32 = 8;
/// ...cut down to this many pixels.
const KEEP: u32 = 4;

pub struct Options {
    /// Pixels per design pixel in the layer images.
    pub scale: f32,
    /// Include the frames' own paint (their background), which is usually a
    /// mockup of what's behind the UI in the game.
    pub background: bool,
    /// The Unity project blends in linear light (Linear color space). Unless
    /// the page does too, see-through pixels are corrected (`correct_for_linear`).
    pub unity_linear: bool,
}

/// A layer image to draw.
struct Layer {
    id: String,
    name: String,
    svg: String,
    /// What it covers, in the layer's own coordinates.
    bounds: Rect,
    /// Its frame (index into `Plan::frames`) and its origin there, when it
    /// and its parents aren't rotated or flipped (so it lands on the frame's
    /// pixel grid, for what's behind later layers).
    frame: usize,
    at: Option<(f64, f64)>,
    /// Shown in the design (it and its parents), and how opaque, all told.
    shown: bool,
    opacity: f64,
    /// Drawn white, to be colored by Unity's `Image.color` (`single_color`).
    tint: Option<flexma_doc::Color>,
}

/// What the export draws, gathered while reading the document.
pub struct Plan {
    manifest: Value,
    layers: Vec<Layer>,
    /// Each frame's size.
    frames: Vec<(f64, f64)>,
    /// (frame id, SVG of the whole frame).
    refs: Vec<(String, String)>,
    fonts: Vec<(String, Vec<u8>)>,
    linear: bool,
    correct: bool,
}

/// Read the frames `ids` from `doc` (the room's document).
pub fn plan(doc: &Document, ids: &[String], opts: &Options, fonts: &Arc<FontDb>, data_dir: &Path, allowed: &HashSet<String>, file_id: &str) -> Result<Plan, String> {
    let measurer = TextMeasurer::new(fonts.clone());
    let paint = PaintSource { data_dir, file_id, share_token: None };
    let mut p = Planner { doc, opts, fonts, data_dir, allowed, paint: &paint, measurer: &measurer, layers: Vec::new(), faces: HashMap::new(), font_files: Vec::new(), warnings: Vec::new() };
    let mut frames = Vec::new();
    let mut sizes = Vec::new();
    let mut refs = Vec::new();
    for id in ids {
        let n = doc.get(id).ok_or_else(|| format!("no node '{id}'"))?;
        if !n.kind.is_frame_like() {
            return Err(format!("'{}' ({id}) isn't a frame; export frames, components or instances", n.name));
        }
        let at = Spot { frame: sizes.len(), at: Some((0.0, 0.0)), shown: true, opacity: 1.0 };
        sizes.push((n.width, n.height));
        frames.push(p.node(n, true, at));
        let layers = if opts.background { Layers::All } else { Layers::WithoutRootPaint };
        let (svg, _) = render::export_svg_as(doc, std::slice::from_ref(id), None, AssetMode::Inline(data_dir, allowed), Some(fonts), &paint, layers).ok_or("nothing to export")?;
        refs.push((id.clone(), svg));
    }
    let linear = render::linear_for(doc, ids);
    let correct = opts.unity_linear && !linear;
    let fonts_json: Vec<Value> = p
        .font_files
        .iter()
        .map(|(file, family, weight, italic, _)| json!({ "file": file, "family": family, "weight": weight, "italic": italic }))
        .collect();
    let manifest = json!({
        "version": 1,
        "scale": opts.scale,
        "linearBlending": linear,
        "correctedForLinear": correct,
        "frames": frames,
        "fonts": fonts_json,
        "warnings": p.warnings,
    });
    let font_files = p.font_files.into_iter().map(|(file, _, _, _, bytes)| (file, bytes)).collect();
    Ok(Plan { manifest, layers: p.layers, frames: sizes, refs, fonts: font_files, linear, correct })
}

struct Planner<'a> {
    doc: &'a Document,
    opts: &'a Options,
    fonts: &'a Arc<FontDb>,
    data_dir: &'a Path,
    allowed: &'a HashSet<String>,
    paint: &'a PaintSource<'a>,
    measurer: &'a TextMeasurer,
    layers: Vec<Layer>,
    /// Font file per (family, weight, italic) asked for.
    faces: HashMap<(String, u16, bool), Option<String>>,
    /// (file name, family, weight, italic, bytes).
    font_files: Vec<(String, String, u16, bool, Vec<u8>)>,
    warnings: Vec<String>,
}

/// Where a layer is: its frame, its origin there (see `Layer::at`), and
/// whether it shows and how opaque it is, with its parents'.
#[derive(Clone, Copy)]
struct Spot {
    frame: usize,
    at: Option<(f64, f64)>,
    shown: bool,
    opacity: f64,
}

impl Planner<'_> {
    fn node(&mut self, n: &Node, root: bool, spot: Spot) -> Value {
        let mut o = Map::new();
        o.insert("id".into(), json!(n.id));
        o.insert("name".into(), json!(n.name));
        o.insert("type".into(), enum_json(&n.kind));
        if !n.visible {
            o.insert("visible".into(), json!(false));
        }
        if n.opacity < 1.0 {
            o.insert("opacity".into(), json!(n.opacity));
        }
        o.insert("x".into(), json!(n.x));
        o.insert("y".into(), json!(n.y));
        o.insert("w".into(), json!(n.width));
        o.insert("h".into(), json!(n.height));
        if n.rotation != 0.0 {
            o.insert("rotation".into(), json!(n.rotation));
        }
        if n.flip_x {
            o.insert("flipX".into(), json!(true));
        }
        if n.flip_y {
            o.insert("flipY".into(), json!(true));
        }
        o.insert("constraints".into(), json!({ "h": enum_json(&n.constraints.horizontal), "v": enum_json(&n.constraints.vertical) }));
        o.insert("sizing".into(), json!({ "h": enum_json(&n.layout_sizing_horizontal), "v": enum_json(&n.layout_sizing_vertical) }));
        if n.layout_absolute {
            o.insert("absolute".into(), json!(true));
        }
        if n.kind.is_frame_like() && n.layout_mode != flexma_doc::LayoutMode::None {
            let mut l = json!({
                "mode": enum_json(&n.layout_mode),
                "padding": [n.padding.top, n.padding.right, n.padding.bottom, n.padding.left],
                "gap": n.item_spacing,
                "crossGap": n.counter_axis_spacing.unwrap_or(n.item_spacing),
                "primary": enum_json(&n.primary_axis_align),
                "counter": enum_json(&n.counter_axis_align),
                "wrap": n.layout_wrap,
            });
            if n.layout_mode == flexma_doc::LayoutMode::Grid {
                l["grid"] = json!({
                    "columns": n.grid_columns,
                    "rows": n.grid_rows,
                    "cell": n.grid_cell_size,
                    "startCorner": enum_json(&n.grid_start_corner),
                    "vertical": n.grid_vertical,
                });
            }
            o.insert("layout".into(), l);
        }
        if n.kind.is_frame_like() && n.clips_content {
            o.insert("clips".into(), json!(true));
        }
        if let Some(c) = &n.component_id {
            o.insert("component".into(), json!(c));
        }
        if let Some(k) = &n.key {
            o.insert("key".into(), json!(k));
        }
        if !n.meta.is_empty() {
            o.insert("meta".into(), Value::Object(n.meta.clone()));
        }
        self.warn_unsupported(n);
        if n.kind == NodeType::Text {
            o.insert("text".into(), self.text(n));
        } else if (!root || self.opts.background) && paints(n) {
            let tint = single_color(n);
            let layers = if tint.is_some() { Layers::OwnPaintWhite } else { Layers::OwnPaint };
            if let Some((svg, bounds)) = render::export_svg_as(self.doc, std::slice::from_ref(&n.id), None, AssetMode::Inline(self.data_dir, self.allowed), Some(self.fonts), self.paint, layers) {
                // The importer fills in the image id once it's drawn.
                o.insert("sprite".into(), json!({ "x": bounds.x, "y": bounds.y, "w": bounds.width, "h": bounds.height }));
                self.layers.push(Layer { id: n.id.clone(), name: n.name.clone(), svg, bounds, frame: spot.frame, at: spot.at, shown: spot.shown, opacity: spot.opacity, tint });
            }
        }
        let doc = self.doc;
        let kids: Vec<Value> = doc
            .children(&n.id)
            .iter()
            .filter_map(|c| doc.get(c))
            .map(|c| {
                let turned = c.rotation != 0.0 || c.flip_x || c.flip_y;
                let at = spot.at.filter(|_| !turned).map(|(x, y)| (x + c.x, y + c.y));
                self.node(c, false, Spot { frame: spot.frame, at, shown: spot.shown && c.visible, opacity: spot.opacity * c.opacity })
            })
            .collect();
        if !kids.is_empty() {
            o.insert("children".into(), Value::Array(kids));
        }
        Value::Object(o)
    }

    fn text(&mut self, n: &Node) -> Value {
        let (ascent, descent) = self.measurer.metrics(n);
        let mut t = json!({
            "characters": n.characters,
            "family": n.font_family,
            "weight": n.font_weight,
            "italic": n.italic,
            "size": n.font_size,
            "lineHeight": n.effective_line_height(),
            "letterSpacing": n.letter_spacing,
            "align": enum_json(&n.text_align),
            "valign": enum_json(&n.text_align_vertical),
            "autoResize": enum_json(&n.text_auto_resize),
            "wrap": n.text_wrap,
            "overflow": enum_json(&n.text_overflow),
            "color": text_color(&n.fills),
            "ascent": ascent,
            "descent": descent,
            "font": self.font(n),
        });
        if let Some(r) = &n.auto_font_size {
            t["autoFontSize"] = json!({ "min": r.min, "max": r.max });
        }
        if let Some(d) = n.text_decoration.as_deref().filter(|d| *d != "NONE") {
            t["decoration"] = json!(d);
        }
        if n.has_visible_stroke() {
            if let Some(Paint::Solid { color, opacity, .. }) = n.visible_strokes().last() {
                t["outline"] = json!({ "color": color.with_alpha(color.a * opacity).to_hex(), "width": n.stroke_weight, "align": enum_json(&n.stroke_align) });
            }
        }
        if let Some(Effect::DropShadow { color, x, y, blur, spread, .. }) = n.effects.iter().find(|e| matches!(e, Effect::DropShadow { visible: true, .. })) {
            t["shadow"] = json!({ "color": color.to_hex(), "x": x, "y": y, "blur": blur, "spread": spread });
        }
        if !n.text_runs.is_empty() {
            let runs: Vec<Value> = n
                .text_runs
                .iter()
                .map(|r| {
                    let mut probe = n.clone();
                    if let Some(f) = &r.font_family {
                        probe.font_family = f.clone();
                    }
                    if let Some(w) = r.font_weight {
                        probe.font_weight = w;
                    }
                    if let Some(i) = r.italic {
                        probe.italic = i;
                    }
                    let mut v = json!({ "start": r.start, "end": r.end });
                    if r.font_family.is_some() || r.font_weight.is_some() || r.italic.is_some() {
                        v["font"] = json!(self.font(&probe));
                        v["weight"] = json!(probe.font_weight);
                        v["italic"] = json!(probe.italic);
                    }
                    if let Some(ls) = r.letter_spacing {
                        v["letterSpacing"] = json!(ls);
                    }
                    if let Some(f) = &r.fills {
                        v["color"] = json!(text_color(f));
                    }
                    if let Some(d) = &r.text_decoration {
                        v["decoration"] = json!(d);
                    }
                    v
                })
                .collect();
            t["runs"] = Value::Array(runs);
        }
        t
    }

    /// The file name of the font `n` draws with, added to the export.
    fn font(&mut self, n: &Node) -> Option<String> {
        let key = (n.font_family.clone(), n.font_weight, n.italic);
        if let Some(f) = self.faces.get(&key) {
            return f.clone();
        }
        let file = match self.measurer.face_file(n) {
            Some((_, index, ..)) if index > 0 => {
                self.warnings.push(format!("Font '{}' is in a font collection, which TextMeshPro can't read; upload it to Flexma as a single .ttf or .otf", n.font_family));
                None
            }
            Some((bytes, _, family, weight, italic)) => {
                if !family.eq_ignore_ascii_case(&n.font_family) {
                    self.warnings.push(format!("Font '{}' isn't in this workspace, so Flexma draws it with '{family}' and so will Unity; add the font in Flexma to use it", n.font_family));
                }
                let name: String = format!("{family}-{weight}{}", if italic { "Italic" } else { "" }).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
                let file = format!("{name}.{}", if bytes.starts_with(b"OTTO") { "otf" } else { "ttf" });
                if !self.font_files.iter().any(|f| f.0 == file) {
                    self.font_files.push((file.clone(), family, weight, italic, bytes));
                }
                Some(file)
            }
            None => None,
        };
        self.faces.insert(key, file.clone());
        file
    }

    fn warn_unsupported(&mut self, n: &Node) {
        let mut warn = |what: &str| self.warnings.push(format!("'{}' ({}): {what}", n.name, n.id));
        if n.effects.iter().any(|e| matches!(e, Effect::BackgroundBlur { visible: true, .. })) {
            warn("background blur isn't exported; uGUI can't blur what's behind a layer without a custom shader");
        }
        if n.blend_mode != flexma_doc::BlendMode::Normal {
            warn("blend modes other than Normal aren't exported");
        }
        if n.kind == NodeType::Text && n.visible_fills().any(|p| !matches!(p, Paint::Solid { .. })) {
            warn("text with a gradient or image fill comes out in its first solid color");
        }
    }
}

/// Whether a layer draws anything of its own.
fn paints(n: &Node) -> bool {
    match n.kind {
        NodeType::Text | NodeType::Group => false,
        NodeType::Paint => true,
        _ => {
            (!n.hide_fill && n.visible_fills().any(shows))
                || (n.stroke_weight > 0.0 && n.visible_strokes().any(shows))
                || n.effects.iter().any(|e| matches!(e, Effect::DropShadow { visible: true, color, .. } if color.a > 0.0))
        }
    }
}

/// The one color a layer paints with: every visible fill and stroke the same
/// solid color, and no effects. Its image then comes white with the color as
/// a tint, so the game can recolor it (an icon that turns green when maxed, a
/// selected tab) the way Unity UI usually does. See-through colors only count
/// when they're near black (dims, scrims): their tint can be corrected for
/// linear blending once for any backdrop (`linear_alpha`), while lighter
/// see-through layers are corrected pixel by pixel against the design.
fn single_color(n: &Node) -> Option<flexma_doc::Color> {
    if n.kind == NodeType::Paint || n.effects.iter().any(|e| matches!(e, Effect::DropShadow { visible: true, .. } | Effect::LayerBlur { visible: true, .. } | Effect::BackgroundBlur { visible: true, .. })) {
        return None;
    }
    let fills = n.visible_fills().filter(|_| !n.hide_fill);
    let strokes = n.visible_strokes().filter(|_| n.stroke_weight > 0.0);
    let mut color: Option<flexma_doc::Color> = None;
    for p in fills.chain(strokes) {
        let Paint::Solid { color: c, opacity, .. } = p else { return None };
        let c = c.with_alpha(c.a * opacity);
        let dark = c.r.max(c.g).max(c.b) <= 0.1;
        if (c.a < 0.999 && !dark) || color.is_some_and(|k| k.to_hex() != c.to_hex()) {
            return None;
        }
        color = Some(c);
    }
    color
}

/// A paint that isn't fully transparent.
fn shows(p: &Paint) -> bool {
    match p {
        Paint::Solid { color, opacity, .. } => color.a * opacity > 0.0,
        _ => true,
    }
}

/// A text's color: its first visible solid fill, with the fill's opacity.
fn text_color(fills: &[Paint]) -> String {
    fills
        .iter()
        .filter(|p| p.is_visible())
        .find_map(|p| match p {
            Paint::Solid { color, opacity, .. } => Some(color.with_alpha(color.a * opacity).to_hex()),
            _ => None,
        })
        .unwrap_or_else(|| "#000000".into())
}

fn enum_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// Draw the plan's images and put everything in a zip.
pub fn draw(plan: Plan, scale: f32, fonts: Arc<FontDb>, images: render::Images) -> Result<Vec<u8>, String> {
    let Plan { mut manifest, layers, frames, refs, fonts: font_files, linear, correct } = plan;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut sprites = Map::new();
    let mut by_layer: HashMap<String, Value> = HashMap::new();
    // Each frame as drawn so far (sRGB blending, like the design), for what's
    // behind the layers that follow.
    let mut canvases: Vec<Option<Pixmap>> =
        frames.iter().map(|&(w, h)| if correct { Pixmap::new((w * scale as f64).ceil().max(1.0) as u32, (h * scale as f64).ceil().max(1.0) as u32) } else { None }).collect();
    let mut empty = Vec::new();
    for l in layers {
        let (mut pm, s) = render::svg_to_pixmap(&l.svg, scale, MAX_SPRITE_PX, fonts.clone(), linear, images)?;
        if pm.pixels().iter().all(|p| p.alpha() == 0) {
            if l.shown {
                empty.push(format!("'{}' ({}) has paint but its image came out empty; tell the Flexma developers", l.name, l.id));
            }
            continue;
        }
        let place = l.at.filter(|_| (s - scale).abs() < 1e-3).map(|(x, y)| (((x + l.bounds.x) * scale as f64).round() as i32, ((y + l.bounds.y) * scale as f64).round() as i32));
        if let (Some(canvas), Some((px, py))) = (canvases.get_mut(l.frame).and_then(Option::as_mut), place) {
            // A tinted (white) image can't hold per-pixel corrections; its edges blend as Unity blends.
            let fixes = if l.tint.is_some() { Vec::new() } else { correct_for_linear(&pm, canvas, px, py) };
            if l.shown {
                let paint = PixmapPaint { opacity: l.opacity as f32, ..PixmapPaint::default() };
                match l.tint {
                    Some(t) => canvas.draw_pixmap(px, py, tinted(&pm, t).as_ref(), &paint, Transform::identity(), None),
                    None => canvas.draw_pixmap(px, py, pm.as_ref(), &paint, Transform::identity(), None),
                }
            }
            let out = pm.pixels_mut();
            for (i, c) in fixes {
                out[i] = c;
            }
        }
        let (pm, border) = compact(pm);
        let (w, h) = (pm.width(), pm.height());
        let png = render::pixmap_png(pm)?;
        let hash = hex::encode(&Sha256::digest(&png)[..8]);
        let file = format!("sprites/{hash}.png");
        if !sprites.contains_key(&hash) {
            sprites.insert(hash.clone(), json!({ "file": file, "w": w, "h": h, "scale": s, "border": border }));
            files.push((file, png));
        }
        let mut sprite = json!({ "id": hash, "x": l.bounds.x, "y": l.bounds.y, "w": l.bounds.width, "h": l.bounds.height });
        if let Some(t) = l.tint {
            let t = if correct && t.a < 0.999 { t.with_alpha(linear_alpha(t.a)) } else { t };
            sprite["tint"] = json!(t.to_hex());
        }
        by_layer.insert(l.id, sprite);
    }
    // Point the layers at their images (layers that drew nothing lose theirs).
    fn link(v: &mut Value, by_layer: &HashMap<String, Value>) {
        let id = v.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        if let Some(o) = v.as_object_mut() {
            match by_layer.get(&id) {
                Some(s) => {
                    o.insert("sprite".into(), s.clone());
                }
                None => {
                    o.remove("sprite");
                }
            }
            if let Some(Value::Array(kids)) = o.get_mut("children") {
                for k in kids {
                    link(k, by_layer);
                }
            }
        }
    }
    if let Some(Value::Array(frames)) = manifest.get_mut("frames") {
        for f in frames {
            link(f, &by_layer);
        }
    }
    let mut ref_files = Map::new();
    for (id, svg) in refs {
        let (png, _, _) = render::svg_to_png(&svg, 1.0, 8192, fonts.clone(), linear, images)?;
        let file = format!("refs/{}.png", file_safe(&id));
        ref_files.insert(id, json!(file));
        files.push((file, png));
    }
    if let Some(Value::Array(w)) = manifest.get_mut("warnings") {
        w.extend(empty.into_iter().map(Value::String));
    }
    manifest["sprites"] = Value::Object(sprites);
    manifest["refs"] = Value::Object(ref_files);
    for (file, bytes) in font_files {
        files.push((format!("fonts/{file}"), bytes));
    }
    files.insert(0, ("manifest.json".into(), serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?));
    zip(files)
}

/// A white image colored by `tint`, the way Unity multiplies `Image.color` in.
fn tinted(pm: &Pixmap, tint: flexma_doc::Color) -> Pixmap {
    let mut out = pm.clone();
    let a = (tint.a.clamp(0.0, 1.0) * 255.0).round() as u16;
    let k = [tint.r, tint.g, tint.b].map(|v| (v.clamp(0.0, 1.0) * tint.a.clamp(0.0, 1.0) * 255.0).round() as u16);
    for p in out.pixels_mut() {
        let m = |v: u8, k: u16| ((v as u16 * k + 127) / 255) as u8;
        *p = PremultipliedColorU8::from_rgba(m(p.red(), k[0]), m(p.green(), k[1]), m(p.blue(), k[2]), m(p.alpha(), a)).unwrap_or(*p);
    }
    out
}

/// The alpha at which black darkens in linear light about as much as it does
/// at `a` in sRGB. One alpha can't match every backdrop exactly (sRGB isn't a
/// pure power curve); 2.1 keeps it within 8 levels from dark to light.
fn linear_alpha(a: f32) -> f32 {
    1.0 - (1.0 - a.clamp(0.0, 1.0)).powf(2.1)
}

/// Unity, in Linear color space, blends see-through pixels in linear light;
/// the design blends them in sRGB, so a 10% white line comes out about twice
/// as bright in Unity. For each see-through pixel of a layer over something
/// opaque in the design (`canvas`, the frame drawn so far, with the layer at
/// `px`, `py`), pick the color that Unity's blend turns into the design's
/// result over that backdrop. The alpha stays, so whatever is really behind
/// at runtime still shows through. Returns the new pixels by index.
fn correct_for_linear(pm: &Pixmap, canvas: &Pixmap, px: i32, py: i32) -> Vec<(usize, PremultipliedColorU8)> {
    use flexma_doc::Color;
    let (w, h, cw, ch) = (pm.width() as i32, pm.height() as i32, canvas.width() as i32, canvas.height() as i32);
    let (src, dst) = (pm.pixels(), canvas.pixels());
    let mut out = Vec::new();
    for y in 0..h {
        let cy = py + y;
        if cy < 0 || cy >= ch {
            continue;
        }
        for x in 0..w {
            let cx = px + x;
            if cx < 0 || cx >= cw {
                continue;
            }
            let i = (y * w + x) as usize;
            let p = src[i];
            let a = p.alpha();
            let b = dst[(cy * cw + cx) as usize];
            if a == 0 || a == 255 || b.alpha() < 128 {
                continue;
            }
            let (pc, bc) = (p.demultiply(), b.demultiply());
            let (c, a) = linear_match([pc.red(), pc.green(), pc.blue()], [bc.red(), bc.green(), bc.blue()], a);
            out.push((i, ColorU8::from_rgba(c[0], c[1], c[2], a).premultiply()));
        }
    }
    out
}

/// The color and alpha (sRGB, 0–255) that Unity's linear blend turns into
/// what the design's sRGB blend makes of `color` at `alpha` over `backdrop`.
/// Usually only the color changes; a dark see-through layer over something
/// bright would need a color darker than black, so its alpha goes up too.
fn linear_match(color: [u8; 3], backdrop: [u8; 3], alpha: u8) -> ([u8; 3], u8) {
    use flexma_doc::Color;
    let a0 = alpha as f32 / 255.0;
    let lin = |v: u8| Color::srgb_to_linear(v as f32 / 255.0);
    let target: [f32; 3] = std::array::from_fn(|k| Color::srgb_to_linear((color[k] as f32 * a0 + backdrop[k] as f32 * (1.0 - a0)) / 255.0));
    let back: [f32; 3] = std::array::from_fn(|k| lin(backdrop[k]));
    let mut a = a0;
    for k in 0..3 {
        let need = (target[k] - (1.0 - a) * back[k]) / a;
        if need < 0.0 && back[k] > 0.0 {
            a = a.max(1.0 - target[k] / back[k]);
        } else if need > 1.0 && back[k] < 1.0 {
            a = a.max((target[k] - back[k]) / (1.0 - back[k]));
        }
    }
    let a = a.clamp(a0, 1.0);
    let c = std::array::from_fn(|k| (Color::linear_to_srgb(((target[k] - (1.0 - a) * back[k]) / a).clamp(0.0, 1.0)) * 255.0).round() as u8);
    (c, (a * 255.0).round() as u8)
}

fn file_safe(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

fn zip(files: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, String> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        for (name, bytes) in files {
            // PNGs and fonts are compressed already.
            let method = if name.ends_with(".json") { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
            z.start_file(name, SimpleFileOptions::default().compression_method(method)).map_err(|e| e.to_string())?;
            z.write_all(&bytes).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf.into_inner())
}

/// Cut the longest run of identical columns, then of identical rows, down to
/// `KEEP` pixels, so the image is a 9-slice whose middle stretches to any
/// size and looks the same. Returns the image and its borders in pixels in
/// Unity's order: left, bottom, right, top (0 on an axis with no such run).
pub fn compact(pm: resvg::tiny_skia::Pixmap) -> (resvg::tiny_skia::Pixmap, [u32; 4]) {
    let (pm, left, right) = cut_columns(pm);
    let (pm, top, bottom) = transpose_cut(pm);
    (pm, [left, bottom, right, top])
}

/// The longest run `[a, b]` of positions whose lines `same(i, i + 1)` match.
fn longest_run(len: u32, same: impl Fn(u32) -> bool) -> Option<(u32, u32)> {
    let (mut best, mut start) = (None::<(u32, u32)>, 0u32);
    for i in 0..len {
        if i + 1 < len && same(i) {
            continue;
        }
        // Positions start..=i are all the same.
        if i - start + 1 >= MIN_RUN && best.is_none_or(|(a, b)| i - start > b - a) {
            best = Some((start, i));
        }
        start = i + 1;
    }
    best
}

/// Columns cut (see `compact`): the image and its left and right borders.
fn cut_columns(pm: resvg::tiny_skia::Pixmap) -> (resvg::tiny_skia::Pixmap, u32, u32) {
    let (w, h) = (pm.width(), pm.height());
    let px = pm.pixels();
    let same = |x: u32| (0..h).all(|y| px[(y * w + x) as usize] == px[(y * w + x + 1) as usize]);
    let Some((a, b)) = longest_run(w, same) else { return (pm, 0, 0) };
    let keep: Vec<u32> = (0..a + KEEP).chain(b + 1..w).collect();
    let nw = keep.len() as u32;
    let Some(mut out) = resvg::tiny_skia::Pixmap::new(nw, h) else { return (pm, 0, 0) };
    {
        let dst = out.pixels_mut();
        for y in 0..h {
            for (nx, &x) in keep.iter().enumerate() {
                dst[(y * nw) as usize + nx] = px[(y * w + x) as usize];
            }
        }
    }
    (out, a, w - 1 - b)
}

/// Rows cut (see `compact`): the image and its top and bottom borders.
fn transpose_cut(pm: resvg::tiny_skia::Pixmap) -> (resvg::tiny_skia::Pixmap, u32, u32) {
    let (w, h) = (pm.width(), pm.height());
    let px = pm.pixels();
    let row = |y: u32| &px[(y * w) as usize..((y + 1) * w) as usize];
    let Some((a, b)) = longest_run(h, |y| row(y) == row(y + 1)) else { return (pm, 0, 0) };
    let keep: Vec<u32> = (0..a + KEEP).chain(b + 1..h).collect();
    let nh = keep.len() as u32;
    let Some(mut out) = resvg::tiny_skia::Pixmap::new(w, nh) else { return (pm, 0, 0) };
    {
        let dst = out.pixels_mut();
        for (ny, &y) in keep.iter().enumerate() {
            dst[ny * w as usize..(ny + 1) * w as usize].copy_from_slice(row(y));
        }
    }
    (out, a, h - 1 - b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use resvg::tiny_skia::{Color, Paint as SkPaint, PathBuilder, Pixmap, Rect as SkRect, Transform};

    /// A rounded box shrinks to its corners plus a few pixels, with borders
    /// that keep the corners.
    #[test]
    fn rounded_box_becomes_a_nine_slice() {
        let mut pm = Pixmap::new(200, 100).unwrap();
        let mut paint = SkPaint::default();
        paint.set_color(Color::from_rgba8(30, 40, 50, 255));
        paint.anti_alias = true;
        let mut pb = PathBuilder::new();
        pb.push_rect(SkRect::from_xywh(10.0, 0.0, 180.0, 100.0).unwrap());
        pb.push_circle(10.0, 10.0, 10.0);
        let path = pb.finish().unwrap();
        pm.fill_path(&path, &paint, resvg::tiny_skia::FillRule::Winding, Transform::identity(), None);
        let (out, border) = compact(pm);
        assert!(out.width() < 40 && out.height() < 40, "{}×{}", out.width(), out.height());
        let [l, b, r, t] = border;
        assert!(l >= 10 && l < 30 && r < 20 && t >= 10 && t < 30 && b < 20, "{border:?}");
        assert_eq!(out.width(), l + KEEP + r);
        assert_eq!(out.height(), t + KEEP + b);
    }

    /// A button with a hard shadow deep inside clipping frames draws (resvg drew
    /// nothing when the shadow's filter region was far bigger than the image).
    #[test]
    fn shadowed_layer_inside_clips_draws() {
        let mut doc = flexma_doc::Document::new_empty();
        let mut n = 0;
        let mut gen = || {
            n += 1;
            format!("g:{n}")
        };
        let frame = |name: &str, x: f64, y: f64, w: f64, h: f64, radius: f64, kids: serde_json::Value| {
            serde_json::json!({ "type": "frame", "name": name, "x": x, "y": y, "w": w, "h": h, "radius": radius, "clip": true, "children": kids })
        };
        let button = serde_json::json!([{ "type": "frame", "name": "Button", "x": 1238, "y": 16, "w": 374, "h": 52, "radius": 12, "fill": "#e8b059",
            "effects": [{ "type": "DROP_SHADOW", "x": 0, "y": 3, "blur": 0, "color": "#9a6a2a" }] }]);
        let trader = frame("Trader", 0.0, 731.0, 1632.0, 84.0, 16.0, button);
        let main = frame("Main", 24.0, 24.0, 1632.0, 815.0, 0.0, serde_json::json!([trader]));
        let body = frame("Body", 0.0, 89.0, 1680.0, 863.0, 0.0, serde_json::json!([main]));
        let window = frame("Window", 120.0, 64.0, 1680.0, 952.0, 24.0, serde_json::json!([body]));
        let screen = frame("Screen", 0.0, 2760.0, 1920.0, 1080.0, 0.0, serde_json::json!([window]));
        let (ops, _) = flexma_doc::friendly::build_create_ops(&doc, "0:1", &[screen], None, &mut gen).unwrap();
        let (_, errs) = doc.apply_all(&ops);
        assert!(errs.is_empty(), "{errs:?}");
        let id = doc.nodes().find(|n| n.name == "Button").unwrap().id.clone();
        let opts = flexma_doc::svg::SvgOptions { own_paint: true, ..Default::default() };
        let (svg, _) = flexma_doc::svg::export(&doc, &[id], &opts).unwrap();
        let (pm, _) = crate::render::svg_to_pixmap(&svg, 2.0, 4096, crate::render::base_fontdb(), false, None).unwrap();
        assert!(pm.pixels().iter().filter(|p| p.alpha() == 255).count() > 70_000, "{svg}");
    }

    /// Unity's linear blend of a corrected pixel lands on the design's sRGB
    /// blend, also for a dark scrim over something bright (its alpha rises).
    #[test]
    fn corrected_pixels_blend_like_the_design() {
        use flexma_doc::Color;
        for (color, backdrop, alpha) in [([255, 255, 255], [21, 28, 24], 23u8), ([13, 18, 15], [150, 200, 240], 140), ([13, 18, 15], [150, 200, 240], 40), ([232, 176, 89], [31, 40, 34], 200)] {
            let (c, a) = linear_match(color, backdrop, alpha);
            let af = a as f32 / 255.0;
            for k in 0..3 {
                let design = (color[k] as f32 * alpha as f32 / 255.0 + backdrop[k] as f32 * (1.0 - alpha as f32 / 255.0)).round();
                let unity = Color::linear_to_srgb(Color::srgb_to_linear(c[k] as f32 / 255.0) * af + Color::srgb_to_linear(backdrop[k] as f32 / 255.0) * (1.0 - af)) * 255.0;
                assert!((design - unity).abs() <= 2.0, "{color:?} at {alpha} over {backdrop:?}: design {design}, unity {unity} ({c:?} at {a})");
            }
        }
    }

    /// A black dim at the corrected alpha darkens in linear light about as the
    /// design's alpha does in sRGB, within 8 levels from dark to light
    /// backdrops. Uncorrected, a 65% dim over mid grey is 32 levels too light.
    #[test]
    fn dims_darken_like_the_design() {
        use flexma_doc::Color;
        for a in [0.5f32, 0.65, 0.72, 0.75, 0.85] {
            for (b, tolerance) in [(40u8, 8.0), (90, 8.0), (128, 8.0), (180, 8.0), (230, 8.0)] {
                let design = b as f32 * (1.0 - a);
                let unity = Color::linear_to_srgb(Color::srgb_to_linear(b as f32 / 255.0) * (1.0 - linear_alpha(a))) * 255.0;
                assert!((design - unity).abs() <= tolerance, "alpha {a} over {b}: design {design}, unity {unity}");
            }
        }
    }

    /// An image with no stretchable middle stays as it is.
    #[test]
    fn noise_stays_whole() {
        let mut pm = Pixmap::new(20, 20).unwrap();
        for (i, p) in pm.pixels_mut().iter_mut().enumerate() {
            let v = (i * 37 % 251) as u8;
            *p = resvg::tiny_skia::PremultipliedColorU8::from_rgba(v, v, v, 255).unwrap();
        }
        let (out, border) = compact(pm);
        assert_eq!((out.width(), out.height(), border), (20, 20, [0, 0, 0, 0]));
    }
}

