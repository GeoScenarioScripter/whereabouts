//! Catalog integration tests on temporary databases.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use tempfile::TempDir;
use whereabouts_catalog::{Catalog, CatalogError, CatalogStats, DatasetFilter, SCHEMA_VERSION};
use whereabouts_core::{
    AuthorityCode, Bounds, CrsDefinition, CrsDefinitionKind, Dataset, DatasetId, DatasetKind,
    DatasetRecord, ExtractionStatus, FieldInfo, FieldKind, Format, Frn, GeometryKind, GeometryType,
    LayerMetadata, LayerRecord, Member, MemberRole, RasterDataType, RasterInfo, ResolvedCrs,
    Timestamp, VolumeId, Wgs84Extent,
};
use whereabouts_source::{Checkpoint, SourceIdentity};

struct Db {
    _dir: TempDir,
    path: PathBuf,
    cat: Catalog,
}

fn db() -> Db {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let cat = Catalog::open(&path).unwrap();
    Db {
        _dir: dir,
        path,
        cat,
    }
}

fn identity(key: &str) -> SourceIdentity {
    SourceIdentity {
        kind: "ntfs".into(),
        key: format!(r"\\?\Volume{{{key}}}\"),
        label: "D:".into(),
    }
}

fn volume(cat: &mut Catalog, key: &str) -> VolumeId {
    cat.write(|tx| tx.register_volume(&identity(key))).unwrap()
}

fn wsen(w: f64, s: f64, e: f64, n: f64) -> Wgs84Extent {
    Wgs84Extent::from_west_south_east_north(w, s, e, n).unwrap()
}

/// A vector layer in an identified CRS with the given WGS84 extent.
fn vector_layer(name: &str, epsg: u32, wgs84: Wgs84Extent) -> LayerRecord {
    LayerRecord {
        metadata: LayerMetadata {
            name: name.into(),
            geometry_type: Some(GeometryType {
                kind: GeometryKind::MultiPolygon,
                has_z: true,
                has_m: false,
            }),
            feature_count: Some(1234),
            crs: Some(CrsDefinition::epsg(epsg)),
            native_bounds: Some(Bounds::new(1.7e6, 5.4e6, 1.8e6, 5.5e6).unwrap()),
            fields: vec![
                FieldInfo {
                    name: "id".into(),
                    kind: FieldKind::Integer64,
                    width: Some(18),
                    precision: Some(0),
                },
                FieldInfo {
                    name: "näme".into(),
                    kind: FieldKind::String,
                    width: Some(80),
                    precision: None,
                },
            ],
            raster: None,
        },
        crs: Some(ResolvedCrs::Identified {
            epsg,
            name: Some(format!("EPSG {epsg}")),
        }),
        wgs84: Some(wgs84),
    }
}

/// A shapefile dataset (main + shx + dbf + prj) with one layer.
fn shapefile(volume: VolumeId, anchor: u64, layer: LayerRecord) -> DatasetRecord {
    let id = DatasetId::new(volume, Frn(anchor));
    let members = vec![
        Member {
            frn: Frn(anchor),
            role: MemberRole::Main,
        },
        Member {
            frn: Frn(anchor + 3),
            role: MemberRole::Prj,
        },
        Member {
            frn: Frn(anchor + 1),
            role: MemberRole::Shx,
        },
        Member {
            frn: Frn(anchor + 2),
            role: MemberRole::Dbf,
        },
    ];
    DatasetRecord {
        dataset: Dataset::new(id, DatasetKind::Vector, Format::Shapefile, members).unwrap(),
        name: format!("ds{anchor}"),
        primary_path: format!(r"D:\gis\ds{anchor}.shp"),
        total_size: Some(4096),
        mtime: Some(Timestamp(
            1_700_000_000_000_000_000 + i64::try_from(anchor & 0xFFFF).unwrap(),
        )),
        status: ExtractionStatus::Ok,
        extracted_at: Some(Timestamp(1_700_000_100_000_000_000)),
        layers: vec![layer],
    }
}

fn pending(volume: VolumeId, anchor: u64, format: Format, mtime: i64) -> DatasetRecord {
    let id = DatasetId::new(volume, Frn(anchor));
    DatasetRecord {
        dataset: Dataset::new(
            id,
            format.default_kind(),
            format,
            vec![Member {
                frn: Frn(anchor),
                role: MemberRole::Main,
            }],
        )
        .unwrap(),
        name: format!("p{anchor}"),
        primary_path: format!(r"D:\p{anchor}"),
        total_size: None,
        mtime: Some(Timestamp(mtime)),
        status: ExtractionStatus::Pending,
        extracted_at: None,
        layers: vec![],
    }
}

fn upsert(cat: &mut Catalog, rec: &DatasetRecord) {
    cat.write(|tx| tx.upsert_dataset(rec)).unwrap();
}

fn find(cat: &Catalog, filter: DatasetFilter) -> Vec<u64> {
    cat.find_datasets(&filter)
        .unwrap()
        .into_iter()
        .map(|id| id.anchor.0)
        .collect()
}

fn bbox(e: Wgs84Extent) -> DatasetFilter {
    DatasetFilter {
        bbox: Some(e),
        ..DatasetFilter::default()
    }
}

fn raw(path: &PathBuf) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

// ---------------------------------------------------------------------------------------------
// Schema and migrations

#[test]
fn migrates_an_empty_database() {
    let d = db();
    assert_eq!(d.cat.stats().unwrap(), CatalogStats::default());
    let conn = raw(&d.path);
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    for table in [
        "volumes",
        "crs",
        "datasets",
        "dataset_files",
        "layers",
        "rasters",
        "layer_extents",
    ] {
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = ?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{table}");
    }
    drop(conn);

    // Re-opening an up-to-date catalog is a no-op and keeps the data.
    let Db {
        _dir,
        path,
        mut cat,
    } = d;
    let v = volume(&mut cat, "a");
    drop(cat);
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(cat.volumes().unwrap().len(), 1);
    assert_eq!(cat.volumes().unwrap()[0].id, v);
}

#[test]
fn rejects_newer_schema() {
    let d = db();
    let path = d.path.clone();
    drop(d.cat);
    raw(&path)
        .execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
        .unwrap();
    match Catalog::open(&path) {
        Err(CatalogError::UnsupportedSchema { found, supported }) => {
            assert_eq!(found, SCHEMA_VERSION + 1);
            assert_eq!(supported, SCHEMA_VERSION);
        }
        other => panic!("expected UnsupportedSchema, got {other:?}"),
    }
}

#[test]
fn read_only_open() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    upsert(
        &mut d.cat,
        &shapefile(v, 100, vector_layer("l", 4326, wsen(1.0, 1.0, 2.0, 2.0))),
    );

    // A reader sees committed data while the writer connection stays open.
    let ro = Catalog::open_read_only(&d.path).unwrap();
    assert_eq!(ro.stats().unwrap().datasets, 1);
    assert!(
        ro.get_dataset(DatasetId::new(v, Frn(100)))
            .unwrap()
            .is_some()
    );

    // A file that was never migrated is refused rather than silently read.
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.db");
    raw(&empty).execute_batch("CREATE TABLE t (x)").unwrap();
    assert!(matches!(
        Catalog::open_read_only(&empty),
        Err(CatalogError::UnsupportedSchema { found: 0, .. })
    ));
}

