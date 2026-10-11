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
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f32,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        visible: bool,
    },
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
            Paint::Image { .. } => Color::rgba(0.8, 0.8, 0.8, 1.0),
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
