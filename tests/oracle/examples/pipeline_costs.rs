//! Reproducible stage01 grouping, persisted snapshot and shared extraction measurements.
//! Run with --release; synthetic counts are not an owner-volume V4 result.
use rayon::prelude::*;
use std::hint::black_box;
use std::time::{Duration, Instant};
use whereabouts_core::VolumeId;
use whereabouts_datasets::{Grouper, MemoryTree};
use whereabouts_nameindex::{NameIndex, synth};
use whereabouts_oracle::{Corpus, adapters, corpus_dir};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}
fn percentiles(mut samples: Vec<Duration>) -> [f64; 3] {
    samples.sort_unstable();
    [0.5, 0.95, 0.99].map(|p| ms(samples[(samples.len() as f64 * p).ceil() as usize - 1]))
}

fn memory(stage: &str) {
    #[cfg(windows)]
    if let Some(m) = whereabouts_ntfs::process_memory() {
        println!(
            "memory stage={stage} working_set_bytes={} private_bytes={} peak_working_set_bytes={}",
            m.working_set, m.pagefile_usage, m.peak_working_set
        );
    }
    #[cfg(not(windows))]
    println!("memory stage={stage} unavailable: use external RSS tooling on this platform");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let entries: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "1000000".into())
        .parse()?;
    memory("initial");
    let mut names = NameIndex::new(synth::ROOT);
    let start = Instant::now();
    for r in synth::volume(entries, 1) {
        names.insert(&r)?;
    }
    names.compact();
    println!(
        "name_build entries={entries} ms={:.3} heap_bytes={} bytes_per_entry={:.2}",
        ms(start.elapsed()),
        names.stats().heap_bytes,
        names.stats().heap_bytes as f64 / entries.max(1) as f64
    );
    let start = Instant::now();
    memory("name_index");
    let mut tree = MemoryTree::new();
    for r in synth::volume(entries, 1) {
        tree.upsert(&r);
    }
    println!(
        "group_tree entries={} ms={:.3} (includes synthetic record regeneration)",
        tree.len(),
        ms(start.elapsed())
    );
    let start = Instant::now();
    memory("name_plus_tree");
    let mut grouper = Grouper::new(VolumeId(1));
    let delta = grouper.full_scan(&tree, synth::ROOT);
    println!(
        "group_scan datasets={} ms={:.3}",
        grouper.datasets().count(),
        ms(start.elapsed())
    );
    black_box(delta);
    memory("name_plus_tree_plus_grouper");
    // Persist into an isolated temp directory; save includes file sync and atomic rename.
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("benchmark.names");
    let start = Instant::now();
    let bytes = names.save_snapshot(&path, None)?;
    println!("snapshot_save bytes={bytes} ms={:.3}", ms(start.elapsed()));
    let start = Instant::now();
    let snapshot = NameIndex::load_snapshot(&path)?;
    println!(
        "snapshot_load entries={} ms={:.3} cache=warm",
        snapshot.index.len(),
        ms(start.elapsed())
    );
    drop(snapshot);
    drop(names);
    drop(tree);
    drop(grouper);

    let corpus = Corpus::load(corpus_dir())?;
    let cases: Vec<_> = corpus
        .cases()
        .iter()
        .filter(|c| corpus.is_present(c) && !c.is_corrupt())
        .collect();
    for adapter in adapters::registry() {
        let mut fresh = Vec::new();
        let mut warm = Vec::new();
        for case in cases.iter().filter(|c| c.format == adapter.format) {
            let resolved = adapters::ResolvedCase::new(case, corpus.case_dir(case));
            // A fresh thread has an empty CRS cache. Start timing inside the thread: no spawn cost.
            let elapsed = std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        let start = Instant::now();
                        black_box((adapter.read)(&resolved));
                        start.elapsed()
                    })
                    .join()
                    .expect("extraction thread")
            });
            fresh.push(elapsed);
            black_box((adapter.read)(&resolved));
            for _ in 0..20 {
                let start = Instant::now();
                black_box((adapter.read)(&resolved));
                warm.push(start.elapsed());
            }
        }
        println!(
            "extraction format={} fresh_thread_ms_p50_p95_p99={:?} warm_ms_p50_p95_p99={:?}",
            adapter.format,
            percentiles(fresh),
            percentiles(warm)
        );
    }
    for workers in [1, 2, 4, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()?;
        let jobs = cases.len() * 100;
        let start = Instant::now();
        pool.install(|| {
            (0..jobs).into_par_iter().for_each(|i| {
                let case = cases[i % cases.len()];
                let adapter = adapters::registry()
                    .iter()
                    .find(|a| a.format == case.format)
                    .expect("adapter");
                black_box((adapter.read)(&adapters::ResolvedCase::new(
                    case,
                    corpus.case_dir(case),
                )));
            })
        });
        let elapsed = start.elapsed();
        println!(
            "extraction_workers workers={workers} jobs={jobs} ms={:.3} datasets_per_second={:.1}",
            ms(elapsed),
            jobs as f64 / elapsed.as_secs_f64()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearest_rank_small_sample_boundaries() {
        assert_eq!(
            percentiles((1..=20).map(Duration::from_millis).collect()),
            [10., 19., 20.]
        );
        assert_eq!(
            percentiles((1..=50).map(Duration::from_millis).collect()),
            [25., 48., 50.]
        );
    }
}
