//! NTFS volume source: `FSCTL_ENUM_USN_DATA` enumeration and USN change-journal tailing.
//!
//! This is the only crate allowed to opt out of the workspace `unsafe_code = "deny"` lint (raw
//! `DeviceIoControl` calls); every `unsafe` block must carry a `// SAFETY:` comment.
//!
//! Spec: `docs/03-architecture.md` §3.1–3.3; stage01 sessions 1.3 (enumeration, this) and 2.2
//! (journal tailing + the `Source` implementation).
//!
//! # Layout
//!
//! | Module | Platform | What |
//! |---|---|---|
//! | [`record`] | all | Pure parsers: `USN_RECORD_V2/V3`, `FSCTL_ENUM_USN_DATA` output, `USN_JOURNAL_DATA`, `MFT_ENUM_DATA_V1` encoding; fixture builders |
//! | [`name`] | all | [`NtfsName`]: lossless (WTF-8) file names |
//! | [`chain`] | all | [`ParentChainReport`]: do parent links resolve to the root? |
//! | [`drive`] | all | Drive-letter parsing |
//! | [`error`] | all | [`Error`], [`ParseError`] |
//! | `volume` | Windows | [`Volume`] (open, info, root FRN, journal query), [`Enumerator`] |
//! | `process` | Windows | [`process_memory`] (working set, for benchmarks) |
//!
//! On non-Windows targets only the pure modules exist, so the crate builds and tests on Linux.
//!
//! # Usage (Windows, elevated)
//!
//! ```no_run
//! # #[cfg(windows)]
//! # fn main() -> Result<(), whereabouts_ntfs::Error> {
//! use whereabouts_ntfs::Volume;
//! let volume = Volume::open("C:")?;
//! let journal = volume.query_journal()?; // None = no active journal (never created by us)
//! let mut count = 0;
//! for record in volume.enumerate() {
//!     let record = record?;
//!     count += usize::from(!record.is_directory());
//! }
//! # Ok(()) }
//! # #[cfg(not(windows))] fn main() {}
//! ```
//!
//! # Guarantees and limits
//!
//! - **Read-only.** The volume is opened with `GENERIC_READ`; no journal is ever created or
//!   deleted; nothing is written.
//! - **Elevation.** Opening `\\.\C:` needs administrator rights; a denied open is reported as
//!   [`Error::NotElevated`]. [`volume_info`] works unelevated.
//! - **Names** are kept lossless as WTF-8 ([`NtfsName`]): identical to UTF-8 for every valid
//!   name, unpaired UTF-16 surrogates preserved.
//! - **Hard links:** enumeration returns one record per MFT file record, i.e. one of a
//!   hard-linked file's names. Other names are not reported (documented M0 limitation).
//! - **Malformed kernel output** never panics: it becomes [`Error::Parse`] or a skipped record.

#![allow(unsafe_code)]

pub mod chain;
pub mod drive;
pub mod error;
pub mod name;
pub mod record;

#[cfg(windows)]
mod process;
#[cfg(windows)]
mod volume;

pub use chain::ParentChainReport;
pub use drive::parse_drive_letter;
pub use error::{Error, ParseError, Result};
pub use name::NtfsName;
pub use record::{JournalInfo, RawRecord, RecordIter, UsnRecord};

#[cfg(windows)]
pub use process::{ProcessMemory, process_memory};
#[cfg(windows)]
pub use volume::{EnumOptions, EnumStats, Enumerator, Volume, VolumeInfo, volume_info};
