//! Property tests: after any sequence of upserts, removals, compactions and snapshot round trips,
//! path reconstruction and search agree with the naive model in `common`.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;

use common::Model;
use proptest::prelude::*;
use whereabouts_core::{FileAttributes, FileName, FileRecord, Frn};
use whereabouts_nameindex::{KindFilter, MatchMode, NameIndex, Query};

const ROOT: Frn = Frn(0x0005_0000_0000_0005);
/// A parent that never exists (stands in for `$Extend`).
const MISSING: Frn = Frn(0x000B_0000_0000_000B);

/// 16 record numbers × 2 sequence numbers: exercises the FRN map's dense-slot collisions.
fn frn(k: u8) -> Frn {
    Frn::from_parts(100 + u64::from(k % 16), 1 + u16::from(k / 16)).unwrap()
}

fn parent(k: u8) -> Frn {
    match k {
        32 => ROOT,
        33 => MISSING,
        k => frn(k),
    }
}

const ALPHABET: &[u16] = &[
    b'a' as u16,
    b'b' as u16,
    b'A' as u16,
    b'B' as u16,
    b'.' as u16,
    b'x' as u16,
    b'1' as u16,
    b' ' as u16,
    0x00DF, // ß
    0x03A3, // Σ
    0x03C2, // ς
    0x00E9, // é
    0x00C9, // É
    0x65E5, // 日
    0xD800, // unpaired surrogate
];

fn name() -> impl Strategy<Value = Vec<u16>> {
    prop::collection::vec(prop::sample::select(ALPHABET), 0..6)
}

#[derive(Debug, Clone)]
enum Op {
    Upsert {
        frn: u8,
        parent: u8,
        name: Vec<u16>,
        dir: bool,
        utf16: bool,
    },
    Remove(u8),
    Compact,
    Snapshot,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => (0..33_u8, 0..34_u8, name(), any::<bool>(), any::<bool>()).prop_map(
            |(f, p, name, dir, utf16)| Op::Upsert { frn: if f == 32 { 255 } else { f }, parent: p, name, dir, utf16 }
        ),
        2 => (0..32_u8).prop_map(Op::Remove),
        1 => Just(Op::Compact),
        1 => Just(Op::Snapshot),
    ]
}

fn query() -> impl Strategy<Value = Query> {
    let text = prop::collection::vec(
        prop::sample::select(&['a', 'B', '.', 'x', 'ß', 'σ', 'É', '日', ' ', '\\', '*', '?'][..]),
        0..4,
    )
    .prop_map(|cs| cs.into_iter().collect::<String>());
    let exts = prop::collection::vec(prop::sample::select(&["", "b", ".X", "a1", "日"][..]), 0..2);
    let kind = prop::sample::select(&[KindFilter::Any, KindFilter::Files, KindFilter::Dirs][..]);
    (text, any::<bool>(), any::<bool>(), exts, kind).prop_map(
        |(text, wildcard, path, exts, kind)| {
            let q = if wildcard {
                Query::wildcard(text)
            } else {
                Query::substring(text)
            };
            q.in_path(path).extensions(exts).kind(kind)
        },
    )
}

fn apply(index: &mut NameIndex, model: &mut Model, op: &Op) {
    match op {
        Op::Upsert {
            frn: f,
            parent: p,
            name,
            dir,
            utf16,
        } => {
            let f = if *f == 255 { ROOT } else { frn(*f) };
            let attrs = if *dir {
                FileAttributes::DIRECTORY
            } else {
                FileAttributes::ARCHIVE
            };
            if *utf16 {
                let bytes: Vec<u8> = name.iter().flat_map(|u| u.to_le_bytes()).collect();
                index
                    .insert_utf16le(f, parent(*p), &bytes, FileAttributes(attrs))
                    .unwrap();
            } else {
                let rec = FileRecord {
                    frn: f,
                    parent: parent(*p),
                    name: FileName::from_utf16(name),
                    attrs: FileAttributes(attrs),
                };
                index.insert(&rec).unwrap();
            }
            model.upsert(f, parent(*p), name.clone(), attrs);
        }
        Op::Remove(k) => {
            let f = frn(*k);
            assert_eq!(index.remove(f), model.recs.contains_key(&f));
            model.remove(f);
        }
        Op::Compact => index.compact(),
        Op::Snapshot => {
            let mut buf = Vec::new();
            index.write_snapshot(&mut buf, None).unwrap();
            *index = NameIndex::read_snapshot(buf.as_slice()).unwrap().index;
        }
    }
}

