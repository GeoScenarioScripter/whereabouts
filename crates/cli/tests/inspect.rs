//! Read-only inspect checks on ordinary temp paths; no index or elevation.
#![allow(clippy::unwrap_used)]
use std::{fs, path::Path, process::Command};
use whereabouts_core::{DatasetRecord, ExtractionStatus};

#[test]
fn inspect_groups_members_and_preserves_input() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/data");
    for (case, target, layers) in [
        ("shapefile/shp_utm33n_points", "points.prj", 1),
        ("geopackage/gpkg_multi_layer", "data.gpkg", 3),
        ("geotiff/tif_northup_utm", "image.tif", 1),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut before = Vec::new();
        for entry in fs::read_dir(corpus.join(case)).unwrap() {
            let entry = entry.unwrap();
            let dest = dir.path().join(entry.file_name());
            fs::copy(entry.path(), &dest).unwrap();
            before.push((
                dest.clone(),
                fs::read(&dest).unwrap(),
                fs::metadata(&dest).unwrap().modified().unwrap(),
            ));
        }
        // Locate the fixture anchor by extension if its descriptive filename differs.
        let target = if dir.path().join(target).exists() {
            dir.path().join(target)
        } else {
            before
                .iter()
                .find(|(p, _, _)| p.extension() == Path::new(target).extension())
                .unwrap()
                .0
                .clone()
        };
        let output = Command::new(env!("CARGO_BIN_EXE_whereabouts"))
            .arg("inspect")
            .arg(target)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<DatasetRecord> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, ExtractionStatus::Ok);
        assert_eq!(rows[0].layers.len(), layers);
        assert!(
            rows[0]
                .layers
                .iter()
                .all(|l| l.crs.is_some() && l.wgs84.is_some())
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), before.len());
        for (path, bytes, modified) in before {
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
        }
    }
}

#[test]
fn orphan_sidecar_is_not_a_dataset_and_bad_content_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("orphan.prj");
    fs::write(&path, b"bad").unwrap();
    let run = |path: &Path| {
        Command::new(env!("CARGO_BIN_EXE_whereabouts"))
            .arg("inspect")
            .arg(path)
            .arg("--json")
            .output()
            .unwrap()
    };
    assert!(!run(&path).status.success());
    let path = dir.path().join("broken.shp");
    fs::write(&path, b"bad").unwrap();
    let result = run(&path);
    assert!(result.status.success());
    let rows: Vec<DatasetRecord> = serde_json::from_slice(&result.stdout).unwrap();
    assert!(matches!(rows[0].status, ExtractionStatus::Error(_)));
}

#[test]
fn file_geodatabase_internal_path_stays_one_folder_dataset() {
    let dir = tempfile::tempdir().unwrap();
    let gdb = dir.path().join("demo.gdb");
    fs::create_dir(&gdb).unwrap();
    let internal = gdb.join("a00000001.gdbtable");
    fs::write(&internal, b"fixture").unwrap();
    fs::write(gdb.join("unrelated.shp"), b"not a standalone dataset").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_whereabouts"))
        .arg("inspect")
        .arg(internal)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<DatasetRecord> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].dataset.format(),
        whereabouts_core::Format::FileGeodatabase
    );
    assert_eq!(rows[0].status, ExtractionStatus::Unsupported);
}

#[test]
fn unsigned_gdb_named_folder_keeps_normal_file_grouping() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("ordinary.gdb");
    fs::create_dir(&folder).unwrap();
    let path = folder.join("broken.shp");
    fs::write(&path, b"bad shapefile").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_whereabouts"))
        .arg("inspect")
        .arg(path)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<DatasetRecord> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].dataset.format(),
        whereabouts_core::Format::Shapefile
    );
}
