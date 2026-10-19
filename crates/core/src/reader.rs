//! The `FormatReader` contract: native, bounded, read-only metadata extraction (D-07).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{DatasetMetadata, ExtractionStatus, Format, MemberRole};

/// A dataset resolved to paths, ready for a reader. Built by the indexer (from a [`Dataset`] and
/// the name index) or by `whereabouts inspect` (from a path, without an index).
///
/// [`Dataset`]: crate::Dataset
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDataset {
    /// The dataset's format; selects the reader.
    pub format: Format,
    /// Path of the anchor file or folder (role `Main`).
    pub anchor: PathBuf,
    /// Every other member with its role (sidecars). Does not contain the anchor.
    pub sidecars: Vec<ResolvedMember>,
}

/// A non-anchor member file of a [`ResolvedDataset`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedMember {
    /// The member's role.
    pub role: MemberRole,
    /// The member's path.
    pub path: PathBuf,
}

impl ResolvedDataset {
    /// Path of the first sidecar with `role`, if any.
    pub fn sidecar(&self, role: MemberRole) -> Option<&Path> {
        self.sidecars
            .iter()
            .find(|m| m.role == role)
            .map(|m| m.path.as_path())
    }

    /// Paths of all sidecars with `role`.
    pub fn sidecars_with(&self, role: MemberRole) -> impl Iterator<Item = &Path> + '_ {
        self.sidecars
            .iter()
            .filter(move |m| m.role == role)
            .map(|m| m.path.as_path())
    }
}

/// Budgets a reader must respect. Readers stop and report `partial` (or fail) rather than exceed
/// them. Defaults are a starting point; the indexer may tune them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadLimits {
    /// Upper bound on bytes read from all member files together for one dataset. Header readers
    /// (SHP, GeoTIFF, …) need far less; streaming formats (GeoJSON, KML) hit it on large files
    /// and must then report `partial`.
    pub max_bytes: u64,
    /// Wall-clock budget for one dataset (e.g. GeoPackage `count(*)` fallback).
    pub time_budget: Duration,
}

impl Default for ReadLimits {
    /// 16 MiB and 2 s.
    fn default() -> Self {
        ReadLimits {
            max_bytes: 16 * 1024 * 1024,
            time_budget: Duration::from_secs(2),
        }
    }
}

/// Why a reader produced no metadata at all. Partial results are `Ok` with
/// [`DatasetMetadata::partial`] set instead.
#[derive(Debug, Error)]
pub enum ReadError {
    /// A member could not be opened or read.
    #[error("I/O error on {}: {source}", path.display())]
    Io {
        /// The file being read.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The content is not valid for the format (bad magic, impossible length/offset, truncation…).
    #[error("malformed {format} data: {reason}")]
    Malformed {
        /// The format being read.
        format: Format,
        /// What was wrong, with enough context to find it (file, offset).
        reason: String,
    },
    /// A member the reader cannot work without is absent (e.g. a shapefile with no `.shp`).
    #[error("missing required {0} member")]
    MissingMember(MemberRole),
    /// Valid data using a feature this reader does not handle (or no reader for the format).
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl ReadError {
    /// The extraction status this error maps to: `Unsupported` for [`ReadError::Unsupported`],
    /// `Error(<message>)` otherwise.
    pub fn status(&self) -> ExtractionStatus {
        match self {
            ReadError::Unsupported(_) => ExtractionStatus::Unsupported,
            other => ExtractionStatus::Error(other.to_string()),
        }
    }
}

/// A native metadata reader for one or more formats.
///
/// # Contract (non-negotiable, see `AGENTS.md` guardrail 5)
/// - **Bounded reads.** Read only the bytes needed for the facts in [`DatasetMetadata`] — headers,
///   directories, metadata blocks — and never more than [`ReadLimits::max_bytes`] in total. Use
///   seeks, not full-file reads. Respect [`ReadLimits::time_budget`].
/// - **Read-only.** Never write to, lock, truncate, touch the mtime of, or create sidecars
///   (`-wal`, `-shm`, `.aux.xml`, …) for user files. Open with read-only, share-friendly modes.
/// - **Never panic on file contents.** Every length, offset and count from a file is untrusted:
///   validate it before use, use checked arithmetic, no `unwrap`/indexing on input-derived data.
///   Malformed input → [`ReadError::Malformed`]; degraded but usable → `Ok` with
///   [`DatasetMetadata::partial`] reasons.
/// - **Don't interpret CRSs.** Return the raw [`CrsDefinition`](crate::CrsDefinition); parsing,
///   identification and WGS84 transforms belong to `whereabouts-crs`.
/// - **Deterministic layer order**, since the index is the layer's `LayerId::ordinal`.
/// - Stateless and thread-safe: one instance serves many worker threads.
pub trait FormatReader: Send + Sync {
    /// Short reader name for logs and reports, e.g. `"shapefile"`.
    fn name(&self) -> &'static str;

    /// The formats this reader handles.
    fn formats(&self) -> &'static [Format];

    /// Extracts metadata for one dataset within `limits`.
    fn read(
        &self,
        dataset: &ResolvedDataset,
        limits: &ReadLimits,
    ) -> Result<DatasetMetadata, ReadError>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    struct Fake;

    impl FormatReader for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn formats(&self) -> &'static [Format] {
            &[Format::Shapefile]
        }

        fn read(&self, ds: &ResolvedDataset, _: &ReadLimits) -> Result<DatasetMetadata, ReadError> {
            match ds.sidecar(MemberRole::Shx) {
                Some(_) => Ok(DatasetMetadata::default()),
                None => Err(ReadError::MissingMember(MemberRole::Shx)),
            }
        }
    }

    #[test]
    fn trait_is_object_safe_and_errors_map_to_status() {
        let readers: Vec<Box<dyn FormatReader>> = vec![Box::new(Fake)];
        let mut ds = ResolvedDataset {
            format: Format::Shapefile,
            anchor: PathBuf::from("a.shp"),
            sidecars: vec![ResolvedMember {
                role: MemberRole::Prj,
                path: "a.prj".into(),
            }],
        };
        let r = &readers[0];
        let err = r.read(&ds, &ReadLimits::default()).unwrap_err();
        assert_eq!(
            err.status(),
            ExtractionStatus::Error("missing required shx member".into())
        );
        ds.sidecars.push(ResolvedMember {
            role: MemberRole::Shx,
            path: "a.shx".into(),
        });
        assert!(r.read(&ds, &ReadLimits::default()).is_ok());
        assert_eq!(ds.sidecar(MemberRole::Prj), Some(Path::new("a.prj")));
        assert_eq!(
            ReadError::Unsupported("x".into()).status(),
            ExtractionStatus::Unsupported
        );
        let json = serde_json::to_string(&ds).unwrap();
        assert_eq!(serde_json::from_str::<ResolvedDataset>(&json).unwrap(), ds);
        let malformed = ReadError::Malformed {
            format: Format::Shapefile,
            reason: "bad".into(),
        };
        assert_eq!(malformed.to_string(), "malformed shp data: bad");
    }
}
