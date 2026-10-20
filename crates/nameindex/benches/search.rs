//! Search latency on synthetic 1M- and 5M-entry volumes (inputs to Q-18).
//!
//! `cargo bench -p whereabouts-nameindex --bench search` (set `WB_BENCH_SIZES=1000000` to skip 5M).

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use whereabouts_nameindex::{NameIndex, Query, synth};

fn build(n: usize) -> NameIndex {
    let mut index = NameIndex::with_capacity(synth::ROOT, n, n * 25);
    for r in synth::volume(n, 1) {
        let _ = index.insert(&r);
    }
    index
}

fn queries() -> Vec<(&'static str, Query)> {
    vec![
        ("sub_1ch_a", Query::substring("a")),
        ("sub_2ch_ro", Query::substring("ro")),
        ("sub_4ch_road", Query::substring("road")),
        ("sub_10ch", Query::substring("roads_2019")),
        ("sub_nohit", Query::substring("qqzx")),
        ("sub_nonascii", Query::substring("straße")),
        ("ext_only_shp", Query::substring("").extensions(["shp"])),
        (
            "sub_road_ext_shp",
            Query::substring("road").extensions(["shp"]),
        ),
        ("wild_star_shp", Query::wildcard("*.shp")),
        ("wild_r_d_tif", Query::wildcard("r*d?.tif")),
        ("path_sub_road", Query::substring("road").in_path(true)),
        ("path_sub_v1", Query::substring("\\v1").in_path(true)),
        (
            "path_sub_data_road",
            Query::substring("data\\road").in_path(true),
        ),
        ("limit100_a", Query::substring("a").limit(100)),
    ]
}

fn sizes() -> Vec<usize> {
    std::env::var("WB_BENCH_SIZES")
        .ok()
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1_000_000, 5_000_000])
}

fn bench(c: &mut Criterion) {
    for n in sizes() {
        let index = build(n);
        let mut group = c.benchmark_group(format!("search_{}k", n / 1000));
        group.sample_size(20);
        for (name, q) in queries() {
            group.bench_with_input(BenchmarkId::from_parameter(name), &q, |b, q| {
                b.iter(|| black_box(index.search(black_box(q))).total);
            });
        }
        group.finish();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
