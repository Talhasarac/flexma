//! Turn SVG markup into editable Flexma layers: a frame of vector layers, one
//! per SVG shape, with fills, gradients, strokes, caps, joins, dashes and fill
//! rules carried over. Used by the MCP `import_svg` tool and the icon library.
//! usvg does the parsing: it resolves CSS, `<use>`, transforms and basic shapes
//! (rect, circle, polygon…) into plain paths, and turns text into outlines.

use crate::render::base_fontdb;
use resvg::usvg::{self, tiny_skia_path::PathSegment, Transform};
use serde_json::{json, Map, Value};

pub const MAX_SVG_BYTES: usize = 2 * 1024 * 1024;
const MAX_PATHS: usize = 3000;

pub struct ImportOptions {
    pub name: String,
    /// Target size; the other side keeps the aspect ratio. Default: the SVG's own size.
    pub width: Option<f64>,
    pub height: Option<f64>,
    /// Merge every shape into one vector layer (icons). Needs one shared style.
    pub flatten: bool,
}

pub struct Imported {
    /// A friendly FRAME spec (with children), ready for `build_create_ops`.
    pub spec: Value,
    pub layers: usize,
    /// Things that couldn't be carried over.
    pub notes: Vec<String>,
}

pub fn import(svg: &str, opts: &ImportOptions) -> Result<Imported, String> {
    if svg.len() > MAX_SVG_BYTES {
        return Err(format!("the SVG is larger than {} MB", MAX_SVG_BYTES / 1024 / 1024));
    }
    let uopt = usvg::Options { fontdb: base_fontdb(), ..Default::default() };
    let tree = usvg::Tree::from_str(svg, &uopt).map_err(|e| format!("couldn't read the SVG: {e}"))?;
    let size = tree.size();
    let (w0, h0) = (size.width() as f64, size.height() as f64);
    let scale = match (opts.width, opts.height) {
        (Some(w), _) if w > 0.0 => w / w0,
        (None, Some(h)) if h > 0.0 => h / h0,
        _ => 1.0,
    };
    let mut ctx = Ctx { scale: scale as f32, shapes: Vec::new(), notes: Vec::new() };
    ctx.walk(tree.root(), 1.0);
    if ctx.shapes.is_empty() {
        return Err("the SVG has no visible shapes to import".into());
    }
    if ctx.shapes.len() > MAX_PATHS {
        return Err(format!("the SVG has {} shapes; the limit is {MAX_PATHS}", ctx.shapes.len()));
    }
    let (w, h) = ((w0 * scale).max(0.01), (h0 * scale).max(0.01));
    let mut notes = ctx.notes;
    notes.sort();
    notes.dedup();

    let shared_style = ctx.shapes.windows(2).all(|p| p[0].style == p[1].style);
    let children: Vec<Value> = if opts.flatten && shared_style {
        let d: Vec<&str> = ctx.shapes.iter().map(|s| s.d.as_str()).collect();
        let union = ctx.shapes.iter().filter_map(|s| s.bounds).fold(None::<[f64; 4]>, |acc, [x, y, w, h]| {
            Some(match acc {
                None => [x, y, w, h],
                Some([ax, ay, aw, ah]) => {
                    let (x0, y0) = (ax.min(x), ay.min(y));
                    [x0, y0, (ax + aw).max(x + w) - x0, (ay + ah).max(y + h) - y0]
                }
            })
        });
        vec![vector_spec(&opts.name, &d.join(" "), &ctx.shapes[0].style, union)]
    } else {
        if opts.flatten {
            notes.push("shapes have different styles, so they were kept as separate layers".into());
        }
        ctx.shapes.iter().enumerate().map(|(i, s)| vector_spec(&s.name(i), &s.d, &s.style, s.bounds)).collect()
    };
    let layers = children.len();
    let spec = json!({
        "type": "frame",
        "name": opts.name,
        "width": round(w),
        "height": round(h),
        "fill": "none",
        "clip": false,
        "children": children,
    });
    Ok(Imported { spec, layers, notes })
}

/// One flattened SVG shape in final (frame) coordinates.
struct Shape {
    id: String,
    d: String,
    bounds: Option<[f64; 4]>,
    style: Map<String, Value>,
}

impl Shape {
    fn name(&self, i: usize) -> String {
        if self.id.is_empty() {
            format!("Path {}", i + 1)
        } else {
            self.id.clone()
        }
    }
}

struct Ctx {
    scale: f32,
    shapes: Vec<Shape>,
    notes: Vec<String>,
}

