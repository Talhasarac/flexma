//! Viewport camera. `screen = world * zoom + offset` in CSS pixels.

use flexma_doc::{Rect, Vec2};

pub const MIN_ZOOM: f64 = 0.02;
pub const MAX_ZOOM: f64 = 256.0;

/// Discrete zoom steps for keyboard zoom (+/-), like most design tools.
const STEPS: &[f64] = &[
    0.02, 0.03, 0.05, 0.0625, 0.1, 0.125, 0.25, 0.333, 0.5, 0.667, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0,
    32.0, 64.0, 128.0, 256.0,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { x: 0.0, y: 0.0, zoom: 1.0 }
    }
}

impl Camera {
    pub fn to_screen(&self, p: Vec2) -> Vec2 {
        Vec2::new(p.x * self.zoom + self.x, p.y * self.zoom + self.y)
    }

    pub fn to_world(&self, p: Vec2) -> Vec2 {
        Vec2::new((p.x - self.x) / self.zoom, (p.y - self.y) / self.zoom)
    }

    pub fn rect_to_screen(&self, r: &Rect) -> Rect {
        Rect::new(r.x * self.zoom + self.x, r.y * self.zoom + self.y, r.width * self.zoom, r.height * self.zoom)
    }

    /// Zoom by `factor`, keeping the world point under `anchor` (screen) fixed.
    pub fn zoom_at(&mut self, factor: f64, anchor: Vec2) {
        let w = self.to_world(anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.x = anchor.x - w.x * self.zoom;
        self.y = anchor.y - w.y * self.zoom;
    }

    pub fn set_zoom_at(&mut self, zoom: f64, anchor: Vec2) {
        let f = zoom / self.zoom;
        self.zoom_at(f, anchor);
    }

    pub fn step(&self, up: bool) -> f64 {
        let eps = 1e-3;
        if up {
            STEPS.iter().copied().find(|s| *s > self.zoom + eps).unwrap_or(MAX_ZOOM)
        } else {
            STEPS.iter().rev().copied().find(|s| *s < self.zoom - eps).unwrap_or(MIN_ZOOM)
        }
    }

    /// Fit `r` (world) into a viewport of `vw`×`vh` CSS px with padding.
    pub fn fit(&mut self, r: &Rect, vw: f64, vh: f64, padding: f64, max_zoom: f64) {
        if r.is_empty() || vw <= 0.0 || vh <= 0.0 {
            return;
        }
        let w = r.width.max(1.0);
        let h = r.height.max(1.0);
        let z = ((vw - padding * 2.0) / w).min((vh - padding * 2.0) / h).clamp(MIN_ZOOM, max_zoom);
        self.zoom = z;
        let c = r.center();
        self.x = vw / 2.0 - c.x * z;
        self.y = vh / 2.0 - c.y * z;
    }

    pub fn visible_world(&self, vw: f64, vh: f64) -> Rect {
        let a = self.to_world(Vec2::ZERO);
        let b = self.to_world(Vec2::new(vw, vh));
        Rect::from_points(a, b)
    }
}
