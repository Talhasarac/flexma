//! Agent-friendly node specs. MCP tools accept a forgiving JSON shape
//! (`fill: "#hex"`, `radius`, `text`, `gap`, nested `children`, `width: "fill"`)
//! and this module normalizes it into canonical node properties and ops.

use crate::color::Color;
use crate::doc::Document;
use crate::fractional;
use crate::node::{Node, NodeId, NodeType};
use crate::ops::Op;
use serde_json::{json, Map, Value};

pub fn parse_type(s: &str) -> Option<NodeType> {
    let t = s.trim().to_ascii_lowercase();
    Some(match t.as_str() {
        "frame" | "artboard" | "board" | "container" | "div" | "stack" => NodeType::Frame,
        "group" => NodeType::Group,
        "rect" | "rectangle" | "box" => NodeType::Rectangle,
        "ellipse" | "circle" | "oval" => NodeType::Ellipse,
        "line" => NodeType::Line,
        "text" | "label" => NodeType::Text,
        "vector" | "path" | "svg" => NodeType::Vector,
        "polygon" | "triangle" => NodeType::Polygon,
        "star" => NodeType::Star,
        "page" => NodeType::Page,
        "component" | "main component" | "symbol" => NodeType::Component,
        "instance" => NodeType::Instance,
        "paint" | "paint layer" | "raster" | "drawing" => NodeType::Paint,
        _ => return None,
    })
}

fn paint_from(v: &Value) -> Result<Value, String> {
    match v {
        Value::String(s) => {
            let c = Color::parse(s).ok_or_else(|| format!("invalid color '{s}'"))?;
            Ok(json!({ "type": "SOLID", "color": c.to_hex() }))
        }
        Value::Object(o) if o.contains_key("type") => {
            let mut o = o.clone();
            if let Some(Value::String(t)) = o.get("type") {
                o.insert("type".into(), Value::String(t.to_ascii_uppercase()));
            }
            Ok(Value::Object(o))
        }
        Value::Object(o) if o.contains_key("color") => {
            let mut o = o.clone();
            o.insert("type".into(), "SOLID".into());
            Ok(Value::Object(o))
        }
        Value::Object(o) if o.contains_key("stops") => {
            let mut o = o.clone();
            o.insert("type".into(), "LINEAR_GRADIENT".into());
            Ok(Value::Object(o))
        }
        Value::Object(o) if o.contains_key("asset") => {
            let mut o = o.clone();
            o.insert("type".into(), "IMAGE".into());
            Ok(Value::Object(o))
        }
        Value::Null => Ok(Value::Null),
        other => Err(format!("invalid paint {other}")),
    }
}

fn paints_from(v: &Value) -> Result<Value, String> {
    match v {
        Value::Array(a) => Ok(Value::Array(
            a.iter().map(paint_from).collect::<Result<Vec<_>, _>>()?.into_iter().filter(|p| !p.is_null()).collect(),
        )),
        Value::Null => Ok(json!([])),
        Value::String(s) if s.eq_ignore_ascii_case("none") || s.is_empty() => Ok(json!([])),
        other => Ok(Value::Array(vec![paint_from(other)?])),
    }
}

fn upper(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(s.to_ascii_uppercase().replace(['-', ' '], "_")),
        o => o.clone(),
    }
}

fn weight(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            let w = match s.to_ascii_lowercase().as_str() {
                "thin" => 100,
                "extralight" | "extra-light" => 200,
                "light" => 300,
                "regular" | "normal" => 400,
                "medium" => 500,
                "semibold" | "semi-bold" => 600,
                "bold" => 700,
                "extrabold" | "extra-bold" => 800,
                "black" | "heavy" => 900,
                other => other.parse().unwrap_or(400),
            };
            json!(w)
        }
        o => o.clone(),
    }
}

fn align(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            let t = s.to_ascii_lowercase();
            Value::String(
                match t.as_str() {
                    "start" | "left" | "top" | "flex-start" | "min" => "START",
                    "center" | "middle" => "CENTER",
                    "end" | "right" | "bottom" | "flex-end" | "max" => "END",
                    "space-between" | "space_between" | "spacebetween" | "between" => "SPACE_BETWEEN",
                    _ => return upper(v),
                }
                .into(),
            )
        }
        o => o.clone(),
    }
}

