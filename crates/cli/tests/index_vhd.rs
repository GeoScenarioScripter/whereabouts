//! Elevated end-to-end acceptance. Every write is on an isolated VHD or a temp directory.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use whereabouts_catalog::Catalog;
use whereabouts_core::ExtractionStatus;
use whereabouts_ntfs::testvol::TestVolume;

fn index(volume: &str, db: &Path) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_whereabouts"))
        .args(["index", "--volume", volume, "--db"])
        .arg(db)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(60) {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "index did not drain within 60s: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{stderr}");
    eprintln!("[index-vhd] {stderr}");
    stderr
}

#[test]
#[ignore = "requires elevation; creates an isolated NTFS VHD and USN journal"]
fn corpus_full_index_restart_and_prj_edit() {
    let volume = TestVolume::create(64).unwrap();
    volume.create_journal(8 << 20, 1 << 20).unwrap();
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    for (dir, case) in [
        ("a", "shapefile/shp_utm33n_points"),
        ("b", "shapefile/shp_utm33n_points"),
        ("c", "geopackage/gpkg_multi_layer"),
        ("d", "geotiff/tif_northup_utm"),
    ] {
        fs::create_dir(volume.path(dir)).unwrap();
        for entry in fs::read_dir(corpus.join("data").join(case)).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), volume.path(dir).join(entry.file_name())).unwrap();
        }
    }
    // Persistence on the indexed volume must not make the journal drain loop feed itself.
    let db = volume.path("catalog.db");
    assert!(index(&volume.volume(), &db).contains("Full index"));
    let catalog = Catalog::open_read_only(&db).unwrap();
    let vol = catalog.volumes().unwrap()[0].id;
    let ids = catalog.dataset_ids(vol).unwrap();
    assert_eq!(
        ids.len(),
        4,
        "sidecars must not become independent datasets"
    );
    for id in &ids {
        let row = catalog.get_dataset(*id).unwrap().unwrap();
        let (case, members) = match row.dataset.format() {
            whereabouts_core::Format::Shapefile => ("shp_utm33n_points", 4),
            whereabouts_core::Format::GeoPackage => ("gpkg_multi_layer", 1),
            whereabouts_core::Format::GeoTiff => ("tif_northup_utm", 1),
            format => panic!("unexpected dataset format {format:?}"),
        };
        let golden: serde_json::Value = serde_json::from_slice(
            &fs::read(corpus.join("expected").join(format!("{case}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(row.status, ExtractionStatus::Ok, "{case}");
        assert_eq!(row.dataset.members().len(), members, "{case}");
        let expected = golden["layers"].as_array().unwrap();
        assert_eq!(row.layers.len(), expected.len(), "{case}");
        for layer in &row.layers {
            let golden = expected
                .iter()
                .find(|g| g["name"].as_str() == Some(&layer.metadata.name))
                .unwrap();
            assert_eq!(
                layer.metadata.feature_count,
                golden["feature_count"].as_u64()
            );
            assert_eq!(
                layer.crs.as_ref().unwrap().epsg().map(u64::from),
                golden["crs"]["epsg"].as_u64()
            );
            let b = layer.metadata.native_bounds.unwrap();
            for (actual, expected) in [b.min_x(), b.min_y(), b.max_x(), b.max_y()]
                .iter()
                .zip(golden["extent"].as_array().unwrap())
            {
                assert!((actual - expected.as_f64().unwrap()).abs() < 1e-6);
            }
            assert!(layer.wgs84.is_some(), "{case}: {}", layer.metadata.name);
        }
    }
    let before: Vec<_> = ids
        .iter()
        .map(|id| catalog.get_dataset(*id).unwrap().unwrap())
        .collect();
    drop(catalog);
    // Each command is a fresh unprivileged-style process consuming persisted state.
    // Query does not enumerate; member-only names must return their owning datasets.
    let query = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_whereabouts"))
            .arg("query")
            .args(args)
            .arg("--json")
            .arg("--db")
            .arg(&db)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Vec<whereabouts_core::DatasetRecord>>(&output.stdout).unwrap()
    };
    assert_eq!(query(&[""]).len(), 4);
    let sidecars = query(&["points.prj"]);
    assert_eq!(sidecars.len(), 2);
    assert!(
        sidecars
            .iter()
            .all(|r| r.dataset.format() == whereabouts_core::Format::Shapefile)
    );
    assert_eq!(query(&["", "--format", "gpkg"]).len(), 1);
    assert_eq!(query(&["", "--format", "gtiff"]).len(), 1);
    assert_eq!(query(&["", "--limit", "1"]).len(), 1);
    assert!(query(&["", "--bbox", "170,-5,-170,5"]).is_empty());
    for row in &before {
        let output = Command::new(env!("CARGO_BIN_EXE_whereabouts"))
            .arg("inspect")
            .arg(&row.primary_path)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let inspected: Vec<whereabouts_core::DatasetRecord> =
            serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(inspected.len(), 1);
        assert_eq!(inspected[0].layers, row.layers);
        assert_eq!(inspected[0].status, row.status);
    }
    {
        use std::io::{BufRead, Read};
        let started = Instant::now();
        let mut child = Command::new(env!("CARGO_BIN_EXE_whereabouts"))
            .args(["query", "points", "--db"])
            .arg(&db)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap(); // Header is not a result.
        line.clear();
        assert!(stdout.read_line(&mut line).unwrap() > 0);
        let first = started.elapsed();
        stdout.read_to_end(&mut Vec::new()).unwrap();
        assert!(child.wait_with_output().unwrap().status.success());
        eprintln!(
            "[query-vhd] process startup to first dataset row: {:.3} ms (debug, 4 corpus datasets)",
            first.as_secs_f64() * 1000.
        );
    }
    // The first process is stopped: a sidecar changes while no indexer is running.
    let path = volume.path("a/points.prj");
    let mut prj = fs::read_to_string(&path).unwrap();
    prj.push('\n');
    fs::write(path, prj).unwrap();
    let result = index(&volume.volume(), &db);
    assert!(result.contains("Resumed"));
    assert!(!result.contains("Full index"));
    let catalog = Catalog::open_read_only(&db).unwrap();
    assert_eq!(catalog.dataset_ids(vol).unwrap(), ids);
    let mut changed = 0;
    for old in before {
        let new = catalog.get_dataset(old.dataset.id()).unwrap().unwrap();
        if new.extracted_at != old.extracted_at {
            changed += 1;
            assert!(new.primary_path.contains("\\a\\"));
            assert_eq!(new.layers, old.layers);
        } else {
            assert_eq!(new, old, "unrelated dataset must be untouched");
        }
    }
    assert_eq!(changed, 1);
    drop(catalog);
    let result = index(&volume.volume(), &db);
    assert!(result.contains("0 events, 0 extractions"), "{result}");
    eprintln!(
        "[index-vhd] 4 corpus datasets (SHP/GPKG/TIFF) match golden facts; restart resumed; .prj updated exactly 1 dataset; idle restart did no extraction"
    );
}
