//! Pure parsers for the byte layouts the NTFS USN APIs exchange.
//!
//! Everything here works on plain byte slices, so it is unit-tested on every platform from
//! fixtures (see `tests` below and `crates/ntfs/tests/data/`). All integers are little-endian
//! (Windows is little-endian on every supported architecture), and every length and offset read
//! from the buffer is validated before use — malformed input yields a [`ParseError`], never a
//! panic.
//!
//! Layouts (from `winioctl.h`):
//!
//! | Structure | Size | Used by |
//! |---|---|---|
//! | `MFT_ENUM_DATA_V1` | 32 | input of `FSCTL_ENUM_USN_DATA` |
//! | `USN_JOURNAL_DATA_V0/V1/V2` | 56 / 64 / 80 | output of `FSCTL_QUERY_USN_JOURNAL` |
//! | `USN_RECORD_V2` | 60 + name | records (64-bit file references) |
//! | `USN_RECORD_V3` | 76 + name | records (128-bit file IDs) |

use crate::error::ParseError;
use crate::name::NtfsName;

/// `FILE_ATTRIBUTE_DIRECTORY`.
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// MFT record number of the volume's root directory (`.`), fixed by the NTFS format.
pub const ROOT_RECORD_NUMBER: u64 = 5;

/// Size of the fixed part of `USN_RECORD_V2` (up to `FileName`).
pub const USN_RECORD_V2_HEADER: usize = 60;
/// Size of the fixed part of `USN_RECORD_V3` (up to `FileName`).
pub const USN_RECORD_V3_HEADER: usize = 76;
/// Size of `USN_JOURNAL_DATA_V0`, the smallest journal answer we accept.
pub const USN_JOURNAL_DATA_V0_SIZE: usize = 56;
/// Size of `USN_JOURNAL_DATA_V2`, the largest journal answer we ask for.
pub const USN_JOURNAL_DATA_V2_SIZE: usize = 80;
/// Size of `MFT_ENUM_DATA_V1`.
pub const MFT_ENUM_DATA_V1_SIZE: usize = 32;

/// The MFT record number (low 48 bits) of a 64-bit file reference number.
#[must_use]
pub const fn record_number(frn: u64) -> u64 {
    frn & 0x0000_FFFF_FFFF_FFFF
}

/// The sequence number (high 16 bits) of a 64-bit file reference number.
#[must_use]
pub const fn sequence_number(frn: u64) -> u16 {
    (frn >> 48) as u16
}

fn read_u16(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

fn read_u32(buf: &[u8], at: usize) -> u32 {
    let mut b = [0_u8; 4];
    b.copy_from_slice(&buf[at..at + 4]);
    u32::from_le_bytes(b)
}

fn read_u64(buf: &[u8], at: usize) -> u64 {
    let mut b = [0_u8; 8];
    b.copy_from_slice(&buf[at..at + 8]);
    u64::from_le_bytes(b)
}

fn read_i64(buf: &[u8], at: usize) -> i64 {
    read_u64(buf, at) as i64
}

fn read_u128(buf: &[u8], at: usize) -> u128 {
    let mut b = [0_u8; 16];
    b.copy_from_slice(&buf[at..at + 16]);
    u128::from_le_bytes(b)
}

/// Serialises `MFT_ENUM_DATA_V1`, the input of `FSCTL_ENUM_USN_DATA`.
///
/// `low_usn..=high_usn` filters records by their last USN (`0..=i64::MAX` = everything);
/// `min_major..=max_major` selects the `USN_RECORD` versions the file system may return.
#[must_use]
pub fn encode_mft_enum_data_v1(
    start_frn: u64,
    low_usn: i64,
    high_usn: i64,
    min_major: u16,
    max_major: u16,
) -> [u8; MFT_ENUM_DATA_V1_SIZE] {
    let mut out = [0_u8; MFT_ENUM_DATA_V1_SIZE];
    out[0..8].copy_from_slice(&start_frn.to_le_bytes());
    out[8..16].copy_from_slice(&low_usn.to_le_bytes());
    out[16..24].copy_from_slice(&high_usn.to_le_bytes());
    out[24..26].copy_from_slice(&min_major.to_le_bytes());
    out[26..28].copy_from_slice(&max_major.to_le_bytes());
    // 28..32: structure padding (alignment of the leading u64).
    out
}

/// State of a volume's USN change journal, from `FSCTL_QUERY_USN_JOURNAL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalInfo {
    /// Identifies this journal instance; changes when the journal is deleted and re-created.
    pub journal_id: u64,
    /// First USN still present in the journal. Checkpoints older than this have been lost.
    pub first_usn: i64,
    /// USN the next change will receive.
    pub next_usn: i64,
    /// Lowest USN valid for this journal instance (may be below `first_usn`).
    pub lowest_valid_usn: i64,
    /// Largest USN the journal can assign before it must be re-created.
    pub max_usn: i64,
    /// Target maximum size of the journal in bytes.
    pub maximum_size: u64,
    /// Bytes added/removed when the journal grows or is trimmed.
    pub allocation_delta: u64,
    /// Lowest `USN_RECORD` major version the journal writes (V1+ answer only).
    pub min_supported_major_version: Option<u16>,
    /// Highest `USN_RECORD` major version the journal writes (V1+ answer only).
    pub max_supported_major_version: Option<u16>,
}