fn check(index: &NameIndex, model: &Model, queries: &[Query]) {
    assert_eq!(index.len(), model.recs.len());
    let listed: BTreeSet<Frn> = index.entries().map(|id| index.frn(id).unwrap()).collect();
    assert_eq!(listed, model.recs.keys().copied().collect());
    for &f in model.recs.keys() {
        let id = index.lookup(f).unwrap();
        assert_eq!(index.path(id), model.path(f), "path of {f}");
        assert_eq!(index.frn(id), Some(f));
        assert_eq!(
            index.name(id).unwrap(),
            common::wtf8(&model.recs[&f].1).as_slice()
        );
    }
    for q in queries {
        let got = index.search(q);
        assert_eq!(got.total, got.hits.len());
        let frns: BTreeSet<Frn> = got.hits.iter().map(|&id| index.frn(id).unwrap()).collect();
        assert_eq!(frns.len(), got.hits.len(), "duplicate hits for {q:?}");
        assert_eq!(frns, model.search(q), "query {q:?}");
        let limited = index.search(&q.clone().limit(2));
        assert_eq!(limited.total, got.total);
        assert_eq!(limited.hits.len(), got.total.min(2));
        assert!(limited.hits.iter().all(|h| got.hits.contains(h)));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn index_matches_naive_model(
        ops in prop::collection::vec(op(), 0..60),
        queries in prop::collection::vec(query(), 1..12),
    ) {
        let mut index = NameIndex::new(ROOT);
        let mut model = Model::new(ROOT);
        for op in &ops {
            apply(&mut index, &mut model, op);
        }
        check(&index, &model, &queries);
        // A final snapshot round trip and compaction preserve everything.
        apply(&mut index, &mut model, &Op::Snapshot);
        check(&index, &model, &queries);
        index.compact();
        check(&index, &model, &queries);
    }
}

#[test]
fn wildcard_and_path_queries_on_a_known_tree() {
    let mut index = NameIndex::new(ROOT);
    let mut model = Model::new(ROOT);
    let ops = [
        (0, 32, "Data", true),
        (1, 0, "Roads.SHP", false),
        (2, 0, "roads.dbf", false),
        (3, 0, "Σίσυφος", true),
        (4, 3, "café.tif", false),
        (5, 33, "$TxfLog", false),
    ];
    for (f, p, n, dir) in ops {
        let units: Vec<u16> = n.encode_utf16().collect();
        apply(
            &mut index,
            &mut model,
            &Op::Upsert {
                frn: f,
                parent: p,
                name: units,
                dir,
                utf16: f % 2 == 0,
            },
        );
    }
    let queries = [
        Query::wildcard("*.shp"),
        Query::wildcard("r*s.???"),
        Query::substring("ROADS"),
        Query::substring("data\\r").in_path(true),
        Query::substring("\\data").in_path(true),
        Query::substring("ς\\caf").in_path(true),
        Query::wildcard("\\data\\*").in_path(true),
        Query::wildcard("*\\σ*\\*.tif").in_path(true),
        Query::substring("txf"),
        Query::substring("txf").in_path(true),
        Query::substring("").extensions(["shp", "tif"]),
        Query::substring("").kind(KindFilter::Dirs),
        Query::substring("a\0"),
        Query {
            mode: MatchMode::Wildcard,
            ..Query::substring("")
        },
    ];
    check(&index, &model, &queries);
    let hits = |q: &Query| index.search(q).total;
    assert_eq!(hits(&Query::wildcard("*.shp")), 1);
    assert_eq!(hits(&Query::substring("data\\r").in_path(true)), 2);
    assert_eq!(hits(&Query::substring("ς\\caf").in_path(true)), 1);
    assert_eq!(
        hits(&Query::substring("txf")),
        1,
        "orphans are found by name"
    );
    assert_eq!(
        hits(&Query::substring("txf").in_path(true)),
        0,
        "but not by path"
    );
    assert_eq!(hits(&Query::substring("").extensions(["SHP", ".tif"])), 2);
}
