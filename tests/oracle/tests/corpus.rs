//! Corpus integrity: manifest ↔ data ↔ goldens stay consistent, the committed corpus stays small,
//! and every edge case the stage01 plan lists stays covered.

use std::collections::BTreeSet;
use std::fs;

use whereabouts_oracle::manifest::{Location, Origin};
use whereabouts_oracle::record::{OracleStatus, SCHEMA};
use whereabouts_oracle::{Corpus, corpus_dir};

/// Committed corpus budget (stage01 plan, session 1.2).
const MAX_COMMITTED_BYTES: u64 = 10_000_000;

/// Edge cases session 1.2 must cover (tests/corpus/README.md has the full tag vocabulary).
const REQUIRED_TAGS: &[&str] = &[
    // projection families
    "crs:utm",
    "crs:lcc",
    "crs:albers",
    "crs:webmercator",
    "crs:polar-stereo",
    "crs:geographic",
    "crs:nztm",
    "crs:antimeridian",
    "crs:esri-wkt",
    "crs:no-authority",
    "crs:none",
    // shapefile encodings
    "enc:cpg",
    "enc:ldid",
    // shapefile
    "empty",
    // geopackage
    "multi-layer",
    "gpkg:stale-bounds",
    "gpkg:missing-bounds",
    "gpkg:attributes",
    "gpkg:tiles",
    // geotiff
    "tiff:north-up",
    "tiff:rotated",
    "tiff:pixel-is-point",
    "tiff:bigtiff",
    "tiff:cog",
    "tiff:overviews",
    "tiff:aux-xml",
    "tiff:worldfile",
    "tiff:nodata",
    // robustness
    "corrupt",
    "real-world",
];

fn corpus() -> Corpus {
    Corpus::load(corpus_dir()).expect("tests/corpus/manifest.toml loads")
}

#[test]
fn at_least_30_committed_cases() {
    let c = corpus();
    let committed = c
        .cases()
        .iter()
        .filter(|k| k.location == Location::Committed)
        .count();
    assert!(committed >= 30, "only {committed} committed cases");
}

#[test]
fn every_required_edge_case_is_covered() {
    let c = corpus();
    let tags: BTreeSet<&str> = c
        .cases()
        .iter()
        .filter(|k| k.location == Location::Committed)
        .flat_map(|k| k.tags.iter().map(String::as_str))
        .collect();
    let missing: Vec<_> = REQUIRED_TAGS
        .iter()
        .filter(|t| !tags.contains(*t))
        .collect();
    assert!(missing.is_empty(), "uncovered edge cases: {missing:?}");
    for format in whereabouts_oracle::manifest::KNOWN_FORMATS {
        assert!(
            c.cases()
                .iter()
                .any(|k| k.format == *format && k.is_corrupt()),
            "no corrupt case for {format}"
        );
    }
}

#[test]
fn committed_case_dirs_hold_exactly_the_listed_files() {
    let c = corpus();
    for case in c
        .cases()
        .iter()
        .filter(|k| k.location == Location::Committed)
    {
        let dir = c.case_dir(case);
        let mut on_disk: Vec<String> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| {
                e.expect("dir entry")
                    .file_name()
                    .into_string()
                    .expect("utf-8 file name")
            })
            .collect();
        on_disk.sort();
        let mut listed = case.files.clone();
        listed.sort();
        assert_eq!(
            on_disk, listed,
            "{}: files on disk differ from manifest",
            case.id
        );
    }
}

#[test]
fn no_orphan_data_dirs_or_goldens() {
    let c = corpus();
    let ids: BTreeSet<&str> = c.cases().iter().map(|k| k.id.as_str()).collect();
    for entry in fs::read_dir(c.root().join("data")).expect("data dir") {
        let format_dir = entry.expect("entry").path();
        for case_dir in fs::read_dir(&format_dir).expect("format dir") {
            let path = case_dir.expect("entry").path();
            let id = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let case = c.case(id);
            assert!(case.is_some(), "{} is not in the manifest", path.display());
            assert_eq!(
                case.map(|k| k.format.as_str()),
                format_dir.file_name().and_then(|n| n.to_str()),
                "{} is under the wrong format directory",
                path.display()
            );
        }
    }
    for entry in fs::read_dir(c.root().join("expected")).expect("expected dir") {
        let path = entry.expect("entry").path();
        let stem = path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        assert!(
            path.extension().is_some_and(|e| e == "json") && ids.contains(stem),
            "{} has no manifest case",
            path.display()
        );
    }
}