#[test]
fn rtree_lookups_use_the_index() {
    let d = db();
    let conn = raw(&d.path);
    let plan = |sql: &str| -> String {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        stmt.query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
            .join(" | ")
    };
    // Strategy 1 = rowid lookup (used by the delete trigger and when reading extents back).
    let p = plan("SELECT * FROM layer_extents WHERE id IN (4, 5, 6, 7)");
    assert!(p.contains("VIRTUAL TABLE INDEX 1"), "{p}");
    // Strategy 2 = R*Tree search on the coordinate constraints.
    let p = plan(
        "SELECT layer_id FROM layer_extents WHERE minx <= 1 AND maxx >= 0 AND miny <= 1 \
         AND maxy >= 0 AND west <= 1 AND east >= 0 AND south <= 1 AND north >= 0",
    );
    assert!(p.contains("VIRTUAL TABLE INDEX 2"), "{p}");
    // Member lookup drives from the FRN index (same SQL as `datasets_with_member`).
    let p = plan(
        "SELECT d.anchor_frn, f.role FROM dataset_files f \
         CROSS JOIN datasets d ON d.id = f.dataset_id \
         WHERE f.frn = 1 AND d.volume_id = 1 ORDER BY d.id",
    );
    assert!(
        p.starts_with("SEARCH f USING INDEX dataset_files_frn"),
        "{p}"
    );
}

// ---------------------------------------------------------------------------------------------
// Upsert / delete

#[test]
fn upsert_is_idempotent_and_round_trips() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let rec = shapefile(
        v,
        0xFFFF_0000_0000_0042,
        vector_layer("parcels", 2193, wsen(174.0, -41.5, 175.0, -41.0)),
    );
    upsert(&mut d.cat, &rec);
    let stats = d.cat.stats().unwrap();
    let row_id = |path: &PathBuf| -> i64 {
        raw(path)
            .query_row("SELECT id FROM datasets", [], |r| r.get(0))
            .unwrap()
    };
    let first_id = row_id(&d.path);

    upsert(&mut d.cat, &rec);
    assert_eq!(d.cat.stats().unwrap(), stats);
    assert_eq!(row_id(&d.path), first_id, "surrogate id is stable");
    assert_eq!(
        stats,
        CatalogStats {
            volumes: 1,
            datasets: 1,
            files: 4,
            layers: 1,
            crs: 1,
            extents: 1,
        }
    );
    let back = d.cat.get_dataset(rec.dataset.id()).unwrap().unwrap();
    assert_eq!(back, rec, "exact round trip, member order included");
}

