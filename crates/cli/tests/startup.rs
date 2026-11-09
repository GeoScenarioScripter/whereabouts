//! Repeatable process-start benchmark; synthetic snapshot only, no source enumeration.
#![allow(clippy::unwrap_used)]
#[allow(dead_code)]
mod common;
use std::{
    io::{BufRead, Read},
    process::{Command, Stdio},
    time::Instant,
};

#[test]
#[ignore = "measurement: cargo test --release -p whereabouts-cli --test startup query_at_gis_scale -- --ignored --nocapture"]
fn query_at_gis_scale() {
    use whereabouts_core::*;
    use whereabouts_source::SourceIdentity;
    for count in [10_000_u64, 100_000] {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = whereabouts_catalog::Catalog::open(dir.path().join("gis.db")).unwrap();
        let mut names = whereabouts_nameindex::NameIndex::new(Frn(1));
        let volume = catalog
            .write(|tx| {
                let volume = tx.register_volume(&SourceIdentity {
                    kind: "synthetic".into(),
                    key: "gis-scale".into(),
                    label: "offline".into(),
                })?;
                for i in 0..count {
                    let name = format!("road_{i:06}");
                    let frn = i + 10;
                    common::add(&mut names, frn, 1, &format!("{name}.shp"));
                    tx.upsert_dataset(&DatasetRecord {
                        dataset: Dataset::new(
                            DatasetId::new(volume, Frn(frn)),
                            DatasetKind::Vector,
                            Format::Shapefile,
                            vec![Member {
                                frn: Frn(frn),
                                role: MemberRole::Main,
                            }],
                        )
                        .unwrap(),
                        name: name.clone(),
                        primary_path: format!("offline/{name}.shp"),
                        total_size: None,
                        mtime: Some(Timestamp(i as i64)),
                        status: ExtractionStatus::Ok,
                        extracted_at: Some(Timestamp(1)),
                        layers: vec![common::layer(4326, 10., 20.)],
                    })?;
                }
                Ok(volume)
            })
            .unwrap();
        let engine = whereabouts_query::QueryEngine::new(0).unwrap();
        for term in ["road_000001", "road_000", "road", ""] {
            if std::env::var("WHEREABOUTS_BENCH_TERM").is_ok_and(|selected| selected != term) {
                continue;
            }
            let mut times = Vec::new();
            let request = whereabouts_query::Request {
                text: term.into(),
                limit: 100,
                ..Default::default()
            };
            // Warm the SQLite/name-index caches once before the five timed calls.
            assert!(
                !engine
                    .search(&names, &catalog, volume, &request)
                    .unwrap()
                    .is_empty()
            );
            for _ in 0..5 {
                let start = Instant::now();
                let rows = engine.search(&names, &catalog, volume, &request).unwrap();
                times.push(start.elapsed().as_secs_f64() * 1000.);
                assert_eq!(rows.len(), if term == "road_000001" { 1 } else { 100 });
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "[query-gis-scale] datasets={count}, names={count}, one layer/dataset, term={term:?}, limit=100, n=5, warm reused query p50/max-of-five={:.3}/{:.3} ms",
                times[2], times[4]
            );
        }
    }
}

#[test]
#[ignore = "measurement: cargo test --release -p whereabouts-cli --test startup -- --ignored --nocapture"]
fn startup_to_first_dataset() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");
    common::fixture(&db, 1_000_000);
    let catalog = whereabouts_catalog::Catalog::open_read_only(&db).unwrap();
    let snapshot =
        whereabouts_nameindex::NameIndex::load_snapshot(&dir.path().join("catalog.db.names"))
            .unwrap();
    let engine = whereabouts_query::QueryEngine::new(0).unwrap();
    let volume = catalog.volumes().unwrap()[0].id;
    for term in ["road.prj", "road", "o", ""] {
        let mut latencies = Vec::new();
        let mut starts = Vec::new();
        for _ in 0..10 {
            let started = Instant::now();
            let rows = engine
                .search(
                    &snapshot.index,
                    &catalog,
                    volume,
                    &whereabouts_query::Request {
                        text: term.into(),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(!rows.is_empty());
            latencies.push(started.elapsed().as_secs_f64() * 1000.);
        }
        // Measure reused queries together, before fresh processes compete for memory.
        for _ in 0..10 {
            let started = Instant::now();
            let mut child = Command::new(env!("CARGO_BIN_EXE_whereabouts"))
                .args(["query", term, "--db"])
                .arg(&db)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            line.clear();
            assert!(stdout.read_line(&mut line).unwrap() > 0);
            starts.push(started.elapsed().as_secs_f64() * 1000.);
            stdout.read_to_end(&mut Vec::new()).unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        latencies.sort_by(f64::total_cmp);
        starts.sort_by(f64::total_cmp);
        eprintln!(
            "[query-startup] 1,000,010 names, 5 catalog datasets (1 hidden orphan), term={term:?}, limit=100, n=10; reused query p50/p95={:.3}/{:.3} ms; process-to-first-dataset p50/p95={:.3}/{:.3} ms",
            latencies[4], latencies[9], starts[4], starts[9]
        );
    }
}
