//! Targeted behaviour: root seeding, out-of-order parents, orphans, renames, delete + recreate,
//! cycles, events, and a large synthetic volume checked against brute force (multi-chunk scans).

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;

use whereabouts_core::{FileAttributes, FileName, FileRecord, Frn};
use whereabouts_nameindex::{
    EntryId, IndexError, KindFilter, MAX_NAME_BYTES, MatchMode, NameIndex, PathError, Query, synth,
};
use whereabouts_source::SourceEvent;

const ROOT: Frn = Frn(0x0005_0000_0000_0005);

fn f(n: u64) -> Frn {
    Frn::from_parts(n, 1).unwrap()
}

fn rec(frn: Frn, parent: Frn, name: &str, dir: bool) -> FileRecord {
    FileRecord {
        frn,
        parent,
        name: FileName::from(name),
        attrs: FileAttributes(if dir { FileAttributes::DIRECTORY } else { 0 }),
    }
}

fn path(index: &NameIndex, frn: Frn) -> Result<String, PathError> {
    index.path_lossy(index.lookup(frn).ok_or(PathError::NotFound)?)
}

#[test]
fn root_is_seeded_and_never_removed() {
    let mut index = NameIndex::new(ROOT);
    assert!(index.is_empty());
    assert_eq!(index.lookup(ROOT), Some(EntryId::ROOT));
    assert_eq!(index.path_lossy(EntryId::ROOT).unwrap(), "\\");
    assert!(!index.remove(ROOT));
    // A record for the root only updates its attributes.
    index.insert(&rec(ROOT, ROOT, "ignored", true)).unwrap();
    assert!(index.is_empty());
    assert_eq!(index.name(EntryId::ROOT), Some(&b""[..]));
    assert_eq!(index.search(&Query::substring("")).total, 0);
}

#[test]
fn children_before_parents_are_linked_later() {
    let mut index = NameIndex::new(ROOT);
    index
        .insert(&rec(f(30), f(20), "roads.shp", false))
        .unwrap();
    index.insert(&rec(f(20), f(10), "gis", true)).unwrap();
    assert_eq!(path(&index, f(30)), Err(PathError::Orphan));
    assert_eq!(index.stats().unresolved, 1);
    index.insert(&rec(f(10), ROOT, "Data", true)).unwrap();
    assert_eq!(path(&index, f(30)).unwrap(), "\\Data\\gis\\roads.shp");
    assert_eq!(index.stats().unresolved, 0);
}

#[test]
fn extend_metadata_stays_orphaned() {
    // `$Extend` (record 11) is never enumerated, so `$RmMetadata` and its children are orphans.
    let mut index = NameIndex::new(ROOT);
    let extend = Frn::from_parts(11, 11).unwrap();
    index
        .insert(&rec(f(27), extend, "$RmMetadata", true))
        .unwrap();
    index.insert(&rec(f(28), f(27), "$TxfLog", true)).unwrap();
    assert_eq!(path(&index, f(28)), Err(PathError::Orphan));
    assert_eq!(index.search(&Query::substring("txf")).total, 1);
    assert_eq!(
        index.search(&Query::substring("txf").in_path(true)).total,
        0
    );
}

