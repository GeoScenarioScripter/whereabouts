#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use whereabouts_core::{DatasetId, FileAttributes, FileName, FileRecord, Frn};
use whereabouts_indexer::{IndexOptions, Indexer};
use whereabouts_source::{
    ChangeBatch, Changes, Checkpoint, RecordSink, Source, SourceError, SourceEvent, SourceIdentity,
};

#[derive(Debug)]
struct FakeSource {
    records: BTreeMap<Frn, FileRecord>,
    batches: VecDeque<Vec<SourceEvent>>,
    cursor: u8,
    scans: usize,
}

fn record(id: u64, parent: u64, name: &str, dir: bool) -> FileRecord {
    FileRecord {
        frn: Frn(id),
        parent: Frn(parent),
        name: FileName::from(name),
        attrs: FileAttributes(if dir { FileAttributes::DIRECTORY } else { 0 }),
    }
}

impl FakeSource {
    fn new(root: &Path) -> Self {
        let mut records = BTreeMap::new();
        records.insert(Frn(1), record(1, 1, "", true));
        let mut id = 10;
        for dir in ["a", "b"] {
            fs::create_dir(root.join(dir)).unwrap();
            records.insert(Frn(id), record(id, 1, dir, true));
            for entry in fs::read_dir(corpus()).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_str().unwrap();
                fs::copy(&path, root.join(dir).join(name)).unwrap();
                let file = id + 1 + records.values().filter(|r| r.parent == Frn(id)).count() as u64;
                records.insert(Frn(file), record(file, id, name, false));
            }
            id += 10;
        }
        Self {
            records,
            batches: VecDeque::new(),
            cursor: 0,
            scans: 0,
        }
    }
    fn named(&self, parent: u64, name: &str) -> FileRecord {
        self.records
            .values()
            .find(|r| r.parent == Frn(parent) && r.name.as_str() == Some(name))
            .unwrap()
            .clone()
    }
    fn push(&mut self, events: Vec<SourceEvent>) {
        for e in &events {
            let r = e.record();
            if matches!(e, SourceEvent::Deleted(_)) {
                self.records.remove(&r.frn);
            } else {
                self.records.insert(r.frn, r.clone());
            }
        }
        self.batches.push_back(events);
    }
}

impl Source for FakeSource {
    fn identity(&self) -> SourceIdentity {
        SourceIdentity {
            kind: "test".into(),
            key: "pipeline".into(),
            label: "test".into(),
        }
    }
    fn root(&self) -> Frn {
        Frn(1)
    }
    fn enumerate(&mut self, sink: &mut dyn RecordSink) -> Result<Checkpoint, SourceError> {
        self.scans += 1;
        for r in self.records.values() {
            if sink.accept(r.clone()).is_break() {
                return Err(SourceError::Cancelled);
            }
        }
        Ok(Checkpoint::new(1, vec![self.cursor]).unwrap())
    }
    fn changes(&mut self, _: &Checkpoint) -> Result<Changes, SourceError> {
        let events = self.batches.pop_front().unwrap_or_default();
        if !events.is_empty() {
            self.cursor += 1;
        }
        Ok(Changes::Batch(ChangeBatch {
            events,
            checkpoint: Checkpoint::new(1, vec![self.cursor]).unwrap(),
        }))
    }
}

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/data/shapefile/shp_utm33n_points")
}

fn ids(indexer: &Indexer) -> Vec<DatasetId> {
    indexer.catalog().dataset_ids(indexer.volume()).unwrap()
}

