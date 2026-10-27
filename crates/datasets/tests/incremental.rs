//! Incremental grouping: every event type, batch ordering, and equivalence with a full scan.
#![allow(clippy::unwrap_used)]

mod common;

use std::cell::Cell;
use std::collections::BTreeSet;

use common::{Fx, ROOT, VOL};
use whereabouts_core::{DatasetId, FileName, Frn};
use whereabouts_datasets::{DirectoryView, Entry, GroupDelta, Grouper};
use whereabouts_source::SourceEvent;

fn id(fx: &Fx, path: &str) -> DatasetId {
    DatasetId::new(VOL, fx.frn(path))
}

fn scanned(fx: &Fx) -> Grouper {
    let mut g = Grouper::new(VOL);
    g.full_scan(&fx.tree, ROOT);
    g
}

/// The incremental state must equal a fresh full scan of the same tree.
fn assert_consistent(fx: &Fx, g: &Grouper) {
    assert_eq!(fx.describe_all(g), fx.describe_all(&fx.scan()));
}

fn apply(fx: &Fx, g: &mut Grouper, events: &[SourceEvent]) -> GroupDelta {
    let d = g.apply_batch(&fx.tree, events);
    assert_consistent(fx, g);
    d
}

#[test]
fn full_scan_reports_everything_added_then_nothing() {
    let fx = Fx::with(&["a.shp", "a.dbf", "d/r.tif", "x.gdb/a00000001.gdbtable"]);
    let mut g = Grouper::new(VOL);
    let first = g.full_scan(&fx.tree, ROOT);
    assert_eq!(first.added, {
        let mut v = vec![id(&fx, "a.shp"), id(&fx, "d/r.tif"), id(&fx, "x.gdb")];
        v.sort();
        v
    });
    assert_eq!(first.dirty().count(), 3);
    assert!(
        g.full_scan(&fx.tree, ROOT).is_empty(),
        "rescan of an unchanged tree"
    );
}

#[test]
fn full_scan_reconciles_after_rescan() {
    let mut fx = Fx::with(&["a.shp", "b.shp", "d/c.tif"]);
    let mut g = scanned(&fx);
    let b = id(&fx, "b.shp");
    let c = id(&fx, "d/c.tif");
    fx.delete("b.shp");
    fx.delete("d/c.tif");
    fx.delete("d/");
    fx.create("a.dbf");
    fx.create("n.gpkg");
    let d = g.full_scan(&fx.tree, ROOT);
    assert_eq!(d.removed, {
        let mut v = vec![b, c];
        v.sort();
        v
    });
    assert_eq!(d.added, [id(&fx, "n.gpkg")]);
    assert_eq!(d.changed, [id(&fx, "a.shp")]);
    assert_consistent(&fx, &g);
}

#[test]
fn created_sidecar_changes_dataset() {
    let mut fx = Fx::with(&["a.shp"]);
    let mut g = scanned(&fx);
    let ev = fx.create("a.dbf");
    let d = apply(&fx, &mut g, &[ev]);
    assert_eq!(d.changed, [id(&fx, "a.shp")]);
    assert!(d.added.is_empty() && d.removed.is_empty());
}

#[test]
fn created_anchor_adopts_sidecars_and_lone_dbf_disappears() {
    let mut fx = Fx::with(&["a.dbf", "a.shx"]);
    let mut g = scanned(&fx);
    let table = id(&fx, "a.dbf");
    let ev = fx.create("a.shp");
    let d = apply(&fx, &mut g, &[ev]);
    assert_eq!(d.added, [id(&fx, "a.shp")]);
    assert_eq!(d.removed, [table], "the .dbf is now a member, not a table");
}

#[test]
fn created_for_known_frn_is_an_upsert() {
    let mut fx = Fx::with(&["a.shp", "a.dbf", "other/"]);
    let mut g = scanned(&fx);
    // Replay: Created for an already known file at the same place.
    let same = SourceEvent::Created(fx.record("a.shp"));
    assert!(apply(&fx, &mut g, std::slice::from_ref(&same)).is_empty());
    // Unpaired rename: the new name arrives as Created; the old location is not in the event.
    let anchor = id(&fx, "a.shp");
    let SourceEvent::Renamed { record, .. } = fx.rename("a.shp", "other/a.shp") else {
        unreachable!()
    };
    let d = apply(&fx, &mut g, &[SourceEvent::Created(record)]);
    assert_eq!(d.changed, [anchor], "moved away from its .dbf");
    assert_eq!(d.added, [id(&fx, "a.dbf")], "left behind as a table");
}