#[test]
fn rename_move_delete_and_recreate() {
    let mut index = NameIndex::new(ROOT);
    index.insert(&rec(f(10), ROOT, "a", true)).unwrap();
    index.insert(&rec(f(11), ROOT, "b", true)).unwrap();
    index.insert(&rec(f(12), f(10), "x.tif", false)).unwrap();
    let before = index.lookup(f(12)).unwrap();
    // Rename + move in one upsert keeps the entry.
    index
        .apply(&SourceEvent::Renamed {
            record: rec(f(12), f(11), "y.TIF", false),
            old_parent: f(10),
            old_name: FileName::from("x.tif"),
        })
        .unwrap();
    assert_eq!(index.lookup(f(12)), Some(before));
    assert_eq!(path(&index, f(12)).unwrap(), "\\b\\y.TIF");
    assert_eq!(index.search(&Query::substring("x.tif")).total, 0);
    assert_eq!(index.search(&Query::wildcard("y.tif")).total, 1);
    // Deleting a directory orphans what is (inconsistently) still under it…
    index
        .apply(&SourceEvent::Deleted(rec(f(11), ROOT, "b", true)))
        .unwrap();
    assert_eq!(path(&index, f(12)), Err(PathError::Orphan));
    // …until the same FRN appears again.
    index
        .apply(&SourceEvent::Created(rec(f(11), ROOT, "B2", true)))
        .unwrap();
    assert_eq!(path(&index, f(12)).unwrap(), "\\B2\\y.TIF");
    assert!(index.stats().dead_slots > 0);
    index.compact();
    assert_eq!(index.stats().dead_slots, 0);
    assert_eq!(path(&index, f(12)).unwrap(), "\\B2\\y.TIF");
    // Modified only changes attributes.
    index
        .apply(&SourceEvent::Modified(FileRecord {
            attrs: FileAttributes(FileAttributes::HIDDEN),
            ..rec(f(12), f(11), "y.TIF", false)
        }))
        .unwrap();
    let id = index.lookup(f(12)).unwrap();
    assert_eq!(
        index.attrs(id),
        Some(FileAttributes(FileAttributes::HIDDEN))
    );
    assert_eq!(
        index.stats().dead_slots,
        0,
        "no new tombstone for an unchanged name"
    );
}

#[test]
fn cycles_are_guarded() {
    let mut index = NameIndex::new(ROOT);
    index.insert(&rec(f(10), f(11), "a", true)).unwrap();
    index.insert(&rec(f(11), f(10), "b", true)).unwrap();
    index.insert(&rec(f(12), f(11), "c", false)).unwrap();
    index.insert(&rec(f(13), f(13), "self", true)).unwrap();
    for n in [10, 11, 12, 13] {
        assert_eq!(path(&index, f(n)), Err(PathError::Cycle));
    }
    assert_eq!(index.search(&Query::substring("").in_path(true)).total, 0);
    assert_eq!(index.search(&Query::substring("")).total, 4);
}

#[test]
fn deep_chains_hit_the_depth_limit() {
    let mut index = NameIndex::new(ROOT);
    let mut parent = ROOT;
    for n in 0..20_000_u64 {
        let frn = f(100 + n);
        index.insert(&rec(frn, parent, "d", true)).unwrap();
        parent = frn;
    }
    assert!(path(&index, f(100 + 16_000)).is_ok());
    assert_eq!(path(&index, f(100 + 19_999)), Err(PathError::Cycle));
    // Path-mode search agrees with `path()`: exactly the entries at depth ≤ MAX_DEPTH (the first
    // 16,384 of the chain) can match, whichever chunk happens to evaluate a deep entry first.
    let max = whereabouts_nameindex::MAX_DEPTH as u64;
    assert!(path(&index, f(100 + max - 1)).is_ok());
    assert_eq!(path(&index, f(100 + max)), Err(PathError::Cycle));
    let within: BTreeSet<Frn> = (0..max).map(|n| f(100 + n)).collect();
    for (q, skip_top) in [
        (Query::substring("d").in_path(true), false),
        (Query::substring("d\\d").in_path(true), true), // `\d` itself has no `d\d`
        (Query::substring("").in_path(true), false),
        (Query::wildcard("*d").in_path(true), false),
    ] {
        let mut want = within.clone();
        if skip_top {
            want.remove(&f(100));
        }
        // Wildcards build every candidate's full path (quadratic on this chain): run them once.
        let runs = if q.mode == MatchMode::Wildcard { 1 } else { 3 };
        for _ in 0..runs {
            let got = index.search(&q);
            let frns: BTreeSet<Frn> = got.hits.iter().map(|&h| index.frn(h).unwrap()).collect();
            assert_eq!(got.total, want.len(), "{q:?}");
            assert_eq!(frns, want, "{q:?}");
        }
    }
    // Evaluating the deepest entry first (a lone chain hanging below the limit) gives the same.
    // Children are inserted before their parents, so the deepest entry has the lowest index.
    let mut deep_first = NameIndex::new(ROOT);
    for n in (0..20_000_u64).rev() {
        let parent = if n == 0 { ROOT } else { f(100_000 + n - 1) };
        deep_first
            .insert(&rec(f(100_000 + n), parent, "d", true))
            .unwrap();
    }
    assert_eq!(deep_first.stats().unresolved, 0);
    assert_eq!(
        deep_first
            .search(&Query::substring("d").in_path(true))
            .total,
        whereabouts_nameindex::MAX_DEPTH
    );
}

