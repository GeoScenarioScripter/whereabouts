//! Prints build time, memory per entry, snapshot size/time and query latency percentiles for
//! synthetic volumes — the numbers recorded for Q-18.
//!
//! `cargo run --release -p whereabouts-nameindex --example measure -- 1000000 5000000`

use std::time::{Duration, Instant};

use whereabouts_nameindex::{NameIndex, Query, synth};

fn pct(sorted: &[Duration], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted.get(i).map_or(0.0, |d| d.as_secs_f64() * 1e3)
}

fn time_query(index: &NameIndex, q: &Query, runs: usize) -> (f64, f64, f64, usize) {
    let mut t = Vec::with_capacity(runs);
    let mut total = 0;
    for _ in 0..runs {
        let s = Instant::now();
        total = index.search(q).total;
        t.push(s.elapsed());
    }
    t.sort();
    (pct(&t, 0.5), pct(&t, 0.95), pct(&t, 0.99), total)
}

fn main() {
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        vec![1_000_000]
    } else {
        sizes
    };
    println!("threads: {}", rayon::current_num_threads());
    for n in sizes {
        let records: Vec<_> = synth::volume(n, 1).collect();
        let start = Instant::now();
        let mut index = NameIndex::new(synth::ROOT);
        for r in &records {
            index.insert(r).expect("insert");
        }
        let build = start.elapsed();
        let names: Vec<String> = records
            .iter()
            .step_by(997)
            .map(|r| r.name.to_string_lossy().to_lowercase())
            .collect();
        drop(records);
        let st = index.stats();
        println!("\n== {n} entries ==");
        println!(
            "build (insert from owned records): {:.2} s ({:.0} k entries/s)",
            build.as_secs_f64(),
            n as f64 / build.as_secs_f64() / 1e3
        );
        println!(
            "heap: {:.1} MiB = {:.1} B/entry (names {:.1} B/entry avg, folded pool {:.1} B/entry)",
            st.heap_bytes as f64 / (1 << 20) as f64,
            st.heap_bytes as f64 / n as f64,
            st.name_bytes as f64 / n as f64,
            st.folded_bytes as f64 / n as f64
        );
        index.compact();
        let st = index.stats();
        println!(
            "heap after compact (exact-size buffers): {:.1} MiB = {:.1} B/entry",
            st.heap_bytes as f64 / (1 << 20) as f64,
            st.heap_bytes as f64 / n as f64
        );

        let mut buf = Vec::new();
        let s = Instant::now();
        index.write_snapshot(&mut buf, None).expect("write");
        let w = s.elapsed();
        let s = Instant::now();
        let loaded = NameIndex::read_snapshot(buf.as_slice()).expect("read");
        let r = s.elapsed();
        println!(
            "snapshot: {:.1} MiB ({:.1} B/entry), write {:.0} ms, load {:.0} ms (in memory)",
            buf.len() as f64 / (1 << 20) as f64,
            buf.len() as f64 / n as f64,
            w.as_secs_f64() * 1e3,
            r.as_secs_f64() * 1e3
        );
        drop(loaded);
        drop(buf);

        println!(
            "\nquery latency by term length (substring, name mode; terms cut from real names):"
        );
        println!("len | p50 ms | p95 ms | p99 ms | median hits");
        for len in 1..=8 {
            let mut lat = Vec::new();
            let mut hits = Vec::new();
            for name in names.iter().filter(|s| s.chars().count() >= len).take(40) {
                let term: String = name.chars().take(len).collect();
                let q = Query::substring(term);
                for _ in 0..3 {
                    let s = Instant::now();
                    let res = index.search(&q);
                    lat.push(s.elapsed());
                    hits.push(res.total);
                }
            }
            lat.sort();
            hits.sort_unstable();
            println!(
                "{len:>3} | {:>6.2} | {:>6.2} | {:>6.2} | {}",
                pct(&lat, 0.5),
                pct(&lat, 0.95),
                pct(&lat, 0.99),
                hits.get(hits.len() / 2).copied().unwrap_or(0)
            );
        }

        println!("\nnamed queries (30 runs each):");
        println!("{:<26} | p50 ms | p95 ms | p99 ms | hits", "query");
        let queries = [
            ("sub 'a'", Query::substring("a")),
            ("sub 'road'", Query::substring("road")),
            ("sub 'qqzx' (no hit)", Query::substring("qqzx")),
            ("sub 'straße'", Query::substring("straße")),
            ("ext shp only", Query::substring("").extensions(["shp"])),
            (
                "sub 'road' + ext shp",
                Query::substring("road").extensions(["shp"]),
            ),
            ("wild '*.shp'", Query::wildcard("*.shp")),
            ("wild 'r*d?.tif'", Query::wildcard("r*d?.tif")),
            ("path sub 'road'", Query::substring("road").in_path(true)),
            ("path sub '\\v1'", Query::substring("\\v1").in_path(true)),
            (
                "path sub 'data\\road'",
                Query::substring("data\\road").in_path(true),
            ),
            (
                "path wild '*\\tile_1*'",
                Query::wildcard("*\\tile_1*").in_path(true),
            ),
            ("sub 'a' limit 100", Query::substring("a").limit(100)),
        ];
        for (label, q) in &queries {
            let (p50, p95, p99, total) = time_query(&index, q, 30);
            println!("{label:<26} | {p50:>6.2} | {p95:>6.2} | {p99:>6.2} | {total}");
        }
    }
}
