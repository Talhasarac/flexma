//! Text line breaking shared by the renderer (real font metrics) and the server
//! (estimated metrics, used for SVG/PNG export and MCP-created text).

use crate::node::Node;
use crate::paint::{TextAutoResize, TextOverflow};
use std::borrow::Cow;

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

/// The lines a text node shows, measured with `measure`: wrapped at its width
/// (fixed-width text with `textWrap`), then cut to its box for the ELLIPSIS and
/// TRUNCATE overflow modes.
pub fn layout_lines(n: &Node, measure: &mut dyn FnMut(&str) -> f64) -> Vec<String> {
    let fixed_w = n.text_auto_resize != TextAutoResize::WidthAndHeight;
    // A hair of slack so text measured to exactly the box width doesn't wrap.
    let max_w = n.width + 0.01;
    let mut lines = wrap_lines(&n.characters, (fixed_w && n.text_wrap).then_some(max_w), measure);
    let mark = match n.text_overflow {
        TextOverflow::Ellipsis => "…",
        TextOverflow::Truncate => "",
        TextOverflow::Visible | TextOverflow::Clip => return lines,
    };
    let mut cut = false;
    if n.text_auto_resize == TextAutoResize::None {
        // At least one line, like TextMeshPro.
        let fit = (((n.height + 0.01) / n.effective_line_height()).floor() as usize).max(1);
        if lines.len() > fit {
            lines.truncate(fit);
            cut = true;
        }
    }
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.iter_mut().enumerate() {
        let too_wide = fixed_w && measure(line) > max_w;
        if too_wide || (i == last && cut) {
            *line = shorten(line, if fixed_w { Some(max_w) } else { None }, mark, measure);
        }
    }
    lines
}

/// The node as drawn. With `autoFontSize`, a fixed box uses the largest font
/// size in the range at which the text fits, like TextMeshPro's Auto Size:
/// every line fits the height, and no word is wider than the box (or, without
/// wrapping, no line). Line height and letter spacing scale with it. `measure`
/// measures at the node's own font size.
pub fn fitted<'a>(n: &'a Node, measure: &mut dyn FnMut(&str) -> f64) -> Cow<'a, Node> {
    let Some(range) = n.auto_font_size else { return Cow::Borrowed(n) };
    if n.text_auto_resize != TextAutoResize::None || n.font_size <= 0.0 {
        return Cow::Borrowed(n);
    }
    let (lo, hi) = (range.min.max(1.0).min(range.max), range.max.max(1.0));
    let base = n.font_size;
    let lh = n.effective_line_height();
    // Text scales linearly with the font size, so test size s by laying out
    // at the base size in a box shrunk by s / base.
    let widest_word = if n.text_wrap { n.characters.split_whitespace().map(|w| measure(w)).fold(0.0, f64::max) } else { 0.0 };
    let mut fits = |s: f64| {
        let k = s / base;
        let w = n.width / k + 0.01;
        let lines = wrap_lines(&n.characters, n.text_wrap.then_some(w), measure);
        let tall_ok = lines.len().max(1) as f64 * lh * k <= n.height + 0.01;
        let wide_ok = if n.text_wrap { widest_word <= w } else { lines.iter().all(|l| measure(l) <= w) };
        tall_ok && wide_ok
    };
    let size = if fits(hi) {
        hi
    } else {
        let (mut a, mut b) = (lo, hi);
        for _ in 0..14 {
            let mid = (a + b) / 2.0;
            if fits(mid) {
                a = mid;
            } else {
                b = mid;
            }
        }
        (a * 10.0).floor() / 10.0
    };
    if (size - base).abs() < 1e-6 {
        return Cow::Borrowed(n);
    }
    let k = size / base;
    let mut out = n.clone();
    out.auto_font_size = None;
    out.font_size = size;
    out.line_height = n.line_height.map(|v| v * k);
    out.letter_spacing = n.letter_spacing * k;
    Cow::Owned(out)
}

