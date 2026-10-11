//! Paints (fills/strokes), effects and the small enums used by node properties.

use crate::color::Color;
use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}
fn is_true(v: &bool) -> bool {
    *v
}
fn is_one(v: &f32) -> bool {
    *v == 1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    /// Position along the gradient, 0..1.
    pub position: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ImageFit {
    #[default]
    Fill,
    Fit,
    Stretch,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Paint {
    Solid {
        color: Color,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f32,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        visible: bool,
    },
    LinearGradient {
        stops: Vec<GradientStop>,
        /// Angle in degrees. 0 = left→right, 90 = top→bottom.
        #[serde(default)]
        angle: f32,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f32,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        visible: bool,
    },
    Image {
        /// Asset id returned by the upload API (`/api/files/{file}/assets`).
        asset: String,
        #[serde(default)]
        fit: ImageFit,
        /// Multiply color, like Unity's `Image.color`. White (or none) leaves the image unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tint: Option<Color>,
        /// 9-slice scaling. When set, the image stretches by slices and `fit` is ignored.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slice: Option<NineSlice>,
        /// Show only part of the image, cut along an axis or swept around a pivot
        /// (Unity's Filled image type).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filled: Option<ImageFilled>,
        /// Use only this region of the image (a sprite in an atlas), in image
        /// pixels. Fit, slice and filled then work on the region.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crop: Option<ImageCrop>,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f32,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        visible: bool,
    },
}

/// 9-slice scaling for image fills (Unity's sliced sprites, CSS `border-image`):
/// the corners keep their size, the edges stretch along one axis and the
/// center stretches both ways.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NineSlice {
    /// Insets from each image edge, in image pixels (Unity's sprite border).
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    /// Layer pixels per image pixel for the border: 1 draws it at the image's
    /// own size, 0.5 suits @2x art. From Unity: reference pixels per unit ÷
    /// (sprite pixels per unit × pixels per unit multiplier).
    #[serde(skip_serializing_if = "is_one")]
    pub scale: f32,
    /// Draw the center patch (Unity's Fill Center). Off leaves only the frame.
    #[serde(skip_serializing_if = "is_true")]
    pub fill_center: bool,
}

impl NineSlice {
    pub fn uniform(v: f32) -> NineSlice {
        NineSlice { top: v, right: v, bottom: v, left: v, scale: 1.0, fill_center: true }
    }

    /// Source insets `[top, right, bottom, left]` for an `iw`×`ih` image, shrunk
    /// so opposite insets never overlap.
    pub fn source_insets(&self, iw: f64, ih: f64) -> [f64; 4] {
        let fit = |a: f32, b: f32, size: f64| {
            let (a, b) = (a.max(0.0) as f64, b.max(0.0) as f64);
            let k = if a + b > size && a + b > 0.0 { size / (a + b) } else { 1.0 };
            (a * k, b * k)
        };
        let (l, r) = fit(self.left, self.right, iw);
        let (t, b) = fit(self.top, self.bottom, ih);
        [t, r, b, l]
    }

    /// Destination insets `[top, right, bottom, left]` in a `w`×`h` box for the
    /// given source insets. Borders shrink proportionally when the box is
    /// smaller than they are, as Unity does.
    pub fn dest_insets(&self, src: [f64; 4], w: f64, h: f64) -> [f64; 4] {
        let s = self.scale.max(0.0) as f64;
        let fit = |a: f64, b: f64, size: f64| {
            let (a, b) = (a * s, b * s);
            let k = if a + b > size && a + b > 0.0 { size.max(0.0) / (a + b) } else { 1.0 };
            (a * k, b * k)
        };
        let (l, r) = fit(src[3], src[1], w);
        let (t, b) = fit(src[0], src[2], h);
        [t, r, b, l]
    }
}

/// Accepts a number (all sides), `[v, h]`, `[top, right, bottom, left]` or an
/// object `{top, right, bottom, left, scale?, fillCenter?}`.
impl<'de> Deserialize<'de> for NineSlice {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<NineSlice, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Obj {
            #[serde(default)]
            top: f32,
            #[serde(default)]
            right: f32,
            #[serde(default)]
            bottom: f32,
            #[serde(default)]
            left: f32,
            #[serde(default = "one")]
            scale: f32,
            #[serde(default = "yes")]
            fill_center: bool,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Num(f32),
            Arr(Vec<f32>),
            Obj(Obj),
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Num(v) => NineSlice::uniform(v),
            Repr::Obj(o) => NineSlice { top: o.top, right: o.right, bottom: o.bottom, left: o.left, scale: o.scale, fill_center: o.fill_center },
            Repr::Arr(a) => match a.len() {
                1 => NineSlice::uniform(a[0]),
                2 => NineSlice { top: a[0], bottom: a[0], left: a[1], right: a[1], ..NineSlice::uniform(0.0) },
                4 => NineSlice { top: a[0], right: a[1], bottom: a[2], left: a[3], ..NineSlice::uniform(0.0) },
                _ => return Err(serde::de::Error::custom("slice array must have 1, 2 or 4 numbers")),
            },
        })
    }
}

