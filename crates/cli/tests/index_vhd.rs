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
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/data/shapefile/shp_utm33n_points");
    for dir in ["a", "b"] {
        fs::create_dir(volume.path(dir)).unwrap();
        for entry in fs::read_dir(&corpus).unwrap() {
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
        2,
        "sidecars must not become independent datasets"
    );
    let golden: serde_json::Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/corpus/expected/shp_utm33n_points.json"),
        )
        .unwrap(),
    )
    .unwrap();
    for id in &ids {
        let row = catalog.get_dataset(*id).unwrap().unwrap();
        assert_eq!(row.status, ExtractionStatus::Ok);
        assert_eq!(row.dataset.members().len(), 4);
        assert_eq!(row.layers.len(), 1);
        let layer = &row.layers[0];
        assert_eq!(
            layer.metadata.feature_count,
            golden["layers"][0]["feature_count"].as_u64()
        );
        assert_eq!(
            layer.crs.as_ref().unwrap().epsg().map(u64::from),
            golden["layers"][0]["crs"]["epsg"].as_u64()
        );
        let b = layer.metadata.native_bounds.unwrap();
        for (actual, expected) in [b.min_x(), b.min_y(), b.max_x(), b.max_y()]
            .iter()
            .zip(golden["layers"][0]["extent"].as_array().unwrap())
        {
            assert!((actual - expected.as_f64().unwrap()).abs() < 1e-6);
        }
        assert!(layer.wgs84.is_some());
    }
    let before: Vec<_> = ids
        .iter()
        .map(|id| catalog.get_dataset(*id).unwrap().unwrap())
        .collect();
    drop(catalog);
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
        "[index-vhd] 2 corpus datasets match golden facts; restart resumed; .prj updated exactly 1 dataset; idle restart did no extraction"
    );
}
