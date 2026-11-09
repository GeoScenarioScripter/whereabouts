//! Ingestion cost and the #26 question: is a borrowed `FileRecord` view in `RecordSink` worth a
//! contract change?
//!
//! Input: 1M synthetic records encoded as real `FSCTL_ENUM_USN_DATA` output (1 MiB batches, via
//! `whereabouts-ntfs`'s `test-fixtures` builders), walked with the same `RecordIter` the enumerator
//! uses. Variants:
//! - `owned_sink_insert`: today's contract — decode each name into an owned `FileName`, build a
//!   `FileRecord`, pass it through `&mut dyn RecordSink`, insert it.
//! - `borrowed_insert`: zero-copy — `NameIndex::insert_utf16le` straight from the kernel buffer.
//! - `owned_records_only` / `walk_only`: the record allocation and the buffer walk on their own.
//!
//! `cargo bench -p whereabouts-nameindex --bench ingest`

use std::hint::black_box;
use std::ops::ControlFlow;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use whereabouts_core::{FileAttributes, FileName, FileRecord, Frn};
use whereabouts_nameindex::{NameIndex, synth};
use whereabouts_ntfs::record::build::{RecordSpec, enum_output, record};
use whereabouts_ntfs::record::parse_enum_output;
use whereabouts_source::RecordSink;

const N: usize = 1_000_000;
const BATCH_BYTES: usize = 1 << 20;

fn batches() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur: Vec<Vec<u8>> = Vec::new();
    let mut size = 8;
    for r in synth::volume(N, 3) {
        let rec = record(&RecordSpec {
            major: 2,
            frn: r.frn.0,
            parent_frn: r.parent.0,
            usn: 0,
            timestamp: 0,
            reason: 0,
            file_attributes: r.attrs.0,
            name: r.name.to_utf16(),
        });
        if size + rec.len() > BATCH_BYTES {
            out.push(enum_output(0, &cur));
            cur.clear();
            size = 8;
        }
        size += rec.len();
        cur.push(rec);
    }
    out.push(enum_output(0, &cur));
    out
}

fn owned(batches: &[Vec<u8>], sink: &mut dyn RecordSink) {
    let mut units = Vec::with_capacity(256);
    for b in batches {
        let Ok((_, iter)) = parse_enum_output(b) else {
            continue;
        };
        for r in iter.flatten() {
            units.clear();
            units.extend(r.name_units());
            let rec = FileRecord {
                frn: Frn(r.frn),
                parent: Frn(r.parent_frn),
                name: FileName::from_utf16(&units),
                attrs: FileAttributes(r.file_attributes),
            };
            if sink.accept(rec).is_break() {
                return;
            }
        }
    }
}

fn bench(c: &mut Criterion) {
    let batches = batches();
    let mut group = c.benchmark_group("ingest_1m");
    group.sample_size(10);
    group.throughput(Throughput::Elements(N as u64));
    group.bench_function("owned_sink_insert", |b| {
        b.iter(|| {
            let mut index = NameIndex::with_capacity(synth::ROOT, N, N * 25);
            let mut sink = |rec: FileRecord| {
                let _ = index.insert(&rec);
                ControlFlow::Continue(())
            };
            owned(&batches, &mut sink);
            black_box(index.len())
        });
    });
    group.bench_function("borrowed_insert", |b| {
        b.iter(|| {
            let mut index = NameIndex::with_capacity(synth::ROOT, N, N * 25);
            for batch in &batches {
                let Ok((_, iter)) = parse_enum_output(batch) else {
                    continue;
                };
                for r in iter.flatten() {
                    let _ = index.insert_utf16le(
                        Frn(r.frn),
                        Frn(r.parent_frn),
                        r.name_utf16le(),
                        FileAttributes(r.file_attributes),
                    );
                }
            }
            black_box(index.len())
        });
    });
    group.bench_function("owned_records_only", |b| {
        b.iter(|| {
            let mut n = 0_usize;
            let mut sink = |rec: FileRecord| {
                n += rec.name.len();
                ControlFlow::Continue(())
            };
            owned(&batches, &mut sink);
            black_box(n)
        });
    });
    group.bench_function("walk_only", |b| {
        b.iter(|| {
            let mut n = 0_usize;
            for batch in &batches {
                let Ok((_, iter)) = parse_enum_output(batch) else {
                    continue;
                };
                n += iter
                    .flatten()
                    .map(|r| r.name_utf16le().len())
                    .sum::<usize>();
            }
            black_box(n)
        });
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
