//! Fractional indexing for sibling order. A key is a base-62 fraction (digits
//! `0-9A-Za-z`, compared as plain strings) that never ends in `0`, so a key can
//! always be generated between any two keys. Concurrent inserts at the same spot
//! may produce equal keys; ties are broken by node id.

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: usize = 62;

fn val(c: u8) -> usize {
    match c {
        b'0'..=b'9' => (c - b'0') as usize,
        b'A'..=b'Z' => (c - b'A') as usize + 10,
        b'a'..=b'z' => (c - b'a') as usize + 36,
        _ => 0,
    }
}

pub fn is_valid_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|c| c.is_ascii_alphanumeric()) && !key.ends_with('0')
}

/// A key strictly between `a` and `b` (`None` = open end). Invalid input keys are
/// treated as open ends.
pub fn key_between(a: Option<&str>, b: Option<&str>) -> String {
    let a = a.filter(|k| is_valid_key(k)).unwrap_or("");
    let b = b.filter(|k| is_valid_key(k));
    let b = match b {
        Some(b) if a < b => Some(b),
        Some(_) => None, // out of order: just append after `a`
        None => None,
    };
    let out = midpoint(a.as_bytes(), b.map(str::as_bytes));
    String::from_utf8(out).unwrap_or_else(|_| "V".into())
}

/// `n` increasing keys between `a` and `b`.
pub fn keys_between(a: Option<&str>, b: Option<&str>, n: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }
    if b.is_none() {
        let mut prev = a.map(str::to_string);
        for _ in 0..n {
            let k = key_between(prev.as_deref(), None);
            prev = Some(k.clone());
            out.push(k);
        }
        return out;
    }
    // Split recursively so keys stay short and balanced.
    fn fill(a: Option<&str>, b: Option<&str>, n: usize, out: &mut Vec<String>) {
        if n == 0 {
            return;
        }
        let mid = key_between(a, b);
        let left = n / 2;
        fill(a, Some(&mid), left, out);
        out.push(mid.clone());
        fill(Some(&mid), b, n - left - 1, out);
    }
    fill(a, b, n, &mut out);
    out
}

fn midpoint(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    if let Some(b) = b {
        // Strip the common prefix (treating `a` as zero-padded).
        let mut n = 0;
        while n < b.len() && a.get(n).copied().unwrap_or(b'0') == b[n] {
            n += 1;
        }
        if n > 0 {
            let mut out = b[..n].to_vec();
            let rest_a = if n < a.len() { &a[n..] } else { &[][..] };
            out.extend(midpoint(rest_a, Some(&b[n..])));
            return out;
        }
    }
    let da = a.first().map(|&c| val(c)).unwrap_or(0);
    let db = b.and_then(|b| b.first()).map(|&c| val(c)).unwrap_or(BASE);
    if db - da > 1 {
        let mid = (da + db + 1) / 2;
        return vec![DIGITS[mid]];
    }
    if let Some(b) = b {
        if b.len() > 1 {
            return vec![b[0]];
        }
    }
    let mut out = vec![DIGITS[da]];
    let rest_a = if a.len() > 1 { &a[1..] } else { &[][..] };
    out.extend(midpoint(rest_a, None));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_ordering() {
        let a = key_between(None, None);
        let b = key_between(Some(&a), None);
        let c = key_between(Some(&a), Some(&b));
        let d = key_between(None, Some(&a));
        assert!(d < a && a < c && c < b, "{d} {a} {c} {b}");
        for k in [&a, &b, &c, &d] {
            assert!(is_valid_key(k), "{k}");
        }
    }

    #[test]
    fn many_inserts_between() {
        let mut lo = key_between(None, None);
        let hi = key_between(Some(&lo), None);
        for _ in 0..500 {
            let k = key_between(Some(&lo), Some(&hi));
            assert!(lo < k && k < hi);
            assert!(is_valid_key(&k));
            lo = k;
        }
        let mut hi2 = hi.clone();
        let lo2 = key_between(None, None);
        for _ in 0..500 {
            let k = key_between(Some(&lo2), Some(&hi2));
            assert!(lo2 < k && k < hi2, "{lo2} {k} {hi2}");
            hi2 = k;
        }
    }

    #[test]
    fn prepend_repeatedly() {
        let mut first = key_between(None, None);
        for _ in 0..300 {
            let k = key_between(None, Some(&first));
            assert!(k < first && is_valid_key(&k), "{k} {first}");
            first = k;
        }
    }

    #[test]
    fn bulk() {
        let keys = keys_between(Some("a"), Some("b"), 50);
        assert_eq!(keys.len(), 50);
        for w in keys.windows(2) {
            assert!(w[0] < w[1]);
        }
        assert!(keys[0].as_str() > "a" && keys[49].as_str() < "b");
        let tail = keys_between(Some("a"), None, 20);
        for w in tail.windows(2) {
            assert!(w[0] < w[1]);
        }
    }
}
