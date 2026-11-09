//! Durable single-volume pipeline. A failed method requires dropping/reopening the indexer.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use whereabouts_catalog::Catalog;
use whereabouts_core::{
    Dataset, DatasetId, DatasetRecord, ExtractionStatus, FileName, FileRecord, Frn, MemberRole,
    ReadLimits, ResolvedDataset, ResolvedMember, Timestamp, VolumeId,
};
use whereabouts_datasets::{GroupDelta, Grouper, MemoryTree, is_relevant_name};
use whereabouts_nameindex::NameIndex;
use whereabouts_source::{Changes, Checkpoint, Source, SourceIdentity};

/// Pipeline or persistence failure. Reopen the indexer before continuing after an error.
#[derive(Debug, Error)]
pub enum IndexError {
    /// Catalog failure.
    #[error(transparent)]
    Catalog(#[from] whereabouts_catalog::CatalogError),
    /// Source failure.
    #[error(transparent)]
    Source(#[from] whereabouts_source::SourceError),
    /// Name-index capacity or content failure.
    #[error(transparent)]
    Names(#[from] whereabouts_nameindex::IndexError),
    /// Snapshot persistence failure.
    #[error(transparent)]
    Snapshot(#[from] whereabouts_nameindex::SnapshotError),
    /// State-file failure.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Worker pool creation failure.
    #[error(transparent)]
    Workers(#[from] rayon::ThreadPoolBuildError),
    /// No usable journal after a rescan; avoid an infinite rescan loop.
    #[error("source requires another full rescan: {0}")]
    HistoryUnavailable(String),
    /// Source/state was used for another volume.
    #[error("source identity or root changed; reopen the indexer")]
    SourceChanged,
}

/// Locations and bounded worker settings for one indexed volume.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Catalog database path.
    pub database: PathBuf,
    /// Name snapshot path. Must be unique per source, even when sharing a catalog.
    pub snapshot: PathBuf,
    /// File-system root corresponding to `Source::root()` (e.g. `X:\`).
    pub root_path: PathBuf,
    /// Dedicated extraction workers; zero is clamped to one.
    pub workers: usize,
    /// Reader limits, per dataset.
    pub limits: ReadLimits,
    /// Minimum delay between transient retries (new changes bypass the delay).
    pub retry_delay: Duration,
}

impl IndexOptions {
    /// Uses `<database>.names` as the snapshot, up to four workers and a two-second retry delay.
    pub fn new(database: impl Into<PathBuf>, root_path: impl Into<PathBuf>) -> Self {
        let database = database.into();
        let snapshot = suffixed(&database, ".names");
        Self {
            database,
            snapshot,
            root_path: root_path.into(),
            workers: std::thread::available_parallelism().map_or(1, |n| n.get().min(4)),
            limits: ReadLimits::default(),
            retry_delay: Duration::from_secs(2),
        }
    }
}

/// Work performed by one catch-up call.
#[derive(Debug, Default, Clone)]
pub struct IndexProgress {
    /// Whether a full enumeration occurred.
    pub enumerated: bool,
    /// Number of journal events applied.
    pub events: usize,
    /// Number of dataset extraction attempts (including retries).
    pub extracted: usize,
    /// Transient jobs left in the durable retry queue.
    pub retries: usize,
    /// Current grouped dataset count.
    pub datasets: usize,
    /// Current live name count.
    pub names: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct RetryState {
    version: u32,
    checkpoint: Checkpoint,
    identity: SourceIdentity,
    root: Frn,
    volume: VolumeId,
    ids: Vec<DatasetId>,
}

/// Stateful indexer for one `Source`, independent of Windows-specific source implementations.
///
/// `catch_up` drains journal batches until empty. Each batch updates names and grouping before
/// resolving any paths. Jobs run newest-mtime first on a dedicated pool. A write-ahead retry
/// ledger is fsynced before catalog commits; the catalog commits before the name snapshot.
/// Matching snapshots resume without enumeration. Corrupt/mismatched state requests a rescan.
/// Only one writer may use a particular snapshot path at a time.
#[derive(Debug)]
pub struct Indexer {
    options: IndexOptions,
    catalog: Catalog,
    volume: VolumeId,
    identity: SourceIdentity,
    root: Frn,
    names: NameIndex,
    tree: MemoryTree,
    grouper: Grouper,
    checkpoint: Option<Checkpoint>,
    retry: BTreeMap<DatasetId, Instant>,
    pool: rayon::ThreadPool,
}

impl Indexer {
    /// Opens persistence, restoring matching state or preparing a full scan on `catch_up`.
    pub fn open(source: &dyn Source, options: IndexOptions) -> Result<Self, IndexError> {
        let mut catalog = Catalog::open(&options.database)?;
        let identity = source.identity();
        let volume = catalog.write(|tx| tx.register_volume(&identity))?;
        let root = source.root();
        let mut names = NameIndex::new(root);
        let mut checkpoint = None;
        let mut retry = BTreeMap::new();
        // Any disagreement is a conservative rescan, including a power-loss rollback of WAL.
        // A ledger is required even when empty, so a lost retry file can never lose work.
        let restored = (|| {
            let saved = NameIndex::load_snapshot(&options.snapshot).ok()?;
            let cp = saved.checkpoint?;
            if saved.index.root_frn() != root || catalog.checkpoint(volume).ok()?? != cp {
                return None;
            }
            let state = read_retry(&suffixed(&options.snapshot, ".retry")).ok()?;
            if state.version != 1
                || state.checkpoint != cp
                || state.volume != volume
                || state.identity != identity
                || state.root != root
                || state.ids.iter().any(|id| id.volume != volume)
            {
                return None;
            }
            Some((saved.index, cp, state.ids))
        })();
        if let Some((index, cp, ids)) = restored {
            names = index;
            checkpoint = Some(cp);
            retry.extend(ids.into_iter().map(|id| (id, Instant::now())));
        }
        let tree = tree_from_names(&names);
        let mut grouper = Grouper::new(volume);
        grouper.full_scan(&tree, root);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(options.workers.clamp(1, 64))
            .thread_name(|n| format!("whereabouts-extract-{n}"))
            .build()?;
        Ok(Self {
            options,
            catalog,
            volume,
            identity,
            root,
            names,
            tree,
            grouper,
            checkpoint,
            retry,
            pool,
        })
    }

    /// Catalog-assigned source identifier.
    pub fn volume(&self) -> VolumeId {
        self.volume
    }

    /// Current name index (FRNs survive compaction; entry IDs do not).
    pub fn names(&self) -> &NameIndex {
        &self.names
    }

    /// Committed catalog for inspection/querying.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Applies all currently available journal batches and attempts due retries.
    ///
    /// A source with no usable journal is enumerated once, then returns `HistoryUnavailable`
    /// instead of looping. The caller decides when a journal should be created; we never do.
    pub fn catch_up(&mut self, source: &mut dyn Source) -> Result<IndexProgress, IndexError> {
        if source.identity() != self.identity || source.root() != self.root {
            return Err(IndexError::SourceChanged);
        }
        let mut progress = IndexProgress::default();
        if self.checkpoint.is_none() {
            self.rescan(source, &mut progress)?;
        }
        while let Some(cp) = self.checkpoint.as_ref() {
            match source.changes(cp)? {
                Changes::FullRescanRequired(reason) => {
                    if progress.enumerated {
                        return Err(IndexError::HistoryUnavailable(format!("{reason:?}")));
                    }
                    tracing::warn!(?reason, "source requires full rescan");
                    self.rescan(source, &mut progress)?;
                }
                Changes::Batch(batch) => {
                    let empty = batch.events.is_empty();
                    progress.events += batch.events.len();
                    for event in &batch.events {
                        if let Err(error) = self.names.apply(event) {
                            if matches!(error, whereabouts_nameindex::IndexError::CapacityExceeded)
                            {
                                self.names.compact();
                                self.names.apply(event)?;
                            } else {
                                return Err(error.into());
                            }
                        }
                        let record = event.record();
                        if record.is_dir() || is_relevant_name(record.name.as_wtf8()) {
                            self.tree.apply(event);
                        } else {
                            // A relevant file may have been renamed to an irrelevant suffix.
                            self.tree.remove(record.frn);
                        }
                    }
                    let delta = self.grouper.apply_batch(&self.tree, &batch.events);
                    if self.names.needs_compaction() {
                        self.names.compact();
                    }
                    self.apply_delta(delta, batch.checkpoint, &mut progress)?;
                    if empty {
                        break;
                    }
                }
            }
        }
        progress.retries = self.retry.len();
        progress.datasets = self.grouper.len();
        progress.names = self.names.len();
        Ok(progress)
    }

    fn rescan(
        &mut self,
        source: &mut dyn Source,
        progress: &mut IndexProgress,
    ) -> Result<(), IndexError> {
        let mut names = NameIndex::new(self.root);
        let mut tree = MemoryTree::new();
        let mut failure = None;
        let result = source.enumerate(&mut |record: FileRecord| {
            if let Err(error) = names.insert(&record) {
                failure = Some(error);
                return ControlFlow::Break(());
            }
            if record.is_dir() || is_relevant_name(record.name.as_wtf8()) {
                tree.upsert(&record);
            }
            ControlFlow::Continue(())
        });
        if let Some(error) = failure {
            return Err(error.into());
        }
        let checkpoint = result?;
        names.compact();
        self.names = names;
        self.tree = tree;
        self.grouper = Grouper::new(self.volume);
        let mut delta = self.grouper.full_scan(&self.tree, self.root);
        let current: BTreeSet<_> = self.grouper.datasets().map(Dataset::id).collect();
        delta.removed = self
            .catalog
            .dataset_ids(self.volume)?
            .into_iter()
            .filter(|id| !current.contains(id))
            .collect();
        self.retry.retain(|id, _| current.contains(id));
        progress.enumerated = true;
        self.apply_delta(delta, checkpoint, progress)
    }

    fn apply_delta(
        &mut self,
        delta: GroupDelta,
        checkpoint: Checkpoint,
        progress: &mut IndexProgress,
    ) -> Result<(), IndexError> {
        let now = Instant::now();
        if delta.is_empty()
            && self.checkpoint.as_ref() == Some(&checkpoint)
            && self.retry.values().all(|due| *due > now)
        {
            return Ok(());
        }
        self.retry
            .retain(|id, _| self.grouper.dataset(*id).is_some());
        for id in delta.dirty() {
            self.retry.insert(id, now);
        }
        for id in &delta.removed {
            self.retry.remove(id);
        }
        let mut rename_ids: BTreeSet<_> = delta.renamed.into_iter().collect();
        for dir in delta.moved_dirs {
            rename_ids.extend(self.grouper.datasets_under(&self.tree, dir));
        }
        let mut writes = Vec::new();
        for id in rename_ids {
            let Some(ds) = self.grouper.dataset(id) else {
                continue;
            };
            let job = self.job(ds.clone());
            if let Some(mut old) = self.catalog.get_dataset(id)?
                && job.resolved.is_ok()
                && old.mtime == job.record.mtime
                && old.total_size == job.record.total_size
            {
                old.dataset = job.record.dataset;
                old.name = job.record.name;
                old.primary_path = job.record.primary_path;
                writes.push(old);
                continue;
            }
            self.retry.insert(id, now);
        }
        // Write ahead: if killed during extraction/catalog/snapshot, all jobs remain recoverable.
        self.save_retry(&checkpoint)?;
        let mut jobs: Vec<_> = self
            .retry
            .iter()
            .filter(|(_, due)| **due <= now)
            .filter_map(|(id, _)| self.grouper.dataset(*id).cloned())
            .map(|ds| self.job(ds))
            .collect();
        jobs.sort_by(|a, b| {
            b.record
                .mtime
                .cmp(&a.record.mtime)
                .then_with(|| a.record.dataset.id().cmp(&b.record.dataset.id()))
        });
        let limits = self.options.limits;
        let results = Mutex::new(Vec::with_capacity(jobs.len()));
        let queue = Mutex::new(VecDeque::from(jobs));
        // Workers take from the same priority-ordered queue; rayon's work-stealing iterator
        // alone could start old jobs before recent ones at the far end of the vector.
        self.pool.scope(|scope| {
            for _ in 0..self.pool.current_num_threads() {
                let (queue, results) = (&queue, &results);
                scope.spawn(move |_| {
                    loop {
                        let job = queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .pop_front();
                        let Some(job) = job else { break };
                        let result = extract_job(job, limits);
                        results
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(result);
                    }
                });
            }
        });
        let mut results = results
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        results.sort_by_key(|(record, _)| record.dataset.id());
        progress.extracted += results.len();
        for (record, retry) in results {
            let id = record.dataset.id();
            if retry {
                tracing::warn!(?id, status = ?record.status, "extraction deferred");
                self.retry
                    .insert(id, Instant::now() + self.options.retry_delay);
                // D-36: never downgrade the last good record on a transient read failure.
                if self.catalog.get_dataset(id)?.is_some() {
                    continue;
                }
            } else {
                self.retry.remove(&id);
            }
            writes.push(record);
        }
        self.catalog.write(|tx| {
            for id in &delta.removed {
                tx.delete_dataset(*id)?;
            }
            for record in &writes {
                tx.upsert_dataset(record)?;
            }
            tx.set_checkpoint(self.volume, &checkpoint)
        })?;
        self.names
            .save_snapshot(&self.options.snapshot, Some(&checkpoint))?;
        // Shrink only after BOTH durable stores commit. An older ledger merely over-retries.
        self.save_retry(&checkpoint)?;
        self.checkpoint = Some(checkpoint);
        Ok(())
    }

    fn job(&self, dataset: Dataset) -> Job {
        let resolved = self.resolve(&dataset);
        let anchor = resolved.as_ref().ok().map(|ds| ds.anchor.as_path());
        let mut total_size = Some(0u64);
        let mut mtime = None;
        if let Ok(ds) = &resolved {
            for path in std::iter::once(&ds.anchor).chain(ds.sidecars.iter().map(|m| &m.path)) {
                match fs::metadata(path) {
                    Ok(md) => {
                        total_size = total_size.and_then(|n| n.checked_add(md.len()));
                        if let Ok(t) = md.modified() {
                            mtime = mtime.max(Timestamp::from_system_time(t));
                        }
                    }
                    Err(_) => {
                        total_size = None;
                    }
                }
            }
        }
        let record = DatasetRecord {
            name: anchor
                .and_then(Path::file_stem)
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("FRN {}", dataset.anchor().0)),
            primary_path: anchor
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            dataset,
            total_size,
            mtime,
            status: ExtractionStatus::Pending,
            extracted_at: None,
            layers: Vec::new(),
        };
        Job { record, resolved }
    }

    fn resolve(&self, dataset: &Dataset) -> Result<ResolvedDataset, String> {
        let path = |frn| {
            let id = self
                .names
                .lookup(frn)
                .ok_or_else(|| format!("unknown FRN {frn:?}"))?;
            let bytes = self.names.path(id).map_err(|e| e.to_string())?;
            let mut path = self.options.root_path.clone();
            for component in bytes.split(|b| *b == b'\\').filter(|c| !c.is_empty()) {
                let name = FileName::from_wtf8(component).map_err(|e| e.to_string())?;
                // Encode lossless UTF-16 through OsString on Windows without platform APIs.
                #[cfg(windows)]
                path.push(name.to_os_string());
                #[cfg(not(windows))]
                path.push(
                    name.as_str()
                        .ok_or_else(|| "non-Unicode NTFS path on this platform".to_owned())?,
                );
            }
            Ok::<_, String>(path)
        };
        Ok(ResolvedDataset {
            format: dataset.format(),
            anchor: path(dataset.anchor())?,
            sidecars: dataset
                .members()
                .iter()
                .filter(|m| m.role != MemberRole::Main)
                .map(|m| {
                    Ok(ResolvedMember {
                        role: m.role,
                        path: path(m.frn)?,
                    })
                })
                .collect::<Result<_, String>>()?,
        })
    }

    fn save_retry(&self, checkpoint: &Checkpoint) -> Result<(), IndexError> {
        let state = RetryState {
            version: 1,
            checkpoint: checkpoint.clone(),
            identity: self.identity.clone(),
            root: self.root,
            volume: self.volume,
            ids: self.retry.keys().copied().collect(),
        };
        let bytes = serde_json::to_vec(&state).map_err(io::Error::other)?;
        if bytes.len() > MAX_RETRY_BYTES as usize {
            return Err(io::Error::other("retry queue exceeds 64 MiB state limit").into());
        }
        let path = suffixed(&self.options.snapshot, ".retry");
        let tmp = suffixed(&path, ".tmp");
        let mut file = File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(tmp, path)?;
        Ok(())
    }
}

#[derive(Debug)]
struct Job {
    record: DatasetRecord,
    resolved: Result<ResolvedDataset, String>,
}

fn extract_job(job: Job, limits: ReadLimits) -> (DatasetRecord, bool) {
    let mut record = job.record;
    let retry = match job.resolved {
        Ok(resolved) => {
            let output = crate::extract(&resolved, &limits);
            record.status = output.status;
            record.layers = output.layers;
            output.retry
        }
        Err(reason) => {
            record.status = ExtractionStatus::Error(reason);
            true
        }
    };
    record.extracted_at = Timestamp::from_system_time(SystemTime::now());
    (record, retry)
}

const MAX_RETRY_BYTES: u64 = 64 << 20;

fn read_retry(path: &Path) -> io::Result<RetryState> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_RETRY_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RETRY_BYTES {
        return Err(io::Error::other("retry state too large"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    value.into()
}

fn tree_from_names(names: &NameIndex) -> MemoryTree {
    let mut tree = MemoryTree::new();
    // entries() deliberately omits the seeded name-index root.
    tree.upsert(&FileRecord {
        frn: names.root_frn(),
        parent: names.root_frn(),
        name: FileName::from(""),
        attrs: whereabouts_core::FileAttributes(whereabouts_core::FileAttributes::DIRECTORY),
    });
    for id in names.entries() {
        let (Some(frn), Some(name), Some(attrs)) = (names.frn(id), names.name(id), names.attrs(id))
        else {
            continue;
        };
        let parent = if frn == names.root_frn() {
            frn
        } else if let Some(parent) = names.parent(id).and_then(|p| names.frn(p)) {
            parent
        } else {
            continue;
        };
        if (attrs.contains(whereabouts_core::FileAttributes::DIRECTORY) || is_relevant_name(name))
            && let Ok(name) = FileName::from_wtf8(name)
        {
            tree.upsert(&FileRecord {
                frn,
                parent,
                name,
                attrs,
            });
        }
    }
    tree
}