#[test]
fn upsert_replaces_members_and_layers() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let mut rec = shapefile(
        v,
        10,
        vector_layer("a", 2193, wsen(174.0, -41.5, 175.0, -41.0)),
    );
    upsert(&mut d.cat, &rec);

    // Sidecar removed, CRS changed, second layer added.
    rec.dataset.remove_member(Frn(13)).unwrap();
    rec.layers = vec![
        vector_layer("a", 4326, wsen(1.0, 1.0, 2.0, 2.0)),
        vector_layer("b", 4326, wsen(170.0, 0.0, -170.0, 1.0)),
    ];
    upsert(&mut d.cat, &rec);
    let s = d.cat.stats().unwrap();
    assert_eq!((s.files, s.layers, s.crs, s.extents), (3, 2, 1, 3));
    assert_eq!(d.cat.get_dataset(rec.dataset.id()).unwrap().unwrap(), rec);
    assert!(d.cat.datasets_with_member(v, Frn(13)).unwrap().is_empty());
    // The old extent is gone from the R*Tree.
    assert!(find(&d.cat, bbox(wsen(174.5, -41.2, 174.6, -41.1))).is_empty());
}

#[test]
fn delete_cascades() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let a = shapefile(
        v,
        10,
        vector_layer("a", 2193, wsen(170.0, 0.0, -170.0, 1.0)),
    );
    let b = shapefile(v, 20, vector_layer("b", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    upsert(&mut d.cat, &a);
    upsert(&mut d.cat, &b);
    assert_eq!(d.cat.stats().unwrap().crs, 1, "shared CRS stored once");

    assert!(d.cat.write(|tx| tx.delete_dataset(a.dataset.id())).unwrap());
    assert!(!d.cat.write(|tx| tx.delete_dataset(a.dataset.id())).unwrap());
    let s = d.cat.stats().unwrap();
    assert_eq!(
        (s.datasets, s.files, s.layers, s.crs, s.extents),
        (1, 4, 1, 1, 1)
    );
    assert!(d.cat.get_dataset(a.dataset.id()).unwrap().is_none());

    assert!(d.cat.write(|tx| tx.delete_dataset(b.dataset.id())).unwrap());
    let s = d.cat.stats().unwrap();
    assert_eq!(
        s,
        CatalogStats {
            volumes: 1,
            ..CatalogStats::default()
        }
    );
}

#[test]
fn delete_volume_cascades() {
    let mut d = db();
    let v1 = volume(&mut d.cat, "a");
    let v2 = volume(&mut d.cat, "b");
    upsert(
        &mut d.cat,
        &shapefile(
            v1,
            10,
            vector_layer("a", 2193, wsen(170.0, 0.0, -170.0, 1.0)),
        ),
    );
    upsert(
        &mut d.cat,
        &shapefile(v2, 10, vector_layer("b", 4326, wsen(1.0, 1.0, 2.0, 2.0))),
    );
    assert!(d.cat.write(|tx| tx.delete_volume(v1)).unwrap());
    let s = d.cat.stats().unwrap();
    assert_eq!(
        s,
        CatalogStats {
            volumes: 1,
            datasets: 1,
            files: 4,
            layers: 1,
            crs: 1,
            extents: 1,
        }
    );
    assert_eq!(
        find(&d.cat, bbox(wsen(-180.0, -90.0, 180.0, 90.0))),
        vec![10]
    );
    assert_eq!(
        d.cat.dataset_ids(v2).unwrap(),
        vec![DatasetId::new(v2, Frn(10))]
    );
}

#[test]
fn failed_write_rolls_back() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let ok = shapefile(v, 10, vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    let mut bad = shapefile(v, 20, vector_layer("b", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    bad.status = ExtractionStatus::Pending; // pending must not carry layers
    let err = d
        .cat
        .write(|tx| {
            tx.upsert_dataset(&ok)?;
            tx.upsert_dataset(&bad)
        })
        .unwrap_err();
    assert!(matches!(err, CatalogError::InvalidRecord(_)), "{err:?}");
    assert_eq!(d.cat.stats().unwrap().datasets, 0);
}

#[test]
fn rejects_unknown_volume_and_out_of_range_values() {
    let mut d = db();
    let rec = shapefile(
        VolumeId(7),
        10,
        vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0)),
    );
    assert!(matches!(
        d.cat.write(|tx| tx.upsert_dataset(&rec)),
        Err(CatalogError::UnknownVolume(VolumeId(7)))
    ));
    let v = volume(&mut d.cat, "a");
    let mut rec = shapefile(v, 10, vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    rec.layers[0].metadata.feature_count = Some(u64::MAX);
    assert!(matches!(
        d.cat.write(|tx| tx.upsert_dataset(&rec)),
        Err(CatalogError::OutOfRange(_))
    ));
}

// ---------------------------------------------------------------------------------------------
// Layers, CRS, rasters

#[test]
fn crs_dedup_and_resolution_update() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let wkt = CrsDefinition {
        kind: CrsDefinitionKind::Wkt1Esri,
        text: r#"PROJCS["NZGD_2000_New_Zealand_Transverse_Mercator"]"#.into(),
        authority: Some(AuthorityCode {
            authority: "EPSG".into(),
            code: "2193".into(),
        }),
    };
    let mut layer = vector_layer("a", 2193, wsen(174.0, -41.5, 175.0, -41.0));
    layer.metadata.crs = Some(wkt.clone());
    layer.crs = Some(ResolvedCrs::Unidentified { name: None });
    let a = shapefile(v, 10, layer.clone());
    upsert(&mut d.cat, &a);

    // Same definition, now identified: one row, resolution upgraded for every user.
    layer.crs = Some(ResolvedCrs::Identified {
        epsg: 2193,
        name: Some("NZGD2000 / New Zealand Transverse Mercator 2000".into()),
    });
    let b = shapefile(v, 20, layer.clone());
    upsert(&mut d.cat, &b);
    assert_eq!(d.cat.stats().unwrap().crs, 1);
    let a_back = d.cat.get_dataset(a.dataset.id()).unwrap().unwrap();
    assert_eq!(a_back.layers[0].crs, layer.crs);
    assert_eq!(a_back.layers[0].metadata.crs, Some(wkt.clone()));
    assert_eq!(
        find(
            &d.cat,
            DatasetFilter {
                epsg: Some(2193),
                ..DatasetFilter::default()
            }
        ),
        vec![10, 20]
    );

    // Same text without the authority is a different definition.
    layer.metadata.crs = Some(CrsDefinition {
        authority: None,
        ..wkt
    });
    upsert(&mut d.cat, &shapefile(v, 30, layer));
    assert_eq!(d.cat.stats().unwrap().crs, 2);
}

#[test]
fn layers_without_crs_or_extent_and_unparsed_crs() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let mut rec = shapefile(v, 10, vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    let mut no_crs = rec.layers[0].clone();
    no_crs.metadata.crs = None;
    no_crs.crs = None;
    no_crs.wgs84 = None;
    no_crs.metadata.native_bounds = None;
    no_crs.metadata.geometry_type = None;
    no_crs.metadata.feature_count = None;
    no_crs.metadata.fields.clear();
    let mut unparsed = rec.layers[0].clone();
    unparsed.metadata.crs = Some(CrsDefinition::new(CrsDefinitionKind::Wkt1, "garbage["));
    unparsed.crs = Some(ResolvedCrs::Unparsed);
    unparsed.wgs84 = None;
    rec.status = ExtractionStatus::Partial("unparseable .prj".into());
    rec.layers = vec![no_crs, unparsed];
    upsert(&mut d.cat, &rec);
    assert_eq!(d.cat.get_dataset(rec.dataset.id()).unwrap().unwrap(), rec);
    assert_eq!(d.cat.stats().unwrap().extents, 0);
    assert!(find(&d.cat, bbox(wsen(-180.0, -90.0, 180.0, 90.0))).is_empty());
}

#[test]
fn raster_round_trip() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let id = DatasetId::new(v, Frn(50));
    let raster = RasterInfo {
        width: 10_000_000_000,
        height: 7,
        bands: 3,
        dtype: RasterDataType::Float32,
        res_x: Some(0.1 + 0.2),
        res_y: Some(-1.0 / 3.0),
        nodata: Some("-3.4028234663852886e+38".into()),
        overview_count: 5,
        geotransform: Some([
            1_234_567.891_011_121_3,
            0.1 + 0.2,
            0.0,
            5_432_109.876_543_21,
            0.0,
            -1.0 / 3.0,
        ]),
    };
    let layer = LayerRecord {
        metadata: LayerMetadata {
            name: "dem".into(),
            crs: Some(CrsDefinition::new(
                CrsDefinitionKind::GeoKeys,
                "ProjectedCSTypeGeoKey=2193",
            )),
            native_bounds: Some(
                Bounds::new(1_234_567.0, 5_000_000.0, 1_300_000.0, 5_432_109.9).unwrap(),
            ),
            raster: Some(raster.clone()),
            ..LayerMetadata::default()
        },
        crs: Some(ResolvedCrs::Identified {
            epsg: 2193,
            name: None,
        }),
        wgs84: Some(wsen(172.1, -45.2, 173.3, -41.0)),
    };
    let rec = DatasetRecord {
        dataset: Dataset::new(
            id,
            DatasetKind::Raster,
            Format::GeoTiff,
            vec![
                Member {
                    frn: Frn(50),
                    role: MemberRole::Main,
                },
                Member {
                    frn: Frn(51),
                    role: MemberRole::Ovr,
                },
                Member {
                    frn: Frn(52),
                    role: MemberRole::AuxXml,
                },
            ],
        )
        .unwrap(),
        name: "dem".into(),
        primary_path: r"D:\dem.tif".into(),
        total_size: Some(1 << 40),
        mtime: Some(Timestamp(-5)),
        status: ExtractionStatus::Ok,
        extracted_at: Some(Timestamp(9)),
        layers: vec![layer],
    };
    upsert(&mut d.cat, &rec);
    assert_eq!(
        d.cat.get_dataset(id).unwrap().unwrap(),
        rec,
        "bit-exact floats"
    );

    // Non-finite values can't be stored faithfully: they come back as absent.
    let mut nan = rec.clone();
    let r = nan.layers[0].metadata.raster.as_mut().unwrap();
    r.res_x = Some(f64::NAN);
    r.geotransform = Some([f64::INFINITY, 1.0, 0.0, 0.0, 0.0, -1.0]);
    upsert(&mut d.cat, &nan);
    let back = d.cat.get_dataset(id).unwrap().unwrap();
    let r = back.layers[0].metadata.raster.as_ref().unwrap();
    assert_eq!((r.res_x, r.geotransform), (None, None));
    assert_eq!(r.res_y, raster.res_y);
    assert_eq!(d.cat.stats().unwrap().layers, 1);
}

