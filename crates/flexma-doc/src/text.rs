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
    if has_runs(n) {
        let lines = rich_layout(n, &mut |st: &Node, s: &str| estimate_width(s, st.font_size, st.letter_spacing, st.font_weight));
        let w = match n.text_auto_resize {
            TextAutoResize::WidthAndHeight => lines.iter().map(|l| l.width).fold(0.0, f64::max).ceil(),
            _ => n.width,
        };
        let h = match n.text_auto_resize {
            TextAutoResize::None => n.height,
            _ => (lines.len().max(1) as f64 * n.effective_line_height()).ceil(),
        };
        return (lines.iter().map(RichLine::text).collect(), w, h);
    }
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

// ---------------------------------------------------------------------------
// Rich text (`textRuns`)
// ---------------------------------------------------------------------------

/// Whether a text node has rich-text runs. Text without them takes the
/// plain path above, unchanged.
pub fn has_runs(n: &Node) -> bool {
    !n.text_runs.is_empty()
}

/// The node as a run draws it.
pub fn run_style(n: &Node, r: &crate::paint::TextRun) -> Node {
    let mut s = n.clone();
    s.text_runs.clear();
    if let Some(f) = &r.font_family {
        s.font_family = f.clone();
    }
    if let Some(w) = r.font_weight {
        s.font_weight = w;
    }
    if let Some(i) = r.italic {
        s.italic = i;
    }
    if let Some(l) = r.letter_spacing {
        s.letter_spacing = l;
    }
    if let Some(f) = &r.fills {
        s.fills = f.clone();
    }
    if let Some(d) = &r.text_decoration {
        s.text_decoration = if d.eq_ignore_ascii_case("NONE") { None } else { Some(d.to_ascii_uppercase()) };
    }
    s
}

/// Part of a line in one style.
pub struct Piece {
    pub text: String,
    pub style: Node,
    pub width: f64,
}

/// A laid-out line of rich text.
pub struct RichLine {
    pub pieces: Vec<Piece>,
    pub width: f64,
}

impl RichLine {
    pub fn text(&self) -> String {
        self.pieces.iter().map(|p| p.text.as_str()).collect()
    }
}

