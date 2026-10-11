//! SVG path data parsing into absolute move/line/cubic/close segments, plus
//! generated geometry for polygons and stars.

use crate::geom::{fmt_num, Rect, Vec2};
use crate::node::{Node, NodeType};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(Vec2),
    Line(Vec2),
    Cubic(Vec2, Vec2, Vec2),
    Close,
}

pub fn parse(d: &str) -> Vec<Seg> {
    let mut p = Parser { s: d.as_bytes(), i: 0 };
    let mut out = Vec::new();
    let mut cur = Vec2::ZERO;
    let mut start = Vec2::ZERO;
    let mut last_ctrl: Option<Vec2> = None; // for S/s
    let mut last_qctrl: Option<Vec2> = None; // for T/t
    let mut cmd = b'M';
    loop {
        p.skip_ws();
        if p.i >= p.s.len() {
            break;
        }
        let c = p.s[p.i];
        if c.is_ascii_alphabetic() {
            cmd = c;
            p.i += 1;
        } else if !(c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit()) {
            p.i += 1;
            continue;
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { Vec2::ZERO };
        let up = cmd.to_ascii_uppercase();
        let mut ctrl = None;
        let mut qctrl = None;
        match up {
            b'M' => {
                let (Some(x), Some(y)) = (p.num(), p.num()) else { break };
                cur = base + Vec2::new(x, y);
                start = cur;
                out.push(Seg::Move(cur));
                // Subsequent pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let (Some(x), Some(y)) = (p.num(), p.num()) else { break };
                cur = base + Vec2::new(x, y);
                out.push(Seg::Line(cur));
            }
            b'H' => {
                let Some(x) = p.num() else { break };
                cur = Vec2::new(if rel { cur.x + x } else { x }, cur.y);
                out.push(Seg::Line(cur));
            }
            b'V' => {
                let Some(y) = p.num() else { break };
                cur = Vec2::new(cur.x, if rel { cur.y + y } else { y });
                out.push(Seg::Line(cur));
            }
            b'C' => {
                let v: Option<Vec<f64>> = (0..6).map(|_| p.num()).collect();
                let Some(v) = v else { break };
                let c1 = base + Vec2::new(v[0], v[1]);
                let c2 = base + Vec2::new(v[2], v[3]);
                let e = base + Vec2::new(v[4], v[5]);
                out.push(Seg::Cubic(c1, c2, e));
                ctrl = Some(c2);
                cur = e;
            }
            b'S' => {
                let v: Option<Vec<f64>> = (0..4).map(|_| p.num()).collect();
                let Some(v) = v else { break };
                let c1 = match last_ctrl {
                    Some(lc) => cur * 2.0 - lc,
                    None => cur,
                };
                let c2 = base + Vec2::new(v[0], v[1]);
                let e = base + Vec2::new(v[2], v[3]);
                out.push(Seg::Cubic(c1, c2, e));
                ctrl = Some(c2);
                cur = e;
            }
            b'Q' => {
                let v: Option<Vec<f64>> = (0..4).map(|_| p.num()).collect();
                let Some(v) = v else { break };
                let q = base + Vec2::new(v[0], v[1]);
                let e = base + Vec2::new(v[2], v[3]);
                out.push(quad_to_cubic(cur, q, e));
                qctrl = Some(q);
                cur = e;
            }
            b'T' => {
                let v: Option<Vec<f64>> = (0..2).map(|_| p.num()).collect();
                let Some(v) = v else { break };
                let q = match last_qctrl {
                    Some(lq) => cur * 2.0 - lq,
                    None => cur,
                };
                let e = base + Vec2::new(v[0], v[1]);
                out.push(quad_to_cubic(cur, q, e));
                qctrl = Some(q);
                cur = e;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (p.num(), p.num(), p.num()) else { break };
                let (Some(large), Some(sweep)) = (p.flag(), p.flag()) else { break };
                let (Some(x), Some(y)) = (p.num(), p.num()) else { break };
                let e = base + Vec2::new(x, y);
                arc_to_cubics(cur, rx, ry, rot, large, sweep, e, &mut out);
                cur = e;
            }
            b'Z' => {
                out.push(Seg::Close);
                cur = start;
            }
            _ => {
                p.i += 1;
            }
        }
        last_ctrl = ctrl;
        last_qctrl = qctrl;
    }
    out
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn flag(&mut self) -> Option<bool> {
        self.skip_ws();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }

    fn num(&mut self) -> Option<f64> {
        self.skip_ws();
        let start = self.i;
        let s = self.s;
        if self.i < s.len() && (s[self.i] == b'-' || s[self.i] == b'+') {
            self.i += 1;
        }
        let mut seen_dot = false;
        let mut seen_digit = false;
        while self.i < s.len() {
            let c = s[self.i];
            if c.is_ascii_digit() {
                seen_digit = true;
                self.i += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i < s.len() && (s[self.i] == b'e' || s[self.i] == b'E') {
            let save = self.i;
            self.i += 1;
            if self.i < s.len() && (s[self.i] == b'-' || s[self.i] == b'+') {
                self.i += 1;
            }
            let digits_start = self.i;
            while self.i < s.len() && s[self.i].is_ascii_digit() {
                self.i += 1;
            }
            if self.i == digits_start {
                self.i = save;
            }
        }
        if !seen_digit {
            self.i = start;
            return None;
        }
        std::str::from_utf8(&s[start..self.i]).ok()?.parse().ok()
    }
}

fn quad_to_cubic(p0: Vec2, q: Vec2, p1: Vec2) -> Seg {
    let c1 = p0 + (q - p0) * (2.0 / 3.0);
    let c2 = p1 + (q - p1) * (2.0 / 3.0);
    Seg::Cubic(c1, c2, p1)
}

#[allow(clippy::too_many_arguments)]
fn arc_to_cubics(p0: Vec2, rx: f64, ry: f64, rot_deg: f64, large: bool, sweep: bool, p1: Vec2, out: &mut Vec<Seg>) {
    if p0 == p1 {
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-9 || ry < 1e-9 {
        out.push(Seg::Line(p1));
        return;
    }
    let (sin_phi, cos_phi) = rot_deg.to_radians().sin_cos();
    let dx = (p0.x - p1.x) / 2.0;
    let dy = (p0.y - p1.y) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den > 0.0 { (num / den).max(0.0).sqrt() } else { 0.0 };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cos_phi * cxp - sin_phi * cyp + (p0.x + p1.x) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (p0.y + p1.y) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        a
    };
    let theta1 = ang(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dtheta = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f64::consts::TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += std::f64::consts::TAU;
    }
    let n = (dtheta.abs() / (std::f64::consts::FRAC_PI_2)).ceil().max(1.0) as usize;
    let delta = dtheta / n as f64;
    let t = 4.0 / 3.0 * (delta / 4.0).tan();
    let point = |theta: f64| -> (Vec2, Vec2) {
        let (s, c) = theta.sin_cos();
        let x = cx + rx * c * cos_phi - ry * s * sin_phi;
        let y = cy + rx * c * sin_phi + ry * s * cos_phi;
        let dx = -rx * s * cos_phi - ry * c * sin_phi;
        let dy = -rx * s * sin_phi + ry * c * cos_phi;
        (Vec2::new(x, y), Vec2::new(dx, dy))
    };
    let mut theta = theta1;
    for i in 0..n {
        let (a, da) = point(theta);
        let (b, db) = point(theta + delta);
        let b = if i == n - 1 { p1 } else { b };
        out.push(Seg::Cubic(a + da * t, b - db * t, b));
        theta += delta;
    }
}

/// Exact bounds of the path (cubic extrema included).
pub fn bounds(segs: &[Seg]) -> Rect {
    let mut r = Rect::empty();
    let mut cur = Vec2::ZERO;
    let mut start = Vec2::ZERO;
    for s in segs {
        match *s {
            Seg::Move(p) => {
                r = r.include_point(p);
                cur = p;
                start = p;
            }
            Seg::Line(p) => {
                r = r.include_point(cur).include_point(p);
                cur = p;
            }
            Seg::Cubic(a, b, c) => {
                r = r.include_point(cur).include_point(c);
                for t in cubic_extrema(cur.x, a.x, b.x, c.x).into_iter().chain(cubic_extrema(cur.y, a.y, b.y, c.y)) {
                    r = r.include_point(cubic_at(cur, a, b, c, t));
                }
                cur = c;
            }
            Seg::Close => cur = start,
        }
    }
    r
}

fn cubic_at(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f64) -> Vec2 {
    let mt = 1.0 - t;
    p0 * (mt * mt * mt) + p1 * (3.0 * mt * mt * t) + p2 * (3.0 * mt * t * t) + p3 * (t * t * t)
}

/// Parameters in (0, 1) where one coordinate of a cubic has a local extremum.
fn cubic_extrema(p0: f64, p1: f64, p2: f64, p3: f64) -> Vec<f64> {
    let a = 3.0 * (-p0 + 3.0 * p1 - 3.0 * p2 + p3);
    let b = 6.0 * (p0 - 2.0 * p1 + p2);
    let c = 3.0 * (p1 - p0);
    let mut out = Vec::new();
    if a.abs() < 1e-12 {
        if b.abs() > 1e-12 {
            out.push(-c / b);
        }
    } else {
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 {
            let sq = disc.sqrt();
            out.push((-b + sq) / (2.0 * a));
            out.push((-b - sq) / (2.0 * a));
        }
    }
    out.retain(|t| *t > 0.0 && *t < 1.0);
    out
}

pub fn to_svg_d(segs: &[Seg]) -> String {
    let mut s = String::new();
    let p = |v: Vec2| format!("{} {}", fmt_num(v.x), fmt_num(v.y));
    for seg in segs {
        if !s.is_empty() {
            s.push(' ');
        }
        match *seg {
            Seg::Move(a) => s.push_str(&format!("M{}", p(a))),
            Seg::Line(a) => s.push_str(&format!("L{}", p(a))),
            Seg::Cubic(a, b, c) => s.push_str(&format!("C{} {} {}", p(a), p(b), p(c))),
            Seg::Close => s.push('Z'),
        }
    }
    s
}

/// Flatten into polylines (one per subpath) with the given tolerance.
pub fn flatten(segs: &[Seg], tolerance: f64) -> Vec<(Vec<Vec2>, bool)> {
    let mut out: Vec<(Vec<Vec2>, bool)> = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    let mut last = Vec2::ZERO;
    for s in segs {
        match *s {
            Seg::Move(p) => {
                if cur.len() > 1 {
                    out.push((std::mem::take(&mut cur), false));
                }
                cur.clear();
                cur.push(p);
                last = p;
            }
            Seg::Line(p) => {
                if cur.is_empty() {
                    cur.push(last);
                }
                cur.push(p);
                last = p;
            }
            Seg::Cubic(a, b, c) => {
                if cur.is_empty() {
                    cur.push(last);
                }
                let approx_len = last.dist(a) + a.dist(b) + b.dist(c);
                let n = ((approx_len / tolerance.max(0.01)).sqrt().ceil() as usize).clamp(2, 64);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let mt = 1.0 - t;
                    let p = last * (mt * mt * mt) + a * (3.0 * mt * mt * t) + b * (3.0 * mt * t * t) + c * (t * t * t);
                    cur.push(p);
                }
                last = c;
            }
            Seg::Close => {
                if cur.len() > 1 {
                    let first = cur[0];
                    out.push((std::mem::take(&mut cur), true));
                    last = first;
                }
            }
        }
    }
    if cur.len() > 1 {
        out.push((cur, false));
    }
    out
}

/// Geometry for a node as path segments in local coordinates, for node types
/// drawn as paths (vector, polygon, star, line).
pub fn node_path(n: &Node) -> Vec<Seg> {
    match n.kind {
        NodeType::Vector => parse(&n.path_data),
        // Lines run along the local x axis; `rotation` sets their angle.
        NodeType::Line => vec![Seg::Move(Vec2::ZERO), Seg::Line(Vec2::new(n.width, 0.0))],
        NodeType::Polygon => {
            let count = n.point_count.clamp(3, 100) as usize;
            regular_points(n.width, n.height, count, None)
        }
        NodeType::Star => {
            let count = n.point_count.clamp(3, 100) as usize;
            regular_points(n.width, n.height, count, Some(n.inner_radius.clamp(0.01, 1.0)))
        }
        _ => Vec::new(),
    }
}

fn regular_points(w: f64, h: f64, count: usize, inner: Option<f64>) -> Vec<Seg> {
    let cx = w / 2.0;
    let cy = h / 2.0;
    let total = if inner.is_some() { count * 2 } else { count };
    let mut out = Vec::with_capacity(total + 1);
    for i in 0..total {
        let ang = -std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::TAU / total as f64;
        let r = match inner {
            Some(ir) if i % 2 == 1 => ir,
            _ => 1.0,
        };
        let p = Vec2::new(cx + ang.cos() * cx * r, cy + ang.sin() * cy * r);
        out.push(if i == 0 { Seg::Move(p) } else { Seg::Line(p) });
    }
    out.push(Seg::Close);
    out
}

/// Translate all points in a path by `d` (used to normalize vector bounds to 0,0).
pub fn translate(segs: &[Seg], d: Vec2) -> Vec<Seg> {
    segs.iter()
        .map(|s| match *s {
            Seg::Move(p) => Seg::Move(p + d),
            Seg::Line(p) => Seg::Line(p + d),
            Seg::Cubic(a, b, c) => Seg::Cubic(a + d, b + d, c + d),
            Seg::Close => Seg::Close,
        })
        .collect()
}

/// Scale all points (used when resizing vector nodes).
pub fn scale(segs: &[Seg], sx: f64, sy: f64) -> Vec<Seg> {
    let m = |p: Vec2| Vec2::new(p.x * sx, p.y * sy);
    segs.iter()
        .map(|s| match *s {
            Seg::Move(p) => Seg::Move(m(p)),
            Seg::Line(p) => Seg::Line(m(p)),
            Seg::Cubic(a, b, c) => Seg::Cubic(m(a), m(b), m(c)),
            Seg::Close => Seg::Close,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let segs = parse("M0 0 L10 0 l0 10 h-10 z");
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[2], Seg::Line(Vec2::new(10.0, 10.0)));
        assert_eq!(segs[3], Seg::Line(Vec2::new(0.0, 10.0)));
    }

    #[test]
    fn parse_compact_numbers() {
        let segs = parse("M1-2.5L.5 3e1");
        assert_eq!(segs[0], Seg::Move(Vec2::new(1.0, -2.5)));
        assert_eq!(segs[1], Seg::Line(Vec2::new(0.5, 30.0)));
        let segs = parse("M0 0 10 10 20 0");
        assert_eq!(segs.len(), 3);
    }

    #[test]
    fn tight_cubic_bounds() {
        // A symmetric arch: control points rise to y=-100, the curve only to y=-75.
        let b = bounds(&parse("M0 0 C0 -100 100 -100 100 0"));
        assert!((b.y + 75.0).abs() < 1e-9, "{b:?}");
        assert_eq!(b.width, 100.0);
    }

    #[test]
    fn arc_ends_at_target() {
        let segs = parse("M0 0 A10 10 0 0 1 20 0");
        match segs.last().unwrap() {
            Seg::Cubic(_, _, e) => assert!((e.x - 20.0).abs() < 1e-9),
            _ => panic!(),
        }
    }
}
