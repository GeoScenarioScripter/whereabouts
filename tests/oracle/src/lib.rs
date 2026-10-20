//! GDAL-as-oracle correctness harness (D-08, D-25). Dev/CI only — never a dependency of a shipped
//! crate, and GDAL itself is never linked: it only ran once, in a pinned container, to produce the
//! committed golden files this crate reads.
//!
//! Spec: `docs/04-formats-and-crs.md` §5; corpus layout and record format:
//! `tests/corpus/README.md`.
//!
//! # Pieces
//!
//! | Module | What |
//! |---|---|
//! | [`manifest`] | `tests/corpus/manifest.toml` → [`Corpus`] / [`Case`] (paths, tags, provenance) |
//! | [`record`] | [`OracleRecord`] (the GDAL golden, `tests/corpus/expected/<id>.json`) and [`NativeRecord`] (what an adapter reports) |
//! | [`compare`] | tolerance-aware comparison of one case, per [`Check`] |
//! | [`adapters`] | the adapter registry: `format → fn(case) -> Option<NativeRecord>` (empty until the readers land) |
//! | [`report`] | per-format agreement tallies, markdown report |
//! | [`baseline`] | `tests/oracle/baseline.toml`: tolerances + minimum agreement rates |
//!
//! `tests/agreement.rs` ties them together: [`evaluate`] every case, print the report, fail if any
//! per-format, per-check agreement rate is below the baseline (or an adapter panicked).
//!
//! # Adding a reader (sessions 2.6 / 3.1 / 3.2)
//! Write `fn read(case: &ResolvedCase<'_>) -> Option<NativeRecord>` next to the registry, add one
//! [`Adapter`] line to [`adapters::ADAPTERS`] listing the [`Check`]s it is ready to be held to, and
//! run `cargo test -p whereabouts-oracle`.

pub mod adapters;
pub mod baseline;
pub mod compare;
pub mod manifest;
pub mod record;
pub mod report;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

pub use adapters::{Adapter, ResolvedCase};
pub use baseline::Baseline;
pub use compare::{Check, Tolerances, compare};
pub use manifest::{Case, Corpus};
pub use record::{NativeRecord, OracleRecord};
pub use report::{CaseOutcome, Report, SkipReason};

/// Errors from loading the corpus, golden files or baseline.
#[derive(Debug, thiserror::Error)]
pub enum OracleError {
    /// A file could not be read.
    #[error("reading {path}: {source}")]
    Io {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A TOML file (manifest, baseline) is malformed.
    #[error("parsing {path}: {source}")]
    Toml {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: toml::de::Error,
    },
    /// A golden JSON file is malformed or does not match the record schema.
    #[error("parsing {path}: {source}")]
    Json {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: serde_json::Error,
    },
    /// The manifest, a golden file or the baseline is well-formed but inconsistent.
    #[error("{0}")]
    Invalid(String),
}

/// `tests/corpus/` in this repository.
pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("corpus")
}

/// `tests/oracle/baseline.toml` in this repository.
pub fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("baseline.toml")
}

/// Run every case of `corpus` through its format's adapter and compare against the golden record.
///
/// Cases are skipped (not failed) when their data is absent (unfetched external samples), when no
/// adapter is registered for the format, or when the adapter declines the case. A panicking adapter
/// is caught and reported as [`CaseOutcome::Panicked`] — readers must never panic (AGENTS.md
/// guardrail 5), so the agreement test treats it as a hard failure.
pub fn evaluate(
    corpus: &Corpus,
    adapters: &[Adapter],
    tolerances: &Tolerances,
) -> Result<Report, OracleError> {
    let mut report = Report::default();
    for case in corpus.cases() {
        let outcome = if !corpus.is_present(case) {
            CaseOutcome::Skipped(SkipReason::DataMissing)
        } else {
            // Every present case must have a valid golden, adapter or not: a broken golden is a
            // corpus bug and must not hide behind "skipped".
            let oracle = corpus.load_expected(case)?;
            match adapters.iter().find(|a| a.format == case.format) {
                None => CaseOutcome::Skipped(SkipReason::NoAdapter),
                Some(adapter) => {
                    let resolved = ResolvedCase::new(case, corpus.case_dir(case));
                    match catch_unwind(AssertUnwindSafe(|| (adapter.read)(&resolved))) {
                        Err(payload) => CaseOutcome::Panicked(panic_message(payload.as_ref())),
                        Ok(None) => CaseOutcome::Skipped(SkipReason::AdapterDeclined),
                        Ok(Some(native)) => CaseOutcome::Compared(compare(
                            case,
                            &oracle,
                            &native,
                            adapter.checks,
                            tolerances,
                        )),
                    }
                }
            }
        };
        report.push(case.id.clone(), case.format.clone(), outcome);
    }
    Ok(report)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}