/// Lay out a node with `textRuns` like `layout_lines` (wrapping, hard
/// breaks, CLIP/ELLIPSIS/TRUNCATE, fixed boxes), measuring each styled piece
/// with `measure(style, text)`. Auto font size isn't applied to rich text.
pub fn rich_layout(n: &Node, measure: &mut dyn FnMut(&Node, &str) -> f64) -> Vec<RichLine> {
    let chars: Vec<char> = n.characters.chars().collect();
    let len = chars.len();
    let mut base = n.clone();
    base.text_runs.clear();
    let mut styles = vec![base];
    let mut style_of = vec![0usize; len];
    for r in &n.text_runs {
        styles.push(run_style(n, r));
        let k = styles.len() - 1;
        for i in r.start.min(len)..r.end.min(len) {
            style_of[i] = k;
        }
    }
    // Width of chars a..b, piece by piece.
    let mut width = |a: usize, b: usize, measure: &mut dyn FnMut(&Node, &str) -> f64| -> f64 {
        let mut w = 0.0;
        let mut i = a;
        while i < b {
            let k = style_of[i];
            let mut j = i;
            while j < b && style_of[j] == k {
                j += 1;
            }
            w += measure(&styles[k], &chars[i..j].iter().collect::<String>());
            i = j;
        }
        w
    };
    let trim_end = |a: usize, mut b: usize| {
        while b > a && chars[b - 1] == ' ' {
            b -= 1;
        }
        b
    };
    let fixed_w = n.text_auto_resize != TextAutoResize::WidthAndHeight;
    let max_w = n.width + 0.01;
    let wrap = fixed_w && n.text_wrap;
    // Lines as char ranges.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut p0 = 0;
    while p0 <= len {
        let p1 = (p0..len).find(|&i| chars[i] == '\n').unwrap_or(len);
        if !wrap || p0 == p1 {
            ranges.push((p0, p1));
        } else {
            // Words keep their trailing spaces.
            let mut words: Vec<(usize, usize)> = Vec::new();
            let mut i = p0;
            while i < p1 {
                let start = i;
                while i < p1 && chars[i] != ' ' {
                    i += 1;
                }
                while i < p1 && chars[i] == ' ' {
                    i += 1;
                }
                words.push((start, i));
            }
            let mut line: Option<(usize, usize)> = None;
            for (w0, w1) in words {
                match line {
                    None => {
                        if width(w0, trim_end(w0, w1), measure) > max_w {
                            // An over-long word: split it by character.
                            let mut ls = w0;
                            for c in w0..w1 {
                                if c > ls && width(ls, c + 1, measure) > max_w {
                                    ranges.push((ls, c));
                                    ls = c;
                                }
                            }
                            line = Some((ls, w1));
                        } else {
                            line = Some((w0, w1));
                        }
                    }
                    Some((ls, _)) if width(ls, trim_end(ls, w1), measure) <= max_w => line = Some((ls, w1)),
                    Some((ls, le)) => {
                        ranges.push((ls, trim_end(ls, le)));
                        line = Some((w0, w1));
                    }
                }
            }
            if let Some((ls, le)) = line {
                ranges.push((ls, trim_end(ls, le)));
            }
        }
        if p1 >= len {
            break;
        }
        p0 = p1 + 1;
    }
    // Overflow modes.
    let mark = match n.text_overflow {
        TextOverflow::Ellipsis => Some("…"),
        TextOverflow::Truncate => Some(""),
        TextOverflow::Visible | TextOverflow::Clip => None,
    };
    let mut cut_last = false;
    if mark.is_some() && n.text_auto_resize == TextAutoResize::None {
        let fit = (((n.height + 0.01) / n.effective_line_height()).floor() as usize).max(1);
        if ranges.len() > fit {
            ranges.truncate(fit);
            cut_last = true;
        }
    }
    let last = ranges.len().saturating_sub(1);
    let mut out = Vec::new();
    for (li, (a, mut b)) in ranges.into_iter().enumerate() {
        let mut tail = None;
        if let Some(mark) = mark {
            let too_wide = fixed_w && width(a, b, measure) > max_w;
            if too_wide || (li == last && cut_last) {
                let k = style_of.get(b.saturating_sub(1).max(a)).copied().unwrap_or(0);
                let mw = if mark.is_empty() { 0.0 } else { measure(&styles[k], mark) };
                while b > a && fixed_w && width(a, trim_end(a, b), measure) + mw > max_w {
                    b -= 1;
                }
                b = trim_end(a, b);
                if !mark.is_empty() {
                    tail = Some(Piece { text: mark.to_string(), style: styles[k].clone(), width: mw });
                }
            }
        }
        let mut pieces = Vec::new();
        let mut i = a;
        while i < b {
            let k = style_of[i];
            let mut j = i;
            while j < b && style_of[j] == k {
                j += 1;
            }
            let text: String = chars[i..j].iter().collect();
            let w = measure(&styles[k], &text);
            pieces.push(Piece { text, style: styles[k].clone(), width: w });
            i = j;
        }
        pieces.extend(tail);
        let width = pieces.iter().map(|p| p.width).sum();
        out.push(RichLine { pieces, width });
    }
    out
}

/// Runs with bold, italic or underline toggled for the characters
/// `start..end`: on, unless all of them already have it. Adjacent characters
/// with the same style merge into one run.
pub fn toggle_runs(n: &Node, start: usize, end: usize, what: &str) -> Vec<crate::paint::TextRun> {
    use crate::paint::TextRun;
    let len = n.characters.chars().count();
    let (start, end) = (start.min(len), end.min(len));
    // Each character's overrides (a run with no range).
    fn keep<T: Clone>(a: &mut Option<T>, b: &Option<T>) {
        if b.is_some() {
            *a = b.clone();
        }
    }
    let mut per: Vec<TextRun> = vec![TextRun::default(); len];
    for r in &n.text_runs {
        for c in per.iter_mut().take(r.end.min(len)).skip(r.start.min(len)) {
            keep(&mut c.font_family, &r.font_family);
            keep(&mut c.font_weight, &r.font_weight);
            keep(&mut c.italic, &r.italic);
            keep(&mut c.letter_spacing, &r.letter_spacing);
            keep(&mut c.fills, &r.fills);
            keep(&mut c.text_decoration, &r.text_decoration);
            keep(&mut c.link, &r.link);
        }
    }
    let has = |c: &TextRun| match what {
        "bold" => c.font_weight.unwrap_or(n.font_weight) >= 600,
        "italic" => c.italic.unwrap_or(n.italic),
        _ => c.text_decoration.as_deref().map(|d| d.eq_ignore_ascii_case("UNDERLINE")).unwrap_or(n.text_decoration.as_deref() == Some("UNDERLINE")),
    };
    let on = !(start..end).all(|i| has(&per[i]));
    for c in per.iter_mut().take(end).skip(start) {
        match what {
            "bold" => c.font_weight = Some(if on { 700 } else { 400 }).filter(|w| (*w >= 600) != (n.font_weight >= 600)),
            "italic" => c.italic = Some(on).filter(|i| *i != n.italic),
            _ => {
                let base = n.text_decoration.as_deref() == Some("UNDERLINE");
                c.text_decoration = (on != base).then(|| if on { "UNDERLINE".to_string() } else { "NONE".to_string() });
            }
        }
    }
    let mut out: Vec<TextRun> = Vec::new();
    for (i, c) in per.into_iter().enumerate() {
        if c == TextRun::default() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == i && TextRun { start: 0, end: 0, ..last.clone() } == c => last.end = i + 1,
            _ => out.push(TextRun { start: i, end: i + 1, ..c }),
        }
    }
    out
}