/// A region of an image in image pixels, from the top-left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ImageCrop {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ImageCrop {
    /// The region clamped to a `w`×`h` image, or `None` if nothing is left.
    pub fn within(&self, w: f64, h: f64) -> Option<ImageCrop> {
        let x = self.x.clamp(0.0, w);
        let y = self.y.clamp(0.0, h);
        let width = self.width.min(w - x);
        let height = self.height.min(h - y);
        (width > 0.0 && height > 0.0).then_some(ImageCrop { x, y, width, height })
    }
}

/// Accepts `{x, y, width|w, height|h}` or `[x, y, width, height]`.
impl<'de> Deserialize<'de> for ImageCrop {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<ImageCrop, D::Error> {
        use serde::de::Error;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Arr([f64; 4]),
            Obj {
                #[serde(default)]
                x: f64,
                #[serde(default)]
                y: f64,
                #[serde(alias = "w")]
                width: f64,
                #[serde(alias = "h")]
                height: f64,
            },
        }
        let (x, y, width, height) = match Repr::deserialize(d).map_err(|_| D::Error::custom("crop must be {x, y, width, height} or [x, y, width, height] in image pixels"))? {
            Repr::Arr([x, y, w, h]) => (x, y, w, h),
            Repr::Obj { x, y, width, height } => (x, y, width, height),
        };
        if width <= 0.0 || height <= 0.0 {
            return Err(D::Error::custom("crop width and height must be positive"));
        }
        Ok(ImageCrop { x, y, width, height })
    }
}

/// Unity's Filled image: progress bars (`HORIZONTAL`, `VERTICAL`) and
/// cooldown wheels (`RADIAL_90`, `RADIAL_180`, `RADIAL_360`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageFilled {
    pub method: FillMethod,
    /// 0..1, the part shown.
    pub amount: f32,
    /// Where the fill starts: the edge (or corner) it grows from, or for
    /// `RADIAL_360` the side its sweep starts at.
    pub origin: FillOrigin,
    /// Radial sweep direction.
    pub clockwise: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FillMethod {
    #[serde(rename = "HORIZONTAL")]
    Horizontal,
    #[serde(rename = "VERTICAL")]
    Vertical,
    #[serde(rename = "RADIAL_90")]
    Radial90,
    #[serde(rename = "RADIAL_180")]
    Radial180,
    #[serde(rename = "RADIAL_360")]
    Radial360,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FillOrigin {
    Left,
    Right,
    Bottom,
    Top,
    BottomLeft,
    TopLeft,
    TopRight,
    BottomRight,
}

impl FillMethod {
    /// The origins this method takes, in Unity's `fillOrigin` order.
    pub fn origins(self) -> &'static [FillOrigin] {
        use FillOrigin::*;
        match self {
            FillMethod::Horizontal => &[Left, Right],
            FillMethod::Vertical => &[Bottom, Top],
            FillMethod::Radial90 => &[BottomLeft, TopLeft, TopRight, BottomRight],
            FillMethod::Radial180 => &[Bottom, Left, Top, Right],
            FillMethod::Radial360 => &[Bottom, Right, Top, Left],
        }
    }

    /// Shader code: 1 horizontal, 2 vertical, 3/4/5 radial 90/180/360.
    pub fn code(self) -> f32 {
        match self {
            FillMethod::Horizontal => 1.0,
            FillMethod::Vertical => 2.0,
            FillMethod::Radial90 => 3.0,
            FillMethod::Radial180 => 4.0,
            FillMethod::Radial360 => 5.0,
        }
    }
}

impl ImageFilled {
    /// The origin as Unity's `fillOrigin` number for this method.
    pub fn origin_index(&self) -> usize {
        self.method.origins().iter().position(|o| *o == self.origin).unwrap_or(0)
    }
}