impl Ctx {
    fn walk(&mut self, group: &usvg::Group, opacity: f32) {
        for node in group.children() {
            match node {
                usvg::Node::Group(g) => self.walk(g, opacity * g.opacity().get()),
                usvg::Node::Path(p) => self.path(p, opacity),
                usvg::Node::Text(t) => self.walk(t.flattened(), opacity),
                usvg::Node::Image(_) => self.notes.push("embedded images were skipped (upload them with upload_image)".into()),
            }
        }
    }

    fn path(&mut self, p: &usvg::Path, opacity: f32) {
        if !p.is_visible() || (p.fill().is_none() && p.stroke().is_none()) {
            return;
        }
        let ts = Transform::from_scale(self.scale, self.scale).pre_concat(p.abs_transform());
        let Some(data) = p.data().clone().transform(ts) else { return };
        let b = data.bounds();
        let mut style = Map::new();
        if let Some(f) = p.fill() {
            if let Some(paint) = self.paint(f.paint(), f.opacity().get(), ts) {
                style.insert("fills".into(), json!([paint]));
            }
            if f.rule() == usvg::FillRule::EvenOdd {
                style.insert("fillRule".into(), json!("EVENODD"));
            }
        }
        // Lines and other zero-area shapes only show their stroke.
        if !style.contains_key("fills") || b.width() < 0.001 || b.height() < 0.001 {
            style.insert("fills".into(), json!([]));
            style.remove("fillRule");
        }
        if let Some(s) = p.stroke() {
            if let Some(paint) = self.paint(s.paint(), s.opacity().get(), ts) {
                // Stroke widths scale with the transform's average scale.
                let k = (ts.sx * ts.sy - ts.kx * ts.ky).abs().sqrt() as f64;
                style.insert("strokes".into(), json!([paint]));
                style.insert("strokeWeight".into(), json!(round(s.width().get() as f64 * k)));
                style.insert(
                    "strokeCap".into(),
                    json!(match s.linecap() {
                        usvg::LineCap::Butt => "NONE",
                        usvg::LineCap::Round => "ROUND",
                        usvg::LineCap::Square => "SQUARE",
                    }),
                );
                style.insert(
                    "strokeJoin".into(),
                    json!(match s.linejoin() {
                        usvg::LineJoin::Round => "ROUND",
                        usvg::LineJoin::Bevel => "BEVEL",
                        _ => "MITER",
                    }),
                );
                if let Some(dash) = s.dasharray() {
                    style.insert("dashPattern".into(), json!(dash.iter().map(|v| round(*v as f64 * k)).collect::<Vec<_>>()));
                }
            }
        }
        if opacity < 0.999 {
            style.insert("opacity".into(), json!(round(opacity as f64)));
        }
        if style.get("fills").map(|f| f == &json!([])).unwrap_or(true) && !style.contains_key("strokes") {
            return;
        }
        self.shapes.push(Shape {
            id: p.id().to_string(),
            d: path_d(&data),
            bounds: Some([b.left() as f64, b.top() as f64, b.width() as f64, b.height() as f64]),
            style,
        });
    }

    fn paint(&mut self, paint: &usvg::Paint, opacity: f32, ts: Transform) -> Option<Value> {
        match paint {
            usvg::Paint::Color(c) => Some(json!({ "type": "SOLID", "color": hex(c, 1.0), "opacity": round(opacity as f64) })),
            usvg::Paint::LinearGradient(g) => {
                let t = ts.pre_concat(g.transform());
                let (mut a, mut b) = (
                    usvg::tiny_skia_path::Point::from_xy(g.x1(), g.y1()),
                    usvg::tiny_skia_path::Point::from_xy(g.x2(), g.y2()),
                );
                t.map_point(&mut a);
                t.map_point(&mut b);
                let angle = ((b.y - a.y) as f64).atan2((b.x - a.x) as f64).to_degrees();
                let stops: Vec<Value> = g
                    .stops()
                    .iter()
                    .map(|s| json!({ "position": round(s.offset().get() as f64), "color": hex(&s.color(), s.opacity().get()) }))
                    .collect();
                Some(json!({ "type": "LINEAR_GRADIENT", "angle": round(angle), "stops": stops, "opacity": round(opacity as f64) }))
            }
            usvg::Paint::RadialGradient(g) => {
                self.notes.push("radial gradients became a solid color (Flexma has linear gradients only)".into());
                let stops = g.stops();
                let s = stops.get(stops.len() / 2)?;
                Some(json!({ "type": "SOLID", "color": hex(&s.color(), s.opacity().get()), "opacity": round(opacity as f64) }))
            }
            usvg::Paint::Pattern(_) => {
                self.notes.push("pattern fills were skipped".into());
                None
            }
        }
    }
}