#[test]
fn many_extent_parts_are_stored_as_their_union() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let parts: Vec<Bounds> = (0..6)
        .map(|i| {
            let x = f64::from(i) * 10.0;
            Bounds::new(x, 0.0, x + 1.0, 1.0).unwrap()
        })
        .collect();
    let rec = shapefile(
        v,
        10,
        vector_layer("a", 4326, Wgs84Extent::new(parts).unwrap()),
    );
    upsert(&mut d.cat, &rec);
    let back = d.cat.get_dataset(rec.dataset.id()).unwrap().unwrap();
    assert_eq!(
        back.layers[0].wgs84.as_ref().unwrap().parts(),
        &[Bounds::new(0.0, 0.0, 51.0, 1.0).unwrap()]
    );
    assert_eq!(d.cat.stats().unwrap().extents, 1);
    // Exactly four parts are kept as they are.
    let four: Vec<Bounds> = (0..4)
        .map(|i| Bounds::new(f64::from(i), 0.0, f64::from(i) + 0.5, 1.0).unwrap())
        .collect();
    let rec = shapefile(
        v,
        20,
        vector_layer("b", 4326, Wgs84Extent::new(four).unwrap()),
    );
    upsert(&mut d.cat, &rec);
    assert_eq!(d.cat.get_dataset(rec.dataset.id()).unwrap().unwrap(), rec);
    assert_eq!(d.cat.stats().unwrap().extents, 5);
    assert!(
        find(&d.cat, bbox(wsen(2.6, 0.0, 2.9, 1.0))) == vec![10],
        "gap between the parts of 20 (10 is the union)"
    );
    assert_eq!(find(&d.cat, bbox(wsen(2.1, 0.0, 2.2, 1.0))), vec![10, 20]);
}

