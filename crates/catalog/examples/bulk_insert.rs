//! Catalog throughput benchmark: batch-inserts synthetic datasets into a fresh temporary catalog,
//! then measures re-upserts, bbox queries and member lookups.
//!
//! ```sh
//! cargo run --release -p whereabouts-catalog --example bulk_insert -- [datasets] [batch]
//! ```
//! Defaults: 100,000 datasets, committed in batches of 1,000. Each dataset is a shapefile with
//! four member files and one layer; ~2 % of extents cross the antimeridian.

#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use whereabouts_catalog::{Catalog, DatasetFilter};
use whereabouts_core::{
    Bounds, CrsDefinition, Dataset, DatasetId, DatasetKind, DatasetRecord, ExtractionStatus,
    FieldInfo, FieldKind, Format, Frn, GeometryKind, GeometryType, LayerMetadata, LayerRecord,
    Member, MemberRole, ResolvedCrs, Timestamp, VolumeId, Wgs84Extent,
};
use whereabouts_source::SourceIdentity;

/// Deterministic xorshift PRNG (no dependency; reproducible numbers).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next() >> 11) as f64 / (1_u64 << 53) as f64;
        lo + unit * (hi - lo)
    }
}

const EPSG_POOL: [u32; 8] = [4326, 3857, 2193, 25832, 32633, 27700, 3460, 2056];