#[test]
fn deleted_sidecar_anchor_and_unknown() {
    let mut fx = Fx::with(&["a.shp", "a.dbf", "a.prj", "readme.txt"]);
    let mut g = scanned(&fx);
    let anchor = id(&fx, "a.shp");
    let ev = fx.delete("a.prj");
    assert_eq!(apply(&fx, &mut g, &[ev]).changed, [anchor]);
    let ev = fx.delete("a.shp");
    let d = apply(&fx, &mut g, &[ev]);
    assert_eq!(d.removed, [anchor]);
    assert_eq!(d.added, [id(&fx, "a.dbf")]);
    // Deleting something never seen (created and deleted within one batch) is harmless.
    let ghost = common::record(Frn(9_999), ROOT, FileName::from("ghost.shp"), false);
    assert!(apply(&fx, &mut g, &[SourceEvent::Deleted(ghost)]).is_empty());
    let ev = fx.delete("readme.txt");
    assert!(apply(&fx, &mut g, &[ev]).is_empty());
}

#[test]
fn anchor_rename_needs_no_reextraction() {
    let mut fx = Fx::with(&["r.tif", "r.tif.ovr", "d/"]);
    let mut g = scanned(&fx);
    let anchor = id(&fx, "r.tif");
    // Rename all members together (one batch): same members, new name.
    let evs = [
        fx.rename("r.tif", "s.tif"),
        fx.rename("r.tif.ovr", "s.tif.ovr"),
    ];
    let d = apply(&fx, &mut g, &evs);
    assert_eq!(d.renamed, [anchor]);
    assert_eq!(d.dirty().count(), 0);
    // Move both into another directory.
    let evs = [
        fx.rename("s.tif.ovr", "d/s.tif.ovr"),
        fx.rename("s.tif", "d/s.tif"),
    ];
    let d = apply(&fx, &mut g, &evs);
    assert_eq!(d.renamed, [anchor]);
    assert_eq!(d.dirty().count(), 0);
    // Rename + modify in one batch: re-extract instead.
    let evs = [
        fx.rename("d/s.tif", "d/t.tif"),
        fx.rename("d/s.tif.ovr", "d/t.tif.ovr"),
        fx.modify("d/t.tif"),
    ];
    let d = apply(&fx, &mut g, &evs);
    assert_eq!(d.modified, [anchor]);
    assert!(d.renamed.is_empty());
}

#[test]
fn anchor_rename_that_leaves_sidecars_behind() {
    let mut fx = Fx::with(&["a.shp", "a.dbf", "a.shx"]);
    let mut g = scanned(&fx);
    let anchor = id(&fx, "a.shp");
    let ev = fx.rename("a.shp", "b.shp");
    let d = apply(&fx, &mut g, &[ev]);
    assert_eq!(d.changed, [anchor], "same ID, different members");
    assert_eq!(d.added, [id(&fx, "a.dbf")]);
    assert!(d.renamed.is_empty());
}

#[test]
fn sidecar_rename_out_of_and_into_a_dataset() {
    let mut fx = Fx::with(&["a.shp", "a.prj", "b.shp"]);
    let mut g = scanned(&fx);
    let ev = fx.rename("a.prj", "b.prj");
    let d = apply(&fx, &mut g, &[ev]);
    let mut want = vec![id(&fx, "a.shp"), id(&fx, "b.shp")];
    want.sort();
    assert_eq!(d.changed, want);
    // Renaming to an irrelevant name and back.
    let ev = fx.rename("b.prj", "b.prj.bak");
    assert_eq!(apply(&fx, &mut g, &[ev]).changed, [id(&fx, "b.shp")]);
    let ev = fx.rename("b.prj.bak", "b.prj");
    assert_eq!(apply(&fx, &mut g, &[ev]).changed, [id(&fx, "b.shp")]);
}

