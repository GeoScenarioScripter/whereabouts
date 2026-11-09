//! Stage01/4.2: destructive fault injection is confined to disposable VHDs/temp directories.
//! No production failpoints: errors are injected through state files and a SQLite trigger.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use whereabouts_catalog::Catalog;
use whereabouts_core::{DatasetRecord, ExtractionStatus, FileRecord, Frn};
use whereabouts_indexer::{IndexOptions, Indexer};
use whereabouts_nameindex::NameIndex;
use whereabouts_ntfs::{NtfsCheckpoint, NtfsVolumeSource, testvol::TestVolume};
use whereabouts_source::{Changes, Checkpoint, RecordSink, Source, SourceError, SourceIdentity};

fn suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    value.into()
}

fn copy_case(volume: &TestVolume, directory: &str, case: &str) {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/data");
    fs::create_dir_all(volume.path(directory)).unwrap();
    for entry in fs::read_dir(corpus.join(case)).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), volume.path(directory).join(entry.file_name())).unwrap();
    }
}

fn fixture() -> (TestVolume, tempfile::TempDir, IndexOptions) {
    let volume = TestVolume::create(512).unwrap();
    volume.create_journal(8 << 20, 1 << 20).unwrap();
    copy_case(&volume, "shape", "shapefile/shp_utm33n_points");
    copy_case(&volume, "package", "geopackage/gpkg_multi_layer");
    copy_case(&volume, "raster", "geotiff/tif_northup_utm");
    // State is off-volume so journal assertions distinguish dataset reads from state writes.
    let temp = tempfile::tempdir().unwrap();
    let options = IndexOptions::new(temp.path().join("catalog.db"), volume.root());
    (volume, temp, options)
}

fn rows(indexer: &Indexer) -> Vec<DatasetRecord> {
    indexer
        .catalog()
        .dataset_ids(indexer.volume())
        .unwrap()
        .into_iter()
        .map(|id| indexer.catalog().get_dataset(id).unwrap().unwrap())
        .collect()
}

fn consistent(indexer: &Indexer, options: &IndexOptions, expected: usize) {
    let rows = rows(indexer);
    assert_eq!(rows.len(), expected);
    for row in rows {
        row.validate().unwrap();
        assert_eq!(row.status, ExtractionStatus::Ok, "{}", row.primary_path);
        assert!(!row.layers.is_empty());
        assert!(row.layers.iter().all(|layer| layer.wgs84.is_some()));
        assert!(indexer.names().lookup(row.dataset.anchor()).is_some());
    }
    let cp = indexer
        .catalog()
        .checkpoint(indexer.volume())
        .unwrap()
        .unwrap();
    assert_eq!(
        NameIndex::load_snapshot(&options.snapshot)
            .unwrap()
            .checkpoint,
        Some(cp.clone())
    );
    let ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(suffix(&options.snapshot, ".retry")).unwrap()).unwrap();
    assert_eq!(ledger["checkpoint"], serde_json::to_value(cp).unwrap());
    assert_eq!(ledger["ids"], serde_json::json!([]));
}

fn run(volume: &TestVolume, options: &IndexOptions, enumerated: bool, expected: usize) {
    let mut source = NtfsVolumeSource::open(&volume.volume()).unwrap();
    let mut indexer = Indexer::open(&source, options.clone()).unwrap();
    let progress = indexer.catch_up(&mut source).unwrap();
    assert_eq!(progress.enumerated, enumerated, "{progress:?}");
    assert_eq!(progress.retries, 0);
    consistent(&indexer, options, expected);
}

fn edit_prj(volume: &TestVolume) {
    let path = volume.path("shape/points.prj");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
}

