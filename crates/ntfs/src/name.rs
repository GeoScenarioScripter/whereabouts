//! Lossless file names.
//!
//! NTFS names are arbitrary sequences of UTF-16 code units: they are usually valid UTF-16 but may
//! contain unpaired surrogates. Converting them to `String` would silently replace those units,
//! and two distinct files could end up with the same name. [`NtfsName`] stores the name as
//! **WTF-8** (<https://simonsapin.github.io/wtf-8/>): byte-identical to UTF-8 for every valid
//! name, with each unpaired surrogate encoded as its own 3-byte sequence. It is:
//!
//! - lossless: [`NtfsName::to_utf16`] returns exactly the code units the file system gave us;
//! - compact: ASCII names take one byte per character (half the size of UTF-16);
//! - cross-platform: usable (and testable) on Linux, unlike `OsString::from_wide`;
//! - the same bytes Rust's own `OsStr::as_encoded_bytes` uses on Windows (verified by a test).

use std::borrow::Cow;
use std::fmt;

/// A file name exactly as stored on the volume, held as WTF-8 bytes.
///
/// Every valid UTF-16 name is valid UTF-8 here, so [`NtfsName::as_str`] succeeds for all but the
/// rare names containing unpaired surrogates.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct NtfsName(Box<[u8]>);

impl NtfsName {
    /// Encodes UTF-16 code units as WTF-8. Never fails: unpaired surrogates are preserved.
    #[must_use]
    pub fn from_utf16<I: IntoIterator<Item = u16>>(units: I) -> Self {
        let iter = units.into_iter();
        let mut out = Vec::with_capacity(iter.size_hint().0);
        for decoded in char::decode_utf16(iter) {
            match decoded {
                Ok(c) => {
                    let mut tmp = [0_u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
                }
                Err(e) => {
                    // Generalised UTF-8 three-byte form of a surrogate (U+D800..=U+DFFF).
                    let u = e.unpaired_surrogate();
                    out.push(0xE0 | (u >> 12) as u8);
                    out.push(0x80 | ((u >> 6) & 0x3F) as u8);
                    out.push(0x80 | (u & 0x3F) as u8);
                }
            }
        }
        Self(out.into_boxed_slice())
    }

    /// Encodes a little-endian UTF-16 byte string (as found in USN records). A trailing odd
    /// byte, which the record parser never produces, is ignored.
    #[must_use]
    pub fn from_utf16le_bytes(bytes: &[u8]) -> Self {
        Self::from_utf16(utf16le_units(bytes))
    }

    /// The WTF-8 bytes. Equal to the UTF-8 encoding whenever the name is valid Unicode.
    #[must_use]
    pub fn as_wtf8(&self) -> &[u8] {
        &self.0
    }

    /// The name as `&str`, or `None` if it contains unpaired surrogates.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// The name as a string, replacing each unpaired surrogate with U+FFFD (the same result as
    /// `OsStr::to_string_lossy` on Windows).
    #[must_use]
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        match self.as_str() {
            Some(s) => Cow::Borrowed(s),
            None => Cow::Owned(String::from_utf16_lossy(&self.to_utf16())),
        }
    }

    /// Decodes back to the exact UTF-16 code units the name was built from.
    #[must_use]
    pub fn to_utf16(&self) -> Vec<u16> {
        let bytes = &self.0;
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let b0 = bytes[i];
            let (cp, len) = if b0 < 0x80 {
                (u32::from(b0), 1)
            } else if b0 & 0xE0 == 0xC0 {
                (u32::from(b0 & 0x1F), 2)
            } else if b0 & 0xF0 == 0xE0 {
                (u32::from(b0 & 0x0F), 3)
            } else {
                (u32::from(b0 & 0x07), 4)
            };
            // The constructor guarantees well-formed sequences; stay panic-free regardless.
            let Some(tail) = bytes.get(i + 1..i + len) else {
                out.push(0xFFFD);
                break;
            };
            let cp = tail
                .iter()
                .fold(cp, |acc, &b| (acc << 6) | u32::from(b & 0x3F));
            if let Ok(c) = u16::try_from(cp) {
                out.push(c);
            } else {
                let v = cp - 0x1_0000;
                out.push(0xD800 | (v >> 10) as u16);
                out.push(0xDC00 | (v & 0x3FF) as u16);
            }
            i += len;
        }
        out
    }

    /// Length of the WTF-8 encoding in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` for the empty name.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Converts to an `OsString` without loss.
    #[cfg(windows)]
    #[must_use]
    pub fn to_os_string(&self) -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&self.to_utf16())
    }
}

