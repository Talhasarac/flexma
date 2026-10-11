//! The bundled Lucide icon set (ISC license, see assets/icons/LUCIDE-LICENSE;
//! rebuild with scripts/build-icons.mjs) for the MCP `search_icons` tool and
//! `{type:"icon"}` nodes. Icons become a frame holding one vector layer.

use crate::svg_import::{self, ImportOptions};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

static DATA: &str = include_str!("../assets/icons/lucide.json");

#[derive(Deserialize)]
pub struct Icon {
    pub svg: String,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
struct Library {
    version: String,
    icons: HashMap<String, Icon>,
}

fn library() -> &'static Library {
    static LIB: OnceLock<Library> = OnceLock::new();
    LIB.get_or_init(|| serde_json::from_str(DATA).expect("bundled icon data is valid JSON"))
}

pub fn version() -> &'static str {
    &library().version
}

pub fn count() -> usize {
    library().icons.len()
}

/// "Search", "lucide:search", "arrow right" → "search", "arrow-right".
fn normalize(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase();
    let n = n.strip_prefix("lucide:").unwrap_or(&n);
    n.split(|c: char| c.is_whitespace() || c == '_').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-")
}

pub fn get(name: &str) -> Option<(&'static str, &'static Icon)> {
    library().icons.get_key_value(normalize(name).as_str()).map(|(k, v)| (k.as_str(), v))
}

/// Icons matching every word of the query (in the name or tags), best first.
pub fn search(query: &str, limit: usize) -> Vec<(&'static str, &'static Icon)> {
    let words: Vec<String> = query.to_ascii_lowercase().split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let joined = normalize(query);
    let mut hits: Vec<(u32, &'static str, &'static Icon)> = library()
        .icons
        .iter()
        .filter_map(|(name, icon)| {
            let mut score = 0;
            for w in &words {
                let s = if name == w || *name == joined {
                    100
                } else if name.split('-').any(|p| p == w) {
                    70
                } else if name.contains(w.as_str()) {
                    50
                } else if icon.tags.iter().any(|t| t == w) {
                    40
                } else if icon.tags.iter().any(|t| t.contains(w.as_str())) {
                    20
                } else {
                    return None;
                };
                score += s;
            }
            Some((score, name.as_str(), icon))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
    hits.into_iter().take(limit).map(|(_, n, i)| (n, i)).collect()
}

/// Expand an `{type:"icon", icon:"search", size, color, strokeWidth, …}` spec
/// into a frame with the icon's vector. Other props (x, y, name, opacity…)
/// stay on the frame.
pub fn expand(spec: &Map<String, Value>) -> Result<Value, String> {
    let wanted = spec
        .get("icon")
        .or_else(|| spec.get("name"))
        .and_then(Value::as_str)
        .ok_or("an icon node needs \"icon\": a name from search_icons, e.g. \"search\"")?;
    let (name, icon) = get(wanted).ok_or_else(|| {
        let close: Vec<&str> = search(wanted, 5).into_iter().map(|(n, _)| n).collect();
        if close.is_empty() {
            format!("there's no icon named '{wanted}'; find one with search_icons")
        } else {
            format!("there's no icon named '{wanted}'; close matches: {}", close.join(", "))
        }
    })?;
    let size = spec.get("size").or_else(|| spec.get("width")).and_then(Value::as_f64).unwrap_or(24.0).clamp(4.0, 2048.0);
    let color = ["color", "stroke", "fill"]
        .iter()
        .find_map(|k| spec.get(*k).and_then(Value::as_str))
        .unwrap_or("#111111");
    if !color.starts_with('#') || color.len() > 9 || !color[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("icon color must be a hex color like #111111, not '{color}'"));
    }
    let stroke = spec.get("strokeWidth").or_else(|| spec.get("strokeWeight")).and_then(Value::as_f64).unwrap_or(2.0).clamp(0.25, 6.0);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="{color}" stroke-width="{stroke}" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
        icon.svg
    );
    let layer = spec
        .get("icon")
        .and(spec.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("icon/{name}"));
    let imported = svg_import::import(&svg, &ImportOptions { name: layer, width: Some(size), height: None, flatten: true })?;
    let mut frame = imported.spec.as_object().cloned().unwrap_or_default();
    for (k, v) in spec {
        if !matches!(k.as_str(), "type" | "icon" | "name" | "size" | "width" | "height" | "color" | "stroke" | "fill" | "strokeWidth" | "strokeWeight") {
            frame.insert(k.clone(), v.clone());
        }
    }
    frame.insert("description".into(), json!(format!("Lucide icon \"{name}\"")));
    Ok(Value::Object(frame))
}

/// Replace icon specs (at any depth) with their expanded frames.
pub fn expand_all(specs: &mut [Value]) -> Result<usize, String> {
    let mut n = 0;
    for s in specs.iter_mut() {
        let Some(o) = s.as_object_mut() else { continue };
        let is_icon = o.get("type").and_then(Value::as_str).map(|t| t.eq_ignore_ascii_case("icon")).unwrap_or(false);
        if is_icon {
            *s = expand(o)?;
            n += 1;
        } else if let Some(Value::Array(children)) = o.get_mut("children") {
            n += expand_all(children)?;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_icons_by_name_and_tag() {
        assert!(count() > 1000);
        assert_eq!(search("search", 3)[0].0, "search");
        assert!(search("magnifier", 10).iter().any(|(n, _)| *n == "search"));
        assert!(search("arrow right", 5).iter().any(|(n, _)| *n == "arrow-right"));
        assert_eq!(get("Lucide:Arrow Right").map(|(n, _)| n), Some("arrow-right"));
    }

    #[test]
    fn expands_icon_nodes() {
        let spec = json!({ "type": "icon", "icon": "heart", "size": 48, "color": "#e11d48", "x": 10 });
        let out = expand(spec.as_object().unwrap()).unwrap();
        assert_eq!(out["name"], "icon/heart");
        assert_eq!((out["width"].as_f64(), out["x"].as_f64()), (Some(48.0), Some(10.0)));
        let vector = &out["children"][0];
        assert_eq!(vector["strokes"][0]["color"], "#e11d48");
        assert_eq!(vector["strokeWeight"], 4.0);
        assert_eq!(vector["strokeCap"], "ROUND");
        assert!(expand(json!({ "type": "icon", "icon": "nope-nope" }).as_object().unwrap()).is_err());
    }
}
