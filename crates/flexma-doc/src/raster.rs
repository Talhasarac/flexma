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

    // ---- textured brushes (all off by default: a plain round brush) ----
    /// Stamp image: a built-in tip id (see `TIPS`). None: round.
    pub tip: Option<String>,
    /// Stamp the tip's own colors (a photo) instead of the brush color.
    pub tip_color: bool,
    /// Tip rotation, degrees.
    pub angle: f32,
    /// 0…1: how far the tip turns to follow the stroke's direction.
    pub follow: f32,
    /// Random variation per dab: size and flow as a fraction, angle in degrees.
    pub size_jitter: f32,
    pub angle_jitter: f32,
    pub flow_jitter: f32,
    /// Random offset from the path, as a fraction of the diameter.
    pub scatter: f32,
    /// Paper grain fixed to the layer: a built-in texture id (see `TEXTURES`).
    pub grain: Option<String>,
    /// Grain size (1 = one texture pixel per layer pixel) and strength 0…1.
    pub grain_scale: f32,
    pub grain_strength: f32,
    /// Color variation fixed to the layer: a built-in color texture id.
    pub tint: Option<String>,
    pub tint_scale: f32,
    pub tint_strength: f32,
    /// Smudge 0…1: above 0 the brush picks up the paint under it and drags
    /// it along; this much of the carried paint survives each dab.
    pub smudge: f32,
    /// While smudging, how much of the brush color is mixed into the carried
    /// paint each dab (mixer brushes).
    pub mix: f32,

    // ---- pen tilt (needs a stroke's `tilt`; off by default) ----
    /// Tilting the pen widens the dab, up to 1 + 2× this when flat (shading
    /// with the side of a pencil), and fades it by up to this much.
    pub tilt_size: f32,
    pub tilt_flow: f32,
    /// The tip turns to the pen's direction instead of the stroke's.
    pub tilt_angle: bool,
}

