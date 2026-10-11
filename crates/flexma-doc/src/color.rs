//! RGBA colors. Serialized as `#rrggbb` / `#rrggbbaa` hex strings because that is
//! what humans and AI agents read and write most reliably. Deserialization also
//! accepts `{ "r": 0..1, "g": .., "b": .., "a": .. }` objects and CSS-ish short hex.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// sRGB-encoded channel to linear light.
    pub fn srgb_to_linear(c: f32) -> f32 {
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    }

    /// Linear-light channel to sRGB encoding.
    pub fn linear_to_srgb(c: f32) -> f32 {
        if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055 }
    }

    /// The same color with its channels in linear light (alpha unchanged).
    pub fn to_linear(self) -> Color {
        Color { r: Self::srgb_to_linear(self.r), g: Self::srgb_to_linear(self.g), b: Self::srgb_to_linear(self.b), a: self.a }
    }

    pub const BLACK: Color = Color::rgba(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Color = Color::rgba(1.0, 1.0, 1.0, 1.0);
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Color { r, g, b, a }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Color { a, ..self }
    }

    pub fn parse(s: &str) -> Option<Color> {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            return parse_hex(hex);
        }
        match s.to_ascii_lowercase().as_str() {
            "black" => Some(Color::BLACK),
            "white" => Some(Color::WHITE),
            "transparent" => Some(Color::TRANSPARENT),
            "red" => Some(Color::rgba(1.0, 0.0, 0.0, 1.0)),
            "green" => Some(Color::rgba(0.0, 0.5, 0.0, 1.0)),
            "blue" => Some(Color::rgba(0.0, 0.0, 1.0, 1.0)),
            "gray" | "grey" => Some(Color::rgba(0.5, 0.5, 0.5, 1.0)),
            other => parse_hex(other),
        }
    }

    pub fn to_hex(self) -> String {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        if c(self.a) == 255 {
            format!("#{:02x}{:02x}{:02x}", c(self.r), c(self.g), c(self.b))
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", c(self.r), c(self.g), c(self.b), c(self.a))
        }
    }

    /// `#rrggbb` without alpha, for SVG `fill` attributes (alpha goes in `fill-opacity`).
    pub fn to_hex_rgb(self) -> String {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", c(self.r), c(self.g), c(self.b))
    }
}

fn parse_hex(hex: &str) -> Option<Color> {
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<Vec<u8>>>()?;
    let (r, g, b, a) = match digits.len() {
        3 => (digits[0] * 17, digits[1] * 17, digits[2] * 17, 255),
        4 => (digits[0] * 17, digits[1] * 17, digits[2] * 17, digits[3] * 17),
        6 => (
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
            255,
        ),
        8 => (
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
            digits[6] * 16 + digits[7],
        ),
        _ => return None,
    };
    Some(Color::rgba(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ))
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Str(String),
            Obj {
                r: f32,
                g: f32,
                b: f32,
                #[serde(default = "one")]
                a: f32,
            },
        }
        fn one() -> f32 {
            1.0
        }
        match Repr::deserialize(d)? {
            Repr::Str(s) => Color::parse(&s)
                .ok_or_else(|| de::Error::custom(format!("invalid color '{s}', expected #rrggbb or #rrggbbaa"))),
            Repr::Obj { r, g, b, a } => {
                // Accept both 0..1 and 0..255 component ranges.
                if r > 1.0 || g > 1.0 || b > 1.0 {
                    Ok(Color::rgba(r / 255.0, g / 255.0, b / 255.0, if a > 1.0 { a / 255.0 } else { a }))
                } else {
                    Ok(Color::rgba(r, g, b, a))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = Color::parse("#ff8000").unwrap();
        assert_eq!(c.to_hex(), "#ff8000");
        let c = Color::parse("#ff800080").unwrap();
        assert_eq!(c.to_hex(), "#ff800080");
        assert_eq!(Color::parse("#fff").unwrap(), Color::WHITE);
    }

    #[test]
    fn serde_forms() {
        let c: Color = serde_json::from_str("\"#000000\"").unwrap();
        assert_eq!(c, Color::BLACK);
        let c: Color = serde_json::from_str(r#"{"r":255,"g":255,"b":255}"#).unwrap();
        assert_eq!(c, Color::WHITE);
    }
}
