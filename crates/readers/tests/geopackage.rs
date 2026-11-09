//! GeoPackage integration: read-only guarantees, strict budgets and SQLite edge cases.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use whereabouts_core::{CrsDefinitionKind, Format, ReadLimits, ResolvedDataset};
use whereabouts_readers::{measure, read};

fn dataset(path: &Path) -> ResolvedDataset {
    ResolvedDataset {
        format: Format::GeoPackage,
        anchor: path.into(),
        sidecars: vec![],
    }
}

fn fixture(path: &Path) -> Connection {
    let c = Connection::open(path).unwrap();
    c.execute_batch("PRAGMA application_id=1196444487;
        CREATE TABLE gpkg_contents(table_name TEXT PRIMARY KEY,data_type TEXT,identifier TEXT,description TEXT,last_change TEXT,min_x REAL,min_y REAL,max_x REAL,max_y REAL,srs_id INTEGER);
        CREATE TABLE gpkg_spatial_ref_sys(srs_name TEXT,srs_id INTEGER NOT NULL PRIMARY KEY,organization TEXT,organization_coordsys_id INTEGER,definition TEXT,description TEXT,definition_12_063 TEXT);
        INSERT INTO gpkg_spatial_ref_sys VALUES('WGS',4326,'EPSG',4326,'GEOGCS[old]','','GEOGCRS[new]');
        CREATE TABLE gpkg_geometry_columns(table_name TEXT,column_name TEXT,geometry_type_name TEXT,srs_id INTEGER,z INTEGER,m INTEGER);
        CREATE TABLE gpkg_ogr_contents(table_name TEXT,feature_count INTEGER);
        CREATE TABLE points(fid INTEGER PRIMARY KEY,geom BLOB);
        INSERT INTO gpkg_contents VALUES('points','features','','','',1,2,3,4,4326);
        INSERT INTO gpkg_geometry_columns VALUES('points','geom','POINT',4326,1,2);
        INSERT INTO gpkg_ogr_contents VALUES('points',2);").unwrap();
    c
}

fn listing(path: &Path) -> Vec<PathBuf> {
    let mut names: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().file_name().into())
        .collect();
    names.sort();
    names
}

#[test]
fn bytes_mtime_and_directory_listing_unchanged_in_delete_and_wal_modes() {
    for wal in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a file #%.gpkg");
        let c = fixture(&path);
        if wal {
            c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_checkpoint(TRUNCATE);")
                .unwrap();
        }
        drop(c);
        let bytes = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let names = listing(dir.path());
        for _ in 0..2 {
            let md = read(&dataset(&path), &ReadLimits::default()).unwrap();
            assert!(md.partial.is_empty(), "{:?}", md.partial);
            assert_eq!(md.layers[0].feature_count, Some(2));
            let crs = md.layers[0].crs.as_ref().unwrap();
            assert_eq!(crs.kind, CrsDefinitionKind::Wkt2);
            assert_eq!(crs.text, "GEOGCRS[new]");
            assert_eq!(crs.authority.as_ref().unwrap().code, "4326");
            let ty = md.layers[0].geometry_type.unwrap();
            assert!(ty.has_z && ty.has_m);
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert_eq!(listing(dir.path()), names);
    }
}

#[test]
fn active_wal_is_deferred_without_touching_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.gpkg");
    let c = fixture(&path);
    c.execute_batch("PRAGMA journal_mode=WAL; INSERT INTO points VALUES(1,NULL);")
        .unwrap();
    let names = listing(dir.path());
    let wal = dir.path().join("a.gpkg-wal");
    let bytes = fs::read(&wal).unwrap();
    let md = read(&dataset(&path), &ReadLimits::default()).unwrap();
    assert!(md.layers.is_empty());
    assert!(md.partial[0].contains("-wal"));
    assert_eq!(listing(dir.path()), names);
    assert_eq!(fs::read(&wal).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn checkpointed_database_in_read_only_directory() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.gpkg");
    drop(fixture(&path));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = read(&dataset(&path), &ReadLimits::default());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_ok());
    assert_eq!(listing(dir.path()), vec![PathBuf::from("a.gpkg")]);
}

#[test]
fn budgets_and_corrupt_pages_never_overread_or_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.gpkg");
    drop(fixture(&path));
    for bytes in [0, 99, 100, 4096, 65536] {
        let limits = ReadLimits {
            max_bytes: bytes,
            ..ReadLimits::default()
        };
        let (_, io) = measure(|| read(&dataset(&path), &limits));
        assert!(io.bytes_read <= bytes, "{bytes}: {io:?}");
    }
    let limits = ReadLimits {
        time_budget: Duration::ZERO,
        ..ReadLimits::default()
    };
    let err = read(&dataset(&path), &limits).unwrap_err();
    assert!(err.to_string().contains("time budget"));
    let original = fs::read(&path).unwrap();
    for cut in [0, 15, 99, 100, 511, 4095, original.len() - 1] {
        fs::write(&path, &original[..cut]).unwrap();
        assert!(
            read(&dataset(&path), &ReadLimits::default()).is_err(),
            "cut {cut}"
        );
    }
    // Mutate header fields, cell pointers, serial types and overflow links reproducibly.
    let mut seed = 12345u64;
    for _ in 0..200 {
        let mut bytes = original.clone();
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let at = seed as usize % bytes.len();
        bytes[at] ^= (seed >> 32) as u8;
        fs::write(&path, bytes).unwrap();
        let (_, io) = measure(|| read(&dataset(&path), &ReadLimits::default()));
        assert!(io.bytes_read <= ReadLimits::default().max_bytes);
    }
}