/// Parses `USN_JOURNAL_DATA_V0`, `_V1` or `_V2` (the version is implied by the length).
///
/// # Errors
/// [`ParseError::Truncated`] when fewer than 56 bytes are given.
pub fn parse_journal_data(buf: &[u8]) -> Result<JournalInfo, ParseError> {
    if buf.len() < USN_JOURNAL_DATA_V0_SIZE {
        return Err(ParseError::Truncated {
            offset: 0,
            needed: USN_JOURNAL_DATA_V0_SIZE,
            available: buf.len(),
        });
    }
    let has_v1 = buf.len() >= 60;
    Ok(JournalInfo {
        journal_id: read_u64(buf, 0),
        first_usn: read_i64(buf, 8),
        next_usn: read_i64(buf, 16),
        lowest_valid_usn: read_i64(buf, 24),
        max_usn: read_i64(buf, 32),
        maximum_size: read_u64(buf, 40),
        allocation_delta: read_u64(buf, 48),
        min_supported_major_version: has_v1.then(|| read_u16(buf, 56)),
        max_supported_major_version: has_v1.then(|| read_u16(buf, 58)),
    })
}

/// One `USN_RECORD_V2`/`V3`, borrowed from the buffer it was parsed from (zero-copy).
///
/// V3 128-bit file IDs are narrowed to the 64-bit NTFS file reference (NTFS zero-extends them;
/// anything else is rejected with [`ParseError::FileIdTooWide`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsnRecord<'a> {
    /// `MajorVersion` of the record (2 or 3).
    pub major_version: u16,
    /// `MinorVersion` of the record.
    pub minor_version: u16,
    /// File reference number: MFT record number (low 48 bits) + sequence number (high 16).
    pub frn: u64,
    /// File reference number of the parent directory.
    pub parent_frn: u64,
    /// USN of the record (for enumeration: the file's last USN, 0 if never journaled).
    pub usn: i64,
    /// `TimeStamp` as a `FILETIME` (100 ns ticks since 1601-01-01 UTC).
    pub timestamp: i64,
    /// `USN_REASON_*` flags (journal reads; 0 for enumeration).
    pub reason: u32,
    /// `USN_SOURCE_*` flags.
    pub source_info: u32,
    /// `SecurityId` (security descriptor index).
    pub security_id: u32,
    /// `FILE_ATTRIBUTE_*` flags.
    pub file_attributes: u32,
    name_utf16le: &'a [u8],
}

impl<'a> UsnRecord<'a> {
    /// The raw file name: little-endian UTF-16, even length, exactly as stored.
    #[must_use]
    pub fn name_utf16le(&self) -> &'a [u8] {
        self.name_utf16le
    }

    /// The file name's UTF-16 code units.
    pub fn name_units(&self) -> impl Iterator<Item = u16> + 'a {
        crate::name::utf16le_units(self.name_utf16le)
    }

    /// The file name, losslessly converted (see [`NtfsName`]).
    #[must_use]
    pub fn name(&self) -> NtfsName {
        NtfsName::from_utf16le_bytes(self.name_utf16le)
    }

    /// `true` if `FILE_ATTRIBUTE_DIRECTORY` is set.
    #[must_use]
    pub fn is_directory(&self) -> bool {
        self.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    /// Copies the record into an owned [`RawRecord`].
    #[must_use]
    pub fn to_raw(&self) -> RawRecord {
        RawRecord {
            frn: self.frn,
            parent_frn: self.parent_frn,
            file_attributes: self.file_attributes,
            usn: self.usn,
            timestamp: self.timestamp,
            name: self.name(),
        }
    }
}

