//! Error types for the `ntfs` crate.
//!
//! [`ParseError`] covers malformed bytes handed back by the file system (pure, cross-platform);
//! [`Error`] covers everything a volume operation can report. Both are plain data so callers can
//! match on them without any Windows types in scope.

/// A byte buffer returned by the file system did not have the documented layout.
///
/// Parsing never panics: every length, offset and count is validated, and a violation becomes
/// one of these variants.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// Fewer bytes were available than the structure needs.
    #[error("buffer too short at offset {offset}: need {needed} bytes, have {available}")]
    Truncated {
        /// Byte offset (within the buffer handed to the parser) where the structure starts.
        offset: usize,
        /// Bytes the structure needs.
        needed: usize,
        /// Bytes actually available from `offset`.
        available: usize,
    },
    /// A record's `RecordLength` is too small to make progress or runs past the buffer end.
    /// The rest of the buffer cannot be trusted after this.
    #[error("record at offset {offset} has invalid length {length} ({available} bytes left)")]
    BadRecordLength {
        /// Byte offset of the record.
        offset: usize,
        /// The declared `RecordLength`.
        length: u32,
        /// Bytes left in the buffer from `offset`.
        available: usize,
    },
    /// The record's fixed header does not fit inside its declared length.
    #[error("record at offset {offset} (version {major}) is {length} bytes, header needs {needed}")]
    HeaderTooLong {
        /// Byte offset of the record.
        offset: usize,
        /// Major version of the record.
        major: u16,
        /// The declared `RecordLength`.
        length: u32,
        /// Size of that version's fixed header.
        needed: usize,
    },
    /// The file-name span (`FileNameOffset`/`FileNameLength`) lies outside the record, overlaps
    /// the fixed header, or has an odd byte length.
    #[error(
        "record at offset {offset}: file name at {name_offset} ({name_length} bytes) is invalid \
         for a {record_length}-byte record"
    )]
    BadName {
        /// Byte offset of the record.
        offset: usize,
        /// The declared `FileNameOffset` (relative to the record).
        name_offset: u16,
        /// The declared `FileNameLength` in bytes.
        name_length: u16,
        /// The declared `RecordLength`.
        record_length: u32,
    },
    /// The record's major version is not 2 or 3 (V4 range-tracking records are not produced by
    /// enumeration).
    #[error("record at offset {offset} has unsupported major version {major}")]
    UnsupportedVersion {
        /// Byte offset of the record.
        offset: usize,
        /// The declared major version.
        major: u16,
    },
    /// A `USN_RECORD_V3` 128-bit file ID has non-zero upper bits, so it is not an NTFS 64-bit
    /// file reference number (NTFS always zero-extends; ReFS does not).
    #[error("record at offset {offset}: 128-bit file ID {id:#034x} is not a 64-bit NTFS reference")]
    FileIdTooWide {
        /// Byte offset of the record.
        offset: usize,
        /// The full 128-bit identifier.
        id: u128,
    },
}

impl ParseError {
    /// `true` when the error concerns one record whose length was still valid, so the records
    /// after it can be parsed; `false` when the rest of the buffer cannot be trusted.
    #[must_use]
    pub fn is_record_local(&self) -> bool {
        matches!(
            self,
            Self::HeaderTooLong { .. }
                | Self::BadName { .. }
                | Self::UnsupportedVersion { .. }
                | Self::FileIdTooWide { .. }
        )
    }
}

