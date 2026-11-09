//! `whereabouts` — M0 command-line prototype (D-20): `index`, `inspect <path>`, `query "<text>"`.
//!
//! Spec: `docs/05-roadmap.md` M0. Commands are added by stage01 sessions 3.3 and 4.1.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use whereabouts_core::{DatasetRecord, Format, VolumeId, Wgs84Extent};

mod inspect;

#[derive(Debug, clap::Args)]
struct QueryArgs {
    /// Literal case-insensitive filename substring; empty selects all datasets.
    #[arg(default_value = "")]
    text: String,
    /// WGS84 west,south,east,north; west > east crosses the antimeridian.
    #[arg(long, value_parser = parse_bbox, allow_hyphen_values = true)]
    bbox: Option<Wgs84Extent>,
    /// Format code, e.g. shp, gpkg, gtiff (repeat or comma-separate).
    #[arg(long, value_delimiter = ',')]
    format: Vec<Format>,
    #[arg(long)]
    epsg: Option<u32>,
    #[arg(long, default_value_t = 100)]
    limit: usize,
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = "whereabouts.db")]
    db: PathBuf,
    /// Snapshot path, defaults to <db>.names. Use a separate snapshot per source.
    #[arg(long)]
    snapshot: Option<PathBuf>,
    /// Catalog volume ID; required when more than one volume has the same checkpoint.
    #[arg(long)]
    volume_id: Option<u32>,
}

#[derive(Debug, Parser)]
#[command(version, about = "GIS-aware local file indexing")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Extract a dataset or member path without creating an index.
    Inspect {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Search saved names and metadata without accessing the indexed volume.
    Query(QueryArgs),
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
        Command::Inspect { path, json } => {
            let records = inspect::inspect(&path)?;
            output(&records, json)?;
            if !json {
                output_layers(&records)?;
            }
            Ok(())
        }
        Command::Query(args) => query(args),
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

fn parse_bbox(value: &str) -> Result<Wgs84Extent, String> {
    let values = value
        .split(',')
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let [west, south, east, north] = values.as_slice() else {
        return Err("bbox requires west,south,east,north".into());
    };
    Wgs84Extent::from_west_south_east_north(*west, *south, *east, *north).map_err(|e| e.to_string())
}

fn query(args: QueryArgs) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use whereabouts_catalog::Catalog;
    use whereabouts_nameindex::NameIndex;
    use whereabouts_query::{QueryEngine, Request};
    let snapshot = NameIndex::load_snapshot(
        &args
            .snapshot
            .unwrap_or_else(|| inspect::snapshot_path(&args.db)),
    )
    .context("cannot load name snapshot; run index first")?;
    let catalog = Catalog::open_read_only(&args.db)?;
    let checkpoint = snapshot
        .checkpoint
        .as_ref()
        .context("snapshot has no checkpoint; run index first")?;
    let mut volumes = Vec::new();
    for volume in catalog.volumes()? {
        if args.volume_id.is_some_and(|id| id != volume.id.0) {
            continue;
        }
        if catalog.checkpoint(volume.id)?.as_ref() == Some(checkpoint) {
            volumes.push(volume.id);
        }
    }
    ensure!(
        volumes.len() == 1,
        "snapshot must match exactly one catalog checkpoint (found {}); use --snapshot/--volume-id or run index again",
        volumes.len()
    );
    let volume: VolumeId = volumes[0];
    let engine = QueryEngine::new(0)?;
    let records = engine.search(
        &snapshot.index,
        &catalog,
        volume,
        &Request {
            text: args.text,
            formats: args.format,
            epsg: args.epsg,
            bbox: args.bbox,
            limit: args.limit,
        },
    )?;
    ensure!(
        catalog.checkpoint(volume)?.as_ref() == Some(checkpoint),
        "catalog changed during query; retry after indexing catches up"
    );
    output(&records, args.json)
}

fn output(records: &[DatasetRecord], json: bool) -> anyhow::Result<()> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    if json {
        serde_json::to_writer_pretty(&mut out, records)?;
        writeln!(out)?;
    } else {
        writeln!(out, "NAME\tFORMAT\tSTATUS\tLAYERS\tEPSG\tPATH")?;
        for record in records {
            let epsg = record
                .layers
                .iter()
                .filter_map(|l| l.crs.as_ref()?.epsg())
                .map(|n| n.to_string())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",");
            writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}\t{}",
                cell(&record.name),
                record.dataset.format().code(),
                record.status.code(),
                record.layers.len(),
                epsg,
                cell(&record.primary_path)
            )?;
        }
    }
    out.flush()?;
    Ok(())
}

fn cell(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

fn output_layers(records: &[DatasetRecord]) -> anyhow::Result<()> {
    use std::io::Write;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for record in records {
        if let Some(reason) = record.status.detail() {
            writeln!(out, "{}: {}", record.status.code(), cell(reason))?;
        }
        for layer in &record.layers {
            writeln!(out, "Layer: {}", cell(&layer.metadata.name))?;
            writeln!(
                out,
                "  Features: {:?}; geometry: {:?}",
                layer.metadata.feature_count, layer.metadata.geometry_type
            )?;
            writeln!(out, "  CRS: {}", serde_json::to_string(&layer.crs)?)?;
            writeln!(
                out,
                "  Native bounds: {}",
                serde_json::to_string(&layer.metadata.native_bounds)?
            )?;
            writeln!(
                out,
                "  WGS84 bounds: {}",
                serde_json::to_string(&layer.wgs84)?
            )?;
            if let Some(raster) = &layer.metadata.raster {
                writeln!(out, "  Raster: {}", serde_json::to_string(raster)?)?;
            }
        }
    }
    out.flush()?;
    Ok(())
}
