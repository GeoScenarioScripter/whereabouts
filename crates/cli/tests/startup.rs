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