/// Errors from volume operations (open, query, enumerate).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The volume argument is not a drive letter such as `C:`, `C:\` or `\\.\C:`.
    #[error("invalid volume {0:?}: expected a drive letter such as `C:`")]
    InvalidVolume(String),
    /// The volume exists but is not formatted NTFS (FAT, exFAT and ReFS are not supported here).
    #[error("volume {volume} uses {found}, not NTFS")]
    NotNtfs {
        /// The volume, e.g. `C:`.
        volume: String,
        /// File-system name reported by `GetVolumeInformationW`.
        found: String,
    },
    /// Opening the raw volume was denied: raw volume access needs an elevated process.
    #[error(
        "access denied opening {volume}: raw volume access needs an elevated (administrator) process"
    )]
    NotElevated {
        /// The volume, e.g. `C:`.
        volume: String,
    },
    /// The change journal is being deleted (`ERROR_JOURNAL_DELETE_IN_PROGRESS`); retry later.
    #[error("the USN change journal on {volume} is being deleted")]
    JournalDeleteInProgress {
        /// The volume, e.g. `C:`.
        volume: String,
    },
    /// Enumeration stopped making progress (the file system returned the same start reference
    /// twice). Reported instead of looping forever.
    #[error("enumeration of {volume} stalled at file reference {frn:#018x}")]
    Stalled {
        /// The volume, e.g. `C:`.
        volume: String,
        /// The start reference that was returned again.
        frn: u64,
    },
    /// A Win32 call failed for a reason not covered by a more specific variant.
    #[error("{op} on {volume} failed: Win32 error {code} ({message})")]
    Os {
        /// The API that failed, e.g. `DeviceIoControl(FSCTL_ENUM_USN_DATA)`.
        op: &'static str,
        /// The volume, e.g. `C:`.
        volume: String,
        /// The Win32 error code (or the raw `HRESULT` bits when it is not a Win32 error).
        code: u32,
        /// The system message for `code`.
        message: String,
    },
    /// The file system returned bytes that do not match the documented layout.
    #[error("malformed {what} returned for {volume}")]
    Parse {
        /// What was being parsed, e.g. `FSCTL_ENUM_USN_DATA output`.
        what: &'static str,
        /// The volume, e.g. `C:`.
        volume: String,
        /// The parse failure.
        #[source]
        source: ParseError,
    },
}

/// Convenience alias for results of volume operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Win32 error codes this crate distinguishes (values from `winerror.h`).
pub mod win32 {
    /// `ERROR_INVALID_FUNCTION`: the FSCTL is not supported by this file system.
    pub const ERROR_INVALID_FUNCTION: u32 = 1;
    /// `ERROR_ACCESS_DENIED`.
    pub const ERROR_ACCESS_DENIED: u32 = 5;
    /// `ERROR_HANDLE_EOF`: `FSCTL_ENUM_USN_DATA` has no more records.
    pub const ERROR_HANDLE_EOF: u32 = 38;
    /// `ERROR_JOURNAL_DELETE_IN_PROGRESS`.
    pub const ERROR_JOURNAL_DELETE_IN_PROGRESS: u32 = 1178;
    /// `ERROR_JOURNAL_NOT_ACTIVE`: the volume has no active USN change journal.
    pub const ERROR_JOURNAL_NOT_ACTIVE: u32 = 1179;

    /// Extracts the Win32 error code from an `HRESULT` built by `HRESULT_FROM_WIN32`
    /// (facility 7, severity bit set). Other `HRESULT`s are returned as their raw bits.
    #[must_use]
    pub fn code_from_hresult(hresult: i32) -> u32 {
        let bits = hresult as u32;
        if bits & 0xFFFF_0000 == 0x8007_0000 {
            bits & 0xFFFF
        } else {
            bits
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hresult_from_win32_round_trips() {
        // HRESULT_FROM_WIN32(ERROR_HANDLE_EOF) = 0x80070026.
        assert_eq!(
            win32::code_from_hresult(0x8007_0026_u32 as i32),
            win32::ERROR_HANDLE_EOF
        );
        assert_eq!(
            win32::code_from_hresult(0x8007_0005_u32 as i32),
            win32::ERROR_ACCESS_DENIED
        );
        assert_eq!(
            win32::code_from_hresult(0x8007_049B_u32 as i32),
            win32::ERROR_JOURNAL_NOT_ACTIVE
        );
        // A non-Win32 HRESULT (E_NOTIMPL) is passed through untouched.
        assert_eq!(
            win32::code_from_hresult(0x8000_4001_u32 as i32),
            0x8000_4001
        );
    }

    #[test]
    fn record_local_classification() {
        assert!(
            ParseError::UnsupportedVersion {
                offset: 0,
                major: 4
            }
            .is_record_local()
        );
        assert!(
            !ParseError::BadRecordLength {
                offset: 0,
                length: 0,
                available: 8
            }
            .is_record_local()
        );
        assert!(
            !ParseError::Truncated {
                offset: 0,
                needed: 8,
                available: 3
            }
            .is_record_local()
        );
    }
}
