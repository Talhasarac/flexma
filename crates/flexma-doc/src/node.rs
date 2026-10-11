//! The node type. Every property is addressable by its JSON name so that
//! multiplayer updates can be expressed as `(node id, property, value)` triples
//! with last-writer-wins semantics per property (the Figma model).

use crate::color::Color;
use crate::paint::*;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use std::sync::OnceLock;

pub type NodeId = String;

/// The id of the document root. Pages are its children.
pub const ROOT_ID: &str = "0:0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NodeType {
    Document,
    Page,
    Frame,
    Group,
    #[default]
    Rectangle,
    Ellipse,
    Line,
    Text,
    Vector,
    Polygon,
    Star,
    /// A main component: a frame whose content is reused by instances.
    Component,
    /// An instance of a component. Its children are derived (never stored).
    Instance,
    /// A shared color or text style (lives at the document root, never drawn).
    Style,
}

/// Derived nodes (the expanded children of instances) have ids containing `~`:
/// `instanceId~sourceId`, nested as `I~N~S`. Regular ids never contain `~`.
pub fn is_derived_id(id: &str) -> bool {
    id.contains('~')
}

impl NodeType {
    pub fn is_container(self) -> bool {
        matches!(
            self,
            NodeType::Document | NodeType::Page | NodeType::Frame | NodeType::Group | NodeType::Component | NodeType::Instance
        )
    }

    /// Frames, components and instances share box styling, clipping and auto layout.
    pub fn is_frame_like(self) -> bool {
        matches!(self, NodeType::Frame | NodeType::Component | NodeType::Instance)
    }

    /// Nodes that can be placed on a canvas (everything but the document, pages and styles).
    pub fn is_scene(self) -> bool {
        !matches!(self, NodeType::Document | NodeType::Page | NodeType::Style)
    }

    pub fn parse(s: &str) -> Option<NodeType> {
        serde_json::from_value(serde_json::Value::String(s.to_ascii_uppercase())).ok()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            NodeType::Document => "DOCUMENT",
            NodeType::Page => "PAGE",
            NodeType::Frame => "FRAME",
            NodeType::Group => "GROUP",
            NodeType::Rectangle => "RECTANGLE",
            NodeType::Ellipse => "ELLIPSE",
            NodeType::Line => "LINE",
            NodeType::Text => "TEXT",
            NodeType::Vector => "VECTOR",
            NodeType::Polygon => "POLYGON",
            NodeType::Star => "STAR",
            NodeType::Component => "COMPONENT",
            NodeType::Instance => "INSTANCE",
            NodeType::Style => "STYLE",
        }
    }

    pub fn default_name(self) -> &'static str {
        match self {
            NodeType::Document => "Document",
            NodeType::Page => "Page",
            NodeType::Frame => "Frame",
            NodeType::Group => "Group",
            NodeType::Rectangle => "Rectangle",
            NodeType::Ellipse => "Ellipse",
            NodeType::Line => "Line",
            NodeType::Text => "Text",
            NodeType::Vector => "Vector",
            NodeType::Polygon => "Polygon",
            NodeType::Star => "Star",
            NodeType::Component => "Component",
            NodeType::Instance => "Instance",
            NodeType::Style => "Style",
        }
    }
}

