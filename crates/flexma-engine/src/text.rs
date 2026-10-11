//! Text measurement and rasterization through a Canvas2D context. Line breaking
//! is shared with the server (`flexma_doc::text::wrap_lines`) so both agree on
//! where lines wrap; only the metrics differ (real fonts here).

use flexma_doc::paint::{TextAlign, TextAlignVertical, TextAutoResize};
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
        let max_w = match n.text_auto_resize {
            TextAutoResize::WidthAndHeight => None,
            _ => Some(n.width + 0.01),
        };
        let mut measure = |s: &str| self.measure(n, s);
        flexma_doc::text::wrap_lines(&n.characters, max_w, &mut measure)
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
        let margin = (n.font_size * 0.35).ceil().max(2.0);
        let lw = w + margin * 2.0;
        let lh_box = h + margin * 2.0;
        let longest = lw.max(lh_box) * scale;
        if longest > max_px as f64 {
            scale *= max_px as f64 / longest;
        }
        let px_w = ((lw * scale).ceil() as u32).max(1);
        let px_h = ((lh_box * scale).ceil() as u32).max(1);
        let lines = self.lines(n);
        let (asc, desc) = self.font_metrics(n);
        self.canvas.set_width(px_w);
        self.canvas.set_height(px_h);
        // Resizing resets context state.
        self.current_font.clear();
        self.current_ls = f64::NAN;
        let ctx = self.ctx.clone();
        let _ = ctx.set_transform(scale, 0.0, 0.0, scale, margin * scale, margin * scale);
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
        let lh = n.effective_line_height();
        let total = lh * lines.len() as f64;
        let y0 = match n.text_align_vertical {
            TextAlignVertical::Top => 0.0,
            TextAlignVertical::Center => (n.height - total) / 2.0,
            TextAlignVertical::Bottom => n.height - total,
        };
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
        Some(Raster { px_w, px_h, rect: [-margin, -margin, lw, lh_box] })
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