/// Built-in brush tips: id → file under `/brushes/`. An image never changes
/// once shipped (strokes replay against it); a new look gets a new id. A
/// retired tip (`potato-1`) leaves this list, so new strokes can't use it,
/// but keeps its file for the strokes already painted with it.
pub const TIPS: &[&str] = &["chalk-1", "roller-1", "knife-1", "cloud-1", "scribble-1", "speckle-1", "spray-1", "splatter-1", "bristle-1", "charcoal-1", "sponge-1", "grass-1", "leaf-1", "sparkle-1"];
/// Built-in grain and color textures (tiled across the layer).
pub const TEXTURES: &[&str] = &["paper-1", "drip-1", "noise-1", "colour-1", "halftone-1", "hatch-1"];
/// Largest smudge dab diameter: a smudge dab reads the layer under it.
pub const MAX_SMUDGE_SIZE: f32 = 400.0;

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
            tip: None,
            tip_color: false,
            angle: 0.0,
            follow: 0.0,
            size_jitter: 0.0,
            angle_jitter: 0.0,
            flow_jitter: 0.0,
            scatter: 0.0,
            grain: None,
            grain_scale: 1.0,
            grain_strength: 0.0,
            tint: None,
            tint_scale: 1.0,
            tint_strength: 0.0,
            smudge: 0.0,
            mix: 0.0,
            tilt_size: 0.0,
            tilt_flow: 0.0,
            tilt_angle: false,
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

    /// Whether dabs follow the pen's tilt (strokes then record it).
    pub fn uses_tilt(&self) -> bool {
        self.tilt_size > 0.0 || self.tilt_flow > 0.0 || self.tilt_angle
    }

    /// A smudging brush (it reads the layer under each dab).
    pub fn smudges(&self) -> bool {
        self.smudge > 0.0 && !self.erase
    }

    /// Clamp to sane values (input from clients). Unknown tips and textures
    /// are dropped.
    pub fn sanitized(&self) -> Brush {
        let f = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        let known = |v: &Option<String>, list: &[&str]| v.clone().filter(|id| list.contains(&id.as_str()));
        let smudge = f(self.smudge, 0.0, 1.0, 0.0);
        let max_size = if smudge > 0.0 { MAX_SMUDGE_SIZE } else { 2000.0 };
        let tip = known(&self.tip, TIPS);
        Brush {
            size: f(self.size, 0.5, max_size, 12.0),
            hardness: f(self.hardness, 0.0, 1.0, 0.8),
            flow: f(self.flow, 0.0, 1.0, 1.0),
            spacing: f(self.spacing, 0.02, 2.0, 0.1),
            color: self.color.map(|c| f(c, 0.0, 1.0, 0.0)),
            erase: self.erase,
            pressure_size: self.pressure_size,
            pressure_flow: self.pressure_flow,
            smoothing: f(self.smoothing, 0.0, 0.95, 0.0),
            // Tip colors without a tip draw nothing different, but would hide
            // the color picker (a saved brush whose tip was retired).
            tip_color: self.tip_color && tip.is_some(),
            tip,
            angle: f(self.angle, -360.0, 360.0, 0.0),
            follow: f(self.follow, 0.0, 1.0, 0.0),
            size_jitter: f(self.size_jitter, 0.0, 1.0, 0.0),
            angle_jitter: f(self.angle_jitter, 0.0, 180.0, 0.0),
            flow_jitter: f(self.flow_jitter, 0.0, 1.0, 0.0),
            scatter: f(self.scatter, 0.0, 4.0, 0.0),
            grain: known(&self.grain, TEXTURES),
            grain_scale: f(self.grain_scale, 0.05, 20.0, 1.0),
            grain_strength: f(self.grain_strength, 0.0, 1.0, 0.0),
            tint: known(&self.tint, TEXTURES),
            tint_scale: f(self.tint_scale, 0.05, 20.0, 1.0),
            tint_strength: f(self.tint_strength, 0.0, 1.0, 0.0),
            smudge,
            mix: f(self.mix, 0.0, 1.0, 0.0),
            tilt_size: f(self.tilt_size, 0.0, 1.0, 0.0),
            tilt_flow: f(self.tilt_flow, 0.0, 1.0, 0.0),
            tilt_angle: self.tilt_angle,
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
    /// Pen tilt at each point, for brushes that use it: (tilt 0 upright … 1
    /// flat, azimuth in radians in the layer's frame). Empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tilt: Vec<[f32; 2]>,
    /// Paint only where the layer already has pixels.
    #[serde(default)]
    pub alpha_lock: bool,
}

impl Stroke {
    /// Layer-pixel bounds of everything the stroke can touch: (x0, y0, x1, y1).
    pub fn bounds(&self) -> [f32; 4] {
        let b = &self.brush;
        // Round dabs stay within half the diameter; tips are drawn with the
        // longer side at the diameter and may be rotated; jitter and scatter
        // reach further.
        let tilt = 1.0 + 2.0 * b.tilt_size;
        let reach = if b.tip.is_some() || b.size_jitter > 0.0 || b.scatter > 0.0 {
            b.diameter(1.0) * ((1.0 + b.size_jitter) * tilt * std::f32::consts::FRAC_1_SQRT_2 + b.scatter)
        } else if b.tilt_size > 0.0 {
            b.diameter(1.0) / 2.0 * tilt
        } else {
            b.diameter(1.0) / 2.0
        };
        let r = reach + 2.0;
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
        if !self.tilt.is_empty() && (self.tilt.len() != self.points.len() || self.tilt.iter().flatten().any(|v| !v.is_finite())) {
            return Err("stroke tilt must have one finite value pair per point".into());
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

/// One brush stamp, in layer px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    pub x: f32,
    pub y: f32,
    /// Half the diameter (for tips: half the longer side).
    pub radius: f32,
    /// 0…1, before the color's alpha.
    pub alpha: f32,
    /// Tip rotation, radians.
    pub angle: f32,
}

/// Where dab placement left off, so a live stroke can be stamped as its
/// points arrive and give exactly the dabs the finished stroke gives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DabCursor {
    /// The next point to walk to.
    next: usize,
    /// Distance still to go along the path before the next dab.
    to_next: f32,
    /// Jitter is random per dab but the same on every client: seeded by the
    /// stroke id and numbered by dab.
    seed: u64,
    count: u32,
}

impl DabCursor {
    /// A cursor for `stroke_id` (its jitter sequence).
    pub fn new(stroke_id: &str) -> DabCursor {
        // FNV-1a.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in stroke_id.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        DabCursor { seed: h, ..DabCursor::default() }
    }

    /// Dabs placed so far.
    pub fn count(&self) -> u32 {
        self.count
    }
}

/// A random number in [0, 1) for dab `i`, channel `k` (SplitMix64).
fn rand01(seed: u64, i: u32, k: u32) -> f32 {
    let mut z = seed ^ ((i as u64) << 8 | k as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// Jitter, scatter and rotation for a dab placed at (x, y) with pressure `pr`
/// and pen `tilt` (tilt, azimuth), travelling in `dir` (radians; None for the
/// first dab).
fn make_dab(brush: &Brush, cursor: &mut DabCursor, x: f32, y: f32, pr: f32, tilt: Option<[f32; 2]>, dir: Option<f32>) -> Dab {
    let i = cursor.count;
    cursor.count += 1;
    let mut dab = Dab { x, y, radius: brush.diameter(pr) / 2.0, alpha: brush.alpha(pr), angle: 0.0 };
    let textured = brush.tip.is_some()
        || brush.size_jitter > 0.0
        || brush.flow_jitter > 0.0
        || brush.angle_jitter > 0.0
        || brush.scatter > 0.0
        || (brush.uses_tilt() && tilt.is_some());
    if !textured {
        // A plain round brush: exactly the dabs it always had.
        return dab;
    }
    let r = |k: u32| rand01(cursor.seed, i, k);
    if brush.size_jitter > 0.0 {
        dab.radius *= 1.0 + brush.size_jitter * (r(0) * 2.0 - 1.0);
    }
    if brush.flow_jitter > 0.0 {
        dab.alpha *= 1.0 - brush.flow_jitter * r(1);
    }
    if brush.scatter > 0.0 {
        let (a, d) = (r(2) * std::f32::consts::TAU, r(3).sqrt() * brush.scatter * brush.diameter(pr));
        dab.x += a.cos() * d;
        dab.y += a.sin() * d;
    }
    let mut base = dir.map(|d| d * brush.follow).unwrap_or(0.0);
    if let Some([t, az]) = tilt {
        dab.radius *= 1.0 + 2.0 * brush.tilt_size * t;
        dab.alpha *= 1.0 - brush.tilt_flow * t;
        if brush.tilt_angle {
            base = az;
        }
    }
    let jitter = if brush.angle_jitter > 0.0 { brush.angle_jitter.to_radians() * (r(4) * 2.0 - 1.0) } else { 0.0 };
    dab.angle = brush.angle.to_radians() + base + jitter;
    dab
}

/// Pen tilt between two points, `t` of the way (the azimuth the short way round).
fn lerp_tilt(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    let tau = std::f32::consts::TAU;
    let d = (b[1] - a[1] + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI;
    [a[0] + (b[0] - a[0]) * t, a[1] + d * t]
}

/// Dabs for `points[cursor..]` (with their `tilt`, when the brush uses it),
/// continuing from `cursor`.
pub fn dabs(brush: &Brush, points: &[[f32; 3]], tilt: &[[f32; 2]], cursor: &mut DabCursor) -> Vec<Dab> {
    let mut out = Vec::new();
    if points.is_empty() {
        return out;
    }
    let tilted = brush.uses_tilt() && tilt.len() >= points.len();
    let tilt_at = |i: usize| if tilted { Some(tilt[i]) } else { None };
    if cursor.next == 0 {
        let p = points[0];
        out.push(make_dab(brush, cursor, p[0], p[1], p[2], tilt_at(0), None));
        cursor.to_next = brush.step(p[2]);
        cursor.next = 1;
    }
    while cursor.next < points.len() {
        let a = points[cursor.next - 1];
        let b = points[cursor.next];
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let dir = (b[1] - a[1]).atan2(b[0] - a[0]);
        let mut d = 0.0;
        // Stamp along the segment, every `step` (which follows the pressure).
        while cursor.to_next <= len - d && len > 0.0 {
            d += cursor.to_next;
            let t = d / len;
            let pr = a[2] + (b[2] - a[2]) * t;
            let tl = tilt_at(cursor.next).map(|tb| lerp_tilt(tilt[cursor.next - 1], tb, t));
            out.push(make_dab(brush, cursor, a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, pr, tl, Some(dir)));
            cursor.to_next = brush.step(pr);
        }
        cursor.to_next -= len - d;
        cursor.next += 1;
    }
    out
}

/// Every dab of a finished stroke.
pub fn all_dabs(stroke: &Stroke) -> Vec<Dab> {
    dabs(&stroke.brush, &stroke.points, &stroke.tilt, &mut DabCursor::new(&stroke.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: Vec<[f32; 3]>) -> Stroke {
        Stroke { id: "s".into(), layer: "l".into(), brush: Brush { size: 10.0, spacing: 0.5, ..Brush::default() }, points, tilt: Vec::new(), alpha_lock: false }
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
            live.extend(dabs(&s.brush, &pts[..n], &[], &mut cur));
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

    /// Round brushes place exactly the dabs they did before textured brushes
    /// existed, so saved paintings replay onto their tile snapshots.
    #[test]
    fn round_dabs_are_pinned() {
        let s = Stroke {
            id: "pin".into(),
            layer: "l".into(),
            brush: Brush { size: 14.0, spacing: 0.15, pressure_size: true, pressure_flow: true, flow: 0.7, ..Brush::default() },
            points: vec![[3.0, 4.0, 0.2], [40.5, 9.25, 0.9], [41.0, 60.0, 0.5], [90.0, 61.0, 1.0]],
            tilt: Vec::new(),
            alpha_lock: false,
        };
        let d = all_dabs(&s);
        assert_eq!(d.len(), 101);
        let pins: [(usize, [u32; 4]); 5] = [
            (0, [1077936128, 1082130432, 1072466756, 1041194025]),
            (1, [1080238992, 1082291632, 1073020426, 1041675477]),
            (7, [1089010195, 1083387200, 1075262580, 1044947573]),
            (50, [1109590161, 1107999054, 1084364233, 1056618703]),
            (100, [1118918968, 1114891339, 1088239184, 1060161178]),
        ];
        for (i, bits) in pins {
            assert_eq!([d[i].x.to_bits(), d[i].y.to_bits(), d[i].radius.to_bits(), d[i].alpha.to_bits()], bits, "dab {i}");
            assert_eq!(d[i].angle, 0.0);
        }
    }

    #[test]
    fn jitter_is_the_same_everywhere_and_incremental() {
        let brush = Brush { tip: Some("chalk-1".into()), size: 30.0, size_jitter: 0.4, angle_jitter: 90.0, flow_jitter: 0.5, scatter: 0.3, follow: 1.0, ..Brush::default() };
        let pts: Vec<[f32; 3]> = (0..30).map(|i| [i as f32 * 5.0, (i as f32 * 0.3).sin() * 30.0, 1.0]).collect();
        let s = Stroke { id: "j1".into(), layer: "l".into(), brush: brush.clone(), points: pts.clone(), tilt: Vec::new(), alpha_lock: false };
        let a = all_dabs(&s);
        assert_eq!(a, all_dabs(&s.clone()));
        let mut cur = DabCursor::new("j1");
        let mut live = Vec::new();
        for n in [1, 3, 4, 17, 30] {
            live.extend(dabs(&brush, &pts[..n], &[], &mut cur));
        }
        assert_eq!(live, a);
        // Another stroke gets other jitter; the radii really vary.
        let b = all_dabs(&Stroke { id: "j2".into(), ..s });
        assert_ne!(a[3].radius, b[3].radius);
        assert!(a.iter().any(|d| d.radius > 15.5) && a.iter().any(|d| d.radius < 14.5));
    }

    #[test]
    fn tilt_widens_and_turns_dabs() {
        let brush = Brush { size: 10.0, spacing: 0.5, pressure_size: false, tilt_size: 0.5, tilt_angle: true, ..Brush::default() };
        let pts: Vec<[f32; 3]> = vec![[0.0, 0.0, 1.0], [40.0, 0.0, 1.0]];
        let upright = Stroke { id: "t".into(), layer: "l".into(), brush: brush.clone(), points: pts.clone(), tilt: vec![[0.0, 1.0], [0.0, 1.0]], alpha_lock: false };
        let flat = Stroke { tilt: vec![[1.0, 1.0], [1.0, 2.0]], ..upright.clone() };
        let (u, f) = (all_dabs(&upright), all_dabs(&flat));
        assert!((u[0].radius - 5.0).abs() < 1e-5 && (f[0].radius - 10.0).abs() < 1e-5, "flat doubles the radius at tilt_size 0.5");
        assert!((u[0].angle - 1.0).abs() < 1e-5, "the tip faces the pen");
        assert!(f.last().unwrap().angle > 1.9, "the azimuth is interpolated along the path");
        // Without tilt data (a mouse) the same brush paints plain dabs.
        let mouse = Stroke { tilt: Vec::new(), ..upright.clone() };
        assert!(all_dabs(&mouse).iter().all(|d| (d.radius - 5.0).abs() < 1e-5 && d.angle == 0.0));
        // Incremental placement gives the same dabs.
        let mut cur = DabCursor::new("t");
        let mut live = dabs(&brush, &flat.points[..1], &flat.tilt[..1], &mut cur);
        live.extend(dabs(&brush, &flat.points, &flat.tilt, &mut cur));
        assert_eq!(live, f);
        // Bad tilt is rejected; old strokes without it still read.
        assert!(Stroke { tilt: vec![[0.0, 0.0]], ..upright.clone() }.validate().is_err());
        let old: Stroke = serde_json::from_str(r#"{"id":"o","layer":"l","brush":{},"points":[[1,2,0.5]]}"#).unwrap();
        assert!(old.tilt.is_empty() && old.validate().is_ok());
        let mid = lerp_tilt([0.0, 6.2], [0.0, 0.1], 0.5)[1];
        assert!(mid > 6.2 || mid < 0.2, "the short way round: {mid}");
    }

    #[test]
    fn unknown_tips_are_dropped() {
        let b = Brush { tip: Some("../../etc".into()), grain: Some("paper-1".into()), smudge: 0.5, size: 900.0, ..Brush::default() }.sanitized();
        assert_eq!(b.tip, None);
        assert_eq!(b.grain.as_deref(), Some("paper-1"));
        assert_eq!(b.size, MAX_SMUDGE_SIZE);
        let retired = Brush { tip: Some("potato-1".into()), tip_color: true, ..Brush::default() }.sanitized();
        assert_eq!((retired.tip, retired.tip_color), (None, false));
    }

    #[test]
    fn old_brush_json_still_reads() {
        let b: Brush = serde_json::from_str(r#"{"size":20,"hardness":0.5,"flow":1,"spacing":0.1,"color":[0,0,0,1],"erase":false,"pressureSize":true,"pressureFlow":false,"smoothing":0.3}"#).unwrap();
        assert_eq!(b.tip, None);
        assert_eq!(b.grain_scale, 1.0);
        assert!(!b.smudges());
    }

    #[test]
    fn rejects_bad_strokes() {
        assert!(stroke(vec![]).validate().is_err());
        assert!(stroke(vec![[f32::NAN, 0.0, 1.0]]).validate().is_err());
        assert!(stroke(vec![[1.0, 2.0, 0.5]]).validate().is_ok());
    }
}

