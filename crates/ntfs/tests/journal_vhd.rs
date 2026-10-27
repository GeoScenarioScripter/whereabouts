//! `NtfsVolumeSource` against a real NTFS volume and USN journal (Windows, elevated).
//!
//! Every test here **writes** (files, journal create/delete), so each one runs on its own
//! throw-away VHD volume ([`TestVolume`]), never on a real disk. They need an elevated shell
//! (`diskpart`, raw volume handle) and are `#[ignore]`; CI runs them on the elevated Windows
//! runner with `cargo test -p whereabouts-ntfs -- --ignored`. The tests share one lock so only
//! one VHD exists at a time.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::io::Write as _;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use whereabouts_core::{FileName, FileRecord, Frn};
use whereabouts_ntfs::testvol::TestVolume;
use whereabouts_ntfs::{Error, NtfsCheckpoint, NtfsVolumeSource, SourceOptions, record};
use whereabouts_source::{ChangeBatch, Changes, Checkpoint, RescanReason, Source, SourceEvent};

static ONE_VHD_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Writes a line straight to stderr, so it shows in the CI log even for passing tests.
fn report(line: &str) {
    let _ = writeln!(std::io::stderr(), "[ntfs-journal] {line}");
}

/// Field order is drop order: the source's volume handle closes before the VHD is detached.
struct Fixture {
    source: NtfsVolumeSource,
    vol: TestVolume,
    _lock: MutexGuard<'static, ()>,
}

fn fixture(journal_max: u64, journal_delta: u64) -> Fixture {
    fixture_sized(64, journal_max, journal_delta, SourceOptions::default())
}

fn fixture_with(journal_max: u64, journal_delta: u64, options: SourceOptions) -> Fixture {
    fixture_sized(64, journal_max, journal_delta, options)
}

/// A fixture on an expandable VHD of `size_mib`. NTFS lets the journal grow far past its
/// requested maximum before trimming (about 40 MiB seen), so tests that churn until the journal
/// wraps need a volume with room for that.
fn fixture_sized(
    size_mib: u32,
    journal_max: u64,
    journal_delta: u64,
    options: SourceOptions,
) -> Fixture {
    let lock = ONE_VHD_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let started = Instant::now();
    let vol = TestVolume::create(size_mib).unwrap();
    vol.create_journal(journal_max, journal_delta).unwrap();
    let source = NtfsVolumeSource::open_with(&vol.volume(), options).unwrap();
    report(&format!(
        "vhd {} ready in {} ms ({})",
        vol.volume(),
        started.elapsed().as_millis(),
        vol.vhd_path().display()
    ));
    Fixture {
        source,
        vol,
        _lock: lock,
    }
}

fn enumerate(source: &mut NtfsVolumeSource) -> (Vec<FileRecord>, Checkpoint) {
    let mut records = Vec::new();
    let cp = source.enumerate(&mut records).unwrap();
    (records, cp)
}

fn batch(source: &mut NtfsVolumeSource, since: &Checkpoint) -> ChangeBatch {
    match source.changes(since).unwrap() {
        Changes::Batch(b) => b,
        other => panic!("expected a batch, got {other:?}"),
    }
}

fn rescan_reason(source: &mut NtfsVolumeSource, since: &Checkpoint) -> RescanReason {
    match source.changes(since).unwrap() {
        Changes::FullRescanRequired(reason) => reason,
        Changes::Batch(b) => panic!("expected a rescan, got {} events", b.events.len()),
    }
}

fn name(s: &str) -> FileName {
    FileName::from(s)
}

/// The events of `batch` that concern `frn`.
fn events_of(batch: &ChangeBatch, frn: Frn) -> Vec<&SourceEvent> {
    batch
        .events
        .iter()
        .filter(|e| e.record().frn == frn)
        .collect()
}