/// Drop characters from the end of `line` until it fits `max_w` with `mark`
/// (e.g. "…") appended.
fn shorten(line: &str, max_w: Option<f64>, mark: &str, measure: &mut dyn FnMut(&str) -> f64) -> String {
    let mut chars: Vec<char> = line.chars().collect();
    loop {
        let s: String = chars.iter().collect::<String>().trim_end().to_string() + mark;
        if chars.is_empty() || max_w.is_none_or(|w| measure(&s) <= w) {
            return if chars.is_empty() && max_w.is_some_and(|w| measure(mark) > w) { String::new() } else { s };
        }
        chars.pop();
    }
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
    let n = &*fitted(n, &mut |s: &str| estimate_width(s, n.font_size, n.letter_spacing, n.font_weight));
    let mut measure = |s: &str| estimate_width(s, n.font_size, n.letter_spacing, n.font_weight);
    let lines = layout_lines(n, &mut measure);
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

    #[test]
    fn auto_font_size_shrinks_to_fit() {
        // 1 unit per char at size 10: widths scale with the size.
        let mut n = Node { characters: "aaaa bbbb cccc dddd".into(), width: 30.0, height: 24.0, font_size: 10.0, line_height: Some(12.0), ..Node::default() };
        n.text_auto_resize = TextAutoResize::None;
        n.auto_font_size = Some(crate::paint::FontSizeRange { min: 4.0, max: 20.0 });
        let mut m = |s: &str| s.chars().count() as f64;
        // One line of 19 chars fits 30 wide up to size 15.79 (two lines would
        // need size 10 to fit 24 tall).
        let f = fitted(&n, &mut m);
        assert!((f.font_size - 15.7).abs() < 0.05, "{}", f.font_size);
        assert!((f.line_height.unwrap() - 12.0 * f.font_size / 10.0).abs() < 1e-9);
        // Plenty of room: the max size.
        n.characters = "ab".into();
        assert_eq!(fitted(&n, &mut m).font_size, 20.0);
        // Never below the min: one 78-char word is 31.2 wide even at size 4.
        n.characters = "abcdefghijklmnopqrstuvwxyz".repeat(3);
        assert_eq!(fitted(&n, &mut m).font_size, 4.0);
        // Only fixed boxes auto-size.
        n.text_auto_resize = TextAutoResize::Height;
        assert_eq!(fitted(&n, &mut m).font_size, 10.0);
    }

    #[test]
    fn wrap_and_overflow_modes() {
        // 1 unit per char ("…" counts as 1), box 11 wide and 2 lines tall.
        let mut m = |s: &str| s.chars().count() as f64;
        let mut n = Node { characters: "hello world foo bar".into(), width: 11.0, height: 2.0, line_height: Some(1.0), ..Node::default() };
        n.text_auto_resize = TextAutoResize::None;
        assert_eq!(layout_lines(&n, &mut m), vec!["hello world", "foo bar"]);
        n.characters = "hello world foo bar baz qux".into();
        assert_eq!(layout_lines(&n, &mut m), vec!["hello world", "foo bar baz", "qux"], "VISIBLE keeps every line");
        n.text_overflow = TextOverflow::Ellipsis;
        assert_eq!(layout_lines(&n, &mut m), vec!["hello world", "foo bar ba…"]);
        n.text_overflow = TextOverflow::Truncate;
        assert_eq!(layout_lines(&n, &mut m), vec!["hello world", "foo bar baz"]);
        // No wrapping: only newlines break, and long lines are cut at the width.
        n.text_wrap = false;
        n.characters = "a long line of text\nshort".into();
        n.text_overflow = TextOverflow::Visible;
        assert_eq!(layout_lines(&n, &mut m), vec!["a long line of text", "short"]);
        n.text_overflow = TextOverflow::Ellipsis;
        assert_eq!(layout_lines(&n, &mut m), vec!["a long lin…", "short"]);
        // Fits: untouched.
        n.characters = "ok".into();
        assert_eq!(layout_lines(&n, &mut m), vec!["ok"]);
        // Hug-width text never overflows.
        n.text_auto_resize = TextAutoResize::WidthAndHeight;
        n.characters = "a long line of text".into();
        assert_eq!(layout_lines(&n, &mut m), vec!["a long line of text"]);
    }
}