fn record(volume: VolumeId, i: u64, rng: &mut Rng) -> DatasetRecord {
    let anchor = 1_000 + i * 8;
    let members = [
        (0, MemberRole::Main),
        (1, MemberRole::Shx),
        (2, MemberRole::Dbf),
        (3, MemberRole::Prj),
    ]
    .map(|(o, role)| Member {
        frn: Frn(anchor + o),
        role,
    });
    let epsg = EPSG_POOL[(rng.next() % EPSG_POOL.len() as u64) as usize];
    let (w, s) = (rng.range(-180.0, 180.0), rng.range(-85.0, 84.0));
    let size = rng.range(0.001, 1.0);
    let (e, n) = (w + size, s + size);
    let wgs84 = if e > 180.0 {
        Wgs84Extent::from_west_south_east_north(w, s, e - 360.0, n).unwrap()
    } else {
        Wgs84Extent::from_west_south_east_north(w, s, e, n).unwrap()
    };
    DatasetRecord {
        dataset: Dataset::new(
            DatasetId::new(volume, Frn(anchor)),
            DatasetKind::Vector,
            Format::Shapefile,
            members.to_vec(),
        )
        .unwrap(),
        name: format!("dataset_{i}"),
        primary_path: format!(r"D:\gis\project_{}\dataset_{i}.shp", i % 997),
        total_size: Some(rng.next() % 100_000_000),
        mtime: Some(Timestamp(1_700_000_000_000_000_000 + i as i64)),
        status: ExtractionStatus::Ok,
        extracted_at: Some(Timestamp(1_700_000_000_000_000_000)),
        layers: vec![LayerRecord {
            metadata: LayerMetadata {
                name: format!("dataset_{i}"),
                geometry_type: Some(GeometryType::xy(GeometryKind::Polygon)),
                feature_count: Some(rng.next() % 1_000_000),
                crs: Some(CrsDefinition::epsg(epsg)),
                native_bounds: Some(Bounds::new(w, s, w + size, n).unwrap()),
                fields: vec![
                    FieldInfo {
                        name: "id".into(),
                        kind: FieldKind::Integer64,
                        width: Some(18),
                        precision: Some(0),
                    },
                    FieldInfo {
                        name: "name".into(),
                        kind: FieldKind::String,
                        width: Some(80),
                        precision: None,
                    },
                ],
                raster: None,
            },
            crs: Some(ResolvedCrs::Identified { epsg, name: None }),
            wgs84: Some(wgs84),
        }],
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let n: u64 = args.next().map_or(100_000, |a| a.parse().unwrap());
    let batch: u64 = args.next().map_or(1_000, |a| a.parse().unwrap());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bench.db");
    let mut cat = Catalog::open(&path).unwrap();
    let volume = cat
        .write(|tx| {
            tx.register_volume(&SourceIdentity {
                kind: "ntfs".into(),
                key: "bench".into(),
                label: "D:".into(),
            })
        })
        .unwrap();

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let records: Vec<DatasetRecord> = (0..n).map(|i| record(volume, i, &mut rng)).collect();

    let insert = |cat: &mut Catalog| {
        let t = Instant::now();
        for chunk in records.chunks(batch as usize) {
            cat.write(|tx| {
                for r in chunk {
                    tx.upsert_dataset(r)?;
                }
                Ok(())
            })
            .unwrap();
        }
        t.elapsed()
    };
    let first = insert(&mut cat);
    let again = insert(&mut cat);
    let stats = cat.stats().unwrap();
    let db_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let wal_bytes = std::fs::metadata(path.with_extension("db-wal"))
        .map(|m| m.len())
        .unwrap_or(0);

    // bbox queries: 1,000 random 1°×1° boxes (2 % crossing the antimeridian).
    let mut times = Vec::new();
    let mut hits = 0;
    for _ in 0..1_000 {
        let w = rng.range(-180.0, 180.0);
        let s = rng.range(-85.0, 84.0);
        let e = if w + 1.0 > 180.0 {
            w + 1.0 - 360.0
        } else {
            w + 1.0
        };
        let filter = DatasetFilter {
            bbox: Some(Wgs84Extent::from_west_south_east_north(w, s, e, s + 1.0).unwrap()),
            ..DatasetFilter::default()
        };
        let t = Instant::now();
        hits += cat.find_datasets(&filter).unwrap().len();
        times.push(t.elapsed());
    }
    times.sort();

    // EPSG + bbox (one layer must satisfy both), 1,000 queries of 10°×10°.
    let mut epsg_times = Vec::new();
    for i in 0..1_000_u64 {
        let w = rng.range(-180.0, 170.0);
        let s = rng.range(-85.0, 75.0);
        let filter = DatasetFilter {
            epsg: Some(EPSG_POOL[(i % EPSG_POOL.len() as u64) as usize]),
            bbox: Some(Wgs84Extent::from_west_south_east_north(w, s, w + 10.0, s + 10.0).unwrap()),
            ..DatasetFilter::default()
        };
        let t = Instant::now();
        let _ = cat.find_datasets(&filter).unwrap();
        epsg_times.push(t.elapsed());
    }
    epsg_times.sort();

    // Member lookups (name-index hit → dataset): 10,000 random sidecar FRNs.
    let t = Instant::now();
    for _ in 0..10_000 {
        let i = rng.next() % n;
        let found = cat
            .datasets_with_member(volume, Frn(1_000 + i * 8 + 2))
            .unwrap();
        assert_eq!(found.len(), 1);
    }
    let lookup = t.elapsed() / 10_000;

    // Full record reads.
    let t = Instant::now();
    for _ in 0..10_000 {
        let i = rng.next() % n;
        cat.get_dataset(DatasetId::new(volume, Frn(1_000 + i * 8)))
            .unwrap()
            .unwrap();
    }
    let get = t.elapsed() / 10_000;

    let rate = |d: Duration| n as f64 / d.as_secs_f64();
    println!("datasets:            {n} (batches of {batch})");
    println!("rows:                {stats:?}");
    println!(
        "first insert:        {first:.2?} ({:.0} datasets/s)",
        rate(first)
    );
    println!(
        "re-upsert (same):    {again:.2?} ({:.0} datasets/s)",
        rate(again)
    );
    println!(
        "db size:             {:.1} MiB (+ WAL {:.1} MiB), {:.0} bytes/dataset",
        db_bytes as f64 / 1_048_576.0,
        wal_bytes as f64 / 1_048_576.0,
        db_bytes as f64 / n as f64
    );
    println!(
        "bbox 1°x1°:          p50 {:.1?}  p95 {:.1?}  p99 {:.1?}  (avg {:.1} hits)",
        percentile(&times, 0.50),
        percentile(&times, 0.95),
        percentile(&times, 0.99),
        hits as f64 / 1_000.0
    );
    println!(
        "epsg + bbox 10°x10°: p50 {:.1?}  p95 {:.1?}  p99 {:.1?}",
        percentile(&epsg_times, 0.50),
        percentile(&epsg_times, 0.95),
        percentile(&epsg_times, 0.99)
    );
    println!("member lookup:       {lookup:.1?} avg");
    println!("get_dataset:         {get:.1?} avg");
}