/// The single event about the entry called `n`, which must be the first event about it; any
/// further events about the same FRN may only be `Modified` (e.g. time-stamp updates).
fn only_event_named<'b>(batch: &'b ChangeBatch, n: &str) -> &'b SourceEvent {
    let first = batch
        .events
        .iter()
        .find(|e| e.record().name == name(n))
        .unwrap_or_else(|| panic!("no event for {n:?} in {:#?}", batch.events));
    let rest = events_of(batch, first.record().frn);
    assert!(
        rest[1..]
            .iter()
            .all(|e| matches!(e, SourceEvent::Modified(_))),
        "{n:?}: {rest:#?}"
    );
    first
}

fn log_batch(label: &str, batch: &ChangeBatch, source: &NtfsVolumeSource) {
    let stats = source.last_change_stats();
    report(&format!(
        "{label}: events={} records={} reads={} bytes={}",
        batch.events.len(),
        stats.records,
        stats.reads,
        stats.bytes
    ));
    for e in &batch.events {
        report(&format!("  {e:?}"));
    }
}

#[test]
#[ignore = "needs an elevated shell: creates and mounts a VHD test volume"]
fn identity_root_and_enumeration() {
    let mut f = fixture(8 << 20, 1 << 20);
    let identity = f.source.identity();
    assert_eq!(identity.kind, "ntfs");
    assert!(identity.key.starts_with(r"\\?\Volume{"), "{identity:?}");
    assert_eq!(identity.label, f.vol.volume());

    std::fs::create_dir(f.vol.path("wb")).unwrap();
    std::fs::write(f.vol.path(r"wb\a.shp"), b"x").unwrap();
    let (records, _) = enumerate(&mut f.source);
    let root = f.source.root();
    assert_eq!(root.record_number(), record::ROOT_RECORD_NUMBER);
    let roots: Vec<_> = records.iter().filter(|r| r.frn == root).collect();
    assert_eq!(roots.len(), 1, "root reported exactly once");
    assert_eq!(roots[0].parent, root);
    assert!(roots[0].is_dir());
    let dir = records.iter().find(|r| r.name == name("wb")).unwrap();
    assert!(dir.is_dir());
    assert_eq!(dir.parent, root);
    let file = records.iter().find(|r| r.name == name("a.shp")).unwrap();
    assert_eq!(file.parent, dir.frn);
    assert!(!file.is_dir());
    let stats = f.source.last_enumerate_stats();
    assert_eq!(stats.skipped, 0);
    assert_eq!(stats.records, records.len() as u64);
    report(&format!(
        "enumerated {} records on the fresh VHD",
        records.len()
    ));
}