// ---------------------------------------------------------------------------------------------
// Queries

#[test]
fn bbox_queries_with_antimeridian_parts() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    // 10: Fiji-like extent crossing the antimeridian (two parts).
    upsert(
        &mut d.cat,
        &shapefile(
            v,
            10,
            vector_layer("fiji", 3460, wsen(176.0, -20.0, -178.0, -15.0)),
        ),
    );
    // 20: just east of the antimeridian; 30: just west of it; 40: Europe.
    upsert(
        &mut d.cat,
        &shapefile(
            v,
            20,
            vector_layer("e", 4326, wsen(-179.9, 50.0, -179.5, 51.0)),
        ),
    );
    upsert(
        &mut d.cat,
        &shapefile(
            v,
            30,
            vector_layer("w", 4326, wsen(179.5, 50.0, 179.9, 51.0)),
        ),
    );
    upsert(
        &mut d.cat,
        &shapefile(v, 40, vector_layer("eu", 4326, wsen(5.0, 45.0, 15.0, 55.0))),
    );
    assert_eq!(d.cat.stats().unwrap().extents, 5);
    let fiji = d
        .cat
        .get_dataset(DatasetId::new(v, Frn(10)))
        .unwrap()
        .unwrap();
    assert!(fiji.layers[0].wgs84.as_ref().unwrap().is_split());

    // Each part of the split extent is found on its own.
    assert_eq!(
        find(&d.cat, bbox(wsen(177.0, -18.0, 178.0, -17.0))),
        vec![10]
    );
    assert_eq!(
        find(&d.cat, bbox(wsen(-179.0, -18.0, -178.5, -17.0))),
        vec![10]
    );
    // Neither part covers 170..175.
    assert!(find(&d.cat, bbox(wsen(170.0, -18.0, 175.0, -17.0))).is_empty());
    // A query box crossing the antimeridian finds things on both sides, not the world.
    assert_eq!(
        find(&d.cat, bbox(wsen(179.0, -90.0, -179.0, 90.0))),
        vec![10, 20, 30]
    );
    assert_eq!(
        find(&d.cat, bbox(wsen(179.0, 49.0, -179.0, 52.0))),
        vec![20, 30]
    );
    // A box covering only 0..10 finds Europe only.
    assert_eq!(find(&d.cat, bbox(wsen(0.0, 0.0, 10.0, 60.0))), vec![40]);
    // The whole world finds everything once each.
    assert_eq!(
        find(&d.cat, bbox(wsen(-180.0, -90.0, 180.0, 90.0))),
        vec![10, 20, 30, 40]
    );
}