/// Accepts `{method, amount?, origin?, clockwise?}`. Names are case-insensitive
/// (`radial360`, `Radial_360`), and `origin` may be Unity's `fillOrigin` number.
impl<'de> Deserialize<'de> for ImageFilled {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<ImageFilled, D::Error> {
        use serde::de::Error;
        #[derive(Deserialize)]
        struct Obj {
            method: String,
            #[serde(default = "one")]
            amount: f32,
            #[serde(default)]
            origin: Option<serde_json::Value>,
            #[serde(default = "yes")]
            clockwise: bool,
        }
        let o = Obj::deserialize(d)?;
        let key = |s: &str| s.to_ascii_uppercase().replace(['_', '-', ' '], "");
        let method = match key(&o.method).as_str() {
            "HORIZONTAL" => FillMethod::Horizontal,
            "VERTICAL" => FillMethod::Vertical,
            "RADIAL90" => FillMethod::Radial90,
            "RADIAL180" => FillMethod::Radial180,
            "RADIAL360" => FillMethod::Radial360,
            _ => return Err(D::Error::custom("filled.method must be HORIZONTAL, VERTICAL, RADIAL_90, RADIAL_180 or RADIAL_360")),
        };
        let valid = method.origins();
        let names = || valid.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>().join(", ");
        let origin = match &o.origin {
            None | Some(serde_json::Value::Null) => valid[0],
            Some(serde_json::Value::Number(n)) => *n
                .as_u64()
                .and_then(|i| valid.get(i as usize))
                .ok_or_else(|| D::Error::custom(format!("filled.origin {n} is out of range for {method:?} (0..{})", valid.len() - 1)))?,
            Some(serde_json::Value::String(s)) => *valid
                .iter()
                .find(|o| key(&format!("{o:?}")) == key(s))
                .ok_or_else(|| D::Error::custom(format!("filled.origin '{s}' doesn't fit {method:?}; use one of {}", names())))?,
            Some(other) => return Err(D::Error::custom(format!("invalid filled.origin {other}"))),
        };
        Ok(ImageFilled { method, amount: o.amount.clamp(0.0, 1.0), origin, clockwise: o.clockwise })
    }
}

impl Paint {
    pub fn solid(color: Color) -> Paint {
        Paint::Solid { color, opacity: 1.0, visible: true }
    }

    pub fn is_visible(&self) -> bool {
        match self {
            Paint::Solid { visible, opacity, .. }
            | Paint::LinearGradient { visible, opacity, .. }
            | Paint::Image { visible, opacity, .. } => *visible && *opacity > 0.0,
        }
    }

    pub fn opacity(&self) -> f32 {
        match self {
            Paint::Solid { opacity, .. }
            | Paint::LinearGradient { opacity, .. }
            | Paint::Image { opacity, .. } => *opacity,
        }
    }

    /// A representative flat color (used for thumbnails, gradients fallbacks, etc.).
    pub fn approx_color(&self) -> Color {
        match self {
            Paint::Solid { color, opacity, .. } => color.with_alpha(color.a * opacity),
            Paint::LinearGradient { stops, opacity, .. } => stops
                .first()
                .map(|s| s.color.with_alpha(s.color.a * opacity))
                .unwrap_or(Color::TRANSPARENT),
            Paint::Image { tint, .. } => {
                let t = tint.unwrap_or(Color::WHITE);
                Color::rgba(0.8 * t.r, 0.8 * t.g, 0.8 * t.b, t.a)
            }
        }
    }
}

/// How a node keeps its place along one axis when its parent frame is resized
/// (Figma's constraints, Unity's anchors).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConstraintType {
    /// Keep the distance to the left (or top) edge.
    #[default]
    Min,
    /// Keep the distance to the right (or bottom) edge.
    Max,
    /// Keep the offset from the parent's center.
    Center,
    /// Keep the distances to both edges, so the node grows with its parent.
    Stretch,
    /// Keep position and size proportional to the parent.
    Scale,
}