macro_rules! define_node {
    ($( $(#[doc = $doc:literal])* $field:ident : $ty:ty = $default:expr => $json:literal ),* $(,)?) => {
        /// A node in the document tree. Positions are relative to the parent node.
        #[derive(Clone, Debug, PartialEq, Deserialize)]
        #[serde(default)]
        pub struct Node {
            pub id: NodeId,
            #[serde(rename = "type")]
            pub kind: NodeType,
            $( $(#[doc = $doc])* #[serde(rename = $json)] pub $field: $ty, )*
        }

        impl Default for Node {
            fn default() -> Self {
                Node { id: String::new(), kind: NodeType::Rectangle, $( $field: $default, )* }
            }
        }

        impl Node {
            /// All settable property names (JSON names).
            pub const PROPS: &'static [&'static str] = &[$($json),*];

            /// Set a property from JSON. Returns an error for unknown keys or bad values.
            pub fn set_prop(&mut self, key: &str, value: &serde_json::Value) -> Result<(), String> {
                match key {
                    $( $json => {
                        self.$field = <$ty as Deserialize>::deserialize(value)
                            .map_err(|e| format!("invalid value for '{}': {}", key, e))?;
                        Ok(())
                    } )*
                    _ => Err(format!("unknown property '{}'", key)),
                }
            }

            /// Read a property as JSON.
            pub fn get_prop(&self, key: &str) -> Option<serde_json::Value> {
                match key {
                    $( $json => serde_json::to_value(&self.$field).ok(), )*
                    _ => None,
                }
            }

            /// Is the property currently at its default value?
            pub fn is_default_prop(&self, key: &str) -> bool {
                let d = default_node();
                match key {
                    $( $json => self.$field == d.$field, )*
                    _ => true,
                }
            }
        }

        impl Serialize for Node {
            /// Compact: only non-default properties are written.
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let d = default_node();
                let mut m = s.serialize_map(None)?;
                m.serialize_entry("id", &self.id)?;
                m.serialize_entry("type", &self.kind)?;
                $( if self.$field != d.$field { m.serialize_entry($json, &self.$field)?; } )*
                m.end()
            }
        }
    };
}

define_node! {
    name: String = String::new() => "name",
    /// Parent node id. `None` only for the document root.
    parent: Option<NodeId> = None => "parent",
    /// Fractional index ordering this node among its siblings (lower = further back).
    index: String = String::from("V") => "index",

    x: f64 = 0.0 => "x",
    y: f64 = 0.0 => "y",
    width: f64 = 0.0 => "width",
    height: f64 = 0.0 => "height",
    /// Degrees, clockwise, around the node's center.
    rotation: f64 = 0.0 => "rotation",
    flip_x: bool = false => "flipX",
    flip_y: bool = false => "flipY",

    visible: bool = true => "visible",
    locked: bool = false => "locked",
    opacity: f64 = 1.0 => "opacity",
    blend_mode: BlendMode = BlendMode::Normal => "blendMode",

    fills: Vec<Paint> = Vec::new() => "fills",
    strokes: Vec<Paint> = Vec::new() => "strokes",
    stroke_weight: f64 = 1.0 => "strokeWeight",
    stroke_align: StrokeAlign = StrokeAlign::Center => "strokeAlign",
    dash_pattern: Vec<f64> = Vec::new() => "dashPattern",
    fill_rule: FillRule = FillRule::Nonzero => "fillRule",
    /// None: round for lines, flat for everything else.
    stroke_cap: Option<StrokeCap> = None => "strokeCap",
    stroke_join: StrokeJoin = StrokeJoin::Miter => "strokeJoin",
    corner_radius: f64 = 0.0 => "cornerRadius",
    /// Optional per-corner radii `[topLeft, topRight, bottomRight, bottomLeft]`; overrides `cornerRadius`.
    corner_radii: Option<[f64; 4]> = None => "cornerRadii",
    effects: Vec<Effect> = Vec::new() => "effects",
    clips_content: bool = false => "clipsContent",
    /// With `clipsContent`: clip children to the alpha of the fills instead of the
    /// shape, so an image's transparent pixels hide them (Unity's Mask).
    clip_alpha: bool = false => "clipAlpha",
    /// Don't draw the fills. They still shape an alpha clip (Unity's Mask with
    /// Show Mask Graphic off).
    hide_fill: bool = false => "hideFill",

    characters: String = String::new() => "characters",
    font_family: String = String::from("Inter") => "fontFamily",
    font_size: f64 = 16.0 => "fontSize",
    font_weight: u16 = 400 => "fontWeight",
    italic: bool = false => "italic",
    text_align: TextAlign = TextAlign::Left => "textAlign",
    text_align_vertical: TextAlignVertical = TextAlignVertical::Top => "textAlignVertical",
    /// Line height in pixels; `None` = automatic (1.2 × font size).
    line_height: Option<f64> = None => "lineHeight",
    letter_spacing: f64 = 0.0 => "letterSpacing",
    text_auto_resize: TextAutoResize = TextAutoResize::WidthAndHeight => "textAutoResize",
    text_decoration: Option<String> = None => "textDecoration",
    /// Set when the text box size is an estimate (e.g. created via MCP); the first
    /// editor that renders it measures it properly and clears the flag.
    needs_measure: bool = false => "needsMeasure",

    /// SVG path data in local coordinates (VECTOR nodes).
    path_data: String = String::new() => "pathData",
    /// Polygon / star point count.
    point_count: u32 = 5 => "pointCount",
    /// Star inner radius ratio 0..1.
    inner_radius: f64 = 0.382 => "innerRadius",

    layout_mode: LayoutMode = LayoutMode::None => "layoutMode",
    item_spacing: f64 = 0.0 => "itemSpacing",
    padding: Padding = Padding::default() => "padding",
    primary_axis_align: LayoutAlign = LayoutAlign::Start => "primaryAxisAlign",
    counter_axis_align: LayoutAlign = LayoutAlign::Start => "counterAxisAlign",
    layout_wrap: bool = false => "layoutWrap",
    /// Horizontal sizing: FIXED, HUG (auto-layout frames) or FILL (children of auto-layout frames).
    layout_sizing_horizontal: SizingMode = SizingMode::Fixed => "layoutSizingHorizontal",
    layout_sizing_vertical: SizingMode = SizingMode::Fixed => "layoutSizingVertical",
    /// Children with absolute positioning are ignored by their parent's auto layout.
    layout_absolute: bool = false => "layoutAbsolute",
    /// How the node follows when its parent frame (or instance) is resized,
    /// like Unity's anchors. Children that auto layout places ignore it.
    constraints: Constraints = Constraints::default() => "constraints",

    /// INSTANCE: the main component it shows.
    component_id: Option<NodeId> = None => "componentId",
    /// INSTANCE: per-node overrides, keyed by the source node id relative to the
    /// instance (`""` = the instance itself, `N~S` = inside nested instance N).
    overrides: serde_json::Map<String, serde_json::Value> = serde_json::Map::new() => "overrides",
    /// Free-form notes, handy for agents to leave intent or component docs.
    description: String = String::new() => "description",
    /// For STYLE nodes: FILL (uses `fills`) or TEXT (uses the font props).
    style_type: StyleType = StyleType::Fill => "styleType",
    /// Shared styles this node uses; their values are applied on every replica.
    fill_style_id: Option<NodeId> = None => "fillStyleId",
    stroke_style_id: Option<NodeId> = None => "strokeStyleId",
    text_style_id: Option<NodeId> = None => "textStyleId",
    /// Canvas background color (PAGE nodes).
    background: Option<Color> = None => "background",
}

static DEFAULT_NODE: OnceLock<Node> = OnceLock::new();

pub fn default_node() -> &'static Node {
    DEFAULT_NODE.get_or_init(Node::default)
}

impl Node {
    pub fn new(id: impl Into<NodeId>, kind: NodeType) -> Node {
        let mut n = Node { id: id.into(), kind, name: kind.default_name().to_string(), ..Node::default() };
        if kind.is_frame_like() {
            n.clips_content = true;
            n.fills = vec![Paint::solid(Color::WHITE)];
        }
        n
    }

    pub fn effective_line_height(&self) -> f64 {
        self.line_height.unwrap_or(self.font_size * 1.2)
    }

    pub fn radii(&self) -> [f64; 4] {
        let r = self.corner_radii.unwrap_or([self.corner_radius; 4]);
        let max = (self.width.abs().min(self.height.abs())) / 2.0;
        [r[0].min(max).max(0.0), r[1].min(max).max(0.0), r[2].min(max).max(0.0), r[3].min(max).max(0.0)]
    }

    pub fn visible_fills(&self) -> impl Iterator<Item = &Paint> {
        self.fills.iter().filter(|p| p.is_visible())
    }

    pub fn visible_strokes(&self) -> impl Iterator<Item = &Paint> {
        self.strokes.iter().filter(|p| p.is_visible())
    }

    pub fn has_visible_stroke(&self) -> bool {
        self.stroke_weight > 0.0 && self.strokes.iter().any(|p| p.is_visible())
    }

    /// JSON object of all properties that differ from defaults (same as serialization).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_serialization() {
        let mut n = Node::new("1:1", NodeType::Rectangle);
        n.width = 10.0;
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["type"], "RECTANGLE");
        assert_eq!(v["width"], 10.0);
        assert!(v.get("height").is_none());
        let back: Node = serde_json::from_value(v).unwrap();
        assert_eq!(back, n);
    }

    #[test]
    fn set_prop() {
        let mut n = Node::new("1:1", NodeType::Rectangle);
        n.set_prop("fills", &serde_json::json!([{ "type": "SOLID", "color": "#ff0000" }])).unwrap();
        assert_eq!(n.fills.len(), 1);
        assert!(n.set_prop("nope", &serde_json::json!(1)).is_err());
        assert!(n.set_prop("x", &serde_json::json!("abc")).is_err());
        n.set_prop("padding", &serde_json::json!(8)).unwrap();
        assert_eq!(n.padding.left, 8.0);
    }
}