#[test]
fn modified_member_marks_dataset_dirty() {
    let mut fx = Fx::with(&[
        "a.shp",
        "a.prj",
        "r.tif",
        "notes.txt",
        "g.gpkg",
        "g.gpkg-wal",
    ]);
    let mut g = scanned(&fx);
    // a.prj... is only in a.shp here (r.tif has stem r).
    let ev = fx.modify("a.prj");
    let d = apply(&fx, &mut g, &[ev]);
    assert_eq!(d.modified, [id(&fx, "a.shp")]);
    assert!(d.changed.is_empty() && d.renamed.is_empty());
    let ev = fx.modify("notes.txt");
    assert!(apply(&fx, &mut g, &[ev]).is_empty());
    // -wal is not a member: writes there do not dirty the GeoPackage (spec: ignore -wal).
    let ev = fx.modify("g.gpkg-wal");
    assert!(apply(&fx, &mut g, &[ev]).is_empty());
}

#[test]
fn shared_member_dirties_every_dataset() {
    let mut fx = Fx::with(&["a.shp", "a.tif", "a.prj"]);
    let mut g = scanned(&fx);
    let ev = fx.modify("a.prj");
    let mut want = vec![id(&fx, "a.shp"), id(&fx, "a.tif")];
    want.sort();
    assert_eq!(apply(&fx, &mut g, &[ev]).modified, want);
}

#[test]
fn modified_unknown_relevant_file_is_a_missed_create() {
    let mut fx = Fx::with(&[]);
    let mut g = scanned(&fx);
    // The file exists in the view but the grouper never saw its Created.
    fx.add("late.gpkg");
    let ev = fx.modify("late.gpkg");
    assert_eq!(apply(&fx, &mut g, &[ev]).added, [id(&fx, "late.gpkg")]);
}

#[test]
fn fgdb_lifecycle() {
    let mut fx = Fx::with(&["work/"]);
    let mut g = scanned(&fx);
    // Out of order: the signature file is moved into x.gdb before x.gdb's Created arrives.
    fx.add("tmp/a00000001.gdbtable");
    let mut g2 = scanned(&fx);
    let mv = fx.rename("tmp/a00000001.gdbtable", "work/x.gdb/a00000001.gdbtable");
    let created = SourceEvent::Created(fx.record("work/x.gdb"));
    let d = apply(&fx, &mut g2, &[mv, created]);
    assert_eq!(d.added, [id(&fx, "work/x.gdb")]);
    drop(g2);
    g.full_scan(&fx.tree, ROOT);
    // Internal file churn: changed (new member) and modified (existing member).
    let ev = fx.create("work/x.gdb/a00000002.gdbtable");
    assert_eq!(apply(&fx, &mut g, &[ev]).changed, [id(&fx, "work/x.gdb")]);
    let ev = fx.modify("work/x.gdb/a00000002.gdbtable");
    assert_eq!(apply(&fx, &mut g, &[ev]).modified, [id(&fx, "work/x.gdb")]);
    // Lock files are irrelevant.
    let ev = fx.create("work/x.gdb/_gdb.host.1.sr.lock");
    assert!(apply(&fx, &mut g, &[ev]).is_empty());
    // Renaming the folder keeps the dataset (anchor rename) and reports the moved dir.
    let gdb = id(&fx, "work/x.gdb");
    let ev = fx.rename("work/x.gdb", "work/y.gdb");
    let d = apply(&fx, &mut g, &[ev]);
    assert_eq!(d.renamed, [gdb]);
    assert_eq!(d.moved_dirs, [gdb.anchor]);
    // Renaming it to a non-.gdb name removes the dataset; contents still never surface.
    let ev = fx.rename("work/y.gdb", "work/y_backup");
    assert_eq!(apply(&fx, &mut g, &[ev]).removed, [gdb]);
    assert!(g.is_empty());
}

