//! Bounded-read check over the committed shapefile corpus (`tests/corpus/data/shapefile`, plus
//! `tests/corpus/external/shapefile` when fetched): every dataset is read with < 64 KiB, and the
//! table of bytes per dataset is printed (run with `--nocapture` to see it).
//!
//! Agreement with GDAL is measured by the oracle (`cargo test -p whereabouts-oracle`); this test
//! only adds the I/O bound and a few facts the oracle record does not compare (fields, CRS kind).

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use whereabouts_core::{
    CrsDefinitionKind, DatasetMetadata, FieldKind, Format, MemberRole, ReadError, ReadLimits,
    ResolvedDataset, ResolvedMember,
};

const BOUND: u64 = 64 * 1024;

fn corpus_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let mut dirs = Vec::new();
    for base in [root.join("data/shapefile"), root.join("external/shapefile")] {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            }
        }
    }
    dirs.sort();
    dirs
}

/// Resolves a case directory: the one `.shp` is the anchor, known sidecars by extension.
fn resolve(dir: &Path) -> ResolvedDataset {
    let mut anchor = None;
    let mut sidecars = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        let role = match ext.as_deref() {
            Some("shp") => {
                anchor = Some(path);
                continue;
            }
            Some("shx") => MemberRole::Shx,
            Some("dbf") => MemberRole::Dbf,
            Some("prj") => MemberRole::Prj,
            Some("cpg") => MemberRole::Cpg,
            _ => MemberRole::Other,
        };
        sidecars.push(ResolvedMember { role, path });
    }
    ResolvedDataset {
        format: Format::Shapefile,
        anchor: anchor.unwrap(),
        sidecars,
    }
}

fn read(
    dir: &Path,
) -> (
    Result<DatasetMetadata, ReadError>,
    whereabouts_readers::IoStats,
) {
    let ds = resolve(dir);
    whereabouts_readers::measure(|| whereabouts_readers::read(&ds, &ReadLimits::default()))
}

#[test]
fn every_corpus_shapefile_reads_under_64_kib() {
    let dirs = corpus_dirs();
    assert!(dirs.len() >= 20, "corpus not found ({} dirs)", dirs.len());
    println!("| case | status | files | reads | bytes read |\n|---|---|---:|---:|---:|");
    let mut max = 0;
    for dir in &dirs {
        let case = dir.file_name().unwrap().to_string_lossy();
        let (result, stats) = read(dir);
        let status = match &result {
            Ok(md) => md.status().code().to_owned(),
            Err(e) => e.status().code().to_owned(),
        };
        println!(
            "| {case} | {status} | {} | {} | {} |",
            stats.files_opened, stats.reads, stats.bytes_read
        );
        assert!(
            stats.bytes_read < BOUND,
            "{case}: {} bytes",
            stats.bytes_read
        );
        max = max.max(stats.bytes_read);
        // Exact outcome per case, so an Io error from a wrongly resolved anchor can't pass.
        match &*case {
            "shp_corrupt_truncated_header" | "shp_corrupt_truncated_records" => {
                assert!(
                    matches!(
                        &result,
                        Err(ReadError::Malformed {
                            format: Format::Shapefile,
                            ..
                        })
                    ),
                    "{case}: expected Malformed, got {result:?}"
                );
            }
            "shp_corrupt_bad_dbf" => {
                let md = result.unwrap();
                assert_eq!(md.partial.len(), 1, "{case}: {:?}", md.partial);
                assert!(md.partial[0].contains("beyond the end"), "{:?}", md.partial);
                let layer = &md.layers[0];
                assert!(layer.fields.is_empty());
                assert_eq!(layer.feature_count, Some(9));
                assert!(layer.native_bounds.is_some());
            }
            other => {
                assert!(
                    !other.contains("corrupt"),
                    "{other}: add its expected outcome"
                );
                let md = result.unwrap();
                assert!(md.partial.is_empty(), "{case}: {:?}", md.partial);
            }
        }
    }
    println!("max bytes read per dataset: {max}");
}

#[test]
fn corpus_facts_beyond_the_oracle_record() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/data/shapefile");

    // Natural Earth land: ESRI .prj, UTF-8 .cpg, OGR field types.
    let (md, _) = read(&data.join("shp_ne_110m_land"));
    let layer = &md.unwrap().layers[0];
    assert_eq!(
        layer.crs.as_ref().unwrap().kind,
        CrsDefinitionKind::Wkt1Esri
    );
    let fields: Vec<_> = layer
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.kind, f.width))
        .collect();
    assert_eq!(
        fields,
        [
            ("featurecla", FieldKind::String, Some(15)),
            ("scalerank", FieldKind::Integer, Some(4)),
            ("min_zoom", FieldKind::Real, Some(6)),
        ]
    );

    // GDAL-written synthetic cases: `id` N(10) → Integer64, `name` C(n) → String.
    for (case, width) in [("shp_utm33n_points", 32), ("shp_cpg_cp936", 40)] {
        let (md, _) = read(&data.join(case));
        let fields: Vec<_> = md.unwrap().layers[0]
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.kind, f.width))
            .collect();
        assert_eq!(
            fields,
            [
                ("id".to_owned(), FieldKind::Integer64, Some(10)),
                ("name".to_owned(), FieldKind::String, Some(width)),
            ],
            "{case}"
        );
    }

    // No .prj → no CRS, still a complete read.
    let (md, _) = read(&data.join("shp_no_prj"));
    let md = md.unwrap();
    assert!(md.partial.is_empty());
    assert_eq!(md.layers[0].crs, None);
}
