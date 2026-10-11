//! Text measurement and rasterization through a Canvas2D context. Line breaking
//! is shared with the server (`flexma_doc::text::wrap_lines`) so both agree on
//! where lines wrap; only the metrics differ (real fonts here).

use flexma_doc::paint::{StrokeAlign, TextAlign, TextAlignVertical, TextAutoResize, TextOverflow};
use flexma_doc::{Color, Node, Paint};
use std::collections::HashMap;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

pub struct TextEngine {
    pub canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    widths: HashMap<String, f64>,
    metrics: HashMap<String, (f64, f64)>,
    current_font: String,
    current_ls: f64,
}

pub struct Raster {
    pub px_w: u32,
    pub px_h: u32,
    /// Local-space rect covered by the raster (includes a margin for overhang).
    pub rect: [f64; 4],
}

pub fn css_color(c: Color) -> String {
    format!(
        "rgba({},{},{},{})",
        (c.r * 255.0).round(),
        (c.g * 255.0).round(),
        (c.b * 255.0).round(),
        c.a
    )
}

pub fn font_css(n: &Node, size: f64) -> String {
    let family = n.font_family.replace(['"', ';'], "");
    format!(
        "{}{} {}px \"{}\", \"Inter Variable\", Inter, system-ui, -apple-system, \"Segoe UI\", sans-serif",
        if n.italic { "italic " } else { "" },
        n.font_weight,
        size,
        family
    )
}

pub fn text_color(n: &Node) -> Color {
    n.fills
        .iter()
        .find(|p| p.is_visible())
        .map(|p| match p {
            Paint::Solid { color, opacity, .. } => color.with_alpha(color.a * opacity),
            other => other.approx_color(),
        })
        .unwrap_or(Color::BLACK)
}

/// A text outline: color, width and alignment (from the top visible stroke).
pub fn text_stroke(n: &Node) -> Option<(Color, f64, StrokeAlign)> {
    if n.stroke_weight <= 0.0 {
        return None;
    }
    let color = n.strokes.iter().rev().find(|p| p.is_visible()).map(|p| match p {
        Paint::Solid { color, opacity, .. } => color.with_alpha(color.a * opacity),
        other => other.approx_color(),
    })?;
    (color.a > 0.0).then_some((color, n.stroke_weight, n.stroke_align))
}

/// How far an outline reaches past the glyphs.
fn stroke_reach(stroke: Option<(Color, f64, StrokeAlign)>) -> f64 {
    match stroke {
        Some((_, w, StrokeAlign::Outside)) => w,
        Some((_, w, StrokeAlign::Center)) => w / 2.0,
        _ => 0.0,
    }
}

impl TextEngine {
    pub fn new() -> Result<TextEngine, JsValue> {
        let doc = web_sys::window().ok_or("no window")?.document().ok_or("no document")?;
        let canvas: HtmlCanvasElement = doc.create_element("canvas")?.dyn_into()?;
        canvas.set_width(16);
        canvas.set_height(16);
        let ctx: CanvasRenderingContext2d = canvas
            .get_context("2d")?
            .ok_or("no 2d context")?
            .dyn_into()?;
        Ok(TextEngine {
            canvas,
            ctx,
            widths: HashMap::new(),
            metrics: HashMap::new(),
            current_font: String::new(),
            current_ls: f64::NAN,
        })
    }

    /// Forget cached metrics (call when web fonts finish loading).
    pub fn reset(&mut self) {
        self.widths.clear();
        self.metrics.clear();
        self.current_font.clear();
        self.current_ls = f64::NAN;
    }

    fn set_font(&mut self, font: &str, letter_spacing: f64) {
        if self.current_font != font {
            self.ctx.set_font(font);
            self.current_font = font.to_string();
        }
        if self.current_ls != letter_spacing {
            let _ = js_sys::Reflect::set(&self.ctx, &"letterSpacing".into(), &format!("{letter_spacing}px").into());
            self.current_ls = letter_spacing;
        }
    }

    pub fn measure(&mut self, n: &Node, s: &str) -> f64 {
        let font = font_css(n, n.font_size);
        let key = format!("{font}|{}|{s}", n.letter_spacing);
        if let Some(w) = self.widths.get(&key) {
            return *w;
        }
        self.set_font(&font, n.letter_spacing);
        let w = self.ctx.measure_text(s).map(|m| m.width()).unwrap_or(0.0);
        if self.widths.len() > 50_000 {
            self.widths.clear();
        }
        self.widths.insert(key, w);
        w
    }