#[test]
fn full_index_restart_drain_and_member_edits_are_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let mut source = FakeSource::new(temp.path());
    let options = IndexOptions::new(temp.path().join("catalog.db"), temp.path());
    let mut indexer = Indexer::open(&source, options.clone()).unwrap();
    let report = indexer.catch_up(&mut source).unwrap();
    assert!(report.enumerated);
    assert_eq!(report.extracted, 2);
    assert_eq!(report.datasets, 2);
    let ids = ids(&indexer);
    let before: Vec<_> = ids
        .iter()
        .map(|id| indexer.catalog().get_dataset(*id).unwrap().unwrap())
        .collect();
    for row in &before {
        assert_eq!(row.layers[0].metadata.feature_count, Some(25));
        assert_eq!(row.layers[0].crs.as_ref().unwrap().epsg(), Some(32633));
        assert!(row.layers[0].wgs84.is_some());
        row.validate().unwrap();
    }
    drop(indexer);
    // Two capped batches while stopped: edits must drain through the second batch.
    for parent in [10, 20] {
        let r = source.named(parent, "points.prj");
        let path = temp
            .path()
            .join(if parent == 10 { "a" } else { "b" })
            .join("points.prj");
        let mut text = fs::read_to_string(&path).unwrap();
        text.push('\n');
        fs::write(path, text).unwrap();
        source.push(vec![SourceEvent::Modified(r)]);
    }
    let mut indexer = Indexer::open(&source, options).unwrap();
    let report = indexer.catch_up(&mut source).unwrap();
    assert!(!report.enumerated);
    assert_eq!(source.scans, 1);
    assert_eq!(report.events, 2);
    assert_eq!(report.extracted, 2);
    let unchanged = indexer.catalog().get_dataset(ids[1]).unwrap().unwrap();
    // Two sidecar events in one batch coalesce into a single dataset job.
    let r = source.named(10, "points.prj");
    source.push(vec![
        SourceEvent::Modified(r.clone()),
        SourceEvent::Modified(r),
    ]);
    let report = indexer.catch_up(&mut source).unwrap();
    assert_eq!(report.extracted, 1);
    assert_eq!(
        indexer.catalog().get_dataset(ids[1]).unwrap().unwrap(),
        unchanged
    );
}

#[test]
fn transient_failure_retains_record_and_durable_retry_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut source = FakeSource::new(temp.path());
    let mut options = IndexOptions::new(temp.path().join("catalog.db"), temp.path());
    options.retry_delay = Duration::from_secs(3600);
    let mut indexer = Indexer::open(&source, options.clone()).unwrap();
    indexer.catch_up(&mut source).unwrap();
    let id = DatasetId::new(indexer.volume(), source.named(10, "points.shp").frn);
    let before = indexer.catalog().get_dataset(id).unwrap().unwrap();
    let anchor = temp.path().join("a/points.shp");
    let bytes = fs::read(&anchor).unwrap();
    fs::remove_file(&anchor).unwrap();
    source.push(vec![SourceEvent::Modified(source.named(10, "points.shp"))]);
    let report = indexer.catch_up(&mut source).unwrap();
    assert_eq!(report.retries, 1);
    assert_eq!(indexer.catalog().get_dataset(id).unwrap().unwrap(), before);
    drop(indexer);
    fs::write(anchor, bytes).unwrap();
    let mut indexer = Indexer::open(&source, options).unwrap();
    let report = indexer.catch_up(&mut source).unwrap();
    assert!(!report.enumerated);
    assert_eq!(report.extracted, 1);
    assert_eq!(report.retries, 0);
    assert_eq!(
        indexer.catalog().get_dataset(id).unwrap().unwrap().layers,
        before.layers
    );
}

#[test]
fn directory_move_updates_paths_without_extraction_and_corrupt_state_rescans() {
    let temp = tempfile::tempdir().unwrap();
    let mut source = FakeSource::new(temp.path());
    let options = IndexOptions::new(temp.path().join("catalog.db"), temp.path());
    let mut indexer = Indexer::open(&source, options.clone()).unwrap();
    indexer.catch_up(&mut source).unwrap();
    fs::rename(temp.path().join("a"), temp.path().join("moved")).unwrap();
    let old = source.records[&Frn(10)].clone();
    let mut moved = old.clone();
    moved.name = FileName::from("moved");
    source.push(vec![SourceEvent::Renamed {
        record: moved,
        old_parent: old.parent,
        old_name: old.name,
    }]);
    let report = indexer.catch_up(&mut source).unwrap();
    assert_eq!(report.extracted, 0);
    let id = DatasetId::new(indexer.volume(), source.named(10, "points.shp").frn);
    assert!(
        indexer
            .catalog()
            .get_dataset(id)
            .unwrap()
            .unwrap()
            .primary_path
            .contains("moved")
    );
    drop(indexer);
    fs::write(&options.snapshot, b"corrupt").unwrap();
    let mut indexer = Indexer::open(&source, options).unwrap();
    assert!(indexer.catch_up(&mut source).unwrap().enumerated);
    assert_eq!(source.scans, 2);
}