#[test]
fn bbox_edges_are_exact() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    // 0.1 is not representable as an f32; the R*Tree rounds outwards, the aux columns don't.
    upsert(
        &mut d.cat,
        &shapefile(v, 10, vector_layer("a", 4326, wsen(10.1, 20.1, 30.1, 40.1))),
    );
    let next_up = |x: f64| f64::from_bits(x.to_bits() + 1);
    let next_down = |x: f64| f64::from_bits(x.to_bits() - 1);
    // Touching edges intersect.
    assert_eq!(find(&d.cat, bbox(wsen(30.1, 0.0, 31.0, 50.0))), vec![10]);
    assert_eq!(find(&d.cat, bbox(wsen(0.0, 0.0, 10.1, 20.1))), vec![10]);
    // One ulp outside does not.
    assert!(find(&d.cat, bbox(wsen(next_up(30.1), 0.0, 31.0, 50.0))).is_empty());
    assert!(find(&d.cat, bbox(wsen(0.0, 0.0, next_down(10.1), 50.0))).is_empty());
    assert!(find(&d.cat, bbox(wsen(0.0, next_up(40.1), 50.0, 50.0))).is_empty());
    assert!(find(&d.cat, bbox(wsen(0.0, 0.0, 50.0, next_down(20.1)))).is_empty());
    // Degenerate (point) query inside.
    assert_eq!(find(&d.cat, bbox(wsen(20.0, 30.0, 20.0, 30.0))), vec![10]);
}

#[test]
fn filters_by_format_epsg_volume_and_limit() {
    let mut d = db();
    let v1 = volume(&mut d.cat, "a");
    let v2 = volume(&mut d.cat, "b");
    upsert(
        &mut d.cat,
        &shapefile(
            v1,
            10,
            vector_layer("a", 2193, wsen(174.0, -41.5, 175.0, -41.0)),
        ),
    );
    upsert(
        &mut d.cat,
        &shapefile(
            v2,
            20,
            vector_layer("b", 4326, wsen(174.0, -41.5, 175.0, -41.0)),
        ),
    );
    upsert(&mut d.cat, &pending(v1, 30, Format::GeoPackage, 5));
    upsert(&mut d.cat, &pending(v1, 40, Format::GeoTiff, 6));

    // A GeoPackage with two layers: EPSG 2193 in Wellington, EPSG 3857 in Europe.
    let mut gpkg = shapefile(
        v1,
        50,
        vector_layer("nz", 2193, wsen(174.0, -41.5, 175.0, -41.0)),
    );
    gpkg.dataset = Dataset::new(
        DatasetId::new(v1, Frn(50)),
        DatasetKind::Vector,
        Format::GeoPackage,
        vec![Member {
            frn: Frn(50),
            role: MemberRole::Main,
        }],
    )
    .unwrap();
    gpkg.layers
        .push(vector_layer("eu", 3857, wsen(5.0, 45.0, 15.0, 55.0)));
    upsert(&mut d.cat, &gpkg);

    let all = DatasetFilter::default();
    assert_eq!(find(&d.cat, all.clone()), vec![10, 20, 30, 40, 50]);
    let f = |formats: Vec<Format>| DatasetFilter {
        formats,
        ..DatasetFilter::default()
    };
    assert_eq!(find(&d.cat, f(vec![Format::GeoPackage])), vec![30, 50]);
    assert_eq!(
        find(&d.cat, f(vec![Format::GeoTiff, Format::Shapefile])),
        vec![10, 20, 40]
    );
    assert_eq!(
        find(
            &d.cat,
            DatasetFilter {
                volume: Some(v2),
                ..all.clone()
            }
        ),
        vec![20]
    );
    assert_eq!(
        find(
            &d.cat,
            DatasetFilter {
                epsg: Some(2193),
                ..all.clone()
            }
        ),
        vec![10, 50]
    );
    assert_eq!(
        find(
            &d.cat,
            DatasetFilter {
                limit: Some(2),
                ..all.clone()
            }
        ),
        vec![10, 20]
    );
    let wellington = wsen(174.7, -41.3, 174.8, -41.2);
    assert_eq!(find(&d.cat, bbox(wellington.clone())), vec![10, 20, 50]);
    // EPSG and bbox must hold for the same layer: the GPKG's 3857 layer is in Europe.
    assert!(
        find(
            &d.cat,
            DatasetFilter {
                epsg: Some(3857),
                bbox: Some(wellington.clone()),
                ..all.clone()
            }
        )
        .is_empty()
    );
    assert_eq!(
        find(
            &d.cat,
            DatasetFilter {
                epsg: Some(3857),
                bbox: Some(wsen(10.0, 50.0, 11.0, 51.0)),
                ..all.clone()
            }
        ),
        vec![50]
    );
    assert_eq!(
        find(
            &d.cat,
            DatasetFilter {
                volume: Some(v1),
                formats: vec![Format::Shapefile],
                epsg: Some(2193),
                bbox: Some(wellington),
                limit: Some(10),
            }
        ),
        vec![10]
    );
}