/// When `props` changes a rich text's characters without giving runs, add
/// its runs adjusted to the edit (so undo restores them too).
pub fn keep_runs(n: &Node, props: &mut serde_json::Map<String, serde_json::Value>) {
    if n.text_runs.is_empty() || props.contains_key("textRuns") {
        return;
    }
    if let Some(new) = props.get("characters").and_then(serde_json::Value::as_str) {
        let runs = adjust_runs(&n.text_runs, &n.characters, new);
        props.insert("textRuns".into(), serde_json::to_value(runs).unwrap_or_default());
    }
}

/// Runs after the text changed from `old` to `new` (an edit): ranges after
/// the change shift, ranges inside it shrink or grow with it.
pub fn adjust_runs(runs: &[crate::paint::TextRun], old: &str, new: &str) -> Vec<crate::paint::TextRun> {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (old_end, new_end) = (a.len() - suf, b.len() - suf);
    // Typing just before a run doesn't take its style; typing at its end
    // continues it, like most editors.
    let map = |i: usize, is_end: bool| -> usize {
        if i < pre {
            i
        } else if i >= old_end {
            i - old_end + new_end
        } else if is_end {
            new_end
        } else {
            pre
        }
    };
    runs.iter()
        .filter_map(|r| {
            let (s, e) = (map(r.start, false), map(r.end, true));
            (e > s).then(|| crate::paint::TextRun { start: s, end: e, ..r.clone() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_text_wraps_by_piece() {
        // 1 unit per char, 2 for bold.
        let mut n = Node { characters: "ab cd ef".into(), width: 8.0, font_size: 10.0, ..Node::default() };
        n.text_auto_resize = TextAutoResize::Height;
        n.text_runs = vec![crate::paint::TextRun { start: 3, end: 5, font_weight: Some(700), ..Default::default() }];
        let mut m = |st: &Node, s: &str| s.chars().count() as f64 * if st.font_weight >= 700 { 2.0 } else { 1.0 };
        let lines = rich_layout(&n, &mut m);
        let texts: Vec<String> = lines.iter().map(RichLine::text).collect();
        assert_eq!(texts, vec!["ab cd", "ef"], "ab + space + bold cd = 7 fits 8");
        assert_eq!(lines[0].pieces.iter().map(|p| (p.text.as_str(), p.style.font_weight)).collect::<Vec<_>>(), vec![("ab ", 400), ("cd", 700)]);
        assert_eq!(lines[0].width, 7.0);
    }

    #[test]
    fn toggling_styles() {
        let n = Node { characters: "Hello world".into(), font_weight: 400, ..Node::default() };
        let r = toggle_runs(&n, 6, 11, "bold");
        assert_eq!((r.len(), r[0].start, r[0].end, r[0].font_weight), (1, 6, 11, Some(700)));
        let n2 = Node { text_runs: r, ..n.clone() };
        // Toggling again turns it off: no runs left.
        assert!(toggle_runs(&n2, 6, 11, "bold").is_empty());
        // Part of it: the rest stays bold.
        let r = toggle_runs(&n2, 6, 8, "bold");
        assert_eq!(r.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>(), vec![(8, 11)]);
    }

    #[test]
    fn runs_follow_edits() {
        let runs = vec![crate::paint::TextRun { start: 6, end: 11, font_weight: Some(700), ..Default::default() }];
        // "Hello world" → "Hello big world": the bold word moves.
        let r = adjust_runs(&runs, "Hello world", "Hello big world");
        assert_eq!((r[0].start, r[0].end), (10, 15));
        // Typing inside the bold word keeps it bold.
        let r = adjust_runs(&runs, "Hello world", "Hello worlds");
        assert_eq!((r[0].start, r[0].end), (6, 12));
        // Deleting it drops the run.
        assert!(adjust_runs(&runs, "Hello world", "Hello ").is_empty());
    }

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
