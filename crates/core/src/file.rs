//! File-level records as produced by a `Source`: identity, parent, name, attributes.

use std::borrow::Cow;
use std::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{CoreError, Frn};

/// One directory entry as seen by a `Source` (enumeration or change event).
///
/// The name index stores exactly this per entry; full paths are rebuilt on demand by walking
/// `parent` links to the source's root (whose `parent` is itself on NTFS).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileRecord {
    /// The entry's own file reference number.
    pub frn: Frn,
    /// FRN of the containing directory.
    pub parent: Frn,
    /// The entry's name within `parent` (a single path component, lossless).
    pub name: FileName,
    /// Raw file attribute bits; the single source of truth for "is a directory".
    pub attrs: FileAttributes,
}

impl FileRecord {
    /// `true` for directories (`FileAttributes::DIRECTORY` is set).
    pub const fn is_dir(&self) -> bool {
        self.attrs.contains(FileAttributes::DIRECTORY)
    }
}

/// Windows file attribute bits (`FILE_ATTRIBUTE_*`), kept as the raw `u32`.
///
/// The constants are plain numbers, so this type is portable; sources on other platforms map
/// their equivalents (hidden, read-only, directory, …) onto these bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileAttributes(pub u32);

impl FileAttributes {
    /// `FILE_ATTRIBUTE_READONLY`
    pub const READONLY: u32 = 0x0000_0001;
    /// `FILE_ATTRIBUTE_HIDDEN`
    pub const HIDDEN: u32 = 0x0000_0002;
    /// `FILE_ATTRIBUTE_SYSTEM`
    pub const SYSTEM: u32 = 0x0000_0004;
    /// `FILE_ATTRIBUTE_DIRECTORY`
    pub const DIRECTORY: u32 = 0x0000_0010;
    /// `FILE_ATTRIBUTE_ARCHIVE`
    pub const ARCHIVE: u32 = 0x0000_0020;
    /// `FILE_ATTRIBUTE_NORMAL`
    pub const NORMAL: u32 = 0x0000_0080;
    /// `FILE_ATTRIBUTE_TEMPORARY`
    pub const TEMPORARY: u32 = 0x0000_0100;
    /// `FILE_ATTRIBUTE_SPARSE_FILE`
    pub const SPARSE_FILE: u32 = 0x0000_0200;
    /// `FILE_ATTRIBUTE_REPARSE_POINT` (symlinks, junctions, cloud placeholders).
    pub const REPARSE_POINT: u32 = 0x0000_0400;
    /// `FILE_ATTRIBUTE_COMPRESSED`
    pub const COMPRESSED: u32 = 0x0000_0800;
    /// `FILE_ATTRIBUTE_OFFLINE`
    pub const OFFLINE: u32 = 0x0000_1000;
    /// `FILE_ATTRIBUTE_NOT_CONTENT_INDEXED`
    pub const NOT_CONTENT_INDEXED: u32 = 0x0000_2000;
    /// `FILE_ATTRIBUTE_ENCRYPTED`
    pub const ENCRYPTED: u32 = 0x0000_4000;

    /// `true` if every bit of `mask` is set.
    pub const fn contains(self, mask: u32) -> bool {
        self.0 & mask == mask
    }
}

/// A single path component, stored losslessly as WTF-8.
///
/// NTFS names are UTF-16 and may contain unpaired surrogates, which `String` cannot hold; dropping
/// them would make such files impossible to open by path. WTF-8 is UTF-8 extended with encoded
/// lone surrogates: identical to UTF-8 for every valid name (> 99.99 % in practice), so
/// [`FileName::as_str`] is a zero-cost borrow in the common case, and the bytes can go straight
/// into the name index's string pool.
///
/// Serde: human-readable formats get a string, or (only for names that are not valid Unicode) an
/// array of UTF-16 code units; binary formats get the WTF-8 bytes. Every representation is
/// validated on deserialisation.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileName(Box<[u8]>);