#[test]
fn count_fallbacks_tiles_coverage_attributes_and_quoted_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.gpkg");
    let c = fixture(&path);
    c.execute_batch("DELETE FROM gpkg_ogr_contents;
        CREATE VIRTUAL TABLE rtree_points_geom USING rtree(id,minx,maxx,miny,maxy);
        INSERT INTO rtree_points_geom VALUES(1,1,3,2,4);
        CREATE TABLE \"a 'quoted' table\"(id INTEGER PRIMARY KEY, value TEXT);
        INSERT INTO \"a 'quoted' table\" VALUES(1,'hi'),(2,'there'),(3,'ok');
        INSERT INTO gpkg_contents(table_name,data_type) VALUES('a ''quoted'' table','attributes');
        INSERT INTO gpkg_contents(table_name,data_type,min_x,min_y,max_x,max_y,srs_id) VALUES('tiles','tiles',1,2,3,4,4326),('coverage','2d-gridded-coverage',1,2,3,4,4326);").unwrap();
    drop(c);
    let md = read(&dataset(&path), &ReadLimits::default()).unwrap();
    assert!(md.partial.is_empty(), "{:?}", md.partial);
    assert_eq!(md.layers.len(), 4);
    assert_eq!(md.layers[0].name, "a 'quoted' table");
    assert_eq!(md.layers[0].feature_count, Some(3));
    assert_eq!(
        md.layers
            .iter()
            .find(|l| l.name == "points")
            .unwrap()
            .feature_count,
        Some(1)
    );
    for name in ["tiles", "coverage"] {
        let l = md.layers.iter().find(|l| l.name == name).unwrap();
        assert!(l.geometry_type.is_none() && l.native_bounds.is_some() && l.crs.is_some());
    }
}

#[test]
fn sqlite_overflow_utf16_and_multiple_table_pages() {
    for encoding in ["UTF-8", "UTF-16le", "UTF-16be"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.gpkg");
        let c = Connection::open(&path).unwrap();
        c.execute_batch(&format!(
            "PRAGMA page_size=512; PRAGMA encoding='{encoding}'; VACUUM;"
        ))
        .unwrap();
        drop(c);
        let c = fixture(&path);
        let wkt = "X".repeat(8000);
        c.execute(
            "UPDATE gpkg_spatial_ref_sys SET definition_12_063=?",
            [&wkt],
        )
        .unwrap();
        c.execute_batch("DELETE FROM gpkg_ogr_contents; BEGIN;")
            .unwrap();
        for i in 0..2000 {
            c.execute("INSERT INTO points VALUES(?,zeroblob(4000))", [i])
                .unwrap();
        }
        c.execute_batch("COMMIT;").unwrap();
        drop(c);
        let (result, io) = measure(|| read(&dataset(&path), &ReadLimits::default()));
        let md = result.unwrap();
        assert!(md.partial.is_empty(), "{encoding}: {:?}", md.partial);
        assert_eq!(md.layers[0].feature_count, Some(2000));
        assert_eq!(md.layers[0].crs.as_ref().unwrap().text, wkt);
        assert!(io.bytes_read < 2_000_000, "{io:?}");
    }
}

#[test]
fn large_geometry_refinement_reads_only_its_envelope_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.gpkg");
    let c = fixture(&path);
    c.execute_batch(
        "UPDATE gpkg_contents SET min_x=NULL,min_y=NULL,max_x=NULL,max_y=NULL;
        UPDATE gpkg_ogr_contents SET feature_count=1;
        CREATE VIRTUAL TABLE rtree_points_geom USING rtree(id,minx,maxx,miny,maxy);
        INSERT INTO rtree_points_geom VALUES(1,1.25,3.75,2.5,4.25);",
    )
    .unwrap();
    let mut blob = b"GP\0\x03\0\0\0\0".to_vec();
    for v in [1.25f64, 3.75, 2.5, 4.25] {
        blob.extend(v.to_le_bytes());
    }
    blob.resize(2 * 1024 * 1024, 0);
    c.execute("INSERT INTO points VALUES(1,?)", [blob]).unwrap();
    drop(c);
    let limits = ReadLimits {
        max_bytes: 65536,
        ..ReadLimits::default()
    };
    let (result, io) = measure(|| read(&dataset(&path), &limits));
    let md = result.unwrap();
    assert_eq!(md.partial.len(), 1, "{:?}", md.partial);
    let b = md.layers[0].native_bounds.unwrap();
    assert_eq!(
        [b.min_x(), b.min_y(), b.max_x(), b.max_y()],
        [1.25, 2.5, 3.75, 4.25]
    );
    assert!(io.bytes_read < 65536, "{io:?}");
}

#[test]
fn all_committed_gpkg_cases_are_bounded_and_corrupt_cases_fail() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/data/geopackage");
    for dir in fs::read_dir(root).unwrap() {
        let dir = dir.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        for file in fs::read_dir(&dir).unwrap() {
            let file = file.unwrap().path();
            if file.extension().is_none_or(|e| e != "gpkg") {
                continue;
            }
            let (result, io) = measure(|| read(&dataset(&file), &ReadLimits::default()));
            eprintln!(
                "{}: {} bytes, {:?}",
                file.display(),
                io.bytes_read,
                result.as_ref().map(|m| &m.partial)
            );
            assert!(io.bytes_read <= ReadLimits::default().max_bytes);
            if dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("corrupt")
            {
                assert!(result.is_err());
            } else {
                assert!(result.is_ok(), "{}: {result:?}", file.display());
            }
        }
    }
}