    /// (ascent, descent) of the font at the node's size.
    fn font_metrics(&mut self, n: &Node) -> (f64, f64) {
        let font = font_css(n, n.font_size);
        if let Some(m) = self.metrics.get(&font) {
            return *m;
        }
        self.set_font(&font, n.letter_spacing);
        let m = self
            .ctx
            .measure_text("Hg")
            .map(|m| (m.font_bounding_box_ascent(), m.font_bounding_box_descent()))
            .unwrap_or((n.font_size * 0.8, n.font_size * 0.2));
        let m = if m.0.is_finite() && m.0 > 0.0 { m } else { (n.font_size * 0.8, n.font_size * 0.2) };
        self.metrics.insert(font, m);
        m
    }

    pub fn lines(&mut self, n: &Node) -> Vec<String> {
        let mut measure = |s: &str| self.measure(n, s);
        flexma_doc::text::layout_lines(n, &mut measure)
    }

    /// Box size the node should have given its auto-resize mode.
    pub fn measure_box(&mut self, n: &Node) -> (f64, f64) {
        let lines = self.lines(n);
        let lh = n.effective_line_height();
        let h = (lines.len().max(1) as f64 * lh).ceil();
        match n.text_auto_resize {
            TextAutoResize::WidthAndHeight => {
                let w = lines.iter().map(|l| self.measure(n, l)).fold(0.0, f64::max);
                (w.ceil().max(1.0), h)
            }
            TextAutoResize::Height => (n.width, h),
            TextAutoResize::None => (n.width, n.height),
        }
    }

    /// Draw the node's text into the shared canvas at `scale` device px per unit.
    pub fn rasterize(&mut self, n: &Node, mut scale: f64, max_px: u32) -> Option<Raster> {
        let w = n.width.max(1.0);
        let h = n.height.max(1.0);
        let stroke = text_stroke(n);
        let margin = (n.font_size * 0.35 + stroke_reach(stroke)).ceil().max(2.0);
        let lines = self.lines(n);
        let lh = n.effective_line_height();
        let total = lh * lines.len() as f64;
        let y0 = match n.text_align_vertical {
            TextAlignVertical::Top => 0.0,
            TextAlignVertical::Center => (n.height - total) / 2.0,
            TextAlignVertical::Bottom => n.height - total,
        };
        // The raster covers the box, plus any text that overflows it visibly.
        let (mut x_min, mut y_min, mut x_max, mut y_max) = (0.0, 0.0, w, h);
        if n.text_overflow == TextOverflow::Visible {
            for line in &lines {
                let lw = self.measure(n, line);
                let x0 = match n.text_align {
                    TextAlign::Left => 0.0,
                    TextAlign::Center => (n.width - lw) / 2.0,
                    TextAlign::Right => n.width - lw,
                };
                x_min = f64::min(x_min, x0);
                x_max = f64::max(x_max, x0 + lw);
            }
            y_min = f64::min(y_min, y0);
            y_max = f64::max(y_max, y0 + total);
        }
        let (x_min, y_min) = (x_min.floor() - margin, y_min.floor() - margin);
        let lw = x_max.ceil() + margin - x_min;
        let lh_box = y_max.ceil() + margin - y_min;
        let longest = lw.max(lh_box) * scale;
        if longest > max_px as f64 {
            scale *= max_px as f64 / longest;
        }
        let px_w = ((lw * scale).ceil() as u32).max(1);
        let px_h = ((lh_box * scale).ceil() as u32).max(1);
        let (asc, desc) = self.font_metrics(n);
        self.canvas.set_width(px_w);
        self.canvas.set_height(px_h);
        // Resizing resets context state.
        self.current_font.clear();
        self.current_ls = f64::NAN;
        let ctx = self.ctx.clone();
        let _ = ctx.set_transform(scale, 0.0, 0.0, scale, -x_min * scale, -y_min * scale);
        self.set_font(&font_css(n, n.font_size), n.letter_spacing);
        let color = css_color(text_color(n));
        ctx.set_fill_style_str(&color);
        ctx.set_stroke_style_str(&color);
        ctx.set_text_baseline("alphabetic");
        let (align, x) = match n.text_align {
            TextAlign::Left => ("left", 0.0),
            TextAlign::Center => ("center", n.width / 2.0),
            TextAlign::Right => ("right", n.width),
        };
        ctx.set_text_align(align);
        // Outlined text is drawn as an opaque coverage mask first, then colored
        // from a distance field (see `outline_from_mask`).
        if stroke.is_some() {
            ctx.set_fill_style_str("#000");
        }
        for (i, line) in lines.iter().enumerate() {
            let top = y0 + lh * i as f64;
            let baseline = top + (lh - (asc + desc)) / 2.0 + asc;
            let _ = ctx.fill_text(line, x, baseline);
            if let Some(deco) = n.text_decoration.as_deref() {
                let lw = self.measure(n, line);
                let x0 = match n.text_align {
                    TextAlign::Left => 0.0,
                    TextAlign::Center => x - lw / 2.0,
                    TextAlign::Right => x - lw,
                };
                let thickness = (n.font_size / 14.0).max(1.0);
                let yy = match deco {
                    "UNDERLINE" => baseline + thickness * 1.5,
                    "STRIKETHROUGH" => baseline - asc * 0.32,
                    _ => continue,
                };
                ctx.fill_rect(x0, yy, lw, thickness);
            }
        }
        if let Some((color, sw, align)) = stroke {
            let _ = outline_from_mask(&ctx, px_w, px_h, text_color(n), color, sw * scale, align);
        }
        if n.text_overflow == TextOverflow::Clip {
            // Keep only what's inside the box (outlines included).
            let _ = ctx.set_transform(scale, 0.0, 0.0, scale, -x_min * scale, -y_min * scale);
            let _ = ctx.set_global_composite_operation("destination-in");
            ctx.set_fill_style_str("#000");
            ctx.fill_rect(0.0, 0.0, n.width, n.height);
            let _ = ctx.set_global_composite_operation("source-over");
        }
        Some(Raster { px_w, px_h, rect: [x_min, y_min, lw, lh_box] })
    }

