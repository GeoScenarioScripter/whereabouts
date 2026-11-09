//! `whereabouts` — M0 command-line prototype (D-20): `index`, `inspect <path>`, `query "<text>"`.
//!
//! Spec: `docs/05-roadmap.md` M0. Commands are added by stage01 sessions 3.3 and 4.1.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about = "GIS-aware local file indexing")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Index a local NTFS volume (requires an elevated Windows shell).
    Index {
        /// Drive letter, for example X:.
        #[arg(long)]
        volume: String,
        /// Catalog database (snapshot and retry state are stored beside it).
        #[arg(long, default_value = "whereabouts.db")]
        db: PathBuf,
        /// Keep polling the journal after initial catch-up. Stop with Ctrl+C.
        #[arg(long)]
        watch: bool,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Index { volume, db, watch } => index(&volume, db, watch),
    }
}

fn index(volume: &str, db: PathBuf, watch: bool) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use whereabouts_indexer::{IndexOptions, Indexer};
        use whereabouts_ntfs::NtfsVolumeSource;
        use whereabouts_source::Source;
        let mut source = NtfsVolumeSource::open(volume)?;
        let options = IndexOptions::new(db, format!("{}\\", source.identity().label));
        eprintln!("Opening {volume}; restoring snapshot or enumerating the volume…");
        let mut indexer = Indexer::open(&source, options)?;
        loop {
            let report = indexer.catch_up(&mut source)?;
            if report.enumerated || report.events != 0 || report.extracted != 0 || !watch {
                eprintln!(
                    "{}: {} names, {} datasets; {} events, {} extractions, {} retries pending",
                    if report.enumerated {
                        "Full index"
                    } else {
                        "Resumed"
                    },
                    report.names,
                    report.datasets,
                    report.events,
                    report.extracted,
                    report.retries
                );
            }
            if !watch {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (volume, db, watch, Duration::ZERO);
        anyhow::bail!("NTFS volume indexing requires Windows and an elevated shell")
    }
}