#[test]
fn lookup_by_member_frn() {
    let mut d = db();
    let v1 = volume(&mut d.cat, "a");
    let v2 = volume(&mut d.cat, "b");
    let a = shapefile(v1, 10, vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    upsert(&mut d.cat, &a);
    // Same FRNs on another volume.
    upsert(
        &mut d.cat,
        &shapefile(v2, 10, vector_layer("b", 2193, wsen(1.0, 1.0, 2.0, 2.0))),
    );

    assert_eq!(
        d.cat.datasets_with_member(v1, Frn(13)).unwrap(),
        vec![(a.dataset.id(), MemberRole::Prj)]
    );
    assert_eq!(
        d.cat.datasets_with_member(v1, Frn(10)).unwrap(),
        vec![(a.dataset.id(), MemberRole::Main)]
    );
    assert_eq!(
        d.cat.datasets_with_member(v2, Frn(11)).unwrap(),
        vec![(DatasetId::new(v2, Frn(10)), MemberRole::Shx)]
    );
    assert!(d.cat.datasets_with_member(v1, Frn(99)).unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Status, rename, pending queue

#[test]
fn status_updates() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let rec = shapefile(v, 10, vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    let id = rec.dataset.id();
    upsert(&mut d.cat, &rec);

    // Partial keeps layers.
    let partial = ExtractionStatus::Partial("stale bounds".into());
    assert!(
        d.cat
            .write(|tx| tx.set_status(id, &partial, Some(Timestamp(7))))
            .unwrap()
    );
    let back = d.cat.get_dataset(id).unwrap().unwrap();
    assert_eq!((back.status, back.layers.len()), (partial, 1));
    assert_eq!(back.extracted_at, Some(Timestamp(7)));

    // Error drops layers, extents and the now-unused CRS.
    let error = ExtractionStatus::Error("truncated header".into());
    assert!(
        d.cat
            .write(|tx| tx.set_status(id, &error, Some(Timestamp(8))))
            .unwrap()
    );
    let back = d.cat.get_dataset(id).unwrap().unwrap();
    assert_eq!(back.status, error);
    assert!(back.layers.is_empty());
    back.validate().unwrap();
    let s = d.cat.stats().unwrap();
    assert_eq!((s.layers, s.extents, s.crs, s.files), (0, 0, 0, 4));

    // Re-queued: pending, keeping the previous extraction time.
    assert!(
        d.cat
            .write(|tx| tx.set_status(id, &ExtractionStatus::Pending, Some(Timestamp(8))))
            .unwrap()
    );
    assert_eq!(d.cat.pending_datasets(None, 10).unwrap(), vec![id]);

    // Invalid combination and unknown dataset.
    assert!(matches!(
        d.cat
            .write(|tx| tx.set_status(id, &ExtractionStatus::Ok, None)),
        Err(CatalogError::InvalidRecord(_))
    ));
    let missing = DatasetId::new(v, Frn(999));
    assert!(
        !d.cat
            .write(|tx| tx.set_status(missing, &ExtractionStatus::Pending, None))
            .unwrap()
    );
}

#[test]
fn rename_keeps_everything_else() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let mut rec = shapefile(v, 10, vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    upsert(&mut d.cat, &rec);
    assert!(
        d.cat
            .write(|tx| tx.rename_dataset(rec.dataset.id(), "roads", r"D:\new\roads.shp"))
            .unwrap()
    );
    rec.name = "roads".into();
    rec.primary_path = r"D:\new\roads.shp".into();
    assert_eq!(d.cat.get_dataset(rec.dataset.id()).unwrap().unwrap(), rec);
    assert!(
        !d.cat
            .write(|tx| tx.rename_dataset(DatasetId::new(v, Frn(1)), "x", "y"))
            .unwrap()
    );
}

#[test]
fn pending_queue_is_most_recent_first() {
    let mut d = db();
    let v1 = volume(&mut d.cat, "a");
    let v2 = volume(&mut d.cat, "b");
    upsert(&mut d.cat, &pending(v1, 1, Format::Shapefile, 100));
    upsert(&mut d.cat, &pending(v1, 2, Format::GeoTiff, 300));
    upsert(&mut d.cat, &pending(v2, 3, Format::GeoPackage, 200));
    let mut no_mtime = pending(v1, 4, Format::GeoPackage, 0);
    no_mtime.mtime = None;
    upsert(&mut d.cat, &no_mtime);
    upsert(
        &mut d.cat,
        &shapefile(v1, 10, vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0))),
    );

    let anchors = |ids: Vec<DatasetId>| ids.into_iter().map(|i| i.anchor.0).collect::<Vec<_>>();
    assert_eq!(
        anchors(d.cat.pending_datasets(None, 10).unwrap()),
        vec![2, 3, 1, 4]
    );
    assert_eq!(
        anchors(d.cat.pending_datasets(Some(v1), 2).unwrap()),
        vec![2, 1]
    );
}

// ---------------------------------------------------------------------------------------------
// Volumes and checkpoints

#[test]
fn volumes_and_checkpoints() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    // Re-registering returns the same id and updates the label.
    let mut renamed = identity("a");
    renamed.label = "E:".into();
    assert_eq!(d.cat.write(|tx| tx.register_volume(&renamed)).unwrap(), v);
    let other = volume(&mut d.cat, "b");
    assert_ne!(other, v);
    let info = d.cat.volume(v).unwrap().unwrap();
    assert_eq!(
        (info.identity.clone(), info.checkpoint_at),
        (renamed.clone(), None)
    );
    assert_eq!(
        d.cat.find_volume("ntfs", &renamed.key).unwrap().unwrap().id,
        v
    );
    assert!(d.cat.find_volume("ntfs", "nope").unwrap().is_none());
    assert!(d.cat.volume(VolumeId(99)).unwrap().is_none());

    assert_eq!(d.cat.checkpoint(v).unwrap(), None);
    let cp = Checkpoint::new(2, vec![1, 2, 3, 4, 5]).unwrap();
    // Datasets and the checkpoint they bring the catalog to commit together.
    let rec = shapefile(v, 10, vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    d.cat
        .write(|tx| {
            tx.upsert_dataset(&rec)?;
            tx.set_checkpoint(v, &cp)
        })
        .unwrap();
    assert_eq!(d.cat.checkpoint(v).unwrap(), Some(cp.clone()));
    assert!(d.cat.volume(v).unwrap().unwrap().checkpoint_at.is_some());
    assert_eq!(d.cat.checkpoint(other).unwrap(), None);

    // A rolled-back batch leaves the old checkpoint.
    let newer = Checkpoint::new(2, vec![9]).unwrap();
    let _ = d.cat.write(|tx| {
        tx.set_checkpoint(v, &newer)?;
        tx.upsert_dataset(&shapefile(
            VolumeId(42),
            1,
            vector_layer("x", 1, wsen(0.0, 0.0, 1.0, 1.0)),
        ))
    });
    assert_eq!(d.cat.checkpoint(v).unwrap(), Some(cp));

    d.cat.write(|tx| tx.clear_checkpoint(v)).unwrap();
    assert_eq!(d.cat.checkpoint(v).unwrap(), None);
    assert!(matches!(
        d.cat.write(|tx| tx.set_checkpoint(VolumeId(99), &newer)),
        Err(CatalogError::UnknownVolume(VolumeId(99)))
    ));
}

#[test]
fn corrupt_checkpoint_is_reported() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    raw(&d.path)
        .execute(
            "UPDATE volumes SET checkpoint = x'0102' WHERE id = ?1",
            [v.0],
        )
        .unwrap();
    assert!(matches!(
        d.cat.checkpoint(v),
        Err(CatalogError::Checkpoint(_))
    ));
}