#[test]
fn directory_events() {
    let mut fx = Fx::with(&["p/q/a.shp", "p/q/a.dbf", "p/b.tif", "z/"]);
    let mut g = scanned(&fx);
    let a = id(&fx, "p/q/a.shp");
    let b = id(&fx, "p/b.tif");
    // Moving a directory changes no dataset, but reports the move.
    let p = fx.frn("p");
    let ev = fx.rename("p/", "z/p/");
    let d = apply(&fx, &mut g, &[ev]);
    assert_eq!(d.moved_dirs, [p]);
    assert!(d.dirty().count() == 0 && d.renamed.is_empty() && d.removed.is_empty());
    let mut under = vec![a, b];
    under.sort();
    assert_eq!(g.datasets_under(&fx.tree, p), under);
    // Modified on a directory is ignored; Created of an empty new directory changes nothing.
    let ev = fx.modify("z/p/");
    assert!(apply(&fx, &mut g, &[ev]).is_empty());
    let ev = fx.create("new/");
    assert!(apply(&fx, &mut g, &[ev]).is_empty());
    // Unpaired directory rename (arrives as Created) of a directory holding datasets.
    let q = fx.frn("z/p/q");
    let SourceEvent::Renamed { record, .. } = fx.rename("z/p/q/", "new/q/") else {
        unreachable!()
    };
    let d = apply(&fx, &mut g, &[SourceEvent::Created(record)]);
    assert_eq!(d.moved_dirs, [q]);
    // Deleting a directory drops its datasets (children first, as NTFS requires).
    let evs = [
        fx.delete("new/q/a.shp"),
        fx.delete("new/q/a.dbf"),
        fx.delete("new/q/"),
    ];
    assert_eq!(apply(&fx, &mut g, &evs).removed, [a]);
}

#[test]
fn deleted_directory_wins_over_stale_children() {
    let fx = Fx::with(&["d/a.shp"]);
    let mut g = scanned(&fx);
    let a = id(&fx, "d/a.shp");
    // The directory's Deleted arrives while the view still lists a child (stale): delete wins.
    let rec = fx.record("d");
    let d = g.apply_batch(&fx.tree, &[SourceEvent::Deleted(rec)]);
    assert_eq!(d.removed, [a]);
    assert!(g.is_empty());
}

#[test]
fn event_order_within_a_batch_does_not_matter() {
    let base = ["a.shp", "a.dbf", "r.tif"];
    let mut results = Vec::new();
    for reversed in [false, true] {
        let mut fx = Fx::with(&base);
        let mut g = scanned(&fx);
        let mut evs = vec![
            fx.create("a.prj"),
            fx.rename("r.tif", "s.tif"),
            fx.create("n/x.gpkg"),
            fx.delete("a.dbf"),
        ];
        // n/ is created by the fixture; report its Created last to simulate out-of-order.
        evs.push(SourceEvent::Created(fx.record("n")));
        if reversed {
            evs.reverse();
        }
        let d = apply(&fx, &mut g, &evs);
        results.push((d, fx.describe_all(&g)));
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(results[0].0.added.len(), 1);
    assert_eq!(results[0].0.renamed.len(), 1);
}

/// Counts directory listings, to prove irrelevant churn does no grouping work.
struct Counting<'a> {
    inner: &'a common::Fx,
    listings: Cell<usize>,
}

impl DirectoryView for Counting<'_> {
    fn for_each_child(&self, dir: Frn, visit: &mut dyn FnMut(Entry<'_>)) {
        self.listings.set(self.listings.get() + 1);
        self.inner.tree.for_each_child(dir, visit);
    }

    fn entry(&self, frn: Frn) -> Option<Entry<'_>> {
        self.inner.tree.entry(frn)
    }
}

#[test]
fn irrelevant_churn_lists_no_directory() {
    let mut fx = Fx::with(&["a.shp", "log.txt"]);
    let mut g = scanned(&fx);
    let evs = [
        fx.create("b.txt"),
        fx.modify("log.txt"),
        fx.rename("b.txt", "c.docx"),
        fx.delete("c.docx"),
    ];
    let view = Counting {
        inner: &fx,
        listings: Cell::new(0),
    };
    assert!(g.apply_batch(&view, &evs).is_empty());
    assert_eq!(view.listings.get(), 0);
    let ev = fx.modify("a.shp");
    let view = Counting {
        inner: &fx,
        listings: Cell::new(0),
    };
    assert_eq!(g.apply(&view, &ev).modified.len(), 1);
    assert_eq!(
        view.listings.get(),
        0,
        "a member's content change needs no regrouping"
    );
}

/// Deterministic pseudo-random generator (no external crates).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

