//! Text line breaking shared by the renderer (real font metrics) and the server
//! (estimated metrics, used for SVG/PNG export and MCP-created text).

use crate::node::Node;
use crate::paint::TextAutoResize;

/// Break `text` into lines. Hard newlines always break; when `max_width` is set,
/// words wrap greedily (very long words are split by character).
pub fn wrap_lines(text: &str, max_width: Option<f64>, measure: &mut dyn FnMut(&str) -> f64) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let Some(maxw) = max_width.filter(|w| *w > 0.0) else {
            out.push(para.to_string());
            continue;
        };
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in split_keep_spaces(para) {
            let candidate = format!("{line}{word}");
            if line.is_empty() || measure(candidate.trim_end()) <= maxw {
                if line.is_empty() && measure(word.trim_end()) > maxw {
                    // Split an over-long word.
                    for ch in word.chars() {
                        let c2 = format!("{line}{ch}");
                        if !line.is_empty() && measure(&c2) > maxw {
                            out.push(std::mem::take(&mut line));
                        }
                        line.push(ch);
                    }
                } else {
                    line = candidate;
                }
            } else {
                out.push(line.trim_end().to_string());
                line = word.trim_start().to_string();
            }
        }
        out.push(line.trim_end().to_string());
    }
    out
}

/// Split into words keeping their trailing spaces attached.
fn split_keep_spaces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            out.push(&s[start..i]);
            start = i;
        } else {
            i += 1;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Rough advance-width estimate for a sans-serif UI font (Inter-like).
pub fn estimate_width(s: &str, font_size: f64, letter_spacing: f64, weight: u16) -> f64 {
    let bold = if weight >= 600 { 1.06 } else { 1.0 };
    let mut w = 0.0;
    for ch in s.chars() {
        let f = match ch {
            'i' | 'l' | 'j' | '!' | '|' | '\'' | '.' | ',' | ':' | ';' => 0.28,
            'f' | 't' | 'r' | 'I' | ' ' | '(' | ')' | '[' | ']' => 0.36,
            'm' | 'w' | 'M' | 'W' => 0.86,
            'A'..='Z' => 0.66,
            '0'..='9' => 0.58,
            c if c.is_ascii() => 0.53,
            _ => 0.9, // CJK, emoji, etc.
        };
        w += f * font_size * bold + letter_spacing;
    }
    w
}

/// Lines and box size for a text node using estimated metrics.
/// Resize a text node's box to its estimated content (per its auto-resize
/// mode) and flag it so a browser measures it exactly.
pub fn fit_box(n: &mut Node) {
    let (_, w, h) = estimate_layout(n);
    match n.text_auto_resize {
        crate::paint::TextAutoResize::WidthAndHeight => {
            n.width = w;
            n.height = h;
        }
        crate::paint::TextAutoResize::Height => n.height = h,
        crate::paint::TextAutoResize::None => {}
    }
    n.needs_measure = true;
}

pub fn estimate_layout(n: &Node) -> (Vec<String>, f64, f64) {
    let mut measure = |s: &str| estimate_width(s, n.font_size, n.letter_spacing, n.font_weight);
    let max_w = match n.text_auto_resize {
        TextAutoResize::WidthAndHeight => None,
        _ => Some(n.width),
    };
    let lines = wrap_lines(&n.characters, max_w, &mut measure);
    let lh = n.effective_line_height();
    let w = match n.text_auto_resize {
        TextAutoResize::WidthAndHeight => lines.iter().map(|l| measure(l)).fold(0.0, f64::max).ceil(),
        _ => n.width,
    };
    let h = match n.text_auto_resize {
        TextAutoResize::None => n.height,
        _ => (lines.len().max(1) as f64 * lh).ceil(),
    };
    (lines, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words() {
        let mut m = |s: &str| s.len() as f64;
        let lines = wrap_lines("hello world foo", Some(11.0), &mut m);
        assert_eq!(lines, vec!["hello world", "foo"]);
        let lines = wrap_lines("a\n\nb", None, &mut m);
        assert_eq!(lines, vec!["a", "", "b"]);
        let lines = wrap_lines("abcdefgh", Some(3.0), &mut m);
        assert_eq!(lines, vec!["abc", "def", "gh"]);
    }
}