#[test]
fn invalid_names_are_rejected_without_panicking() {
    let mut index = NameIndex::new(ROOT);
    assert_eq!(
        index.insert_wtf8(f(1), ROOT, &[0xFF, 0x41], FileAttributes::default()),
        Err(IndexError::InvalidName)
    );
    assert_eq!(
        index.insert_utf16le(f(1), ROOT, &[0x41], FileAttributes::default()),
        Err(IndexError::InvalidName)
    );
    let long = vec![b'a'; MAX_NAME_BYTES + 1];
    assert_eq!(
        index.insert_wtf8(f(1), ROOT, &long, FileAttributes::default()),
        Err(IndexError::NameTooLong(MAX_NAME_BYTES + 1))
    );
    assert!(index.is_empty());
}

#[test]
fn sparse_and_colliding_frns_use_the_overflow_map() {
    let mut index = NameIndex::new(ROOT);
    let far = Frn(0x0001_7FFF_FFFF_FFF0);
    index.insert(&rec(far, ROOT, "far", false)).unwrap();
    index
        .insert(&rec(Frn::from_parts(40, 1).unwrap(), ROOT, "s1", false))
        .unwrap();
    index
        .insert(&rec(Frn::from_parts(40, 2).unwrap(), ROOT, "s2", false))
        .unwrap();
    assert_eq!(path(&index, far).unwrap(), "\\far");
    assert_eq!(
        path(&index, Frn::from_parts(40, 2).unwrap()).unwrap(),
        "\\s2"
    );
    assert!(index.remove(Frn::from_parts(40, 1).unwrap()));
    assert_eq!(
        path(&index, Frn::from_parts(40, 2).unwrap()).unwrap(),
        "\\s2"
    );
    assert!(
        index.stats().heap_bytes < 1 << 20,
        "far FRN must not allocate a huge table"
    );
}

/// Large enough for several parallel chunks; checked against brute force over the accessors.
#[test]
fn synthetic_volume_search_matches_brute_force() {
    let records: Vec<FileRecord> = synth::volume(120_000, 42).collect();
    let mut index = NameIndex::new(synth::ROOT);
    for r in &records {
        index.insert(r).unwrap();
    }
    // Rename a slice so stale segments sit in the pool.
    for r in records.iter().step_by(97) {
        let renamed = format!("{}~1", r.name);
        index
            .insert(&FileRecord {
                name: FileName::from(renamed),
                ..r.clone()
            })
            .unwrap();
    }
    let mut model = common::Model::new(synth::ROOT);
    for id in index.entries() {
        let frn = index.frn(id).unwrap();
        let parent = index.frn(index.parent(id).unwrap()).unwrap();
        let name = FileName::from_wtf8(index.name(id).unwrap())
            .unwrap()
            .to_utf16();
        model.upsert(frn, parent, name, index.attrs(id).unwrap().0);
    }
    let avg = index.stats().name_bytes as f64 / index.stats().slots as f64;
    assert!((15.0..40.0).contains(&avg), "average name length {avg}");
    for q in [
        Query::substring("a"),
        Query::substring("road"),
        Query::substring("~1"),
        Query::substring("STRASSE"),
        Query::substring("straße"),
        Query::substring("zzzq"),
        Query::substring("").extensions(["shp", "dbf"]),
        Query::substring("").kind(KindFilter::Dirs),
        Query::wildcard("img_2010*.jpg"),
        Query::wildcard("*.?pkg"),
        Query::wildcard("*"),
        Query::substring("\\v1").in_path(true),
        Query::substring("data\\").in_path(true),
        Query::wildcard("*\\tile_1*").in_path(true),
    ] {
        let got = index.search(&q);
        let frns: BTreeSet<Frn> = got.hits.iter().map(|&h| index.frn(h).unwrap()).collect();
        assert_eq!(frns.len(), got.total);
        assert_eq!(frns, model.search(&q), "{q:?}");
    }
}