impl ConstraintType {
    /// Also takes Figma's UI names (LEFT, RIGHT, LEFT_RIGHT, TOP_BOTTOM, ...), any case.
    pub fn parse(s: &str) -> Option<ConstraintType> {
        let k: String = s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
        Some(match k.as_str() {
            "MIN" | "LEFT" | "TOP" | "START" => ConstraintType::Min,
            "MAX" | "RIGHT" | "BOTTOM" | "END" => ConstraintType::Max,
            "CENTER" | "CENTRE" | "MIDDLE" => ConstraintType::Center,
            "STRETCH" | "LEFTRIGHT" | "TOPBOTTOM" | "BOTH" => ConstraintType::Stretch,
            "SCALE" | "PROPORTIONAL" => ConstraintType::Scale,
            _ => return None,
        })
    }

    /// From one axis of a Unity RectTransform's anchors (fractions of the parent,
    /// measured from the left or the top).
    pub fn from_anchors(min: f64, max: f64) -> ConstraintType {
        let near = |a: f64, b: f64| (a - b).abs() < 1e-3;
        if near(min, max) {
            if near(min, 0.0) {
                ConstraintType::Min
            } else if near(min, 1.0) {
                ConstraintType::Max
            } else if near(min, 0.5) {
                ConstraintType::Center
            } else {
                ConstraintType::Scale
            }
        } else if near(min, 0.0) && near(max, 1.0) {
            ConstraintType::Stretch
        } else {
            ConstraintType::Scale
        }
    }
}