#[test]
#[ignore = "requires elevation; creates an isolated VHD and churns its tiny journal"]
fn journal_wrap_rescans_and_removes_stale_catalog_rows() {
    let (volume, _temp, options) = fixture();
    volume.create_journal(1 << 20, 64 << 10).unwrap();
    run(&volume, &options, true, 3);
    let saved = NameIndex::load_snapshot(&options.snapshot)
        .unwrap()
        .checkpoint
        .unwrap();
    let position = NtfsCheckpoint::from_checkpoint(&saved)
        .unwrap()
        .journal
        .unwrap();
    assert!(position.next_usn > 0);
    // These changes must be discovered by enumeration, not just successful journal replay.
    fs::remove_dir_all(volume.path("raster")).unwrap();
    copy_case(&volume, "new", "geotiff/tif_northup_utm");
    let source = NtfsVolumeSource::open(&volume.volume()).unwrap();
    fs::create_dir(volume.path("churn")).unwrap();
    let started = Instant::now();
    let mut files = 0;
    loop {
        for _ in 0..50 {
            let path = volume.path(format!("churn/{}{files}", "x".repeat(200)));
            fs::write(&path, b"x").unwrap();
            fs::remove_file(path).unwrap();
            files += 1;
        }
        if source.volume().query_journal().unwrap().unwrap().first_usn > position.next_usn {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "journal did not wrap after {files} files"
        );
    }
    eprintln!(
        "[resilience] journal wrapped after {files} files in {:?}",
        started.elapsed()
    );
    drop(source);
    run(&volume, &options, true, 3);
    let catalog = Catalog::open_read_only(&options.database).unwrap();
    let ids = catalog
        .dataset_ids(catalog.volumes().unwrap()[0].id)
        .unwrap();
    let paths: Vec<_> = ids
        .into_iter()
        .map(|id| catalog.get_dataset(id).unwrap().unwrap().primary_path)
        .collect();
    assert!(paths.iter().any(|p| p.contains("\\new\\")));
    assert!(paths.iter().all(|p| !p.contains("\\raster\\")));
    run(&volume, &options, false, 3);
}

#[test]
#[ignore = "requires elevation; deletes/recreates only an isolated VHD journal"]
fn journal_recreation_and_missing_history_recover_after_reopen() {
    let (volume, _temp, options) = fixture();
    run(&volume, &options, true, 3);
    let cp = NameIndex::load_snapshot(&options.snapshot)
        .unwrap()
        .checkpoint
        .unwrap();
    let old_id = NtfsCheckpoint::from_checkpoint(&cp)
        .unwrap()
        .journal
        .unwrap()
        .journal_id;
    volume.delete_journal().unwrap();
    let mut source = NtfsVolumeSource::open(&volume.volume()).unwrap();
    let mut indexer = Indexer::open(&source, options.clone()).unwrap();
    assert!(matches!(
        indexer.catch_up(&mut source),
        Err(whereabouts_indexer::IndexError::HistoryUnavailable(_))
    ));
    drop(indexer); // Failed pipeline methods require reopening.
    drop(source);
    edit_prj(&volume);
    volume.create_journal(8 << 20, 1 << 20).unwrap();
    let source = NtfsVolumeSource::open(&volume.volume()).unwrap();
    assert_ne!(
        source.volume().query_journal().unwrap().unwrap().journal_id,
        old_id
    );
    drop(source);
    run(&volume, &options, true, 3);
    run(&volume, &options, false, 3);
}

