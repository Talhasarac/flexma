//! Paint layers: brush strokes rasterized into 256 px tiles.
//!
//! A paint layer's pixels never go into the document (it's saved whole every
//! few seconds). Instead each stroke is a list of points (layer px and pen
//! pressure) plus the brush it was painted with. Every client turns a stroke
//! into the same *dabs*, round brush stamps placed along the path, and draws
//! them on the GPU in the order the server assigned, so all copies converge.
//! Tiles are saved as PNG snapshots now and then, so files open fast and
//! server-side exports show the painting.

use serde::{Deserialize, Serialize};

/// Tile size in layer pixels.
pub const TILE: u32 = 256;
/// Largest paint layer side, in pixels.
pub const MAX_LAYER_SIDE: f64 = 8192.0;
/// Most points one stroke may have.
pub const MAX_POINTS: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Brush {
    /// Diameter in layer px at full pressure.
    pub size: f32,
    /// 0 = soft (airbrush) … 1 = hard edge.
    pub hardness: f32,
    /// Alpha of each dab; overlapping dabs build up.
    pub flow: f32,
    /// Distance between dabs, as a fraction of the diameter.
    pub spacing: f32,
    /// Straight (not premultiplied) sRGB color and alpha.
    pub color: [f32; 4],
    pub erase: bool,
    /// Pen pressure changes the size / the flow.
    pub pressure_size: bool,
    pub pressure_flow: bool,
    /// Stabilizer 0…1: how much the input path is smoothed (applied while
    /// painting; strokes store the smoothed points).
    pub smoothing: f32,
}

impl Default for Brush {
    fn default() -> Self {
        Brush {
            size: 12.0,
            hardness: 0.8,
            flow: 1.0,
            spacing: 0.1,
            color: [0.07, 0.07, 0.07, 1.0],
            erase: false,
            pressure_size: true,
            pressure_flow: false,
            smoothing: 0.35,
        }
    }
}

impl Brush {
    /// Dab diameter at a pressure.
    pub fn diameter(&self, pressure: f32) -> f32 {
        let p = pressure.clamp(0.0, 1.0);
        self.size * if self.pressure_size { 0.08 + 0.92 * p } else { 1.0 }
    }

    /// Dab alpha at a pressure (before the color's own alpha).
    pub fn alpha(&self, pressure: f32) -> f32 {
        let p = pressure.clamp(0.0, 1.0);
        self.flow.clamp(0.0, 1.0) * if self.pressure_flow { p } else { 1.0 }
    }

    /// Distance to the next dab.
    pub fn step(&self, pressure: f32) -> f32 {
        (self.spacing.clamp(0.02, 2.0) * self.diameter(pressure)).max(0.5)
    }

    /// Clamp to sane values (input from clients).
    pub fn sanitized(&self) -> Brush {
        let f = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        Brush {
            size: f(self.size, 0.5, 2000.0, 12.0),
            hardness: f(self.hardness, 0.0, 1.0, 0.8),
            flow: f(self.flow, 0.0, 1.0, 1.0),
            spacing: f(self.spacing, 0.02, 2.0, 0.1),
            color: self.color.map(|c| f(c, 0.0, 1.0, 0.0)),
            erase: self.erase,
            pressure_size: self.pressure_size,
            pressure_flow: self.pressure_flow,
            smoothing: f(self.smoothing, 0.0, 0.95, 0.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    /// Chosen by the painting client; unique per file.
    pub id: String,
    /// The PAINT layer's node id.
    pub layer: String,
    pub brush: Brush,
    /// Points in layer pixels: (x, y, pressure 0…1).
    pub points: Vec<[f32; 3]>,
    /// Paint only where the layer already has pixels.
    #[serde(default)]
    pub alpha_lock: bool,
}

impl Stroke {
    /// Layer-pixel bounds of everything the stroke can touch: (x0, y0, x1, y1).
    pub fn bounds(&self) -> [f32; 4] {
        let r = self.brush.diameter(1.0) / 2.0 + 2.0;
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for p in &self.points {
            b[0] = b[0].min(p[0] - r);
            b[1] = b[1].min(p[1] - r);
            b[2] = b[2].max(p[0] + r);
            b[3] = b[3].max(p[1] + r);
        }
        b
    }

    /// The tiles the stroke can touch (within a layer of `w`×`h` px).
    pub fn tiles(&self, w: f64, h: f64) -> Vec<(i32, i32)> {
        if self.points.is_empty() {
            return Vec::new();
        }
        tiles_in(self.bounds(), w, h)
    }

    /// Reject malformed strokes before storing or drawing them.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.id.len() > 64 || self.layer.is_empty() || self.layer.len() > 128 {
            return Err("bad stroke id or layer".into());
        }
        if self.points.is_empty() || self.points.len() > MAX_POINTS {
            return Err(format!("a stroke needs 1 to {MAX_POINTS} points"));
        }
        if self.points.iter().flatten().any(|v| !v.is_finite()) {
            return Err("stroke points must be finite numbers".into());
        }
        Ok(())
    }
}

/// Tiles overlapping `b` = (x0, y0, x1, y1), clamped to a `w`×`h` layer.
pub fn tiles_in(b: [f32; 4], w: f64, h: f64) -> Vec<(i32, i32)> {
    let t = TILE as f32;
    let max_x = ((w as f32 / t).ceil() as i32 - 1).max(0);
    let max_y = ((h as f32 / t).ceil() as i32 - 1).max(0);
    let x0 = ((b[0] / t).floor() as i32).max(0);
    let y0 = ((b[1] / t).floor() as i32).max(0);
    let x1 = ((b[2] / t).floor() as i32).min(max_x);
    let y1 = ((b[3] / t).floor() as i32).min(max_y);
    let mut out = Vec::new();
    for ty in y0..=y1 {
        for tx in x0..=x1 {
            out.push((tx, ty));
        }
    }
    out
}

/// One round brush stamp, in layer px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    /// 0…1, before the color's alpha.
    pub alpha: f32,
}

