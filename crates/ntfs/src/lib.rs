//! NTFS volume source: `FSCTL_ENUM_USN_DATA` enumeration and USN change-journal tailing.
//!
//! This is the only crate allowed to opt out of the workspace `unsafe_code = "deny"` lint (raw
//! `DeviceIoControl` calls); every `unsafe` block must carry a `// SAFETY:` comment.
//!
//! Spec: `docs/03-architecture.md` §3.1–3.3; stage01 sessions 1.3 (enumeration) and 2.2
//! (journal tailing + the `Source` implementation, `NtfsVolumeSource`).
//!
//! # Layout
//!
//! | Module | Platform | What |
//! |---|---|---|
//! | [`record`] | all | Pure parsers: `USN_RECORD_V2/V3`, `FSCTL_ENUM_USN_DATA` output, `USN_JOURNAL_DATA`, `MFT_ENUM_DATA_V1` encoding; fixture builders |
//! | [`journal`] | all | `FSCTL_READ_USN_JOURNAL` input/output; `USN_REASON_*` → `SourceEvent` folding ([`Coalescer`]) |
//! | [`checkpoint`] | all | [`NtfsCheckpoint`] payload (volume serial, journal ID, next USN, pending renames) and the rescan decision |
//! | [`mft`] | all | Pure parsers for the raw `$MFT`: volume geometry, `FILE` records and fixups, attributes, runlists, `$FILE_NAME` |
//! | [`mft_scan`] | all | [`MftScanner`]: enumerate by reading `$MFT` in large chunks, yielding the same [`RawRecord`] as the USN API (2.7) |
//! | [`name`] | all | [`NtfsName`]: lossless (WTF-8) file names |
//! | [`chain`] | all | [`ParentChainReport`]: do parent links resolve to the root? |
//! | [`drive`] | all | Drive-letter parsing |
//! | [`error`] | all | [`Error`], [`ParseError`] |
//! | `volume` | Windows | `Volume` (open, info, root FRN, GUID path, journal query/read), `Enumerator` |
//! | `source` | Windows | `NtfsVolumeSource`: `whereabouts_source::Source` for one volume |
//! | `testvol` | Windows, `test-fixtures` | `TestVolume`: a throw-away VHD volume for tests that write |
//! | `process` | Windows | `process_memory` (working set, for benchmarks) |
//!
//! On non-Windows targets only the pure modules exist, so the crate builds and tests on Linux.
//!
//! # Usage (Windows, elevated)
//!
//! ```no_run
//! # #[cfg(windows)]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use whereabouts_ntfs::NtfsVolumeSource;
//! use whereabouts_source::{Changes, Source};
//!
//! let mut source = NtfsVolumeSource::open("D:")?;
//! let mut records = Vec::new();
//! let mut checkpoint = source.enumerate(&mut records)?; // taken before enumerating (D-29)
//! loop {
//!     match source.changes(&checkpoint)? {
//!         Changes::Batch(batch) => {
//!             // apply batch.events, then commit:
//!             checkpoint = batch.checkpoint;
//!         }
//!         Changes::FullRescanRequired(_reason) => break, // enumerate again
//!     }
//!     # break;
//! }
//! # Ok(()) }
//! # #[cfg(not(windows))] fn main() {}
//! ```
//!
//! # Guarantees and limits
//!
//! - **Read-only.** The volume is opened with `GENERIC_READ`; no journal is ever created or
//!   deleted; nothing is written. (The test-only `testvol` helper creates and deletes journals,
//!   on its own VHD only.)
//! - **No journal, no history.** A volume without an active journal can be enumerated, but
//!   every `changes` call reports `FullRescanRequired(JournalMissing)`; creating a journal is
//!   the caller's decision (D-31).
//! - **Elevation.** Opening `\\.\C:` needs administrator rights; a denied open is reported as
//!   [`Error::NotElevated`]. `volume_info` works unelevated.
//! - **64-bit file IDs only** (#24): ReFS is rejected at open (`NotNtfs`); a record with a
//!   128-bit file ID makes the source return `SourceError::Unsupported` instead of truncating.
//! - **Drive letters only** (#25): mount points and `\\?\Volume{GUID}\` paths are not accepted.
//! - **Names** are kept lossless as WTF-8 ([`NtfsName`]): identical to UTF-8 for every valid
//!   name, unpaired UTF-16 surrogates preserved.
//! - **Hard links:** enumeration returns one record per MFT file record, i.e. one of a
//!   hard-linked file's names. Other names are not reported (documented M0 limitation).
//! - **Local volumes only** (D-03): network drives and `SUBST` aliases (which can report NTFS)
//!   are rejected with [`Error::NotLocal`] before anything is opened.
//! - **Raw `$MFT` reads are untrusted input** too: every length, offset and count is validated,
//!   allocations are bounded, and a damaged record is counted and skipped, never a panic. The
//!   raw method is an internal option ([`EnumerationMethod::RawMft`]); the default stays the
//!   USN API until Q-16 is decided.
//! - **Malformed kernel output** never panics: it becomes [`Error::Parse`] or a skipped record.
//! - **Fixture builders** (`record::build`) exist only for this crate's tests or with the
//!   `test-fixtures` feature (enable it from `[dev-dependencies]`).

#![allow(unsafe_code)]

pub mod chain;
pub mod checkpoint;
pub mod drive;
pub mod error;
pub mod journal;
pub mod mft;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod mft_build;
pub mod mft_scan;
pub mod name;
pub mod record;

#[cfg(test)]
mod mft_tests;

#[cfg(windows)]
mod process;
#[cfg(windows)]
mod source;
#[cfg(all(windows, any(test, feature = "test-fixtures")))]
pub mod testvol;
#[cfg(windows)]
mod volume;

pub use chain::ParentChainReport;
pub use checkpoint::{JournalPosition, NtfsCheckpoint};
pub use drive::parse_drive_letter;
pub use error::{Error, ParseError, Result};
pub use journal::{Coalescer, PendingRename};
pub use mft::{MftParseError, VolumeGeometry};
pub use mft_scan::{ByteSource, MftError, MftScanOptions, MftScanStats, MftScanner};
pub use name::NtfsName;
pub use record::{JournalInfo, RawRecord, RecordIter, UsnRecord};

#[cfg(windows)]
pub use process::{ProcessMemory, process_memory};
#[cfg(windows)]
pub use source::{
    ChangeStats, EnumerateStats, EnumerationMethod, NtfsVolumeSource, SOURCE_KIND, SourceOptions,
};
#[cfg(windows)]
pub use volume::{
    EnumOptions, EnumStats, Enumerator, Volume, VolumeInfo, ensure_local, volume_info,
};