#[test]
#[ignore = "requires elevation; injects inconsistent persistence only into disposable state"]
fn corrupt_missing_and_mismatched_restart_state_rescans() {
    let (volume, _temp, options) = fixture();
    run(&volume, &options, true, 3);
    let ledger_path = suffix(&options.snapshot, ".retry");
    let stale_ledger = fs::read(&ledger_path).unwrap();
    let stale_snapshot = fs::read(&options.snapshot).unwrap();
    edit_prj(&volume);
    run(&volume, &options, false, 3);
    assert_ne!(fs::read(&ledger_path).unwrap(), stale_ledger);
    for fault in [
        "corrupt snapshot",
        "stale snapshot",
        "catalog rollback",
        "missing ledger",
        "corrupt ledger",
        "stale ledger",
        "foreign ledger",
        "wrong version",
        "interrupted ledger temp",
    ] {
        match fault {
            "corrupt snapshot" => {
                let mut bytes = fs::read(&options.snapshot).unwrap();
                let end = bytes.len() - 1;
                bytes[end] ^= 0xff;
                fs::write(&options.snapshot, bytes).unwrap();
                assert!(NameIndex::load_snapshot(&options.snapshot).is_err());
            }
            "stale snapshot" => fs::write(&options.snapshot, &stale_snapshot).unwrap(),
            "catalog rollback" => {
                let mut catalog = Catalog::open(&options.database).unwrap();
                let id = catalog.volumes().unwrap()[0].id;
                let old = NameIndex::read_snapshot(stale_snapshot.as_slice())
                    .unwrap()
                    .checkpoint
                    .unwrap();
                catalog.write(|tx| tx.set_checkpoint(id, &old)).unwrap();
            }
            "missing ledger" => fs::remove_file(&ledger_path).unwrap(),
            "corrupt ledger" => fs::write(&ledger_path, b"{broken").unwrap(),
            "stale ledger" => fs::write(&ledger_path, &stale_ledger).unwrap(),
            "foreign ledger" | "wrong version" => {
                let mut state: serde_json::Value =
                    serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
                if fault == "foreign ledger" {
                    state["identity"]["key"] = "another volume".into();
                } else {
                    state["version"] = 99.into();
                }
                fs::write(&ledger_path, serde_json::to_vec(&state).unwrap()).unwrap();
            }
            "interrupted ledger temp" => {
                fs::remove_file(&ledger_path).unwrap();
                fs::write(suffix(&ledger_path, ".tmp"), b"partial write").unwrap();
            }
            _ => unreachable!(),
        }
        run(&volume, &options, true, 3);
        run(&volume, &options, false, 3);
        eprintln!("[resilience] recovered {fault}");
    }
    // Crash after snapshot replacement but before shrinking the ledger: matching checkpoint
    // with an extra retry is safe to replay, without enumeration or lost metadata.
    let source = NtfsVolumeSource::open(&volume.volume()).unwrap();
    let indexer = Indexer::open(&source, options.clone()).unwrap();
    let before = rows(&indexer);
    drop(indexer);
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    state["ids"] = serde_json::json!([before[0].dataset.id()]);
    fs::write(&ledger_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let mut source = source;
    let mut indexer = Indexer::open(&source, options.clone()).unwrap();
    let progress = indexer.catch_up(&mut source).unwrap();
    assert!(!progress.enumerated);
    assert_eq!(progress.extracted, 1);
    consistent(&indexer, &options, 3);
    for (old, new) in before.iter().zip(rows(&indexer)) {
        assert_eq!(old.layers, new.layers);
    }
}

#[test]
#[ignore = "requires elevation; injects ledger/catalog/snapshot failures in disposable state"]
fn persistence_failures_reopen_without_losing_work() {
    let (volume, _temp, options) = fixture();
    run(&volume, &options, true, 3);
    for fault in ["ledger sync", "catalog commit", "snapshot replace"] {
        let mut source = NtfsVolumeSource::open(&volume.volume()).unwrap();
        let mut indexer = Indexer::open(&source, options.clone()).unwrap();
        let before = rows(&indexer);
        let cp = indexer.catalog().checkpoint(indexer.volume()).unwrap();
        let snapshot = fs::read(&options.snapshot).unwrap();
        let ledger_path = suffix(&options.snapshot, ".retry");
        let ledger = fs::read(&ledger_path).unwrap();
        edit_prj(&volume);
        let block = match fault {
            "ledger sync" => Some(suffix(&ledger_path, ".tmp")),
            "snapshot replace" => Some(suffix(&options.snapshot, ".tmp")),
            _ => None,
        };
        if let Some(path) = &block {
            fs::create_dir(path).unwrap();
        }
        if fault == "catalog commit" {
            // Fire after dataset upserts, at the checkpoint write, to verify transaction rollback.
            rusqlite::Connection::open(&options.database).unwrap().execute_batch(
                "CREATE TRIGGER fail_checkpoint BEFORE UPDATE OF checkpoint ON volumes BEGIN SELECT RAISE(ABORT, 'resilience injection'); END;"
            ).unwrap();
        }
        let error = indexer.catch_up(&mut source).unwrap_err();
        assert_eq!(fs::read(&options.snapshot).unwrap(), snapshot);
        if fault == "snapshot replace" {
            assert_ne!(indexer.catalog().checkpoint(indexer.volume()).unwrap(), cp);
        } else {
            assert_eq!(indexer.catalog().checkpoint(indexer.volume()).unwrap(), cp);
            assert_eq!(rows(&indexer), before);
        }
        if fault == "ledger sync" {
            assert_eq!(fs::read(&ledger_path).unwrap(), ledger);
        } else {
            assert_ne!(fs::read(&ledger_path).unwrap(), ledger);
        }
        drop(indexer);
        drop(source);
        if let Some(path) = block {
            fs::remove_dir(path).unwrap();
        }
        if fault == "catalog commit" {
            rusqlite::Connection::open(&options.database)
                .unwrap()
                .execute_batch("DROP TRIGGER fail_checkpoint")
                .unwrap();
        }
        run(&volume, &options, fault != "ledger sync", 3);
        run(&volume, &options, false, 3);
        eprintln!("[resilience] recovered {fault}: {error}");
    }
}

#[test]
#[ignore = "requires elevation; holds a real GeoPackage transaction only on a VHD copy"]
fn active_geopackage_transaction_keeps_metadata_and_retries_after_restart() {
    let (volume, _temp, mut options) = fixture();
    options.retry_delay = Duration::from_secs(3600);
    run(&volume, &options, true, 3);
    let mut source = NtfsVolumeSource::open(&volume.volume()).unwrap();
    let mut indexer = Indexer::open(&source, options.clone()).unwrap();
    let before = rows(&indexer)
        .into_iter()
        .find(|r| r.primary_path.ends_with("multi.gpkg"))
        .unwrap();
    let db = volume.path("package/multi.gpkg");
    let writer = rusqlite::Connection::open(&db).unwrap();
    writer.execute_batch("PRAGMA journal_mode=DELETE; BEGIN IMMEDIATE; UPDATE gpkg_contents SET identifier = identifier || ' changed';").unwrap();
    assert!(fs::metadata(suffix(&db, "-journal")).unwrap().len() > 0);
    // A same-byte header write supplies an anchor event even while SQLite buffers dirty pages.
    // It changes no SQLite data and is confined to this disposable copy.
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().write(true).open(&db).unwrap();
        file.write_all(b"SQLite format 3\0").unwrap();
        file.sync_all().unwrap();
    }
    let progress = indexer.catch_up(&mut source).unwrap();
    assert_eq!(progress.retries, 1, "{progress:?}");
    assert_eq!(
        indexer
            .catalog()
            .get_dataset(before.dataset.id())
            .unwrap()
            .unwrap(),
        before
    );
    drop(indexer);
    writer.execute_batch("ROLLBACK").unwrap();
    drop(writer);
    let mut indexer = Indexer::open(&source, options.clone()).unwrap();
    let progress = indexer.catch_up(&mut source).unwrap();
    assert!(!progress.enumerated);
    assert_eq!(progress.retries, 0);
    assert!(progress.extracted >= 1);
    let after = indexer
        .catalog()
        .get_dataset(before.dataset.id())
        .unwrap()
        .unwrap();
    assert_eq!(after.layers, before.layers);
    assert_eq!(after.status, before.status);
    consistent(&indexer, &options, 3);
}