/// An owned enumeration record: `(frn, parent_frn, name, attributes)` plus the last USN and
/// time stamp, which cost nothing extra to keep.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawRecord {
    /// File reference number (record number + sequence number).
    pub frn: u64,
    /// Parent directory's file reference number.
    pub parent_frn: u64,
    /// `FILE_ATTRIBUTE_*` flags.
    pub file_attributes: u32,
    /// The file's last USN (0 if never journaled).
    pub usn: i64,
    /// `TimeStamp` as a `FILETIME`.
    pub timestamp: i64,
    /// The file name (one name per record; hard links share an FRN — see crate docs).
    pub name: NtfsName,
}

impl RawRecord {
    /// `true` if `FILE_ATTRIBUTE_DIRECTORY` is set.
    #[must_use]
    pub fn is_directory(&self) -> bool {
        self.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }
}

/// Parses one record at the start of `buf`; `offset` is only used in error messages.
///
/// Returns the record and its `RecordLength` (the distance to the next record).
///
/// # Errors
/// Structural errors (the returned length can't be trusted: [`ParseError::Truncated`],
/// [`ParseError::BadRecordLength`]) and record-local ones (see
/// [`ParseError::is_record_local`]).
pub fn parse_record(buf: &[u8], offset: usize) -> Result<(UsnRecord<'_>, usize), ParseError> {
    let length = record_length(buf, offset)?;
    parse_record_body(&buf[..length], offset).map(|record| (record, length))
}

/// Parses one record whose `RecordLength` has already been validated: `rec` is exactly the
/// record's bytes. Every error from here is record-local.
fn parse_record_body(rec: &[u8], offset: usize) -> Result<UsnRecord<'_>, ParseError> {
    let length = rec.len();
    let major = read_u16(rec, 4);
    let record_length = length as u32;
    let header = match major {
        2 => USN_RECORD_V2_HEADER,
        3 => USN_RECORD_V3_HEADER,
        _ => return Err(ParseError::UnsupportedVersion { offset, major }),
    };
    if length < header {
        return Err(ParseError::HeaderTooLong {
            offset,
            major,
            length: record_length,
            needed: header,
        });
    }
    // Field offsets after the file-reference pair differ by 16 bytes between V2 and V3.
    let (frn, parent_frn, tail) = if major == 2 {
        (read_u64(rec, 8), read_u64(rec, 16), 24)
    } else {
        let narrow =
            |id: u128| u64::try_from(id).map_err(|_| ParseError::FileIdTooWide { offset, id });
        (narrow(read_u128(rec, 8))?, narrow(read_u128(rec, 24))?, 40)
    };
    let name_length = read_u16(rec, tail + 32);
    let name_offset = read_u16(rec, tail + 34);
    let name_start = usize::from(name_offset);
    let name_end = name_start + usize::from(name_length);
    if name_start < header || name_end > length || !name_length.is_multiple_of(2) {
        return Err(ParseError::BadName {
            offset,
            name_offset,
            name_length,
            record_length,
        });
    }
    Ok(UsnRecord {
        major_version: major,
        minor_version: read_u16(rec, 6),
        frn,
        parent_frn,
        usn: read_i64(rec, tail),
        timestamp: read_i64(rec, tail + 8),
        reason: read_u32(rec, tail + 16),
        source_info: read_u32(rec, tail + 20),
        security_id: read_u32(rec, tail + 24),
        file_attributes: read_u32(rec, tail + 28),
        name_utf16le: &rec[name_start..name_end],
    })
}

/// Reads and validates `RecordLength` (≥ 8 and within `buf`).
fn record_length(buf: &[u8], offset: usize) -> Result<usize, ParseError> {
    if buf.len() < 8 {
        return Err(ParseError::Truncated {
            offset,
            needed: 8,
            available: buf.len(),
        });
    }
    let length = read_u32(buf, 0);
    match usize::try_from(length) {
        Ok(len) if len >= 8 && len <= buf.len() => Ok(len),
        _ => Err(ParseError::BadRecordLength {
            offset,
            length,
            available: buf.len(),
        }),
    }
}

/// Iterator over the `USN_RECORD`s packed in a byte slice.
///
/// Record-local errors are yielded and iteration continues with the next record; a structural
/// error is yielded once and ends the iteration.
#[derive(Debug, Clone)]
pub struct RecordIter<'a> {
    buf: &'a [u8],
    pos: usize,
    base_offset: usize,
    fused: bool,
}

impl<'a> RecordIter<'a> {
    /// Iterates the records in `buf`; `base_offset` is the position of `buf` within the
    /// original output buffer (used in error offsets).
    #[must_use]
    pub fn new(buf: &'a [u8], base_offset: usize) -> Self {
        Self {
            buf,
            pos: 0,
            base_offset,
            fused: false,
        }
    }

