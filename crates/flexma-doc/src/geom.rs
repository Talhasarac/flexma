//! Minimal 2D geometry: points, axis-aligned rects and affine transforms.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
    pub const fn new(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }
    pub fn len(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn dist(self, o: Vec2) -> f64 {
        (self - o).len()
    }
}

impl std::ops::Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}
impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, s: f64) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}

/// Axis-aligned rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect { x, y, width, height }
    }

    pub fn from_points(a: Vec2, b: Vec2) -> Rect {
        let x0 = a.x.min(b.x);
        let y0 = a.y.min(b.y);
        Rect::new(x0, y0, (a.x - b.x).abs(), (a.y - b.y).abs())
    }

    /// An "empty" rect that unions as identity.
    pub fn empty() -> Rect {
        Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY)
    }

    pub fn is_empty(&self) -> bool {
        !(self.width.is_finite() && self.height.is_finite()) || self.width < 0.0 || self.height < 0.0
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
    pub fn center(&self) -> Vec2 {
        Vec2::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn include_point(&self, p: Vec2) -> Rect {
        self.union(&Rect::new(p.x, p.y, 0.0, 0.0))
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        !(self.is_empty() || o.is_empty())
            && self.x <= o.right()
            && o.x <= self.right()
            && self.y <= o.bottom()
            && o.y <= self.bottom()
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    pub fn contains_rect(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }

    pub fn inflate(&self, d: f64) -> Rect {
        Rect::new(self.x - d, self.y - d, self.width + 2.0 * d, self.height + 2.0 * d)
    }
}

/// Affine transform `[a c e; b d f; 0 0 1]` (column-vector convention, like canvas2d / SVG).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn translate(x: f64, y: f64) -> Affine {
        Affine { e: x, f: y, ..Affine::IDENTITY }
    }

    pub fn scale(sx: f64, sy: f64) -> Affine {
        Affine { a: sx, d: sy, ..Affine::IDENTITY }
    }

    pub fn rotate_deg(deg: f64) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }

    /// `self * o` — applies `o` first, then `self`.
    pub fn then_before(&self, o: &Affine) -> Affine {
        self.mul(o)
    }

    pub fn mul(&self, o: &Affine) -> Affine {
        Affine {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }

    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }

    pub fn apply_vector(&self, p: Vec2) -> Vec2 {
        Vec2::new(self.a * p.x + self.c * p.y, self.b * p.x + self.d * p.y)
    }

    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    pub fn invert(&self) -> Option<Affine> {
        let det = self.det();
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Affine {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            e: (self.c * self.f - self.d * self.e) * inv,
            f: (self.b * self.e - self.a * self.f) * inv,
        })
    }

    /// Bounding box of a local rect after transformation.
    pub fn map_rect(&self, r: &Rect) -> Rect {
        let pts = [
            self.apply(Vec2::new(r.x, r.y)),
            self.apply(Vec2::new(r.right(), r.y)),
            self.apply(Vec2::new(r.right(), r.bottom())),
            self.apply(Vec2::new(r.x, r.bottom())),
        ];
        let mut out = Rect::empty();
        for p in pts {
            out = out.include_point(p);
        }
        out
    }

    /// Rotation angle in degrees.
    pub fn rotation_deg(&self) -> f64 {
        self.b.atan2(self.a).to_degrees()
    }

    pub fn to_svg(&self) -> String {
        format!(
            "matrix({} {} {} {} {} {})",
            fmt_num(self.a),
            fmt_num(self.b),
            fmt_num(self.c),
            fmt_num(self.d),
            fmt_num(self.e),
            fmt_num(self.f)
        )
    }
}

/// Local transform of a node within its parent: translate to (x, y), then rotate
/// and flip around the node's center.
pub fn local_transform(x: f64, y: f64, w: f64, h: f64, rotation: f64, flip_x: bool, flip_y: bool) -> Affine {
    let t = Affine::translate(x, y);
    if rotation == 0.0 && !flip_x && !flip_y {
        return t;
    }
    let cx = w / 2.0;
    let cy = h / 2.0;
    let to_center = Affine::translate(cx, cy);
    let from_center = Affine::translate(-cx, -cy);
    let rot = Affine::rotate_deg(rotation);
    let flip = Affine::scale(if flip_x { -1.0 } else { 1.0 }, if flip_y { -1.0 } else { 1.0 });
    t.mul(&to_center).mul(&rot).mul(&flip).mul(&from_center)
}

/// Format a number compactly for SVG/JSON output.
pub fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{:.3}", v);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" { "0".to_string() } else { s.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_roundtrip() {
        let t = local_transform(10.0, 20.0, 100.0, 50.0, 30.0, false, true);
        let inv = t.invert().unwrap();
        let p = Vec2::new(3.0, 4.0);
        let q = inv.apply(t.apply(p));
        assert!((p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9);
    }

    #[test]
    fn rotation_keeps_center() {
        let t = local_transform(0.0, 0.0, 100.0, 50.0, 90.0, false, false);
        let c = t.apply(Vec2::new(50.0, 25.0));
        assert!((c.x - 50.0).abs() < 1e-9 && (c.y - 25.0).abs() < 1e-9);
    }
}
