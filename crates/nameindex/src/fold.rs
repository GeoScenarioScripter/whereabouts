//! Text primitives: case folding, WTF-8 decoding/encoding, wildcard matching.
//!
//! Every function here takes untrusted bytes (names come from the file system, patterns from the
//! user) and never panics: malformed sequences are copied or skipped, never indexed blindly.

/// Folds one code point for case-insensitive comparison.
///
/// The mapping is 1:1 on code points, so `?` in a wildcard always stands for one character of the
/// original name: `lower(upper(c))` when both steps map to a single character (this merges e.g.
/// `ς`/`σ`/`Σ` and `ẞ`/`ß`), else `lower(c)` when that is a single character, else `c` itself
/// (`ß` stays `ß`, `İ` stays `İ`). ASCII takes a fast path.
#[must_use]
pub fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    if let Some(u) = single(c.to_uppercase())
        && let Some(l) = single(u.to_lowercase())
    {
        return l;
    }
    single(c.to_lowercase()).unwrap_or(c)
}

fn single(mut it: impl Iterator<Item = char>) -> Option<char> {
    let first = it.next()?;
    it.next().is_none().then_some(first)
}

/// Folds a string with [`fold_char`] (the same folding the index applies to names and patterns).
#[must_use]
pub fn fold_str(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

/// Appends the folded form of WTF-8 `src` to `out`. Unpaired surrogates are copied unchanged;
/// ill-formed bytes (never present in validated names) are copied byte by byte.
pub(crate) fn fold_wtf8_into(src: &[u8], out: &mut Vec<u8>) {
    let mut rest = src;
    while let Some((&b, tail)) = rest.split_first() {
        if b.is_ascii() {
            out.push(b.to_ascii_lowercase());
            rest = tail;
            continue;
        }
        match decode_one(rest) {
            Some((cp, len)) => {
                match char::from_u32(cp) {
                    Some(c) => push_char(out, fold_char(c)),
                    None => out.extend_from_slice(rest.get(..len).unwrap_or_default()),
                }
                rest = rest.get(len..).unwrap_or_default();
            }
            None => {
                out.push(b);
                rest = tail;
            }
        }
    }
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0_u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Appends the WTF-8 form of little-endian UTF-16 `src` to `out` (unpaired surrogates kept, as
/// `FileName::from_utf16` does). Returns `false`, appending nothing, if `src` has an odd length.
pub(crate) fn utf16le_to_wtf8(src: &[u8], out: &mut Vec<u8>) -> bool {
    if !src.len().is_multiple_of(2) {
        return false;
    }
    out.reserve(src.len() / 2);
    let units = src.chunks_exact(2).map(|c| {
        u16::from_le_bytes([
            c.first().copied().unwrap_or(0),
            c.get(1).copied().unwrap_or(0),
        ])
    });
    for unit in char::decode_utf16(units) {
        match unit {
            Ok(c) if c.is_ascii() => out.push(c as u8),
            Ok(c) => push_char(out, c),
            Err(e) => {
                let s = e.unpaired_surrogate();
                out.extend_from_slice(&[
                    0xE0 | (s >> 12) as u8,
                    0x80 | ((s >> 6) & 0x3F) as u8,
                    0x80 | (s & 0x3F) as u8,
                ]);
            }
        }
    }
    true
}

/// Decodes one generalised-UTF-8 sequence (surrogate code points allowed) at the start of `b`:
/// `(code point, byte length)`, or `None` for an ill-formed or truncated sequence.
pub(crate) fn decode_one(b: &[u8]) -> Option<(u32, usize)> {
    let cont = |i: usize| {
        b.get(i)
            .copied()
            .filter(|x| x & 0xC0 == 0x80)
            .map(|x| u32::from(x & 0x3F))
    };
    let b0 = *b.first()?;
    match b0 {
        0x00..=0x7F => Some((u32::from(b0), 1)),
        0xC2..=0xDF => Some(((u32::from(b0 & 0x1F) << 6) | cont(1)?, 2)),
        0xE0..=0xEF => {
            let b1 = cont(1)?;
            if b0 == 0xE0 && b1 < 0x20 {
                return None;
            }
            Some(((u32::from(b0 & 0x0F) << 12) | (b1 << 6) | cont(2)?, 3))
        }
        0xF0..=0xF4 => {
            let b1 = cont(1)?;
            if (b0 == 0xF0 && b1 < 0x10) || (b0 == 0xF4 && b1 > 0x0F) {
                return None;
            }
            let cp = (u32::from(b0 & 0x07) << 18) | (b1 << 12) | (cont(2)? << 6) | cont(3)?;
            Some((cp, 4))
        }
        _ => None,
    }
}

/// `true` if `b` is well-formed WTF-8 (the encoding of `whereabouts_core::FileName`): generalised
/// UTF-8 in which a lead surrogate is never directly followed by a trail surrogate.
pub(crate) fn is_wtf8(b: &[u8]) -> bool {
    if std::str::from_utf8(b).is_ok() {
        return true;
    }
    let mut rest = b;
    let mut prev_lead = false;
    while !rest.is_empty() {
        let Some((cp, len)) = decode_one(rest) else {
            return false;
        };
        if prev_lead && (0xDC00..=0xDFFF).contains(&cp) {
            return false;
        }
        prev_lead = (0xD800..=0xDBFF).contains(&cp);
        rest = rest.get(len..).unwrap_or_default();
    }
    true
}

/// Byte length of the (generalised) UTF-8 sequence that starts with `lead`; 1 for stray bytes.
fn char_len(lead: u8) -> usize {
    match lead {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// Anchored wildcard match of folded `text` against folded `pat`: `*` matches any run of
/// characters (including none, and including `\` in path mode), `?` exactly one character, every
/// other byte itself. Linear-space greedy matcher with single-star backtracking (O(n·m) worst
/// case, no recursion).
pub(crate) fn wild_match(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0_usize, 0_usize);
    // (pattern index after the last `*`, text index where that star's run currently ends)
    let mut star: Option<(usize, usize)> = None;
    loop {
        let pc = pat.get(p).copied();
        if pc == Some(b'*') {
            p += 1;
            star = Some((p, t));
            continue;
        }
        let Some(&tc) = text.get(t) else {
            return pat.get(p..).unwrap_or_default().iter().all(|&c| c == b'*');
        };
        let step = match pc {
            Some(b'?') => Some(char_len(tc).min(text.len() - t)),
            Some(c) if c == tc => Some(1),
            _ => None,
        };
        if let Some(step) = step {
            p += 1;
            t += step;
            continue;
        }
        match star {
            Some((sp, st)) => {
                let Some(&lead) = text.get(st) else {
                    return false;
                };
                let next = st + char_len(lead).min(text.len() - st);
                star = Some((sp, next));
                p = sp;
                t = next;
            }
            None => return false,
        }
    }
}

/// The longest run of literal bytes in a wildcard pattern (used as a pre-filter).
pub(crate) fn longest_literal(pat: &[u8]) -> &[u8] {
    pat.split(|&c| c == b'*' || c == b'?')
        .max_by_key(|s| s.len())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        fold_wtf8_into(s.as_bytes(), &mut out);
        out
    }

    #[test]
    fn folding_examples() {
        assert_eq!(fold("Roads_2019.SHP"), b"roads_2019.shp");
        assert_eq!(fold("ΣΊΣΥΦΟΣ ς"), "σίσυφοσ σ".as_bytes());
        assert_eq!(fold("Straße ẞ"), "straße ß".as_bytes());
        assert_eq!(fold("İstanbul"), "İstanbul".as_bytes());
        assert_eq!(fold("\u{212A}elvin"), b"kelvin");
        assert_eq!(fold_str("ÉCOLE"), "école");
    }

    #[test]
    fn folding_is_idempotent_and_one_to_one() {
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let f = fold_char(c);
            assert_eq!(fold_char(f), f, "U+{:04X}", c as u32);
        }
    }

    #[test]
    fn surrogates_survive_folding() {
        let mut name = Vec::new();
        assert!(utf16le_to_wtf8(&[0x41, 0, 0x00, 0xD8, 0x42, 0], &mut name));
        assert_eq!(name, [0x41, 0xED, 0xA0, 0x80, 0x42]);
        assert!(is_wtf8(&name));
        let mut folded = Vec::new();
        fold_wtf8_into(&name, &mut folded);
        assert_eq!(folded, [0x61, 0xED, 0xA0, 0x80, 0x62]);
    }

    #[test]
    fn utf16_conversion_matches_core() {
        let s = "Ōtautahi 🗺 données.tif";
        let bytes: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut out = Vec::new();
        assert!(utf16le_to_wtf8(&bytes, &mut out));
        assert_eq!(out, s.as_bytes());
        assert!(!utf16le_to_wtf8(&[0x41], &mut out));
    }

    #[test]
    fn wtf8_validation() {
        assert!(is_wtf8(b"plain"));
        assert!(is_wtf8(&[0xED, 0xB0, 0x80, 0xED, 0xA0, 0x80]));
        assert!(!is_wtf8(&[0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]));
        assert!(!is_wtf8(&[0xFF]));
        assert!(!is_wtf8(&[0xE2, 0x82]));
    }

    #[test]
    fn wildcards() {
        let m = |p: &str, t: &str| wild_match(p.as_bytes(), t.as_bytes());
        assert!(m("*.shp", "roads.shp"));
        assert!(!m("*.shp", "roads.shp.xml"));
        assert!(m("r*d?.tif", "roads.tif"));
        assert!(m("r*d?.tif", "rd1.tif"));
        assert!(!m("r*d?.tif", "rd.tif"));
        assert!(m("?", "é"));
        assert!(!m("??", "é"));
        assert!(m("*é*", "café"));
        assert!(m("", ""));
        assert!(!m("", "a"));
        assert!(m("***", ""));
        assert!(m("a*b*c", "aXbYbZc"));
        assert!(!m("a*b*c", "aXbYbZ"));
        // `?` never splits a multi-byte character, even after a star.
        assert!(m("*?", "日本"));
        assert!(!m("*?x", "日本"));
        // Malformed text never panics.
        assert!(!wild_match(b"?a", &[0xE0]));
        assert!(wild_match(b"*", &[0xF0, 0x80]));
    }

    #[test]
    fn literal_prefilter() {
        assert_eq!(longest_literal(b"*ro?_*2019*"), b"2019");
        assert_eq!(longest_literal(b"r*roads*x"), b"roads");
        assert_eq!(longest_literal(b"*?*"), b"");
    }
}