impl FileName {
    /// Builds a name from UTF-16 code units (as returned by Windows APIs). Never fails: unpaired
    /// surrogates are preserved.
    pub fn from_utf16(units: &[u16]) -> Self {
        let mut out = Vec::with_capacity(units.len());
        for unit in char::decode_utf16(units.iter().copied()) {
            match unit {
                Ok(c) => {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
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
        FileName(out.into_boxed_slice())
    }

    /// Validates and wraps WTF-8 bytes (e.g. read back from a snapshot).
    pub fn from_wtf8(bytes: &[u8]) -> Result<Self, CoreError> {
        validate_wtf8(bytes).map_err(CoreError::InvalidFileName)?;
        Ok(FileName(bytes.into()))
    }

    /// The WTF-8 bytes. Equal to the UTF-8 bytes whenever the name is valid Unicode.
    pub fn as_wtf8(&self) -> &[u8] {
        &self.0
    }

    /// The name as `&str`, or `None` if it contains an unpaired surrogate.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// The name for display: borrowed when valid Unicode, else lone surrogates become U+FFFD.
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        match self.as_str() {
            Some(s) => Cow::Borrowed(s),
            None => Cow::Owned(
                CodePoints::new(&self.0)
                    .map(|cp| char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect(),
            ),
        }
    }

    /// The name as UTF-16 code units (exact inverse of [`FileName::from_utf16`]).
    pub fn to_utf16(&self) -> Vec<u16> {
        let mut out = Vec::with_capacity(self.0.len());
        for cp in CodePoints::new(&self.0) {
            if cp >= 0x1_0000 {
                let v = cp - 0x1_0000;
                out.push(0xD800 | (v >> 10) as u16);
                out.push(0xDC00 | (v & 0x3FF) as u16);
            } else {
                out.push(cp as u16);
            }
        }
        out
    }

    /// Length of the WTF-8 representation in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` for the empty name.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Converts to an `OsString` usable in a Windows path (lossless).
    #[cfg(windows)]
    pub fn to_os_string(&self) -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&self.to_utf16())
    }
}

impl From<&str> for FileName {
    fn from(s: &str) -> Self {
        FileName(s.as_bytes().into())
    }
}

impl From<String> for FileName {
    fn from(s: String) -> Self {
        FileName(s.into_bytes().into_boxed_slice())
    }
}

impl fmt::Debug for FileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.to_string_lossy(), f)
    }
}

impl fmt::Display for FileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}

impl Serialize for FileName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !serializer.is_human_readable() {
            return serializer.serialize_bytes(&self.0);
        }
        match self.as_str() {
            Some(s) => serializer.serialize_str(s),
            None => self.to_utf16().serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for FileName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            deserializer.deserialize_any(FileNameVisitor {
                human_readable: true,
            })
        } else {
            deserializer.deserialize_byte_buf(FileNameVisitor {
                human_readable: false,
            })
        }
    }
}

struct FileNameVisitor {
    human_readable: bool,
}

impl<'de> Visitor<'de> for FileNameVisitor {
    type Value = FileName;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.human_readable {
            f.write_str("a string or an array of UTF-16 code units")
        } else {
            f.write_str("WTF-8 bytes")
        }
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<FileName, E> {
        Ok(FileName::from(v))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<FileName, E> {
        Ok(FileName::from(v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<FileName, E> {
        FileName::from_wtf8(v).map_err(E::custom)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<FileName, A::Error> {
        if self.human_readable {
            let mut units = Vec::new();
            while let Some(u) = seq.next_element::<u16>()? {
                units.push(u);
            }
            Ok(FileName::from_utf16(&units))
        } else {
            let mut bytes = Vec::new();
            while let Some(b) = seq.next_element::<u8>()? {
                bytes.push(b);
            }
            FileName::from_wtf8(&bytes).map_err(de::Error::custom)
        }
    }
}

/// Decodes generalised UTF-8 (surrogate code points allowed) one code point at a time.
/// Yields `None`-terminated; stops at the first ill-formed sequence (callers validate first).
struct CodePoints<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> CodePoints<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        CodePoints { bytes, pos: 0 }
    }
}

impl Iterator for CodePoints<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let (cp, len) = decode_one(self.bytes.get(self.pos..)?)?;
        self.pos += len;
        Some(cp)
    }
}

/// Decodes one generalised-UTF-8 sequence at the start of `b`: `(code point, byte length)`.
/// Rejects overlong forms and values above U+10FFFF; accepts U+D800..U+DFFF.
fn decode_one(b: &[u8]) -> Option<(u32, usize)> {
    // Payload bits of the continuation byte at `i`, or `None` if it isn't one.
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
                return None; // overlong
            }
            Some(((u32::from(b0 & 0x0F) << 12) | (b1 << 6) | cont(2)?, 3))
        }
        0xF0..=0xF4 => {
            let b1 = cont(1)?;
            if (b0 == 0xF0 && b1 < 0x10) || (b0 == 0xF4 && b1 > 0x0F) {
                return None; // overlong or > U+10FFFF
            }
            let cp = (u32::from(b0 & 0x07) << 18) | (b1 << 12) | (cont(2)? << 6) | cont(3)?;
            Some((cp, 4))
        }
        _ => None,
    }
}