/// The single UTF-16LE decoder of the crate: code units of a little-endian byte string (a
/// trailing odd byte is ignored).
pub(crate) fn utf16le_units(bytes: &[u8]) -> impl Iterator<Item = u16> + '_ {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
}

impl fmt::Debug for NtfsName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.to_string_lossy(), f)
    }
}

impl fmt::Display for NtfsName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl From<&str> for NtfsName {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn valid_names_are_plain_utf8() {
        for s in [
            "",
            "a.shp",
            "Größe.tif",
            "東京都.gpkg",
            "🗺️ map.geojson",
            "a b\u{7f}",
        ] {
            let n = NtfsName::from_utf16(units(s));
            assert_eq!(n.as_str(), Some(s));
            assert_eq!(n.as_wtf8(), s.as_bytes());
            assert_eq!(n.to_utf16(), units(s));
            assert_eq!(n, NtfsName::from(s));
        }
    }

    #[test]
    fn unpaired_surrogates_round_trip() {
        let cases: &[&[u16]] = &[
            &[0xD800],
            &[0xDFFF],
            &[0x61, 0xD83D, 0x62],     // lone high surrogate between ASCII
            &[0xDE00, 0xD83D],         // trail before lead: two lone surrogates
            &[0xD83D, 0xDE00, 0xDC00], // a valid pair then a lone trail
            &[0xD83D, 0xD83D, 0xDE00], // lone lead then a valid pair
            &[0x4E2D, 0xDBFF, 0x002E, 0x74], // CJK, lone lead, ".t"
        ];
        for &case in cases {
            let n = NtfsName::from_utf16(case.iter().copied());
            assert_eq!(n.to_utf16(), case, "round trip of {case:x?}");
            assert!(n.as_str().is_none(), "{case:x?} must not be valid UTF-8");
            // Lossy form matches std's UTF-16 lossy decoding.
            assert_eq!(n.to_string_lossy(), String::from_utf16_lossy(case));
        }
    }

    #[test]
    fn distinct_names_stay_distinct() {
        // Two names that differ only in an unpaired surrogate must not collide.
        let a = NtfsName::from_utf16([0x61, 0xD800]);
        let b = NtfsName::from_utf16([0x61, 0xD801]);
        assert_ne!(a, b);
        assert_eq!(a.to_string_lossy(), b.to_string_lossy());
    }

    #[test]
    fn le_bytes_constructor() {
        let n = NtfsName::from_utf16le_bytes(&[0x61, 0x00, 0x2E, 0x00, 0x74, 0x00, 0xFF]);
        assert_eq!(n.as_str(), Some("a.t"));
    }

    #[test]
    fn exhaustive_single_and_pair_units_round_trip() {
        for u in 0..=u16::MAX {
            let n = NtfsName::from_utf16([u]);
            assert_eq!(n.to_utf16(), [u]);
        }
        for hi in (0xD800..=0xDBFF).step_by(37) {
            for lo in (0xDC00..=0xDFFF).step_by(41) {
                let n = NtfsName::from_utf16([hi, lo]);
                assert_eq!(n.to_utf16(), [hi, lo]);
                assert_eq!(n.len(), 4);
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn matches_std_wtf8_on_windows() {
        use std::os::windows::ffi::OsStringExt;
        let cases: &[&[u16]] = &[&[0x61, 0xD800, 0x62], &[0xD83D, 0xDE00], &[0x4E2D, 0xDC00]];
        for &case in cases {
            let os = std::ffi::OsString::from_wide(case);
            let n = NtfsName::from_utf16(case.iter().copied());
            assert_eq!(n.as_wtf8(), os.as_encoded_bytes());
            assert_eq!(n.to_os_string(), os);
        }
    }
}