fn vector_spec(name: &str, d: &str, style: &Map<String, Value>, bounds: Option<[f64; 4]>) -> Value {
    let mut v = style.clone();
    v.insert("type".into(), json!("vector"));
    v.insert("name".into(), json!(name));
    v.insert("absolute".into(), json!(true));
    // Vector paths live in the layer's own coordinates: move them to the
    // layer's top-left corner.
    match bounds {
        Some([x, y, w, h]) => {
            v.insert("x".into(), json!(round(x)));
            v.insert("y".into(), json!(round(y)));
            v.insert("width".into(), json!(round(w).max(0.01)));
            v.insert("height".into(), json!(round(h).max(0.01)));
            v.insert("path".into(), json!(translate_d(d, -x, -y)));
        }
        None => {
            v.insert("path".into(), json!(d));
        }
    }
    Value::Object(v)
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn hex(c: &usvg::Color, alpha: f32) -> String {
    if alpha >= 0.999 {
        format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.red, c.green, c.blue, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
    }
}

fn num(v: f32) -> String {
    let r = (v as f64 * 1000.0).round() / 1000.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

fn path_d(p: &usvg::tiny_skia_path::Path) -> String {
    let mut out = String::new();
    for seg in p.segments() {
        if !out.is_empty() {
            out.push(' ');
        }
        match seg {
            PathSegment::MoveTo(a) => out.push_str(&format!("M{} {}", num(a.x), num(a.y))),
            PathSegment::LineTo(a) => out.push_str(&format!("L{} {}", num(a.x), num(a.y))),
            PathSegment::QuadTo(a, b) => out.push_str(&format!("Q{} {} {} {}", num(a.x), num(a.y), num(b.x), num(b.y))),
            PathSegment::CubicTo(a, b, c) => {
                out.push_str(&format!("C{} {} {} {} {} {}", num(a.x), num(a.y), num(b.x), num(b.y), num(c.x), num(c.y)))
            }
            PathSegment::Close => out.push('Z'),
        }
    }
    out
}

/// Shift absolute path data (as written by `path_d`) by (dx, dy). Every
/// command's numbers come in x/y pairs.
fn translate_d(d: &str, dx: f64, dy: f64) -> String {
    let mut out = String::with_capacity(d.len());
    let mut is_x = true;
    for part in d.split(' ').filter(|p| !p.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        let mut digits = part;
        if let Some(c) = part.chars().next().filter(|c| c.is_ascii_alphabetic()) {
            out.push(c);
            digits = &part[1..];
            is_x = true;
        }
        if digits.is_empty() {
            continue;
        }
        let v: f64 = digits.parse().unwrap_or(0.0);
        out.push_str(&num((if is_x { v + dx } else { v + dy }) as f32));
        is_x = !is_x;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(flatten: bool) -> ImportOptions {
        ImportOptions { name: "Logo".into(), width: None, height: None, flatten }
    }

    #[test]
    fn imports_shapes_with_styles() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50" width="100" height="50">
            <rect id="bg" x="10" y="10" width="30" height="20" fill="#ff0000"/>
            <circle cx="70" cy="25" r="10" fill="none" stroke="#0000ff" stroke-width="4" stroke-linecap="round"/>
        </svg>"##;
        let out = import(svg, &opts(false)).unwrap();
        assert_eq!(out.layers, 2);
        let kids = out.spec["children"].as_array().unwrap();
        assert_eq!(kids[0]["name"], "bg");
        assert_eq!((kids[0]["x"].as_f64(), kids[0]["width"].as_f64()), (Some(10.0), Some(30.0)));
        assert!(kids[0]["path"].as_str().unwrap().starts_with("M0 0"));
        assert_eq!(kids[1]["strokes"][0]["color"], "#0000ff");
        assert_eq!(kids[1]["strokeCap"], "ROUND");
        assert_eq!(kids[1]["fills"], json!([]));
    }

    #[test]
    fn scales_and_flattens_icons() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#111111" stroke-width="2"><path d="M4 4h16"/><path d="M4 12h16"/></svg>"##;
        let out = import(svg, &ImportOptions { width: Some(48.0), ..opts(true) }).unwrap();
        assert_eq!(out.layers, 1);
        assert_eq!(out.spec["width"], 48.0);
        assert_eq!(out.spec["children"][0]["strokeWeight"], 4.0);
    }

    #[test]
    fn translates_path_data() {
        assert_eq!(translate_d("M10 20 L30 40 Z", -10.0, -20.0), "M0 0 L20 20 Z");
        assert_eq!(translate_d("C1 2 3 4 5 6", 1.0, 1.0), "C2 3 4 5 6 7");
    }
}