/// Normalize friendly props into canonical node props. `kind` is the node type
/// the props apply to (affects e.g. `color` on text).
pub fn normalize_props(kind: NodeType, input: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for (k, v) in input {
        match k.as_str() {
            "type" | "children" | "id" | "x1" | "y1" | "x2" | "y2" => {}
            "w" => {
                out.insert("width".into(), v.clone());
            }
            "h" => {
                out.insert("height".into(), v.clone());
            }
            "width" | "height" => {
                if let Value::String(s) = v {
                    let mode = match s.to_ascii_lowercase().as_str() {
                        "fill" | "fill_container" | "stretch" | "100%" => "FILL",
                        "hug" | "hug_contents" | "auto" | "fit" => "HUG",
                        _ => return Err(format!("'{k}' must be a number, \"fill\" or \"hug\"")),
                    };
                    let key = if k == "width" { "layoutSizingHorizontal" } else { "layoutSizingVertical" };
                    out.insert(key.into(), Value::String(mode.into()));
                } else {
                    out.insert(k.clone(), v.clone());
                }
            }
            "fill" | "fills" | "background" if !(k == "background" && kind == NodeType::Page) => {
                out.insert("fills".into(), paints_from(v)?);
            }
            "color" if kind == NodeType::Text => {
                out.insert("fills".into(), paints_from(v)?);
            }
            "stroke" | "strokes" | "border" => {
                out.insert("strokes".into(), paints_from(v)?);
            }
            "strokeWidth" | "borderWidth" | "strokeWeight" => {
                out.insert("strokeWeight".into(), v.clone());
            }
            "radius" | "borderRadius" | "cornerRadius" => match v {
                Value::Array(_) => {
                    out.insert("cornerRadii".into(), v.clone());
                }
                _ => {
                    out.insert("cornerRadius".into(), v.clone());
                }
            },
            "text" | "content" | "characters" | "value" => {
                out.insert("characters".into(), v.clone());
            }
            "font" | "fontFamily" => {
                out.insert("fontFamily".into(), v.clone());
            }
            "size" | "fontSize" => {
                out.insert("fontSize".into(), v.clone());
            }
            "weight" | "fontWeight" => {
                out.insert("fontWeight".into(), weight(v));
            }
            "align" | "textAlign" if kind == NodeType::Text => {
                out.insert("textAlign".into(), upper(v));
            }
            "verticalAlign" | "textAlignVertical" => {
                out.insert("textAlignVertical".into(), align(v).as_str().map(|s| if s == "START" { "TOP" } else if s == "END" { "BOTTOM" } else { s }).map(|s| Value::String(s.into())).unwrap_or(v.clone()));
            }
            "layout" | "layoutMode" | "direction" | "flexDirection" => {
                let m = match v.as_str().map(|s| s.to_ascii_lowercase()) {
                    Some(s) if s == "row" || s == "horizontal" || s == "h" => "HORIZONTAL",
                    Some(s) if s == "column" || s == "col" || s == "vertical" || s == "v" => "VERTICAL",
                    Some(s) if s == "grid" => "GRID",
                    Some(s) if s == "none" || s.is_empty() => "NONE",
                    _ => return Err(format!("'{k}' must be HORIZONTAL, VERTICAL, GRID or NONE")),
                };
                out.insert("layoutMode".into(), m.into());
            }
            "rowGap" | "lineGap" | "counterAxisSpacing" => {
                out.insert("counterAxisSpacing".into(), v.clone());
            }
            "columns" | "gridColumns" => {
                out.insert("gridColumns".into(), v.clone());
            }
            "rows" | "gridRows" => {
                out.insert("gridRows".into(), v.clone());
            }
            "cellSize" | "gridCellSize" => {
                let cell = match v {
                    Value::Array(a) if a.len() == 2 => v.clone(),
                    Value::Object(o) => json!([o.get("width").or_else(|| o.get("w")).or_else(|| o.get("x")), o.get("height").or_else(|| o.get("h")).or_else(|| o.get("y"))]),
                    Value::Null => Value::Null,
                    _ => return Err(format!("'{k}' must be [width, height]")),
                };
                out.insert("gridCellSize".into(), cell);
            }
            "startCorner" | "gridStartCorner" => {
                let c = match v.as_str().map(|s| s.to_ascii_uppercase().replace(['-', ' '], "_")).as_deref() {
                    Some("TOP_LEFT" | "UPPER_LEFT") => "TOP_LEFT",
                    Some("TOP_RIGHT" | "UPPER_RIGHT") => "TOP_RIGHT",
                    Some("BOTTOM_LEFT" | "LOWER_LEFT") => "BOTTOM_LEFT",
                    Some("BOTTOM_RIGHT" | "LOWER_RIGHT") => "BOTTOM_RIGHT",
                    _ => return Err(format!("'{k}' must be TOP_LEFT, TOP_RIGHT, BOTTOM_LEFT or BOTTOM_RIGHT")),
                };
                out.insert("gridStartCorner".into(), c.into());
            }
            "startAxis" => {
                let vertical = match v.as_str().map(|s| s.to_ascii_lowercase()).as_deref() {
                    Some("horizontal") => false,
                    Some("vertical") => true,
                    _ => return Err(format!("'{k}' must be HORIZONTAL or VERTICAL")),
                };
                out.insert("gridVertical".into(), Value::Bool(vertical));
            }
            "gap" | "spacing" | "itemSpacing" => {
                out.insert("itemSpacing".into(), v.clone());
            }
            "justify" | "justifyContent" | "primaryAxisAlign" | "mainAxisAlign" => {
                out.insert("primaryAxisAlign".into(), align(v));
            }
            "alignItems" | "counterAxisAlign" | "crossAxisAlign" => {
                out.insert("counterAxisAlign".into(), align(v));
            }
            "align" => {
                out.insert("counterAxisAlign".into(), align(v));
            }
            "clip" | "clipsContent" | "overflowHidden" => match v.as_str().map(|s| s.to_ascii_lowercase()) {
                // "alpha": clip to the fills' alpha (an image mask, like Unity's Mask).
                Some(s) if s == "alpha" => {
                    out.insert("clipsContent".into(), Value::Bool(true));
                    out.insert("clipAlpha".into(), Value::Bool(true));
                }
                Some(s) if s == "shape" || s == "rect" => {
                    out.insert("clipsContent".into(), Value::Bool(true));
                    out.insert("clipAlpha".into(), Value::Bool(false));
                }
                Some(s) if s == "none" => {
                    out.insert("clipsContent".into(), Value::Bool(false));
                }
                Some(_) => return Err(format!("'{k}' must be true, false, \"alpha\" or \"shape\"")),
                None => {
                    out.insert("clipsContent".into(), v.clone());
                }
            },
            "autoSize" | "autoFontSize" | "fontSizeRange" => {
                let range = match v {
                    Value::Null | Value::Bool(false) => Value::Null,
                    Value::Array(a) if a.len() == 2 && a.iter().all(Value::is_number) => json!({ "min": a[0], "max": a[1] }),
                    Value::Object(o) if o.get("min").is_some_and(Value::is_number) && o.get("max").is_some_and(Value::is_number) => json!({ "min": o["min"], "max": o["max"] }),
                    _ => return Err(format!("'{k}' must be {{\"min\": n, \"max\": n}}, [min, max] or false")),
                };
                out.insert("autoFontSize".into(), range);
            }
            "meta" | "metadata" | "userData" => {
                let v = match v {
                    Value::Null => json!({}),
                    Value::Object(_) => v.clone(),
                    _ => return Err(format!("'{k}' must be a JSON object")),
                };
                if v.to_string().len() > 64 * 1024 {
                    return Err(format!("'{k}' is larger than 64 KB"));
                }
                out.insert("meta".into(), v);
            }
            "wrap" | "flexWrap" | "layoutWrap" if kind.is_frame_like() => {
                let on = match v {
                    Value::Bool(b) => *b,
                    Value::String(s) => !matches!(s.to_ascii_lowercase().as_str(), "nowrap" | "no_wrap" | "none" | "false"),
                    _ => return Err(format!("'{k}' must be true or false")),
                };
                out.insert("layoutWrap".into(), Value::Bool(on));
            }
            "wrap" | "textWrap" | "wordWrap" => {
                out.insert("textWrap".into(), v.clone());
            }
            "overflow" if kind.is_frame_like() => match v.as_str().map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("hidden" | "clip") => {
                    out.insert("clipsContent".into(), Value::Bool(true));
                }
                Some("visible") => {
                    out.insert("clipsContent".into(), Value::Bool(false));
                }
                _ => return Err(format!("'{k}' on a frame must be \"hidden\" or \"visible\"")),
            },
            "overflow" | "textOverflow" | "overflowMode" => {
                let mode = match v.as_str().map(|s| s.to_ascii_lowercase()).as_deref() {
                    Some("visible" | "overflow") => "VISIBLE",
                    Some("clip" | "hidden" | "masking") => "CLIP",
                    Some("ellipsis") => "ELLIPSIS",
                    Some("truncate") => "TRUNCATE",
                    _ => return Err(format!("'{k}' must be VISIBLE, CLIP, ELLIPSIS or TRUNCATE")),
                };
                out.insert("textOverflow".into(), Value::String(mode.into()));
            }
            "clipAlpha" | "alphaMask" => {
                if v.as_bool() == Some(true) && !input.contains_key("clip") && !input.contains_key("clipsContent") {
                    out.insert("clipsContent".into(), Value::Bool(true));
                }
                out.insert("clipAlpha".into(), v.clone());
            }
            "shadow" | "dropShadow" => {
                let mut effects = Vec::new();
                let list = match v {
                    Value::Array(a) => a.clone(),
                    Value::Null | Value::Bool(false) => vec![],
                    o => vec![o.clone()],
                };
                for s in list {
                    let mut o = s.as_object().cloned().unwrap_or_default();
                    o.insert("type".into(), "DROP_SHADOW".into());
                    o.entry("color").or_insert(json!("#00000040"));
                    o.entry("y").or_insert(json!(4));
                    o.entry("blur").or_insert(json!(12));
                    effects.push(Value::Object(o));
                }
                out.insert("effects".into(), Value::Array(effects));
            }
            "path" | "d" | "pathData" => {
                out.insert("pathData".into(), v.clone());
            }
            "points" | "pointCount" => {
                out.insert("pointCount".into(), v.clone());
            }
            "hidden" => {
                out.insert("visible".into(), Value::Bool(!v.as_bool().unwrap_or(false)));
            }
            "absolute" | "layoutAbsolute" => {
                out.insert("layoutAbsolute".into(), v.clone());
            }
            "layoutSizingHorizontal" | "layoutSizingVertical" | "textAutoResize" | "blendMode" | "strokeAlign"
            | "textDecoration" => {
                out.insert(k.clone(), upper(v));
            }
            "fillRule" | "strokeJoin" => {
                out.insert(k.clone(), upper(v));
            }
            "strokeCap" | "strokeLinecap" | "linecap" => {
                let cap = upper(v);
                out.insert("strokeCap".into(), if cap == json!("BUTT") { json!("NONE") } else { cap });
            }
            "strokeLinejoin" | "linejoin" => {
                out.insert("strokeJoin".into(), upper(v));
            }
            "primaryAxisSizing" | "counterAxisSizing" => {
                return Err(format!("use layoutSizingHorizontal / layoutSizingVertical instead of '{k}'"));
            }
            _ => {
                if crate::node::Node::PROPS.contains(&k.as_str()) {
                    out.insert(k.clone(), v.clone());
                } else {
                    return Err(format!(
                        "unknown property '{k}'. Known: {} (plus shorthands fill, stroke, radius, text, gap, layout, justify, alignItems, shadow, w, h)",
                        crate::node::Node::PROPS.join(", ")
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// Build ops creating `specs` (each may contain nested `children`) under `parent`,
/// appended after existing children (or at `index` position if given).
/// Returns the ops and the ids of the top-level created nodes.
pub fn build_create_ops(
    doc: &Document,
    parent: &str,
    specs: &[Value],
    insert_at: Option<usize>,
    new_id: &mut dyn FnMut() -> NodeId,
) -> Result<(Vec<Op>, Vec<NodeId>), String> {
    let parent_node = doc.get(parent).ok_or_else(|| format!("parent '{parent}' does not exist"))?;
    if !parent_node.kind.is_container() {
        return Err(format!("parent '{parent}' is a {} and cannot have children", parent_node.kind.as_str()));
    }
    let keys = {
        let sibs = doc.children(parent);
        let pos = insert_at.unwrap_or(sibs.len()).min(sibs.len());
        let before = if pos == 0 { None } else { doc.get(&sibs[pos - 1]).map(|n| n.index.clone()) };
        let after = sibs.get(pos).and_then(|id| doc.get(id)).map(|n| n.index.clone());
        fractional::keys_between(before.as_deref(), after.as_deref(), specs.len())
    };
    let mut ops = Vec::new();
    let mut top = Vec::new();
    let mut ctx = BuildCtx { doc, scratch: None };
    for (spec, key) in specs.iter().zip(keys) {
        let id = build_one(&mut ctx, spec, parent, parent_node.kind, key, &mut ops, new_id)?;
        top.push(id);
    }
    Ok((ops, top))
}

/// The document specs are built against. Components created earlier in the
/// same batch are found in a scratch copy with the ops so far applied.
struct BuildCtx<'a> {
    doc: &'a Document,
    /// The copy, and how many ops it has applied.
    scratch: Option<(Document, usize)>,
}

impl BuildCtx<'_> {
    /// The document to resolve component `key` in.
    fn for_component(&mut self, key: &str, ops: &[Op]) -> &Document {
        if self.scratch.is_none() && find_component(self.doc, key).is_some() {
            return self.doc;
        }
        let (scratch, applied) = self.scratch.get_or_insert_with(|| (self.doc.clone(), 0));
        if *applied < ops.len() {
            let (eff, _) = scratch.apply_all(&ops[*applied..]);
            *applied = ops.len();
            // Expand instances, so overrides can reach layers inside nested ones.
            crate::components::sync_instances(scratch, &eff.changed, &eff.removed);
        }
        scratch
    }
}

/// Find a main component by id, `key` or name.
pub fn find_component(doc: &Document, key: &str) -> Option<NodeId> {
    if doc.get(key).map(|n| n.kind == NodeType::Component).unwrap_or(false) {
        return Some(key.to_string());
    }
    let (mut by_key, mut by_name) = (None, None);
    doc.walk(crate::node::ROOT_ID, &mut |n| {
        if n.kind == NodeType::Component {
            if by_key.is_none() && n.key.as_deref() == Some(key) {
                by_key = Some(n.id.clone());
            }
            if by_name.is_none() && n.name == key {
                by_name = Some(n.id.clone());
            }
        }
    });
    by_key.or(by_name)
}

/// Build an INSTANCE node from a friendly spec: `{type:"instance", component:"Button",
/// text:"Buy", overrides:{"Label":{text:"Buy", color:"#fff"}}}`.
fn build_instance(doc: &Document, obj: &Map<String, Value>, node: &mut Node) -> Result<(), String> {
    let ckey = obj
        .get("component")
        .or_else(|| obj.get("componentId"))
        .or_else(|| obj.get("of"))
        .and_then(Value::as_str)
        .ok_or("instances need \"component\": the main component's id or name")?;
    let cid = find_component(doc, ckey).ok_or_else(|| format!("no component '{ckey}' in this file (a component made in the same batch must come before its instances)"))?;
    let comp = doc.get(&cid).unwrap();
    node.component_id = Some(cid.clone());
    // Start from the component's look, not frame defaults (a white fill and
    // clipping), which would read as overrides.
    for k in crate::components::INHERITED {
        if let Some(v) = comp.get_prop(k) {
            let _ = node.set_prop(k, &v);
        }
    }
    node.width = comp.width;
    node.height = comp.height;
    node.layout_sizing_horizontal = comp.layout_sizing_horizontal;
    node.layout_sizing_vertical = comp.layout_sizing_vertical;
    node.name = comp.name.clone();
    let mut overrides = Map::new();
    // `text` shorthand: the component's first text layer.
    if let Some(t) = obj.get("text") {
        let mut first_text = None;
        doc.walk(&cid, &mut |n| {
            if first_text.is_none() && n.kind == NodeType::Text {
                first_text = Some(n.id.clone());
            }
        });
        let tid = first_text.ok_or("this component has no text layer for \"text\"")?;
        let mut o = Map::new();
        o.insert("characters".into(), t.clone());
        o.insert("needsMeasure".into(), Value::Bool(true));
        overrides.insert(tid, Value::Object(o));
    }
    if let Some(Value::Object(ov)) = obj.get("overrides") {
        for (k, v) in ov {
            let key = crate::components::resolve_override_key(doc, &cid, k)
                .ok_or_else(|| format!("override target '{k}' is not a layer in component '{}'", comp.name))?;
            let target_kind = if key.is_empty() { NodeType::Instance } else { doc.get(&key).map(|n| n.kind).unwrap_or(NodeType::Rectangle) };
            let props = v.as_object().ok_or_else(|| format!("override for '{k}' must be an object"))?;
            let mut norm = normalize_props(target_kind, props)?;
            if target_kind == NodeType::Text && norm.contains_key("characters") {
                norm.insert("needsMeasure".into(), Value::Bool(true));
            }
            let entry = overrides.entry(key).or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(e) = entry {
                e.extend(norm);
            }
        }
    }
    // Estimate the size of overridden text so hugging layouts are right before
    // a browser measures it exactly.
    for (key, val) in overrides.iter_mut() {
        let Some(src) = doc.get(key).filter(|n| n.kind == NodeType::Text) else { continue };
        let Some(obj) = val.as_object_mut() else { continue };
        if !obj.contains_key("characters") && !obj.contains_key("fontSize") {
            continue;
        }
        let mut probe = src.clone();
        for (k, v) in obj.iter() {
            let _ = probe.set_prop(k, v);
        }
        let (_, w, h) = crate::text::estimate_layout(&probe);
        if probe.text_auto_resize == crate::paint::TextAutoResize::WidthAndHeight && !obj.contains_key("width") {
            obj.insert("width".into(), serde_json::json!(w));
        }
        if probe.text_auto_resize != crate::paint::TextAutoResize::None && !obj.contains_key("height") {
            obj.insert("height".into(), serde_json::json!(h));
        }
    }
    node.overrides = overrides;
    Ok(())
}

/// Image types agents may use; see `expand_image`.
pub fn is_image_type(t: &str) -> bool {
    matches!(t.trim().to_ascii_lowercase().as_str(), "image" | "img" | "picture" | "photo")
}

/// `{type:"image", asset, fit?, tint?, slice?}` becomes a rectangle filled with
/// the image (images are fills, like in Figma). `fit` is FILL (default, crops
/// to cover), FIT (letterbox) or STRETCH; CSS-style `cover`/`contain` work too.
pub fn expand_image(spec: &Value) -> Result<Option<Value>, String> {
    let Some(obj) = spec.as_object() else { return Ok(None) };
    if !obj.get("type").and_then(Value::as_str).map(is_image_type).unwrap_or(false) {
        return Ok(None);
    }
    let asset = ["asset", "src", "image"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))
        .ok_or("an image node needs \"asset\": an id from upload_image or list_images")?;
    let fit = match obj.get("fit").and_then(Value::as_str).unwrap_or("FILL").to_ascii_uppercase().as_str() {
        "FILL" | "COVER" => "FILL",
        "FIT" | "CONTAIN" => "FIT",
        "STRETCH" => "STRETCH",
        "TILE" | "TILED" | "REPEAT" => "TILE",
        other => return Err(format!("unknown image fit '{other}' (use FILL, FIT, STRETCH or TILE)")),
    };
    let mut paint = json!({ "type": "IMAGE", "asset": asset, "fit": fit });
    for k in ["tint", "slice", "filled", "crop", "tileScale"] {
        if let Some(v) = obj.get(k) {
            paint[k] = v.clone();
        }
    }
    let mut out = obj.clone();
    for k in ["asset", "src", "image", "fit", "tint", "slice", "filled", "crop", "tileScale", "fill", "fills", "background"] {
        out.remove(k);
    }
    out.insert("type".into(), json!("RECTANGLE"));
    out.entry("name").or_insert(json!("Image"));
    out.insert("fills".into(), json!([paint]));
    Ok(Some(Value::Object(out)))
}

fn build_one(
    ctx: &mut BuildCtx,
    spec: &Value,
    parent: &str,
    parent_kind: NodeType,
    index: String,
    ops: &mut Vec<Op>,
    new_id: &mut dyn FnMut() -> NodeId,
) -> Result<NodeId, String> {
    let mut expanded = expand_image(spec)?;
    // Style refs by name: {fillStyle:"Brand/Primary", textStyle:"Heading/H1"}.
    if let Some(o) = spec.as_object().filter(|o| o.contains_key("fillStyle") || o.contains_key("strokeStyle") || o.contains_key("textStyle")) {
        let mut o = expanded.as_ref().and_then(Value::as_object).cloned().unwrap_or_else(|| o.clone());
        crate::styles::resolve_refs(ctx.doc, &mut o)?;
        expanded = Some(Value::Object(o));
    }
    let spec = expanded.as_ref().unwrap_or(spec);
    let obj = spec.as_object().ok_or("each node spec must be an object")?;
    let ty = obj.get("type").and_then(|v| v.as_str()).ok_or("node spec needs a \"type\" (FRAME, RECTANGLE, ELLIPSE, TEXT, LINE, VECTOR, POLYGON, STAR, GROUP, COMPONENT, INSTANCE, IMAGE)")?;
    let kind = parse_type(ty).ok_or_else(|| format!("unknown node type '{ty}'"))?;
    if kind == NodeType::Page && parent_kind != NodeType::Document {
        return Err("pages can only be created at the document root".into());
    }
    if parent_kind == NodeType::Instance {
        return Err("instances can't contain other layers; add them to the main component".into());
    }
    let id = new_id();
    let mut node = Node::new(id.clone(), kind);
    node.parent = Some(parent.to_string());
    node.index = index;
    // Sensible defaults (like Figma's).
    match kind {
        NodeType::Rectangle | NodeType::Ellipse | NodeType::Polygon | NodeType::Star | NodeType::Vector => {
            node.width = 100.0;
            node.height = 100.0;
            node.fills = vec![crate::paint::Paint::solid(Color::rgba(0.85, 0.85, 0.85, 1.0))];
        }
        NodeType::Frame | NodeType::Component => {
            node.width = 100.0;
            node.height = 100.0;
        }
        NodeType::Instance => {
            let ckey = obj.get("component").or_else(|| obj.get("componentId")).or_else(|| obj.get("of")).and_then(Value::as_str).unwrap_or_default();
            build_instance(ctx.for_component(ckey, ops), obj, &mut node)?;
        }
        NodeType::Line => {
            node.width = 100.0;
            node.strokes = vec![crate::paint::Paint::solid(Color::BLACK)];
        }
        NodeType::Text => {
            node.fills = vec![crate::paint::Paint::solid(Color::BLACK)];
        }
        _ => {}
    }
    let props = if kind == NodeType::Instance {
        let rest: Map<String, Value> = obj
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "component" | "componentId" | "of" | "overrides" | "text"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        normalize_props(kind, &rest)?
    } else {
        normalize_props(kind, obj)?
    };
    // A frame with auto layout and no explicit size hugs its content by default.
    let has_layout = props.get("layoutMode").and_then(|v| v.as_str()).map(|s| s != "NONE").unwrap_or(false);
    for (k, v) in &props {
        node.set_prop(k, v)?;
    }
    if kind == NodeType::Instance {
        // Inherited props given at creation are instance overrides (else the
        // main component's values would win on the next sync).
        let mut own = Map::new();
        for (k, v) in &props {
            if crate::components::INHERITED.contains(&k.as_str()) {
                own.insert(k.clone(), v.clone());
            }
        }
        if !own.is_empty() {
            let entry = node.overrides.entry(String::new()).or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(e) = entry {
                e.extend(own);
            }
        }
    }
    if matches!(kind, NodeType::Frame | NodeType::Component) && has_layout {
        if !obj.contains_key("width") && !obj.contains_key("w") {
            node.layout_sizing_horizontal = crate::paint::SizingMode::Hug;
        }
        if !obj.contains_key("height") && !obj.contains_key("h") {
            node.layout_sizing_vertical = crate::paint::SizingMode::Hug;
        }
    }
    if kind == NodeType::Line {
        if let (Some(x1), Some(y1), Some(x2), Some(y2)) = (
            obj.get("x1").and_then(Value::as_f64),
            obj.get("y1").and_then(Value::as_f64),
            obj.get("x2").and_then(Value::as_f64),
            obj.get("y2").and_then(Value::as_f64),
        ) {
            let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
            let ang = (y2 - y1).atan2(x2 - x1).to_degrees();
            let cx = (x1 + x2) / 2.0;
            let cy = (y1 + y2) / 2.0;
            node.width = len;
            node.height = 0.0;
            node.rotation = ang;
            node.x = cx - len / 2.0;
            node.y = cy;
        }
    }
    if kind == NodeType::Vector && !node.path_data.is_empty() && !obj.contains_key("width") && !obj.contains_key("w") {
        let b = crate::path::bounds(&crate::path::parse(&node.path_data));
        if !b.is_empty() {
            node.width = b.right().max(1.0);
            node.height = b.bottom().max(1.0);
        }
    }
    if kind == NodeType::Text {
        if !obj.contains_key("width") && !obj.contains_key("w") {
            // keep auto width
        } else if !obj.contains_key("textAutoResize") {
            node.text_auto_resize = crate::paint::TextAutoResize::Height;
        }
        if node.layout_sizing_horizontal == crate::paint::SizingMode::Fill && !obj.contains_key("textAutoResize") {
            node.text_auto_resize = crate::paint::TextAutoResize::Height;
        }
        // An overflow mode or auto size with a given height means a fixed box.
        let given_h = obj.contains_key("height") || obj.contains_key("h");
        if (node.text_overflow != crate::paint::TextOverflow::Visible || node.auto_font_size.is_some()) && given_h && !obj.contains_key("textAutoResize") {
            node.text_auto_resize = crate::paint::TextAutoResize::None;
        }
        let (_, w, h) = crate::text::estimate_layout(&node);
        if node.text_auto_resize == crate::paint::TextAutoResize::WidthAndHeight {
            node.width = w;
        }
        if node.text_auto_resize != crate::paint::TextAutoResize::None {
            node.height = h;
        }
        node.needs_measure = true;
    }
    if node.name.is_empty() {
        node.name = if kind == NodeType::Text {
            node.characters.chars().take(40).collect()
        } else {
            kind.default_name().to_string()
        };
    }
    let children = obj.get("children").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if !children.is_empty() && !kind.is_container() {
        return Err(format!("{} nodes cannot have children; use a FRAME or GROUP", kind.as_str()));
    }
    ops.push(Op::Create { node });
    let keys = fractional::keys_between(None, None, children.len());
    for (c, key) in children.iter().zip(keys) {
        build_one(ctx, c, &id, kind, key, ops, new_id)?;
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nested_card() {
        let mut doc = Document::new_empty();
        let mut n = 0;
        let mut gen = || {
            n += 1;
            format!("t:{n}")
        };
        let spec = json!([{
            "type": "frame", "name": "Card", "layout": "vertical", "gap": 8, "padding": 16,
            "fill": "#ffffff", "radius": 12, "shadow": { "blur": 20 },
            "children": [
                { "type": "text", "text": "Title", "size": 20, "weight": "bold" },
                { "type": "rect", "w": 200, "h": 100, "fill": "#3366ff" },
                { "type": "text", "text": "Body copy", "width": "fill" }
            ]
        }]);
        let (ops, top) = build_create_ops(&doc, "0:1", spec.as_array().unwrap(), None, &mut gen).unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(ops.len(), 4);
        let (_, errs) = doc.apply_all(&ops);
        assert!(errs.is_empty(), "{errs:?}");
        crate::layout::relayout_all(&mut doc);
        let card = doc.get(&top[0]).unwrap();
        assert!(card.width >= 232.0, "{}", card.width);
        let body = doc.children(&top[0])[2].clone();
        assert_eq!(doc.get(&body).unwrap().width, card.width - 32.0);
    }

    #[test]
    fn image_shorthand_becomes_an_image_fill() {
        let mut doc = Document::new_empty();
        let mut gen = || "img:1".to_string();
        let spec = json!([{ "type": "image", "asset": "abc123", "width": 320, "height": 200, "fit": "contain", "radius": 12 }]);
        let (ops, top) = build_create_ops(&doc, "0:1", spec.as_array().unwrap(), None, &mut gen).unwrap();
        let (_, errs) = doc.apply_all(&ops);
        assert!(errs.is_empty(), "{errs:?}");
        let n = doc.get(&top[0]).unwrap();
        assert_eq!((n.kind, n.name.as_str(), n.width, n.corner_radius), (NodeType::Rectangle, "Image", 320.0, 12.0));
        match &n.fills[..] {
            [crate::paint::Paint::Image { asset, fit, .. }] => {
                assert_eq!(asset, "abc123");
                assert_eq!(format!("{fit:?}").to_uppercase(), "FIT");
            }
            other => panic!("expected one image fill, got {other:?}"),
        }
        assert!(expand_image(&json!({ "type": "image" })).is_err(), "asset is required");
    }

    #[test]
    fn clip_alpha_shorthands() {
        let norm = |v: Value| normalize_props(NodeType::Frame, v.as_object().unwrap()).unwrap();
        let m = norm(json!({ "clip": "alpha", "hideFill": true }));
        assert_eq!((m["clipsContent"].clone(), m["clipAlpha"].clone(), m["hideFill"].clone()), (json!(true), json!(true), json!(true)));
        let m = norm(json!({ "clipAlpha": true }));
        assert_eq!(m["clipsContent"], json!(true));
        let m = norm(json!({ "clip": "shape" }));
        assert_eq!((m["clipsContent"].clone(), m["clipAlpha"].clone()), (json!(true), json!(false)));
        assert!(normalize_props(NodeType::Frame, json!({ "clip": "sprite" }).as_object().unwrap()).is_err());
    }

    #[test]
    fn text_wrap_and_overflow() {
        let mut doc = Document::new_empty();
        let mut gen = || "t:1".to_string();
        let spec = json!([{ "type": "text", "text": "A long item name", "width": 60, "height": 20, "wrap": false, "overflow": "ellipsis" }]);
        let (ops, top) = build_create_ops(&doc, "0:1", spec.as_array().unwrap(), None, &mut gen).unwrap();
        let (_, errs) = doc.apply_all(&ops);
        assert!(errs.is_empty(), "{errs:?}");
        let n = doc.get(&top[0]).unwrap();
        assert_eq!((n.text_wrap, n.text_overflow, n.text_auto_resize, n.height), (false, crate::paint::TextOverflow::Ellipsis, crate::paint::TextAutoResize::None, 20.0));
        let norm = |kind, v: Value| normalize_props(kind, v.as_object().unwrap());
        assert_eq!(norm(NodeType::Text, json!({ "overflow": "masking" })).unwrap()["textOverflow"], json!("CLIP"));
        assert_eq!(norm(NodeType::Frame, json!({ "overflow": "hidden" })).unwrap()["clipsContent"], json!(true));
        assert!(norm(NodeType::Text, json!({ "overflow": "scroll" })).is_err());
        assert_eq!(norm(NodeType::Text, json!({ "autoSize": [10, 24] })).unwrap()["autoFontSize"], json!({ "min": 10, "max": 24 }));
        assert!(norm(NodeType::Text, json!({ "autoSize": true })).is_err());
    }

    #[test]
    fn meta_is_a_bounded_object() {
        let norm = |v: Value| normalize_props(NodeType::Frame, v.as_object().unwrap());
        let m = norm(json!({ "meta": { "unity": { "component": "Button" }, "onClick": "OpenShop" } })).unwrap();
        assert_eq!(m["meta"]["onClick"], json!("OpenShop"));
        assert_eq!(norm(json!({ "meta": null })).unwrap()["meta"], json!({}));
        assert!(norm(json!({ "meta": "x" })).is_err());
        assert!(norm(json!({ "meta": { "big": "x".repeat(70_000) } })).is_err());
    }

    #[test]
    fn components_can_be_used_in_the_batch_that_makes_them() {
        let mut doc = Document::new_empty();
        let mut n = 0;
        let mut gen = || {
            n += 1;
            format!("b:{n}")
        };
        let spec = json!([
            { "type": "frame", "name": "Library", "children": [
                { "type": "component", "name": "Chip", "layout": "horizontal", "padding": 8, "children": [
                    { "type": "text", "name": "Label", "text": "Chip" } ] } ] },
            { "type": "frame", "name": "Screen", "layout": "vertical", "children": [
                { "type": "instance", "component": "Chip", "text": "One" },
                { "type": "instance", "component": "Chip", "overrides": { "Label": { "text": "Two" } } } ] }
        ]);
        let (ops, top) = build_create_ops(&doc, "0:1", spec.as_array().unwrap(), None, &mut gen).unwrap();
        let (_, errs) = doc.apply_all(&ops);
        assert!(errs.is_empty(), "{errs:?}");
        crate::components::sync_all(&mut doc);
        // Instance layers are derived: `ii~source` children.
        let screen = doc.children(&top[1]).to_vec();
        let texts: Vec<String> = screen
            .iter()
            .filter_map(|i| doc.children(i).iter().filter_map(|c| doc.get(c)).find(|c| c.kind == NodeType::Text).map(|c| c.characters.clone()))
            .collect();
        assert_eq!(texts, vec!["One", "Two"]);
        // Used before it's made: a clear error.
        let early = json!([{ "type": "instance", "component": "Badge" }, { "type": "component", "name": "Badge" }]);
        let err = build_create_ops(&doc, "0:1", early.as_array().unwrap(), None, &mut || "x:1".into()).unwrap_err();
        assert!(err.contains("must come before"), "{err}");
    }

    #[test]
    fn rejects_unknown_props() {
        let r = normalize_props(NodeType::Rectangle, json!({ "colour": "#fff" }).as_object().unwrap());
        assert!(r.is_err());
    }
}