#[test]
fn random_batches_match_full_scan() {
    const STEMS: &[&str] = &["a", "A", "b", "c.d"];
    const EXTS: &[&str] = &[
        "shp",
        "SHX",
        "dbf",
        "prj",
        "tif",
        "tfw",
        "jpg",
        "jgw",
        "gpkg",
        "gpkg-wal",
        "txt",
        "shp.xml",
        "tif.aux.xml",
    ];
    const DIRS: &[&str] = &["", "d1", "d1/d2", "x.gdb", "y"];
    for seed in 1..=40u64 {
        let mut rng = Lcg(seed);
        let mut fx = Fx::new();
        let mut g = scanned(&fx);
        let mut mirror: BTreeSet<DatasetId> = BTreeSet::new();
        for _batch in 0..25 {
            let mut evs = Vec::new();
            for _ in 0..1 + rng.next(4) {
                let files: Vec<String> = all_files(&fx);
                let dir = DIRS[rng.next(DIRS.len())];
                let name = if rng.next(10) == 0 {
                    "a00000001.gdbtable".to_string()
                } else {
                    format!(
                        "{}.{}",
                        STEMS[rng.next(STEMS.len())],
                        EXTS[rng.next(EXTS.len())]
                    )
                };
                let path = if dir.is_empty() {
                    name
                } else {
                    format!("{dir}/{name}")
                };
                let exists = files.contains(&path);
                match rng.next(6) {
                    0 | 1 if !exists => {
                        // Directories created on the way are reported too (possibly after).
                        let before = all_dirs(&fx);
                        evs.push(fx.create(&path));
                        for d in all_dirs(&fx).difference(&before) {
                            evs.push(SourceEvent::Created(fx.record(d)));
                        }
                    }
                    2 if !files.is_empty() => {
                        let victim = files[rng.next(files.len())].clone();
                        evs.push(fx.delete(&victim));
                    }
                    3 if !files.is_empty() && !exists => {
                        let from = files[rng.next(files.len())].clone();
                        let before = all_dirs(&fx);
                        evs.push(fx.rename(&from, &path));
                        for d in all_dirs(&fx).difference(&before) {
                            evs.push(SourceEvent::Created(fx.record(d)));
                        }
                    }
                    4 if !files.is_empty() => {
                        let f = files[rng.next(files.len())].clone();
                        evs.push(fx.modify(&f));
                    }
                    5 => {
                        // Rename a directory between a .gdb and a plain name.
                        let dirs: Vec<String> = all_dirs(&fx).into_iter().collect();
                        if let Some(d) = dirs.get(rng.next(dirs.len())).cloned() {
                            let to = match d.strip_suffix(".gdb") {
                                Some(s) => s.to_string(),
                                None => format!("{d}.gdb"),
                            };
                            if !all_dirs(&fx).contains(&to) && !files.contains(&to) {
                                evs.push(fx.rename(&format!("{d}/"), &format!("{to}/")));
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Shuffle: the grouper must not depend on event order.
            for i in (1..evs.len()).rev() {
                evs.swap(i, rng.next(i + 1));
            }
            let d = g.apply_batch(&fx.tree, &evs);
            assert_eq!(
                fx.describe_all(&g),
                fx.describe_all(&fx.scan()),
                "seed {seed}: incremental state diverged after {evs:#?}"
            );
            for r in &d.removed {
                assert!(mirror.remove(r), "seed {seed}: removed unknown {r}");
            }
            for a in &d.added {
                assert!(mirror.insert(*a), "seed {seed}: added twice {a}");
            }
            for x in d.changed.iter().chain(&d.modified).chain(&d.renamed) {
                assert!(mirror.contains(x), "seed {seed}: delta names unknown {x}");
            }
            let now: BTreeSet<DatasetId> = g.datasets().map(|ds| ds.id()).collect();
            assert_eq!(
                mirror, now,
                "seed {seed}: deltas do not add up to the state"
            );
        }
    }
}

fn all_files(fx: &Fx) -> Vec<String> {
    let mut out = Vec::new();
    walk(fx, ROOT, &mut |p, is_dir| {
        if !is_dir {
            out.push(p);
        }
    });
    out.sort();
    out
}

fn all_dirs(fx: &Fx) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    walk(fx, ROOT, &mut |p, is_dir| {
        if is_dir {
            out.insert(p);
        }
    });
    out
}

fn walk(fx: &Fx, dir: Frn, f: &mut dyn FnMut(String, bool)) {
    let mut kids = Vec::new();
    fx.tree
        .for_each_child(dir, &mut |e| kids.push((e.frn, e.is_dir)));
    for (frn, is_dir) in kids {
        f(fx.path_of(frn), is_dir);
        if is_dir {
            walk(fx, frn, f);
        }
    }
}