    /// Small UI label (frame names, size badges) rendered at device scale.
    pub fn rasterize_label(&mut self, text: &str, size: f64, weight: u16, fg: &str, bg: Option<&str>, dpr: f64) -> (u32, u32) {
        let font = format!("{weight} {size}px \"Inter Variable\", Inter, system-ui, -apple-system, sans-serif");
        self.set_font(&font, 0.0);
        let tw = self.ctx.measure_text(text).map(|m| m.width()).unwrap_or(0.0);
        let (padx, pady) = if bg.is_some() { (5.0, 2.0) } else { (0.0, 1.0) };
        let w = tw + padx * 2.0;
        let h = size * 1.3 + pady * 2.0;
        let px_w = ((w * dpr).ceil() as u32).max(1);
        let px_h = ((h * dpr).ceil() as u32).max(1);
        self.canvas.set_width(px_w);
        self.canvas.set_height(px_h);
        self.current_font.clear();
        self.current_ls = f64::NAN;
        let ctx = self.ctx.clone();
        let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
        if let Some(bg) = bg {
            ctx.set_fill_style_str(bg);
            ctx.begin_path();
            let r = 3.0;
            let _ = ctx.round_rect_with_f64(0.0, 0.0, w, h, r);
            ctx.fill();
        }
        self.set_font(&font, 0.0);
        ctx.set_fill_style_str(fg);
        ctx.set_text_baseline("middle");
        ctx.set_text_align("left");
        let _ = ctx.fill_text(text, padx, h / 2.0 + 0.5);
        (px_w, px_h)
    }
}

// ---------------------------------------------------------------------------
// Text outlines from a distance field
// ---------------------------------------------------------------------------