#[test]
#[ignore = "needs an elevated shell: creates and mounts a VHD test volume"]
fn create_modify_rename_move_delete_produce_events() {
    let mut f = fixture(8 << 20, 1 << 20);
    let (_, cp) = enumerate(&mut f.source);
    let root = f.source.root();

    std::fs::create_dir(f.vol.path("wb")).unwrap();
    let b = batch(&mut f.source, &cp);
    log_batch("mkdir", &b, &f.source);
    let SourceEvent::Created(dir) = only_event_named(&b, "wb") else {
        panic!("{:#?}", b.events)
    };
    assert!(dir.is_dir());
    assert_eq!(dir.parent, root);
    let dir = dir.frn;
    let cp = b.checkpoint;

    // Create.
    std::fs::write(f.vol.path(r"wb\a.shp"), b"one").unwrap();
    let b = batch(&mut f.source, &cp);
    log_batch("create", &b, &f.source);
    let SourceEvent::Created(file) = only_event_named(&b, "a.shp") else {
        panic!("{:#?}", b.events)
    };
    assert_eq!(file.parent, dir);
    assert!(!file.is_dir());
    let file = file.frn;
    let cp = b.checkpoint;

    // Modify.
    let mut handle = std::fs::OpenOptions::new()
        .append(true)
        .open(f.vol.path(r"wb\a.shp"))
        .unwrap();
    handle.write_all(b" two").unwrap();
    drop(handle);
    let b = batch(&mut f.source, &cp);
    log_batch("modify", &b, &f.source);
    let ev = events_of(&b, file);
    assert!(!ev.is_empty(), "{:#?}", b.events);
    assert!(
        ev.iter()
            .all(|e| matches!(e, SourceEvent::Modified(r) if r.name == name("a.shp"))),
        "{ev:#?}"
    );
    let cp = b.checkpoint;

    // Rename in place.
    std::fs::rename(f.vol.path(r"wb\a.shp"), f.vol.path(r"wb\b.shp")).unwrap();
    let b = batch(&mut f.source, &cp);
    log_batch("rename", &b, &f.source);
    let SourceEvent::Renamed {
        record,
        old_parent,
        old_name,
    } = only_event_named(&b, "b.shp")
    else {
        panic!("{:#?}", b.events)
    };
    assert_eq!(record.frn, file);
    assert_eq!(record.parent, dir);
    assert_eq!((*old_parent, old_name), (dir, &name("a.shp")));
    let cp = b.checkpoint;

    // Move into a new sub-directory (two batches: mkdir, then move).
    std::fs::create_dir(f.vol.path(r"wb\sub")).unwrap();
    let b = batch(&mut f.source, &cp);
    let SourceEvent::Created(sub) = only_event_named(&b, "sub") else {
        panic!("{:#?}", b.events)
    };
    assert_eq!(sub.parent, dir);
    let sub = sub.frn;
    let cp = b.checkpoint;
    std::fs::rename(f.vol.path(r"wb\b.shp"), f.vol.path(r"wb\sub\b.shp")).unwrap();
    let b = batch(&mut f.source, &cp);
    log_batch("move", &b, &f.source);
    let SourceEvent::Renamed {
        record,
        old_parent,
        old_name,
    } = only_event_named(&b, "b.shp")
    else {
        panic!("{:#?}", b.events)
    };
    assert_eq!((record.frn, record.parent), (file, sub));
    assert_eq!((*old_parent, old_name), (dir, &name("b.shp")));
    let cp = b.checkpoint;

    // Delete.
    std::fs::remove_file(f.vol.path(r"wb\sub\b.shp")).unwrap();
    let b = batch(&mut f.source, &cp);
    log_batch("delete", &b, &f.source);
    let ev = events_of(&b, file);
    assert!(
        matches!(ev[..], [SourceEvent::Deleted(r)] if r.name == name("b.shp") && r.parent == sub),
        "{ev:#?}"
    );
    let cp = b.checkpoint;

    // Nothing more happened to our file, and the checkpoint stays usable. Not asserted empty:
    // Windows may still write to a freshly mounted volume (System Volume Information) on its
    // own schedule; the batch is logged so CI shows what an idle VHD produces.
    let b = batch(&mut f.source, &cp);
    log_batch("idle", &b, &f.source);
    assert!(
        b.events.iter().all(|e| e.record().frn != file),
        "{:#?}",
        b.events
    );
}