#[test]
fn committed_corpus_fits_the_size_budget() {
    let c = corpus();
    let total: u64 = c
        .cases()
        .iter()
        .filter(|k| k.location == Location::Committed)
        .flat_map(|k| {
            let dir = c.case_dir(k);
            k.files.iter().map(move |f| dir.join(f))
        })
        .map(|p| fs::metadata(&p).map(|m| m.len()).unwrap_or(0))
        .sum();
    assert!(
        total <= MAX_COMMITTED_BYTES,
        "committed corpus is {total} bytes (budget {MAX_COMMITTED_BYTES})"
    );
}

#[test]
fn every_case_has_a_valid_golden_from_one_oracle_version() {
    let c = corpus();
    let mut versions = BTreeSet::new();
    for case in c.cases() {
        // External goldens are committed too, even though their data is fetched on demand.
        let rec = c
            .load_expected(case)
            .unwrap_or_else(|e| panic!("{}: {e}", case.id));
        assert_eq!(rec.schema, SCHEMA);
        versions.insert((rec.oracle.gdal.clone(), rec.oracle.proj.clone()));
        if rec.status == OracleStatus::Ok {
            assert!(!rec.layers.is_empty(), "{}: ok but no layers", case.id);
        } else {
            assert!(rec.error.is_some(), "{}: error without message", case.id);
            assert!(
                case.is_corrupt(),
                "{}: GDAL refused a non-corrupt case",
                case.id
            );
        }
        for layer in &rec.layers {
            if let Some([x0, y0, x1, y1]) = layer.extent {
                assert!(x0 <= x1 && y0 <= y1, "{}: inverted extent", case.id);
            }
            if let Some([w, s, e, n]) = layer.wgs84_extent {
                assert!(
                    (-90.0..=90.0).contains(&s) && s <= n && n <= 90.0,
                    "{}",
                    case.id
                );
                assert!((-180.0..=180.0).contains(&w) && (-180.0..=180.0).contains(&e));
                assert_eq!(layer.crosses_antimeridian, w > e, "{}", case.id);
            }
        }
    }
    assert_eq!(
        versions.len(),
        1,
        "goldens from mixed oracle versions: {versions:?}"
    );
}

#[test]
fn provenance_is_recorded() {
    let c = corpus();
    for case in c.cases() {
        match case.origin {
            Origin::Synthetic => {
                assert!(case.provenance.contains("generate.py"), "{}", case.id);
                assert_eq!(case.location, Location::Committed, "{}", case.id);
            }
            Origin::RealWorld => {
                assert!(case.provenance.starts_with("https://"), "{}", case.id);
                assert!(case.has_tag("real-world"), "{}", case.id);
            }
        }
    }
}

#[test]
fn empty_registry_reports_everything_as_skipped() {
    let c = corpus();
    let report = whereabouts_oracle::evaluate(&c, &[], &Default::default()).expect("evaluate");
    assert_eq!(report.cases.len(), c.cases().len());
    assert!(report.disagreements().is_empty());
    for (format, summary) in report.by_format() {
        assert_eq!(summary.compared, 0, "{format}");
        assert!(summary.checks.is_empty(), "{format}");
    }
}

/// Feeding every golden back in as if a perfect reader produced it must agree on every check:
/// exercises the comparison rules (NaN nodata, antimeridian, rasters vs vectors) on real records.
#[test]
fn a_perfect_reader_agrees_on_every_check() {
    use whereabouts_oracle::record::{NativeLayer, NativeRaster, NativeRecord, NativeStatus};
    use whereabouts_oracle::{Check, Tolerances, compare};

    let c = corpus();
    let mut compared = 0;
    for case in c.cases().iter().filter(|k| !k.is_corrupt()) {
        let rec = c.load_expected(case).expect("golden");
        let native = NativeRecord {
            status: NativeStatus::Ok,
            layers: rec
                .layers
                .iter()
                .map(|l| NativeLayer {
                    name: l.name.clone(),
                    geometry_type: l.geometry_type.clone(),
                    has_z: l.has_z,
                    has_m: l.has_m,
                    feature_count: l.feature_count,
                    extent: l.extent,
                    raster: l.raster.as_ref().map(|r| NativeRaster {
                        width: r.width,
                        height: r.height,
                        band_count: r.band_count,
                        data_types: Some(r.data_types.clone()),
                        nodata: Some(r.nodata.clone()),
                        resolution: r.resolution,
                    }),
                    epsg: l.crs.epsg,
                    wgs84_extent: l.wgs84_extent,
                })
                .collect(),
        };
        let results = compare(case, &rec, &native, &Check::ALL, &Tolerances::default());
        assert!(
            results.iter().all(|r| r.verdict.agrees()),
            "{}: {results:?}",
            case.id
        );
        compared += results.len();
    }
    assert!(compared > 100, "only {compared} checks evaluated");
}