    /// Resumes iteration at byte `pos` of `buf` (offsets in errors are positions in `buf`).
    /// Lets a caller that owns the buffer keep only a cursor between calls (the enumerator does
    /// this; 2.2's journal reader can do the same) instead of re-implementing the record walk.
    #[must_use]
    pub fn resume(buf: &'a [u8], pos: usize) -> Self {
        Self {
            buf,
            pos,
            base_offset: 0,
            fused: false,
        }
    }

    /// Byte position of the next record in `buf`, or `None` once a structural error has ended
    /// the iteration.
    #[must_use]
    pub fn position(&self) -> Option<usize> {
        (!self.fused).then_some(self.pos)
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
}

impl<'a> Iterator for RecordIter<'a> {
    type Item = Result<UsnRecord<'a>, ParseError>;

    /// The crate's one record walker: validate `RecordLength` (a failure is structural and ends
    /// the iteration), then parse the body (a failure is record-local: yielded, then skipped).
    fn next(&mut self) -> Option<Self::Item> {
        if self.fused || self.pos >= self.buf.len() {
            return None;
        }
        let offset = self.base_offset + self.pos;
        let rest = &self.buf[self.pos..];
        let length = match record_length(rest, offset) {
            Ok(length) => length,
            Err(e) => {
                self.fused = true;
                return Some(Err(e));
            }
        };
        self.pos += length;
        Some(parse_record_body(&rest[..length], offset))
    }
}

/// Splits `FSCTL_ENUM_USN_DATA` output into the next start file reference (the first 8 bytes)
/// and an iterator over the records that follow.
///
/// # Errors
/// [`ParseError::Truncated`] when the output is shorter than 8 bytes.
pub fn parse_enum_output(buf: &[u8]) -> Result<(u64, RecordIter<'_>), ParseError> {
    if buf.len() < 8 {
        return Err(ParseError::Truncated {
            offset: 0,
            needed: 8,
            available: buf.len(),
        });
    }
    Ok((read_u64(buf, 0), RecordIter::new(&buf[8..], 8)))
}

/// What to do after one successful `FSCTL_ENUM_USN_DATA` call (see [`enum_step`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumStep {
    /// The reply holds records; continue from `next_start` once they are consumed.
    Records {
        /// `StartFileReferenceNumber` for the next call.
        next_start: u64,
    },
    /// Header-only reply with a larger start reference: no record in this MFT range passed the
    /// USN filter, but enumeration is not over — call again from `next_start`.
    Advance {
        /// `StartFileReferenceNumber` for the next call.
        next_start: u64,
    },
    /// Header-only reply that did not move: enumeration is complete.
    End,
    /// The returned start reference did not move forward although records were returned, or
    /// went backwards: continuing could loop or repeat records.
    Stalled {
        /// The start reference the kernel returned.
        next_start: u64,
    },
}

/// Decides how enumeration proceeds after a call made with `current_start` returned `reply`
/// (the bytes the kernel wrote). Pure, so the termination logic is unit-tested.
///
/// The normal end of enumeration is `ERROR_HANDLE_EOF`, handled by the caller before this.
///
/// # Errors
/// [`ParseError::Truncated`] when the reply is shorter than its 8-byte header.
pub fn enum_step(current_start: u64, reply: &[u8]) -> Result<EnumStep, ParseError> {
    let (next_start, records) = parse_enum_output(reply)?;
    let header_only = records.remaining() == 0;
    Ok(match (header_only, next_start.cmp(&current_start)) {
        (true, std::cmp::Ordering::Equal) => EnumStep::End,
        (true, std::cmp::Ordering::Greater) => EnumStep::Advance { next_start },
        (false, std::cmp::Ordering::Greater) => EnumStep::Records { next_start },
        _ => EnumStep::Stalled { next_start },
    })
}

/// Test-and-fixture helpers that build records in the documented layout.
///
/// Only compiled for this crate's tests or with the `test-fixtures` feature, which other crates
/// enable as a dev-dependency (e.g. 2.2's journal tests) — not part of the production API.
#[cfg(any(test, feature = "test-fixtures"))]
pub mod build {
    use super::{USN_RECORD_V2_HEADER, USN_RECORD_V3_HEADER};

    /// Fields of a record to synthesise.
    #[derive(Debug, Clone)]
    pub struct RecordSpec {
        /// 2 or 3 (anything else is written as given, for negative tests).
        pub major: u16,
        /// File reference (for V3, the low 64 bits of the file ID).
        pub frn: u64,
        /// Parent file reference.
        pub parent_frn: u64,
        /// USN.
        pub usn: i64,
        /// `FILETIME`.
        pub timestamp: i64,
        /// `USN_REASON_*`.
        pub reason: u32,
        /// `FILE_ATTRIBUTE_*`.
        pub file_attributes: u32,
        /// Name as UTF-16 code units.
        pub name: Vec<u16>,
    }