#[test]
#[ignore = "needs an elevated shell: creates and mounts a VHD test volume"]
fn truncated_batches_carry_a_split_rename() {
    // 200-character names make every record 464 bytes, so a 1 KiB read buffer returns exactly
    // two records per reply; a 1-byte batch cap ends each batch after the first reply that
    // produces an event.
    let options = SourceOptions {
        journal_buffer_size: 1 << 10,
        max_batch_bytes: 1,
        ..SourceOptions::default()
    };
    let mut f = fixture_with(8 << 20, 1 << 20, options);
    let long = |c: char| format!("{c}{}", "x".repeat(199));
    let (d, a, b) = (long('d'), long('a'), long('b'));
    std::fs::create_dir(f.vol.path("t")).unwrap();
    std::fs::write(f.vol.path(format!(r"t\{d}")), b"d").unwrap();
    std::fs::write(f.vol.path(format!(r"t\{a}")), b"a").unwrap();
    let (records, cp) = enumerate(&mut f.source);
    let dir = records.iter().find(|r| r.name == name("t")).unwrap().frn;
    let a_frn = records.iter().find(|r| r.name == name(&a)).unwrap().frn;

    // One delete record, then the rename's old-name, new-name and close records:
    // replies [delete d, old name a], [new name b, close b].
    std::fs::remove_file(f.vol.path(format!(r"t\{d}"))).unwrap();
    std::fs::rename(f.vol.path(format!(r"t\{a}")), f.vol.path(format!(r"t\{b}"))).unwrap();

    let mut cp = cp;
    let mut all = Vec::new();
    let (mut truncated, mut carried) = (0, 0);
    for round in 0.. {
        assert!(round < 50, "never drained: {all:#?}");
        let batch = batch(&mut f.source, &cp);
        let stats = f.source.last_change_stats();
        let pending = NtfsCheckpoint::from_checkpoint(&batch.checkpoint)
            .unwrap()
            .pending_renames;
        log_batch(&format!("truncating round {round}"), &batch, &f.source);
        report(&format!(
            "  truncated={} pending={pending:?}",
            stats.truncated
        ));
        if stats.truncated {
            truncated += 1;
            assert!(
                !batch.events.is_empty(),
                "a truncated batch must not be empty"
            );
        }
        if let Some(p) = pending.iter().find(|p| p.frn == a_frn) {
            carried += 1;
            assert_eq!((p.old_parent, &p.old_name), (dir, &name(&a)));
        }
        let done = batch.events.is_empty() && !stats.truncated;
        all.extend(batch.events);
        cp = batch.checkpoint;
        if done {
            break;
        }
    }
    assert!(truncated > 0, "the cap never truncated a batch");
    assert!(
        carried > 0,
        "the rename was never carried across batches: {all:#?}"
    );
    let ours: Vec<_> = all.iter().filter(|e| e.record().parent == dir).collect();
    assert!(
        ours.iter()
            .any(|e| matches!(e, SourceEvent::Deleted(r) if r.name == name(&d))),
        "{ours:#?}"
    );
    let renamed: Vec<_> = ours.iter().filter(|e| e.record().frn == a_frn).collect();
    assert!(
        matches!(
            renamed[..],
            [SourceEvent::Renamed { record, old_parent, old_name }]
                if record.name == name(&b) && *old_parent == dir && *old_name == name(&a)
        ),
        "{renamed:#?}"
    );
}

#[test]
#[ignore = "needs an elevated shell: creates and mounts a VHD test volume"]
fn one_batch_is_coalesced_per_file() {
    let mut f = fixture(8 << 20, 1 << 20);
    std::fs::create_dir(f.vol.path("wb")).unwrap();
    let (_, cp) = enumerate(&mut f.source);

    // Written under a temporary name, then renamed: one Created under the final name.
    std::fs::write(f.vol.path(r"wb\~tmp.tif"), vec![7_u8; 4096]).unwrap();
    std::fs::rename(f.vol.path(r"wb\~tmp.tif"), f.vol.path(r"wb\ortho.tif")).unwrap();
    // Created and deleted within the batch: Deleted.
    std::fs::write(f.vol.path(r"wb\gone.txt"), b"x").unwrap();
    std::fs::remove_file(f.vol.path(r"wb\gone.txt")).unwrap();
    // Written several times: one Modified.
    std::fs::write(f.vol.path(r"wb\many.prj"), b"1").unwrap();

    let b = batch(&mut f.source, &cp);
    log_batch("coalesced", &b, &f.source);
    let SourceEvent::Created(ortho) = only_event_named(&b, "ortho.tif") else {
        panic!("{:#?}", b.events)
    };
    assert!(
        b.events.iter().all(|e| e.record().name != name("~tmp.tif")),
        "{:#?}",
        b.events
    );
    assert_eq!(events_of(&b, ortho.frn).len(), 1);
    let gone: Vec<_> = b
        .events
        .iter()
        .filter(|e| e.record().name == name("gone.txt"))
        .collect();
    assert!(matches!(gone[..], [SourceEvent::Deleted(_)]), "{gone:#?}");
    let cp = b.checkpoint;

    for i in 0..5 {
        std::fs::write(f.vol.path(r"wb\many.prj"), format!("{i}")).unwrap();
    }
    let b = batch(&mut f.source, &cp);
    log_batch("rewritten", &b, &f.source);
    let many: Vec<_> = b
        .events
        .iter()
        .filter(|e| e.record().name == name("many.prj"))
        .collect();
    assert!(matches!(many[..], [SourceEvent::Modified(_)]), "{many:#?}");
}

