#![allow(clippy::unwrap_used)]
use std::path::Path;
use std::process::{Command, Output};
use whereabouts_catalog::Catalog;
use whereabouts_core::*;
use whereabouts_nameindex::NameIndex;
use whereabouts_source::{Checkpoint, SourceIdentity};

pub fn run(args: &[&str], db: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_whereabouts"))
        .args(args)
        .arg("--db")
        .arg(db)
        .output()
        .unwrap()
}
pub fn json(args: &[&str], db: &Path) -> Vec<DatasetRecord> {
    let output = run(args, db);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
pub fn layer(epsg: u32, west: f64, east: f64) -> LayerRecord {
    LayerRecord {
        metadata: LayerMetadata {
            name: "layer".into(),
            crs: Some(CrsDefinition::epsg(epsg)),
            native_bounds: Some(Bounds::new(-180., -10., 180., 10.).unwrap()),
            ..Default::default()
        },
        crs: Some(ResolvedCrs::Identified { epsg, name: None }),
        wgs84: Some(Wgs84Extent::from_west_south_east_north(west, -10., east, 10.).unwrap()),
    }
}
pub fn add(names: &mut NameIndex, frn: u64, parent: u64, name: &str) {
    names
        .insert(&FileRecord {
            frn: Frn(frn),
            parent: Frn(parent),
            name: name.into(),
            attrs: FileAttributes(0),
        })
        .unwrap();
}
pub fn fixture(db: &Path, extra_names: usize) {
    let mut catalog = Catalog::open(db).unwrap();
    let mut names = NameIndex::new(Frn(1));
    let cp = Checkpoint::new(1, b"query-fixture".to_vec()).unwrap();
    catalog
        .write(|tx| {
            let volume = tx.register_volume(&SourceIdentity {
                kind: "fixture".into(),
                key: "one".into(),
                label: "offline".into(),
            })?;
            for (i, name, mtime, parent) in [
                (10, "road", 1, 1),
                (20, "road_old", 2, 1),
                (30, "road_new", 3, 1),
                (40, "broad", 99, 1),
                (50, "road_orphan", 999, 999),
            ] {
                add(&mut names, i, parent, &format!("{name}.shp"));
                add(&mut names, i + 1, 1, &format!("{name}.prj"));
                let dataset = Dataset::new(
                    DatasetId::new(volume, Frn(i)),
                    DatasetKind::Vector,
                    Format::Shapefile,
                    vec![
                        Member {
                            frn: Frn(i),
                            role: MemberRole::Main,
                        },
                        Member {
                            frn: Frn(i + 1),
                            role: MemberRole::Prj,
                        },
                    ],
                )
                .unwrap();
                tx.upsert_dataset(&DatasetRecord {
                    dataset,
                    name: name.into(),
                    primary_path: format!("offline/{name}.shp"),
                    total_size: None,
                    mtime: Some(Timestamp(mtime)),
                    status: ExtractionStatus::Ok,
                    extracted_at: Some(Timestamp(1)),
                    layers: if i == 10 {
                        vec![layer(4326, 175., -175.)]
                    } else {
                        vec![layer(4326, 10., 20.), layer(3857, 175., -175.)]
                    },
                })?;
            }
            tx.set_checkpoint(volume, &cp)
        })
        .unwrap();
    for i in 0..extra_names {
        add(
            &mut names,
            1000 + i as u64,
            1,
            &format!("ordinary_file_{i}.txt"),
        );
    }
    let mut snapshot = db.as_os_str().to_os_string();
    snapshot.push(".names");
    names
        .save_snapshot(Path::new(&snapshot), Some(&cp))
        .unwrap();
}