/// Color a text mask (opaque glyphs on transparent) as fill + outline, like
/// TextMeshPro: from the distance to the glyph edge rather than canvas
/// strokeText, which draws seams where a font's contours overlap (common in
/// variable fonts, including Inter). `width` is in device pixels.
fn outline_from_mask(ctx: &CanvasRenderingContext2d, w: u32, h: u32, fill: Color, stroke: Color, width: f64, align: StrokeAlign) -> Result<(), JsValue> {
    let img = ctx.get_image_data(0.0, 0.0, w as f64, h as f64)?;
    let mut px = img.data().0;
    let n = (w * h) as usize;
    let cov: Vec<f32> = (0..n).map(|i| px[i * 4 + 3] as f32 / 255.0).collect();
    // Squared distances to the nearest inside / outside pixel.
    let inside = |c: f32| c >= 0.5;
    let mut to_in: Vec<f32> = cov.iter().map(|&c| if inside(c) { 0.0 } else { f32::INFINITY }).collect();
    let mut to_out: Vec<f32> = cov.iter().map(|&c| if inside(c) { f32::INFINITY } else { 0.0 }).collect();
    edt(&mut to_in, w as usize, h as usize);
    edt(&mut to_out, w as usize, h as usize);
    let t = width.max(0.0) as f32;
    // Base layer (outline color) reaches `base` px past the edge; the top
    // layer (fill color) stops `top` px inside it. NaN = use the coverage.
    let (base, top) = match align {
        StrokeAlign::Outside => (t, f32::NAN),
        StrokeAlign::Center => (t / 2.0, -t / 2.0),
        StrokeAlign::Inside => (f32::NAN, -t),
    };
    let ramp = |edge: f32, sd: f32| (edge - sd + 0.5).clamp(0.0, 1.0);
    for i in 0..n {
        // Signed distance in px: negative inside the glyph.
        let sd = if inside(cov[i]) { 0.5 - to_out[i].sqrt() } else { to_in[i].sqrt() - 0.5 };
        let sa = if base.is_nan() { cov[i] } else { ramp(base, sd) } * stroke.a;
        let fa = if top.is_nan() { cov[i] } else { ramp(top, sd) } * fill.a;
        let a = fa + sa * (1.0 - fa);
        let o = i * 4;
        if a <= 0.0 {
            px[o..o + 4].copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let mix = |f: f32, s: f32| ((f * fa + s * sa * (1.0 - fa)) / a * 255.0).round().clamp(0.0, 255.0) as u8;
        px[o] = mix(fill.r, stroke.r);
        px[o + 1] = mix(fill.g, stroke.g);
        px[o + 2] = mix(fill.b, stroke.b);
        px[o + 3] = (a * 255.0).round() as u8;
    }
    let out = web_sys::ImageData::new_with_u8_clamped_array_and_sh(wasm_bindgen::Clamped(&px), w, h)?;
    ctx.put_image_data(&out, 0.0, 0.0)
}

/// In-place 2D squared Euclidean distance transform (Felzenszwalb &
/// Huttenlocher): 0 at feature pixels, infinity elsewhere on input.
fn edt(grid: &mut [f32], w: usize, h: usize) {
    let len = w.max(h);
    let (mut f, mut d, mut v, mut z) = (vec![0.0f32; len], vec![0.0f32; len], vec![0usize; len], vec![0.0f32; len + 1]);
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        edt_1d(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&grid[y * w..(y + 1) * w]);
        edt_1d(&f[..w], &mut d[..w], &mut v, &mut z);
        grid[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
}

fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        if f[q].is_infinite() && f[v[k]].is_infinite() {
            continue;
        }
        loop {
            let p = v[k];
            let s = if f[p].is_infinite() {
                f32::NEG_INFINITY
            } else {
                ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32)
            };
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // First parabola: replace it.
                v[k] = q;
                z[k + 1] = f32::INFINITY;
                break;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
            break;
        }
    }
    let mut j = 0usize;
    for q in 0..n {
        while z[j + 1] < q as f32 {
            j += 1;
        }
        let p = v[j];
        d[q] = if f[p].is_infinite() { f32::INFINITY } else { (q as f32 - p as f32).powi(2) + f[p] };
    }
}

#[cfg(test)]
mod edt_tests {
    use super::edt;

    #[test]
    fn distances_to_the_nearest_feature() {
        // A 7×1 row with one feature at x=2.
        let mut g = vec![f32::INFINITY; 7];
        g[2] = 0.0;
        edt(&mut g, 7, 1);
        assert_eq!(g, vec![4.0, 1.0, 0.0, 1.0, 4.0, 9.0, 16.0]);
        // 2D: feature at the center of a 5×5 grid.
        let mut g = vec![f32::INFINITY; 25];
        g[12] = 0.0;
        edt(&mut g, 5, 5);
        assert_eq!(g[0], 8.0);
        assert_eq!(g[7], 1.0);
        assert_eq!(g[24], 8.0);
    }
}