#[test]
#[ignore = "needs an elevated shell: creates and mounts a VHD test volume"]
fn checkpoint_older_than_first_usn_requires_rescan() {
    // A small journal, so churn wraps it quickly.
    let mut f = fixture_sized(512, 1 << 20, 64 << 10, SourceOptions::default());
    // Journal a few records first: on a fresh journal the checkpoint USN would be 0, and the
    // kernel treats StartUsn = 0 as "from the first record" instead of failing (seen in CI).
    std::fs::write(f.vol.path("seed.txt"), b"x").unwrap();
    let (_, cp) = enumerate(&mut f.source);
    let pos = NtfsCheckpoint::from_checkpoint(&cp)
        .unwrap()
        .journal
        .unwrap();
    assert!(pos.next_usn > 0, "{pos:?}");

    std::fs::create_dir(f.vol.path("churn")).unwrap();
    let long = "x".repeat(200);
    let started = Instant::now();
    let mut files = 0_u32;
    let first_usn = loop {
        for _ in 0..50 {
            let p = f.vol.path(format!(r"churn\{long}{files}"));
            std::fs::write(&p, b"1").unwrap();
            std::fs::remove_file(&p).unwrap();
            files += 1;
        }
        let j = f.source.volume().query_journal().unwrap().unwrap();
        if j.first_usn > pos.next_usn {
            break j.first_usn;
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "journal never wrapped: first_usn={} next_usn={} after {files} files",
            j.first_usn,
            j.next_usn
        );
    };
    report(&format!(
        "journal wrapped after {files} files in {} ms: checkpoint usn={} first_usn={first_usn}",
        started.elapsed().as_millis(),
        pos.next_usn
    ));

    // The kernel itself refuses the purged USN with ERROR_JOURNAL_ENTRY_DELETED ...
    let mut buf = vec![0_u64; 8192];
    let err = f
        .source
        .volume()
        .read_journal(pos.next_usn, pos.journal_id, &mut buf)
        .unwrap_err();
    assert!(matches!(err, Error::JournalEntryDeleted { .. }), "{err:?}");
    // ... and the source asks for a rescan instead.
    assert_eq!(
        rescan_reason(&mut f.source, &cp),
        RescanReason::JournalWrapped
    );

    // After the rescan, the new checkpoint works.
    let (_, cp) = enumerate(&mut f.source);
    let _ = batch(&mut f.source, &cp);
}

#[test]
#[ignore = "needs an elevated shell: creates and mounts a VHD test volume"]
fn deleted_and_recreated_journal_requires_rescan() {
    let mut f = fixture(8 << 20, 1 << 20);
    let (_, cp) = enumerate(&mut f.source);
    let old_id = f
        .source
        .volume()
        .query_journal()
        .unwrap()
        .unwrap()
        .journal_id;

    f.vol.delete_journal().unwrap();
    assert!(f.source.volume().query_journal().unwrap().is_none());
    assert_eq!(
        rescan_reason(&mut f.source, &cp),
        RescanReason::JournalMissing
    );
    // Enumeration works without a journal; its checkpoint records that there was none.
    let (records, cp_none) = enumerate(&mut f.source);
    report(&format!(
        "enumeration without a journal: {} records",
        records.len()
    ));
    assert!(records.len() > 1, "more than the synthesised root");
    assert_eq!(f.source.last_enumerate_stats().skipped, 0);
    assert_eq!(
        NtfsCheckpoint::from_checkpoint(&cp_none).unwrap().journal,
        None
    );
    assert_eq!(
        rescan_reason(&mut f.source, &cp_none),
        RescanReason::JournalMissing
    );

    f.vol.create_journal(8 << 20, 1 << 20).unwrap();
    let new_id = f
        .source
        .volume()
        .query_journal()
        .unwrap()
        .unwrap()
        .journal_id;
    report(&format!("journal id {old_id:#x} -> {new_id:#x}"));
    assert_ne!(old_id, new_id);
    assert_eq!(
        rescan_reason(&mut f.source, &cp),
        RescanReason::JournalRecreated
    );
    assert_eq!(
        rescan_reason(&mut f.source, &cp_none),
        RescanReason::JournalRecreated
    );

    let (_, cp) = enumerate(&mut f.source);
    std::fs::write(f.vol.path("after.txt"), b"x").unwrap();
    let b = batch(&mut f.source, &cp);
    assert!(matches!(
        only_event_named(&b, "after.txt"),
        SourceEvent::Created(_)
    ));
}

