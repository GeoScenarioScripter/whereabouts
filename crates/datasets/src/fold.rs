//! Case-insensitive name keys.
//!
//! NTFS compares names through its upcase table: a 1:1 mapping of UTF-16 code units to upper
//! case. [`fold`] approximates it with Unicode's simple upper-case mapping, applied only where a
//! character maps to exactly one character (`ß` stays `ß`, as on NTFS), so the folded key never
//! merges names that NTFS keeps apart in the common cases. Names that are not valid Unicode
//! (WTF-8 with lone surrogates) fold their ASCII letters only, which is lossless.
//!
//! Every extension the grouper matches is ASCII, and ASCII folding preserves byte lengths, so a
//! suffix of `n` bytes in a key is also the last `n` bytes of the original name.

/// The case-folded key of a WTF-8 name (upper case).
pub(crate) fn fold(name: &[u8]) -> Vec<u8> {
    if name.is_ascii() {
        return name.to_ascii_uppercase();
    }
    match std::str::from_utf8(name) {
        Ok(s) => {
            let mut out = String::with_capacity(s.len());
            for c in s.chars() {
                let mut upper = c.to_uppercase();
                match (upper.next(), upper.next()) {
                    (Some(u), None) => out.push(u),
                    _ => out.push(c),
                }
            }
            out.into_bytes()
        }
        Err(_) => name.to_ascii_uppercase(),
    }
}

/// The folded extension of a folded key: the bytes after the last `.`, if any.
pub(crate) fn extension(key: &[u8]) -> Option<&[u8]> {
    let dot = key.iter().rposition(|&b| b == b'.')?;
    key.get(dot + 1..)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_unicode_folding() {
        assert_eq!(fold(b"Roads.Shp"), b"ROADS.SHP");
        assert_eq!(fold("Ärger.shp".as_bytes()), "ÄRGER.SHP".as_bytes());
        // ß upper-cases to two characters: kept as is, like the NTFS upcase table.
        assert_eq!(fold("straße.shp".as_bytes()), "STRAßE.SHP".as_bytes());
        // A lone surrogate (WTF-8 ED A0 80) survives; ASCII around it folds.
        let wtf8 = [b'a', 0xED, 0xA0, 0x80, b'.', b's', b'h', b'p'];
        assert_eq!(
            fold(&wtf8),
            [b'A', 0xED, 0xA0, 0x80, b'.', b'S', b'H', b'P']
        );
    }

    #[test]
    fn extensions() {
        assert_eq!(extension(b"A.B.SHP"), Some(&b"SHP"[..]));
        assert_eq!(extension(b"GDB"), None);
        assert_eq!(extension(b"A."), Some(&b""[..]));
        assert_eq!(extension(b".SHP"), Some(&b"SHP"[..]));
    }
}
