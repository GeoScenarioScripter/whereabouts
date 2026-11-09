//! Portable process-level acceptance: the indexed volume need not exist.
#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use whereabouts_catalog::Catalog;
use whereabouts_source::Checkpoint;

#[test]
fn only_the_ranked_page_hydrates_layer_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");
    fixture(&db, 300);
    let raw = rusqlite::Connection::open(&db).unwrap();
    raw.execute("UPDATE layers SET fields_json = 'invalid JSON' WHERE dataset_id = (SELECT id FROM datasets WHERE name = 'road_old')", []).unwrap();
    // Selective member lookups and the broad/empty summary paths must all rank and
    // apply the limit before touching the unselected record's malformed layer body.
    for (term, expected) in [("road", "road"), ("o", "broad"), ("", "broad")] {
        let rows = json(&["query", term, "--json", "--limit", "1"], &db);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, expected);
        assert!(
            !run(&["query", term, "--limit", "100"], &db)
                .status
                .success()
        );
    }
}

#[test]
fn ranking_members_filters_and_output() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");
    fixture(&db, 300);
    let rows = json(&["query", "ROAD", "--json"], &db);
    assert_eq!(
        rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["road", "road_new", "road_old", "broad"]
    );
    assert_eq!(json(&["query", "road.prj", "--json"], &db).len(), 2);
    assert_eq!(
        json(&["query", "o", "--json"], &db)
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["broad", "road_new", "road_old", "road"]
    );
    assert_eq!(
        json(&["query", "", "--json", "--limit", "1"], &db)[0].name,
        "broad"
    );
    assert!(json(&["query", "road", "--json", "--limit", "0"], &db).is_empty());
    assert!(json(&["query", "zzzzz", "--json"], &db).is_empty());
    assert!(json(&["query", "road", "--json", "--format", "gpkg"], &db).is_empty());
    let rows = json(
        &[
            "query",
            "road",
            "--json",
            "--bbox",
            "170,-5,-170,5",
            "--epsg",
            "4326",
            "--format",
            "shp",
        ],
        &db,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "road");
    assert_eq!(
        json(
            &[
                "query",
                "road",
                "--json",
                "--bbox",
                "-179,-5,-176,5",
                "--epsg",
                "4326"
            ],
            &db
        )
        .len(),
        1
    );
    let table = run(&["query", "road"], &db);
    assert!(table.status.success());
    assert!(String::from_utf8_lossy(&table.stdout).contains("NAME\tFORMAT"));
    for bbox in ["NaN,0,1,1", "0,2,1,1", "0,0,1", "-181,0,1,1"] {
        assert!(
            !run(&["query", "--bbox", bbox], &db).status.success(),
            "{bbox}"
        );
    }
}

#[test]
fn corrupt_missing_mismatched_and_ambiguous_snapshots_fail_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");
    fixture(&db, 0);
    let mut catalog = Catalog::open(&db).unwrap();
    let volume = catalog.volumes().unwrap()[0].id;
    catalog
        .write(|tx| tx.set_checkpoint(volume, &Checkpoint::new(1, vec![9]).unwrap()))
        .unwrap();
    assert!(!run(&["query", "road"], &db).status.success());
    fixture(&db, 0);
    catalog
        .write(|tx| {
            let id = tx.register_volume(&whereabouts_source::SourceIdentity {
                kind: "fixture".into(),
                key: "two".into(),
                label: "two".into(),
            })?;
            tx.set_checkpoint(id, &Checkpoint::new(1, b"query-fixture".to_vec()).unwrap())
        })
        .unwrap();
    assert!(!run(&["query", "road"], &db).status.success());
    assert_eq!(
        json(
            &[
                "query",
                "road",
                "--json",
                "--volume-id",
                &volume.0.to_string()
            ],
            &db
        )
        .len(),
        4
    );
    let snapshot = dir.path().join("catalog.db.names");
    std::fs::write(&snapshot, b"bad").unwrap();
    assert!(!run(&["query", "road"], &db).status.success());
    std::fs::remove_file(snapshot).unwrap();
    assert!(!run(&["query", "road"], &db).status.success());
}