/// Where dab placement left off, so a live stroke can be stamped as its
/// points arrive and give exactly the dabs the finished stroke gives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DabCursor {
    /// The next point to walk to.
    next: usize,
    /// Distance still to go along the path before the next dab.
    to_next: f32,
}

/// Dabs for `points[cursor..]`, continuing from `cursor`.
pub fn dabs(brush: &Brush, points: &[[f32; 3]], cursor: &mut DabCursor) -> Vec<Dab> {
    let mut out = Vec::new();
    if points.is_empty() {
        return out;
    }
    if cursor.next == 0 {
        let p = points[0];
        out.push(Dab { x: p[0], y: p[1], radius: brush.diameter(p[2]) / 2.0, alpha: brush.alpha(p[2]) });
        cursor.to_next = brush.step(p[2]);
        cursor.next = 1;
    }
    while cursor.next < points.len() {
        let a = points[cursor.next - 1];
        let b = points[cursor.next];
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let mut d = 0.0;
        // Stamp along the segment, every `step` (which follows the pressure).
        while cursor.to_next <= len - d && len > 0.0 {
            d += cursor.to_next;
            let t = d / len;
            let pr = a[2] + (b[2] - a[2]) * t;
            out.push(Dab {
                x: a[0] + (b[0] - a[0]) * t,
                y: a[1] + (b[1] - a[1]) * t,
                radius: brush.diameter(pr) / 2.0,
                alpha: brush.alpha(pr),
            });
            cursor.to_next = brush.step(pr);
        }
        cursor.to_next -= len - d;
        cursor.next += 1;
    }
    out
}

/// Every dab of a finished stroke.
pub fn all_dabs(stroke: &Stroke) -> Vec<Dab> {
    dabs(&stroke.brush, &stroke.points, &mut DabCursor::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: Vec<[f32; 3]>) -> Stroke {
        Stroke { id: "s".into(), layer: "l".into(), brush: Brush { size: 10.0, spacing: 0.5, ..Brush::default() }, points, alpha_lock: false }
    }

    #[test]
    fn dabs_follow_spacing() {
        let s = stroke(vec![[0.0, 0.0, 1.0], [100.0, 0.0, 1.0]]);
        let d = all_dabs(&s);
        // Diameter 10 at full pressure, spacing 0.5 → a dab every 5 px.
        assert_eq!(d.len(), 21);
        assert!((d[1].x - 5.0).abs() < 1e-4);
        assert!((d[20].x - 100.0).abs() < 1e-3);
    }

    #[test]
    fn incremental_matches_whole_stroke() {
        let pts: Vec<[f32; 3]> = (0..40).map(|i| [i as f32 * 3.7, (i as f32 * 0.4).sin() * 20.0, 0.3 + (i % 7) as f32 * 0.1]).collect();
        let s = stroke(pts.clone());
        let whole = all_dabs(&s);
        let mut cur = DabCursor::default();
        let mut live = Vec::new();
        for n in [1, 2, 5, 6, 17, 40] {
            live.extend(dabs(&s.brush, &pts[..n], &mut cur));
        }
        assert_eq!(live.len(), whole.len());
        for (a, b) in live.iter().zip(&whole) {
            assert!((a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3 && (a.radius - b.radius).abs() < 1e-4);
        }
    }

    #[test]
    fn pressure_scales_size() {
        let b = Brush { size: 20.0, pressure_size: true, ..Brush::default() };
        assert!(b.diameter(0.0) < 3.0);
        assert_eq!(b.diameter(1.0), 20.0);
        let b = Brush { pressure_size: false, ..b };
        assert_eq!(b.diameter(0.2), 20.0);
    }

    #[test]
    fn tiles_cover_bounds_within_layer() {
        let s = stroke(vec![[250.0, 10.0, 1.0], [260.0, 10.0, 1.0]]);
        assert_eq!(s.tiles(512.0, 512.0), vec![(0, 0), (1, 0)]);
        // Clamped to the layer: nothing past its last tile, nothing negative.
        let s = stroke(vec![[-50.0, -50.0, 1.0], [600.0, 10.0, 1.0]]);
        assert_eq!(s.tiles(300.0, 100.0), vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn rejects_bad_strokes() {
        assert!(stroke(vec![]).validate().is_err());
        assert!(stroke(vec![[f32::NAN, 0.0, 1.0]]).validate().is_err());
        assert!(stroke(vec![[1.0, 2.0, 0.5]]).validate().is_ok());
    }
}