#[test]
#[ignore = "requires elevation; verifies reads against an isolated VHD journal"]
fn indexing_reads_do_not_emit_events_for_dataset_members() {
    let (volume, _temp, options) = fixture();
    let mut observer = NtfsVolumeSource::open(&volume.volume()).unwrap();
    let checkpoint = observer
        .enumerate(&mut |_| ControlFlow::Continue(()))
        .unwrap();
    let mut position = NtfsCheckpoint::from_checkpoint(&checkpoint)
        .unwrap()
        .journal
        .unwrap();
    run(&volume, &options, true, 3);
    let source = NtfsVolumeSource::open(&volume.volume()).unwrap();
    let indexer = Indexer::open(&source, options.clone()).unwrap();
    let members: BTreeSet<_> = rows(&indexer)
        .iter()
        .flat_map(|row| row.dataset.members().iter().map(|m| m.frn))
        .collect();
    assert!(members.len() >= 6);
    // Read again through extraction (independent of the name-index-only idle restart).
    drop(indexer);
    fs::remove_file(&options.snapshot).unwrap();
    run(&volume, &options, true, 3);
    let mut buffer = vec![0_u64; 8192];
    for round in 0..100 {
        let reply = observer
            .volume()
            .read_journal(position.next_usn, position.journal_id, &mut buffer)
            .unwrap();
        assert!(reply.len() >= 8);
        for record in whereabouts_ntfs::RecordIter::new(&reply[8..], 8) {
            let record = record.unwrap();
            assert!(
                !members.contains(&Frn(record.frn)),
                "read emitted raw USN record: {record:?}"
            );
        }
        if reply.len() == 8 {
            return;
        }
        let next = i64::from_le_bytes(reply[..8].try_into().unwrap());
        assert!(next > position.next_usn);
        position.next_usn = next;
        assert!(round < 99, "journal did not drain");
    }
}