    /// Encodes a record with the name right after the header, padded to 8 bytes like the
    /// kernel does.
    #[must_use]
    pub fn record(spec: &RecordSpec) -> Vec<u8> {
        let header = if spec.major == 3 {
            USN_RECORD_V3_HEADER
        } else {
            USN_RECORD_V2_HEADER
        };
        let name_len = spec.name.len() * 2;
        let len = (header + name_len).div_ceil(8) * 8;
        let mut out = vec![0_u8; len];
        out[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        out[4..6].copy_from_slice(&spec.major.to_le_bytes());
        let tail = if spec.major == 3 {
            out[8..16].copy_from_slice(&spec.frn.to_le_bytes());
            out[24..32].copy_from_slice(&spec.parent_frn.to_le_bytes());
            40
        } else {
            out[8..16].copy_from_slice(&spec.frn.to_le_bytes());
            out[16..24].copy_from_slice(&spec.parent_frn.to_le_bytes());
            24
        };
        out[tail..tail + 8].copy_from_slice(&spec.usn.to_le_bytes());
        out[tail + 8..tail + 16].copy_from_slice(&spec.timestamp.to_le_bytes());
        out[tail + 16..tail + 20].copy_from_slice(&spec.reason.to_le_bytes());
        out[tail + 28..tail + 32].copy_from_slice(&spec.file_attributes.to_le_bytes());
        out[tail + 32..tail + 34].copy_from_slice(&(name_len as u16).to_le_bytes());
        out[tail + 34..tail + 36].copy_from_slice(&(header as u16).to_le_bytes());
        for (i, unit) in spec.name.iter().enumerate() {
            out[header + 2 * i..header + 2 * i + 2].copy_from_slice(&unit.to_le_bytes());
        }
        out
    }

    /// Encodes `FSCTL_ENUM_USN_DATA` output: the next start reference, then the records.
    #[must_use]
    pub fn enum_output(next_start: u64, records: &[Vec<u8>]) -> Vec<u8> {
        let mut out = next_start.to_le_bytes().to_vec();
        for r in records {
            out.extend_from_slice(r);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::build::{RecordSpec, enum_output, record};
    use super::*;

    fn spec(major: u16, frn: u64, parent: u64, name: &str) -> RecordSpec {
        RecordSpec {
            major,
            frn,
            parent_frn: parent,
            usn: 0x1234_5678,
            timestamp: 133_000_000_000_000_000,
            reason: 0,
            file_attributes: 0x20,
            name: name.encode_utf16().collect(),
        }
    }

    /// A `USN_RECORD_V2` written out byte by byte from the `winioctl.h` layout — independent of
    /// the `build` helper, so a mistake in one is caught by the other.
    const V2_FIXTURE: [u8; 72] = [
        0x48, 0x00, 0x00, 0x00, // RecordLength = 72
        0x02, 0x00, 0x00, 0x00, // MajorVersion = 2, MinorVersion = 0
        0x2A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, // FRN: record 42, sequence 3
        0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, // parent: root (record 5, seq 5)
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Usn = 4096
        0x00, 0x80, 0x3E, 0xD5, 0xDE, 0xB1, 0x9D, 0x01, // TimeStamp (FILETIME)
        0x00, 0x00, 0x00, 0x00, // Reason
        0x00, 0x00, 0x00, 0x00, // SourceInfo
        0x07, 0x01, 0x00, 0x00, // SecurityId = 263
        0x20, 0x00, 0x00, 0x00, // FileAttributes = ARCHIVE
        0x0A, 0x00, // FileNameLength = 10
        0x3C, 0x00, // FileNameOffset = 60
        b'a', 0, b'.', 0, b's', 0, b'h', 0, b'p', 0, // "a.shp"
        0x00, 0x00, // padding to 8
    ];

    /// The same file as a `USN_RECORD_V3` (128-bit IDs, zero-extended), directory flag set,
    /// name with a non-BMP character.
    const V3_FIXTURE: [u8; 88] = [
        0x58, 0x00, 0x00, 0x00, // RecordLength = 88
        0x03, 0x00, 0x00, 0x00, // MajorVersion = 3, MinorVersion = 0
        0x2A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, // FileId128
        0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, // parent
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Usn = 4096
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // TimeStamp
        0x02, 0x00, 0x00, 0x00, // Reason = USN_REASON_DATA_EXTEND
        0x00, 0x00, 0x00, 0x00, // SourceInfo
        0x00, 0x00, 0x00, 0x00, // SecurityId
        0x10, 0x00, 0x00, 0x00, // FileAttributes = DIRECTORY
        0x08, 0x00, // FileNameLength = 8
        0x4C, 0x00, // FileNameOffset = 76
        0x3D, 0xD8, 0xFA, 0xDD, b'x', 0x00, 0x00, 0xD8, // U+1F5FA "x" + lone U+D800
        0x00, 0x00, 0x00, 0x00, // padding to 8
    ];

    #[test]
    fn parses_v2_fixture() {
        let (r, len) = parse_record(&V2_FIXTURE, 0).expect("valid fixture");
        assert_eq!(len, 72);
        assert_eq!(r.major_version, 2);
        assert_eq!(record_number(r.frn), 42);
        assert_eq!(sequence_number(r.frn), 3);
        assert_eq!(record_number(r.parent_frn), ROOT_RECORD_NUMBER);
        assert_eq!(r.usn, 4096);
        assert_eq!(r.timestamp, 0x019D_B1DE_D53E_8000);
        assert_eq!(r.security_id, 263);
        assert!(!r.is_directory());
        assert_eq!(r.name().as_str(), Some("a.shp"));
        assert_eq!(r.to_raw().name.as_str(), Some("a.shp"));
    }

    #[test]
    fn parses_v3_fixture() {
        let (r, len) = parse_record(&V3_FIXTURE, 0).expect("valid fixture");
        assert_eq!(len, 88);
        assert_eq!(r.major_version, 3);
        assert_eq!(r.frn, 0x0003_0000_0000_002A);
        assert_eq!(r.parent_frn, 0x0005_0000_0000_0005);
        assert_eq!(r.reason, 2);
        assert!(r.is_directory());
        let name = r.name();
        assert_eq!(name.to_utf16(), [0xD83D, 0xDDFA, u16::from(b'x'), 0xD800]);
        assert_eq!(name.to_string_lossy(), "\u{1F5FA}x\u{FFFD}");
    }

    #[test]
    fn builder_matches_hand_written_fixture() {
        let name: Vec<u16> = "a.shp".encode_utf16().collect();
        let built = record(&RecordSpec {
            major: 2,
            frn: 0x0003_0000_0000_002A,
            parent_frn: 0x0005_0000_0000_0005,
            usn: 4096,
            timestamp: 0x019D_B1DE_D53E_8000,
            reason: 0,
            file_attributes: 0x20,
            name,
        });
        let mut expected = V2_FIXTURE;
        expected[48..52].fill(0); // the builder leaves SecurityId at 0
        assert_eq!(built, expected);
    }

    #[test]
    fn rejects_wide_v3_file_id() {
        let mut buf = V3_FIXTURE;
        buf[20] = 1; // a bit in the upper 64 bits of the file ID
        let err = parse_record(&buf, 0).expect_err("must reject");
        assert!(matches!(err, ParseError::FileIdTooWide { .. }));
        assert!(err.is_record_local());
    }

    #[test]
    fn enum_output_round_trip_mixed_versions() {
        let a = spec(2, 0x0001_0000_0000_0010, 0x0005_0000_0000_0005, "data");
        let b = spec(3, 0x0001_0000_0000_0011, 0x0001_0000_0000_0010, "roads.shp");
        let c = spec(2, 0x0001_0000_0000_0012, 0x0001_0000_0000_0010, "");
        let buf = enum_output(0x0000_0000_0000_0013, &[record(&a), record(&b), record(&c)]);
        let (next, iter) = parse_enum_output(&buf).expect("valid");
        assert_eq!(next, 0x13);
        let records: Vec<_> = iter.collect::<Result<_, _>>().expect("all valid");
        assert_eq!(records.len(), 3);
        assert_eq!(records[1].name().as_str(), Some("roads.shp"));
        assert_eq!(records[1].parent_frn, records[0].frn);
        assert!(records[2].name().is_empty());
    }

    #[test]
    fn empty_enum_output_has_no_records() {
        let buf = enum_output(77, &[]);
        let (next, mut iter) = parse_enum_output(&buf).expect("valid");
        assert_eq!(next, 77);
        assert!(iter.next().is_none());
        assert!(parse_enum_output(&buf[..7]).is_err());
    }

    #[test]
    fn unsupported_version_is_skipped_not_fatal() {
        let a = spec(2, 16, 5, "a");
        let b = spec(2, 17, 5, "b");
        let mut v4 = record(&a);
        v4[4] = 4; // MajorVersion = 4
        let buf = [v4, record(&b)].concat();
        let items: Vec<_> = RecordIter::new(&buf, 0).collect();
        assert_eq!(items.len(), 2);
        assert!(matches!(
            items[0],
            Err(ParseError::UnsupportedVersion { major: 4, .. })
        ));
        assert_eq!(items[1].as_ref().map(|r| r.frn), Ok(17));
    }

    #[test]
    fn bad_length_fuses_iterator() {
        let a = spec(2, 16, 5, "a");
        let mut bad = record(&a);
        bad[0..4].copy_from_slice(&4_u32.to_le_bytes()); // shorter than 8: can't progress
        let buf = [bad, record(&a)].concat();
        let items: Vec<_> = RecordIter::new(&buf, 0).collect();
        assert_eq!(items.len(), 1);
        assert!(matches!(
            items[0],
            Err(ParseError::BadRecordLength { length: 4, .. })
        ));

        let mut overrun = record(&a);
        overrun[0..4].copy_from_slice(&4096_u32.to_le_bytes());
        let items: Vec<_> = RecordIter::new(&overrun, 0).collect();
        assert!(matches!(
            items[..],
            [Err(ParseError::BadRecordLength { .. })]
        ));
    }

    #[test]
    fn bad_name_spans_are_rejected() {
        let mut buf = V2_FIXTURE;
        buf[56] = 0x0B; // odd FileNameLength
        assert!(matches!(
            parse_record(&buf, 0),
            Err(ParseError::BadName { .. })
        ));
        let mut buf = V2_FIXTURE;
        buf[56] = 0x40; // name runs past the record
        assert!(matches!(
            parse_record(&buf, 0),
            Err(ParseError::BadName { .. })
        ));
        let mut buf = V2_FIXTURE;
        buf[58] = 0x10; // name offset inside the fixed header
        assert!(matches!(
            parse_record(&buf, 0),
            Err(ParseError::BadName { .. })
        ));
        let mut buf = V2_FIXTURE;
        buf[0] = 56; // record shorter than the V2 header
        assert!(matches!(
            parse_record(&buf[..56], 0),
            Err(ParseError::HeaderTooLong { .. })
        ));
    }

    #[test]
    fn journal_data_versions() {
        let mut v2 = [0_u8; USN_JOURNAL_DATA_V2_SIZE];
        v2[0..8].copy_from_slice(&0x01D9_0000_0000_0001_u64.to_le_bytes());
        v2[8..16].copy_from_slice(&100_i64.to_le_bytes());
        v2[16..24].copy_from_slice(&5000_i64.to_le_bytes());
        v2[32..40].copy_from_slice(&i64::MAX.to_le_bytes());
        v2[40..48].copy_from_slice(&(32_u64 << 20).to_le_bytes());
        v2[48..56].copy_from_slice(&(8_u64 << 20).to_le_bytes());
        v2[56..58].copy_from_slice(&2_u16.to_le_bytes());
        v2[58..60].copy_from_slice(&4_u16.to_le_bytes());
        let j = parse_journal_data(&v2).expect("v2");
        assert_eq!(j.journal_id, 0x01D9_0000_0000_0001);
        assert_eq!((j.first_usn, j.next_usn, j.max_usn), (100, 5000, i64::MAX));
        assert_eq!(j.maximum_size, 32 << 20);
        assert_eq!(j.min_supported_major_version, Some(2));
        assert_eq!(j.max_supported_major_version, Some(4));

        let j0 = parse_journal_data(&v2[..USN_JOURNAL_DATA_V0_SIZE]).expect("v0");
        assert_eq!(j0.next_usn, 5000);
        assert_eq!(j0.max_supported_major_version, None);
        assert!(matches!(
            parse_journal_data(&v2[..55]),
            Err(ParseError::Truncated {
                needed: 56,
                available: 55,
                ..
            })
        ));
    }

    #[test]
    fn mft_enum_data_layout() {
        let b = encode_mft_enum_data_v1(0x0102_0304_0506_0708, 0, i64::MAX, 2, 3);
        assert_eq!(&b[0..8], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(&b[8..16], &[0; 8]);
        assert_eq!(&b[16..24], &i64::MAX.to_le_bytes());
        assert_eq!(&b[24..28], &[2, 0, 3, 0]);
        assert_eq!(&b[28..32], &[0; 4]);
    }

    #[test]
    fn frn_helpers() {
        assert_eq!(record_number(0x0005_0000_0000_0005), 5);
        assert_eq!(sequence_number(0x0005_0000_0000_0005), 5);
        assert_eq!(sequence_number(u64::MAX), u16::MAX);
    }

    #[test]
    fn enum_step_with_records_advances() {
        let buf = enum_output(0x40, &[record(&spec(2, 0x3F, 5, "a"))]);
        assert_eq!(
            enum_step(0, &buf),
            Ok(EnumStep::Records { next_start: 0x40 })
        );
    }

    #[test]
    fn enum_step_header_only_with_larger_start_keeps_going() {
        // With a USN filter (low_usn > 0) a whole MFT range can be filtered out: the kernel
        // returns just the header with a larger start reference. That is not the end.
        let buf = enum_output(0x2_0000, &[]);
        assert_eq!(
            enum_step(0x1_0000, &buf),
            Ok(EnumStep::Advance {
                next_start: 0x2_0000
            })
        );
    }

    #[test]
    fn enum_step_header_only_without_progress_ends() {
        let buf = enum_output(0x1_0000, &[]);
        assert_eq!(enum_step(0x1_0000, &buf), Ok(EnumStep::End));
    }

    #[test]
    fn enum_step_detects_stalls_and_regressions() {
        let rec = record(&spec(2, 0x10, 5, "a"));
        // Records but no progress.
        let same = enum_output(0x40, std::slice::from_ref(&rec));
        assert_eq!(
            enum_step(0x40, &same),
            Ok(EnumStep::Stalled { next_start: 0x40 })
        );
        // Start reference went backwards, with or without records.
        let back = enum_output(0x20, std::slice::from_ref(&rec));
        assert_eq!(
            enum_step(0x40, &back),
            Ok(EnumStep::Stalled { next_start: 0x20 })
        );
        let back_empty = enum_output(0x20, &[]);
        assert_eq!(
            enum_step(0x40, &back_empty),
            Ok(EnumStep::Stalled { next_start: 0x20 })
        );
        // Shorter than the header.
        assert!(matches!(
            enum_step(0, &[0; 7]),
            Err(ParseError::Truncated { .. })
        ));
    }

    #[test]
    fn resumed_iterator_matches_fresh_one() {
        let recs: Vec<Vec<u8>> = (0..4_u16)
            .map(|i| record(&spec(2 + i % 2, 16 + u64::from(i), 5, "x")))
            .collect();
        let buf = recs.concat();
        let mut it = RecordIter::resume(&buf, 0);
        let mut seen = Vec::new();
        while let Some(pos) = it.position() {
            let mut step = RecordIter::resume(&buf, pos);
            match step.next() {
                Some(r) => seen.push(r.map(|r| r.frn)),
                None => break,
            }
            it = step;
        }
        let fresh: Vec<_> = RecordIter::new(&buf, 0).map(|r| r.map(|r| r.frn)).collect();
        assert_eq!(seen, fresh);
        assert_eq!(seen.len(), 4);
    }

    /// Deterministic xorshift so the fuzz-style tests need no extra dependency.
    struct XorShift(u64);
    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn sample_output() -> Vec<u8> {
        let names = ["a.shp", "a.dbf", "Ortho 2024.tif", "東京.gpkg", ""];
        let records: Vec<Vec<u8>> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let s = spec(if i % 2 == 0 { 2 } else { 3 }, 16 + i as u64, 5, n);
                record(&s)
            })
            .collect();
        enum_output(21, &records)
    }

    fn drain(buf: &[u8]) -> usize {
        match parse_enum_output(buf) {
            Ok((_, iter)) => iter
                .map(|r| match r {
                    Ok(r) => r.name().len() + usize::from(r.is_directory()),
                    Err(_) => 1,
                })
                .sum(),
            Err(_) => 0,
        }
    }

    #[test]
    fn truncated_output_never_panics() {
        let buf = sample_output();
        for cut in 0..=buf.len() {
            drain(&buf[..cut]);
            if cut <= USN_JOURNAL_DATA_V2_SIZE {
                let _ = parse_journal_data(&buf[..cut]);
            }
        }
    }

    #[test]
    fn mutated_output_never_panics() {
        let base = sample_output();
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        for _ in 0..20_000 {
            let mut buf = base.clone();
            let flips = 1 + (rng.next() % 6) as usize;
            for _ in 0..flips {
                let at = (rng.next() as usize) % buf.len();
                buf[at] = rng.next() as u8;
            }
            drain(&buf);
        }
        // Pure noise, too.
        for len in [0_usize, 7, 8, 9, 63, 64, 200, 4096] {
            let buf: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            drain(&buf);
        }
    }
}