impl<'de> Deserialize<'de> for ConstraintType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<ConstraintType, D::Error> {
        let s = String::deserialize(d)?;
        ConstraintType::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown constraint '{s}' (use MIN, MAX, CENTER, STRETCH or SCALE)")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Default)]
pub struct Constraints {
    pub horizontal: ConstraintType,
    pub vertical: ConstraintType,
}

/// Accepts `{horizontal, vertical}` (a missing axis is MIN), one string for
/// both axes, or Unity anchors `{anchorMin: [x, y], anchorMax: [x, y]}`. Unity
/// measures y from the bottom, so they are flipped here.
impl<'de> Deserialize<'de> for Constraints {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Constraints, D::Error> {
        use serde::de::Error;
        use serde_json::Value;
        let kind = |v: &Value| -> Result<ConstraintType, D::Error> {
            let s = v.as_str().ok_or_else(|| D::Error::custom("a constraint must be a string"))?;
            ConstraintType::parse(s).ok_or_else(|| D::Error::custom(format!("unknown constraint '{s}' (use MIN, MAX, CENTER, STRETCH or SCALE)")))
        };
        let point = |v: &Value| -> Option<(f64, f64)> {
            match v {
                Value::Array(a) if a.len() == 2 => Some((a[0].as_f64()?, a[1].as_f64()?)),
                Value::Object(o) => Some((o.get("x")?.as_f64()?, o.get("y")?.as_f64()?)),
                _ => None,
            }
        };
        let v = Value::deserialize(d)?;
        match &v {
            Value::String(_) => {
                let c = kind(&v)?;
                Ok(Constraints { horizontal: c, vertical: c })
            }
            Value::Object(o) if o.contains_key("anchorMin") || o.contains_key("anchorMax") => {
                let bad = || D::Error::custom("Unity anchors need anchorMin and anchorMax as [x, y] or {x, y}");
                let (x0, y0) = o.get("anchorMin").and_then(point).ok_or_else(bad)?;
                let (x1, y1) = o.get("anchorMax").and_then(point).ok_or_else(bad)?;
                Ok(Constraints { horizontal: ConstraintType::from_anchors(x0, x1), vertical: ConstraintType::from_anchors(1.0 - y1, 1.0 - y0) })
            }
            Value::Object(o) => Ok(Constraints {
                horizontal: o.get("horizontal").map(kind).transpose()?.unwrap_or_default(),
                vertical: o.get("vertical").map(kind).transpose()?.unwrap_or_default(),
            }),
            _ => Err(D::Error::custom("constraints must be {horizontal, vertical}, one string, or {anchorMin, anchorMax}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Effect {
    DropShadow {
        color: Color,
        #[serde(default)]
        x: f32,
        #[serde(default)]
        y: f32,
        #[serde(default)]
        blur: f32,
        #[serde(default)]
        spread: f32,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        visible: bool,
    },
    LayerBlur {
        radius: f32,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        visible: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TextAlignVertical {
    #[default]
    Top,
    Center,
    Bottom,
}

/// How a text node's box follows its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TextAutoResize {
    /// Width and height grow to fit the text (single line unless it has newlines).
    #[default]
    WidthAndHeight,
    /// Fixed width, text wraps, height grows.
    Height,
    /// Fixed box.
    None,
}

/// What a text node does with text that doesn't fit its box (TextMeshPro's
/// overflow modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TextOverflow {
    /// Draw it past the box.
    #[default]
    Visible,
    /// Cut it off at the box edges.
    Clip,
    /// Drop the lines (and, without wrapping, the characters) that don't fit,
    /// ending the last shown one with "…".
    Ellipsis,
    /// Like `Ellipsis`, without the "…".
    Truncate,
}

/// A font size range for text that shrinks to fit its box (TextMeshPro's
/// Auto Size).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontSizeRange {
    pub min: f64,
    pub max: f64,
}

/// What a STYLE node holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StyleType {
    #[default]
    Fill,
    Text,
}

/// How overlapping sub-paths fill: EVENODD punches holes (icons, glyphs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FillRule {
    #[default]
    Nonzero,
    Evenodd,
}

/// Stroke ends. NONE is a flat (butt) end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StrokeCap {
    None,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StrokeJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StrokeAlign {
    #[default]
    Center,
    Inside,
    Outside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LayoutMode {
    #[default]
    None,
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LayoutAlign {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

/// How a node sizes itself along an axis inside (or as) an auto-layout frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SizingMode {
    #[default]
    Fixed,
    /// Shrink to fit content (auto-layout frames only).
    Hug,
    /// Stretch to fill the parent auto-layout frame.
    Fill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Default)]
pub struct Padding {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Padding {
    pub fn uniform(v: f64) -> Padding {
        Padding { top: v, right: v, bottom: v, left: v }
    }
}

/// Padding accepts a number (uniform), `[v, h]`, `[t, r, b, l]` or an object.
impl<'de> Deserialize<'de> for Padding {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Padding, D::Error> {
        #[derive(Deserialize)]
        struct Obj {
            #[serde(default)]
            top: f64,
            #[serde(default)]
            right: f64,
            #[serde(default)]
            bottom: f64,
            #[serde(default)]
            left: f64,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            // Order matters: serde structs also accept sequences, so try arrays first.
            Num(f64),
            Arr(Vec<f64>),
            Obj(Obj),
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Num(v) => Padding::uniform(v),
            Repr::Obj(o) => Padding { top: o.top, right: o.right, bottom: o.bottom, left: o.left },
            Repr::Arr(a) => match a.len() {
                1 => Padding::uniform(a[0]),
                2 => Padding { top: a[0], bottom: a[0], left: a[1], right: a[1] },
                4 => Padding { top: a[0], right: a[1], bottom: a[2], left: a[3] },
                _ => return Err(serde::de::Error::custom("padding array must have 1, 2 or 4 numbers")),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn image_paint_with_tint_and_slice() {
        let p: Paint = serde_json::from_value(json!({ "type": "IMAGE", "asset": "a", "tint": "#ff8800", "slice": [4, 8] })).unwrap();
        let Paint::Image { tint, slice: Some(s), .. } = &p else { panic!("{p:?}") };
        assert_eq!(tint.unwrap().to_hex(), "#ff8800");
        assert_eq!((s.top, s.right, s.bottom, s.left, s.scale, s.fill_center), (4.0, 8.0, 4.0, 8.0, 1.0, true));
        // Round trip keeps defaults out.
        assert_eq!(serde_json::to_value(&p).unwrap()["slice"], json!({ "top": 4.0, "right": 8.0, "bottom": 4.0, "left": 8.0 }));
        let s: NineSlice = serde_json::from_value(json!({ "left": 10, "right": 10, "scale": 0.5, "fillCenter": false })).unwrap();
        assert_eq!((s.top, s.left, s.scale, s.fill_center), (0.0, 10.0, 0.5, false));
        // Insets never overlap: in the image, or in a box smaller than the borders.
        let src = NineSlice::uniform(40.0).source_insets(60.0, 100.0);
        assert_eq!(src, [40.0, 30.0, 40.0, 30.0]);
        assert_eq!(NineSlice::uniform(10.0).dest_insets([10.0; 4], 10.0, 100.0), [10.0, 5.0, 10.0, 5.0]);
        // Old documents without the new fields still load.
        let old: Paint = serde_json::from_value(json!({ "type": "IMAGE", "asset": "a", "fit": "FIT" })).unwrap();
        assert!(matches!(old, Paint::Image { tint: None, slice: None, fit: ImageFit::Fit, .. }));
    }
}