// The parent owns the VHD and kills only its own child. The child cannot allocate a VHD.
struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Debug)]
struct PausedSource {
    source: NtfsVolumeSource,
    marker: PathBuf,
}
impl Source for PausedSource {
    fn identity(&self) -> SourceIdentity {
        self.source.identity()
    }
    fn root(&self) -> Frn {
        self.source.root()
    }
    fn enumerate(&mut self, sink: &mut dyn RecordSink) -> Result<Checkpoint, SourceError> {
        let marker = &self.marker;
        self.source.enumerate(&mut |record: FileRecord| {
            let pause = record.name.as_str() == Some("points.shp");
            let result = sink.accept(record);
            if pause {
                fs::write(marker, b"enumeration in progress").unwrap();
                // A rendezvous, not a timing guess: parent kills after observing this marker.
                loop {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            result
        })
    }
    fn changes(&mut self, cp: &Checkpoint) -> Result<Changes, SourceError> {
        self.source.changes(cp)
    }
}

#[test]
#[ignore = "child-only worker for killed_process_mid_index_converges; no work without parent env"]
fn crash_child() {
    let Some(volume) = std::env::var_os("WB_RESILIENCE_VOLUME") else {
        return;
    };
    let db = PathBuf::from(std::env::var_os("WB_RESILIENCE_DB").unwrap());
    let marker = PathBuf::from(std::env::var_os("WB_RESILIENCE_MARKER").unwrap());
    let volume = volume.to_str().unwrap();
    if std::env::var_os("WB_RESILIENCE_AFTER_LEDGER").is_some() {
        let mut source = NtfsVolumeSource::open(volume).unwrap();
        let options = IndexOptions::new(db, format!("{volume}\\"));
        let mut indexer = Indexer::open(&source, options).unwrap();
        fs::write(&marker, b"opened").unwrap();
        let started = Instant::now();
        while !marker.with_extension("go").exists() {
            assert!(started.elapsed() < Duration::from_secs(60));
            std::thread::sleep(Duration::from_millis(10));
        }
        indexer.catch_up(&mut source).unwrap();
        panic!("worker completed despite parent holding the catalog write lock");
    }
    let mut source = PausedSource {
        source: NtfsVolumeSource::open(volume).unwrap(),
        marker,
    };
    let options = IndexOptions::new(db, format!("{volume}\\"));
    let mut indexer = Indexer::open(&source, options).unwrap();
    indexer.catch_up(&mut source).unwrap();
    panic!("worker completed without pausing");
}

#[test]
#[ignore = "requires elevation; kills its own child after durable ledger write on an isolated VHD"]
fn killed_after_ledger_sync_before_catalog_commit_converges() {
    let (volume, temp, options) = fixture();
    run(&volume, &options, true, 3);
    let old_snapshot = fs::read(&options.snapshot).unwrap();
    let ledger_path = suffix(&options.snapshot, ".retry");
    let old_ledger = fs::read(&ledger_path).unwrap();
    edit_prj(&volume);
    let marker = temp.path().join("ledger-worker-ready");
    let mut child = KillOnDrop(
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "crash_child", "--nocapture"])
            .env("WB_RESILIENCE_VOLUME", volume.volume())
            .env("WB_RESILIENCE_DB", &options.database)
            .env("WB_RESILIENCE_MARKER", &marker)
            .env("WB_RESILIENCE_AFTER_LEDGER", "1")
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    while !marker.exists() {
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(started.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(10));
    }
    // The child opened successfully; now block only its next catalog transaction, after
    // write-ahead ledger persistence. No production timing hooks or arbitrary kill delay.
    let blocker = rusqlite::Connection::open(&options.database).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    fs::write(marker.with_extension("go"), b"continue").unwrap();
    let started = Instant::now();
    loop {
        if let Ok(bytes) = fs::read(&ledger_path)
            && bytes != old_ledger
            && let Ok(state) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && !state["ids"].as_array().unwrap().is_empty()
        {
            break;
        }
        assert!(child.0.try_wait().unwrap().is_none());
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "child did not sync retry ledger before catalog busy timeout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    blocker.execute_batch("ROLLBACK").unwrap();
    drop(blocker);
    assert_eq!(fs::read(&options.snapshot).unwrap(), old_snapshot);
    let catalog = Catalog::open_read_only(&options.database).unwrap();
    let id = catalog.volumes().unwrap()[0].id;
    assert_eq!(
        catalog.checkpoint(id).unwrap(),
        NameIndex::read_snapshot(old_snapshot.as_slice())
            .unwrap()
            .checkpoint
    );
    drop(catalog);
    run(&volume, &options, true, 3);
    run(&volume, &options, false, 3);
}

#[test]
#[ignore = "requires elevation; kills its own child indexing an isolated VHD"]
fn killed_process_mid_index_converges() {
    let (volume, temp, options) = fixture();
    for existing in [false, true] {
        if existing {
            run(&volume, &options, false, 3);
            // Force rescan with a previously committed catalog; the killed rescan must not lose it.
            fs::remove_file(&options.snapshot).unwrap();
            edit_prj(&volume);
        }
        let marker = temp.path().join("child-ready");
        let mut child = KillOnDrop(
            Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "crash_child", "--nocapture"])
                .env("WB_RESILIENCE_VOLUME", volume.volume())
                .env("WB_RESILIENCE_DB", &options.database)
                .env("WB_RESILIENCE_MARKER", &marker)
                .spawn()
                .unwrap(),
        );
        let started = Instant::now();
        while !marker.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "worker exited before rendezvous"
            );
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "worker never reached enumeration"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        child.0.kill().unwrap();
        assert!(!child.0.wait().unwrap().success());
        fs::remove_file(marker).unwrap();
        run(&volume, &options, true, 3);
        run(&volume, &options, false, 3);
        eprintln!("[resilience] killed mid-enumeration; converged (existing catalog={existing})");
    }
}