#[test]
#[ignore = "needs an elevated shell: creates and mounts a VHD test volume"]
fn foreign_or_garbage_checkpoints_require_rescan() {
    let mut f = fixture(8 << 20, 1 << 20);
    let (_, cp) = enumerate(&mut f.source);
    let mut other = NtfsCheckpoint::from_checkpoint(&cp).unwrap();
    other.volume_serial ^= 1;
    assert_eq!(
        rescan_reason(&mut f.source, &other.to_checkpoint().unwrap()),
        RescanReason::CheckpointMismatch
    );
    for garbage in [
        Checkpoint::new(1, vec![1, 2, 3]).unwrap(),
        Checkpoint::new(99, cp.data().to_vec()).unwrap(),
    ] {
        assert_eq!(
            rescan_reason(&mut f.source, &garbage),
            RescanReason::CheckpointMismatch
        );
    }
    // A checkpoint from the future of this journal.
    let mut ahead = NtfsCheckpoint::from_checkpoint(&cp).unwrap();
    if let Some(j) = ahead.journal.as_mut() {
        j.next_usn += 1 << 40;
    }
    assert_eq!(
        rescan_reason(&mut f.source, &ahead.to_checkpoint().unwrap()),
        RescanReason::CheckpointMismatch
    );
}

#[test]
#[ignore = "needs an elevated shell: creates and mounts a VHD test volume"]
fn raw_mft_enumeration_matches_usn_and_replays_a_margin() {
    use whereabouts_ntfs::EnumerationMethod;

    // 100 bytes of replay margin is not a record boundary: the kernel must accept it.
    let raw_options = SourceOptions {
        enumeration: EnumerationMethod::RawMft,
        raw_replay_margin: 100,
        ..SourceOptions::default()
    };
    let mut f = fixture_with(1 << 20, 64 << 10, raw_options);
    std::fs::create_dir(f.vol.path("gis")).unwrap();
    for i in 0..30 {
        std::fs::write(f.vol.path(format!(r"gis\layer{i}.shp")), b"x").unwrap();
    }
    std::fs::write(f.vol.path("long file name with spaces.tif"), b"x").unwrap();

    let (raw, cp) = enumerate(&mut f.source);
    let stats = f.source.last_enumerate_stats();
    assert_eq!(stats.skipped, 0);
    report(&format!("raw enumerate: {} records, {stats:?}", raw.len()));

    // The same volume through the USN API.
    let mut usn_source = NtfsVolumeSource::open(&f.vol.volume()).unwrap();
    let (usn, _) = enumerate(&mut usn_source);
    let key = |r: &FileRecord| (r.frn, r.parent, r.name.as_wtf8().to_vec(), r.attrs.0);
    let mut a: Vec<_> = raw.iter().map(key).collect();
    let mut b: Vec<_> = usn.iter().map(key).collect();
    a.sort();
    b.sort();
    assert_eq!(a, b);
    assert!(
        raw.iter()
            .any(|r| r.name.to_string_lossy() == "layer29.shp")
    );

    // The raw checkpoint replays history from before the scan: the changes call must succeed
    // (not an error, not a rescan) and report the files created just before the scan.
    let replayed = batch(&mut f.source, &cp);
    report(&format!("replayed {} events", replayed.events.len()));
    assert!(
        replayed
            .events
            .iter()
            .any(|e| e.record().name.to_string_lossy() == "layer29.shp"
                || e.record().name.to_string_lossy() == "gis"),
        "{:#?}",
        replayed.events
    );
}