#[test]
fn corrupt_rows_are_errors_not_panics() {
    let mut d = db();
    let v = volume(&mut d.cat, "a");
    let rec = shapefile(v, 10, vector_layer("a", 2193, wsen(1.0, 1.0, 2.0, 2.0)));
    upsert(&mut d.cat, &rec);
    let id = rec.dataset.id();
    let conn = raw(&d.path);
    for (sql, undo) in [
        (
            "UPDATE datasets SET format = 'nope'",
            "UPDATE datasets SET format = 'shp'",
        ),
        (
            "UPDATE dataset_files SET role = 'nope' WHERE role = 'prj'",
            "UPDATE dataset_files SET role = 'prj' WHERE role = 'nope'",
        ),
        (
            "UPDATE layers SET fields_json = '{'",
            "UPDATE layers SET fields_json = '[]'",
        ),
        (
            "UPDATE layers SET geometry_type = 'Blob'",
            "UPDATE layers SET geometry_type = 'Point'",
        ),
        (
            "UPDATE layers SET minx = NULL",
            "UPDATE layers SET minx = 1.7e6",
        ),
        (
            "UPDATE layers SET minx = 1e300",
            "UPDATE layers SET minx = 1.7e6",
        ),
        (
            "UPDATE layers SET ordinal = 5",
            "UPDATE layers SET ordinal = 0",
        ),
        (
            "UPDATE layers SET wgs84_parts = 3",
            "UPDATE layers SET wgs84_parts = 1",
        ),
        (
            "UPDATE layers SET feature_count = -1",
            "UPDATE layers SET feature_count = 1",
        ),
        (
            "UPDATE crs SET status = 'identified', epsg = NULL",
            "UPDATE crs SET epsg = 2193",
        ),
        (
            "UPDATE crs SET def_kind = 'nope'",
            "UPDATE crs SET def_kind = 'epsg'",
        ),
        (
            "UPDATE datasets SET status = 'ok', status_detail = 'x'",
            "UPDATE datasets SET status_detail = NULL",
        ),
    ] {
        conn.execute_batch(sql).unwrap();
        match d.cat.get_dataset(id) {
            Err(CatalogError::Corrupt(_)) => {}
            other => panic!("{sql}: expected Corrupt, got {other:?}"),
        }
        conn.execute_batch(undo).unwrap();
        d.cat.get_dataset(id).unwrap().unwrap();
    }
}