/// Checks that `bytes` is well-formed WTF-8: generalised UTF-8 in which a lead surrogate is never
/// directly followed by a trail surrogate (such pairs must be encoded as one 4-byte sequence).
fn validate_wtf8(bytes: &[u8]) -> Result<(), String> {
    let mut pos = 0;
    let mut prev_lead = false;
    while pos < bytes.len() {
        let rest = bytes.get(pos..).unwrap_or_default();
        let (cp, len) =
            decode_one(rest).ok_or_else(|| format!("ill-formed sequence at byte {pos}"))?;
        let is_trail = (0xDC00..=0xDFFF).contains(&cp);
        if prev_lead && is_trail {
            return Err(format!(
                "surrogate pair encoded as two sequences at byte {pos}"
            ));
        }
        prev_lead = (0xD800..=0xDBFF).contains(&cp);
        pos += len;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn valid_unicode_is_plain_utf8() {
        let name = FileName::from_utf16(&utf16("Straßen_Ōtautahi 🗺.shp"));
        assert_eq!(name.as_str(), Some("Straßen_Ōtautahi 🗺.shp"));
        assert_eq!(name.as_wtf8(), "Straßen_Ōtautahi 🗺.shp".as_bytes());
        assert_eq!(name.to_utf16(), utf16("Straßen_Ōtautahi 🗺.shp"));
        assert_eq!(name, FileName::from("Straßen_Ōtautahi 🗺.shp"));
    }

    #[test]
    fn lone_surrogates_round_trip() {
        let units = vec![0x0061, 0xD800, 0x0062, 0xDFFF, 0xDBFF];
        let name = FileName::from_utf16(&units);
        assert_eq!(name.as_str(), None);
        assert_eq!(name.to_utf16(), units);
        assert_eq!(name.to_string_lossy(), "a\u{FFFD}b\u{FFFD}\u{FFFD}");
        assert_eq!(FileName::from_wtf8(name.as_wtf8()).unwrap(), name);
    }

    #[test]
    fn wtf8_validation_rejects_ill_formed_input() {
        for bad in [
            &[0xFF][..],
            &[0xC0, 0x80],             // overlong NUL
            &[0xE0, 0x80, 0x80],       // overlong
            &[0xF4, 0x90, 0x80, 0x80], // > U+10FFFF
            &[0xE2, 0x82],             // truncated
            &[0x80],                   // stray continuation
            // U+D83D followed by U+DE00: must be the 4-byte form of U+1F600
            &[0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80],
        ] {
            assert!(FileName::from_wtf8(bad).is_err(), "{bad:x?}");
        }
        // A trail surrogate followed by a lead surrogate is fine.
        assert!(FileName::from_wtf8(&[0xED, 0xB0, 0x80, 0xED, 0xA0, 0x80]).is_ok());
    }

    #[test]
    fn serde_json_round_trip() {
        let plain = FileName::from("roads.shp");
        assert_eq!(serde_json::to_string(&plain).unwrap(), "\"roads.shp\"");
        let odd = FileName::from_utf16(&[0x0078, 0xD800]);
        let json = serde_json::to_string(&odd).unwrap();
        assert_eq!(json, "[120,55296]");
        assert_eq!(serde_json::from_str::<FileName>(&json).unwrap(), odd);
        assert_eq!(
            serde_json::from_str::<FileName>("\"roads.shp\"").unwrap(),
            plain
        );
        assert!(serde_json::from_str::<FileName>("42").is_err());
    }

    #[test]
    fn file_record_round_trip() {
        let rec = FileRecord {
            frn: Frn(0x0001_0000_0000_0123),
            parent: Frn(0x0005_0000_0000_0005),
            name: FileName::from("parcels.gpkg"),
            attrs: FileAttributes(FileAttributes::ARCHIVE | FileAttributes::HIDDEN),
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert_eq!(serde_json::from_str::<FileRecord>(&json).unwrap(), rec);
        assert!(rec.attrs.contains(FileAttributes::HIDDEN));
        assert!(!rec.is_dir());
        let gdb = FileRecord {
            name: FileName::from("city.gdb"),
            attrs: FileAttributes(FileAttributes::DIRECTORY),
            ..rec
        };
        assert!(gdb.is_dir());
    }
}
