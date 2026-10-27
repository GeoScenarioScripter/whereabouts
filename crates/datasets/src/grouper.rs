//! The stateful grouper: datasets per directory, member → dataset index, change handling.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use whereabouts_core::{Dataset, DatasetId, Frn, VolumeId};
use whereabouts_source::SourceEvent;

use crate::rules::{Grouped, group_directory, is_relevant_name};
use crate::view::DirectoryView;

/// What a full scan or a batch of change events did to the datasets.
///
/// Each dataset appears in at most one of `added`, `changed`, `modified`, `renamed` and
/// `removed`. Every list is sorted and free of duplicates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupDelta {
    /// New datasets: extract them.
    pub added: Vec<DatasetId>,
    /// Membership, format or kind changed (a sidecar appeared, left or changed role): replace
    /// the dataset's files and re-extract.
    pub changed: Vec<DatasetId>,
    /// Same membership, but a member's content or attributes changed: re-extract (after the
    /// caller's content-stamp check).
    pub modified: Vec<DatasetId>,
    /// The anchor was renamed or moved and nothing else changed: update the dataset's name and
    /// path, no re-extraction (if the content stamp is unchanged; the caller checks it).
    pub renamed: Vec<DatasetId>,
    /// Datasets that no longer exist: delete them.
    pub removed: Vec<DatasetId>,
    /// Directories that were renamed or moved. The paths of every dataset beneath them changed
    /// even though the datasets did not ([`Grouper::datasets_under`] lists them).
    pub moved_dirs: Vec<Frn>,
}

impl GroupDelta {
    /// The dirty set: datasets that need (re-)extraction (`added`, `changed`, `modified`).
    pub fn dirty(&self) -> impl Iterator<Item = DatasetId> + '_ {
        self.added
            .iter()
            .chain(&self.changed)
            .chain(&self.modified)
            .copied()
    }

    /// `true` if nothing changed.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.changed.is_empty()
            && self.modified.is_empty()
            && self.renamed.is_empty()
            && self.removed.is_empty()
            && self.moved_dirs.is_empty()
    }
}

/// A dataset with the directory that produced it and its anchor's location.
#[derive(Debug, Clone)]
struct Stored {
    dataset: Dataset,
    /// The directory whose grouping produced the dataset (the anchor's parent, or the anchor
    /// itself for folder datasets).
    unit: Frn,
    parent: Frn,
    name: Box<[u8]>,
}

/// Datasets a file belongs to (almost always one).
#[derive(Debug, Clone)]
enum Owners {
    One(Frn),
    Many(Vec<Frn>),
}

impl Owners {
    fn as_slice(&self) -> &[Frn] {
        match self {
            Owners::One(a) => std::slice::from_ref(a),
            Owners::Many(v) => v,
        }
    }
}

/// Groups one volume's files into datasets and keeps them current as the volume changes.
///
/// The grouper never reads file contents: everything comes from names through a
/// [`DirectoryView`]. Datasets are keyed by `DatasetId` = (volume, anchor FRN) (D-27), so a
/// dataset keeps its ID across anchor renames and sidecar changes.
///
/// Usage:
/// 1. After enumerating, call [`Grouper::full_scan`] with the view and the root FRN.
/// 2. For each change batch, apply the events to the view first, then call
///    [`Grouper::apply_batch`]; act on the returned [`GroupDelta`].
/// 3. On `FullRescanRequired`, rebuild the view and call [`Grouper::full_scan`] again: it
///    reconciles with the previous state and reports the differences.
///
/// Change handling is order-independent within a batch and idempotent: a `Created` for a known
/// FRN is an upsert, a `Deleted` for an unknown FRN is harmless, and a file listed under a
/// directory whose own `Created` comes later in the batch is grouped with that directory.
///
/// Limitation (M0 hard-link scope): an FRN has one location. If a view lists the same anchor
/// FRN in two directories, the first directory grouped keeps the dataset.
#[derive(Debug, Clone)]
pub struct Grouper {
    volume: VolumeId,
    /// Anchor FRN → dataset.
    datasets: HashMap<Frn, Stored>,
    /// Directory FRN → anchors of the datasets it produced (only non-empty entries).
    units: HashMap<Frn, Vec<Frn>>,
    /// Member FRN (anchor included) → anchors of the datasets it belongs to.
    members: HashMap<Frn, Owners>,
}

impl Grouper {
    /// An empty grouper for one volume.
    pub fn new(volume: VolumeId) -> Self {
        Grouper {
            volume,
            datasets: HashMap::new(),
            units: HashMap::new(),
            members: HashMap::new(),
        }
    }

    /// The volume whose datasets this grouper holds.
    pub fn volume(&self) -> VolumeId {
        self.volume
    }

    /// Number of datasets.
    pub fn len(&self) -> usize {
        self.datasets.len()
    }

    /// `true` if there are no datasets.
    pub fn is_empty(&self) -> bool {
        self.datasets.is_empty()
    }

    /// The dataset with this ID, if it exists (and is on this grouper's volume).
    pub fn dataset(&self, id: DatasetId) -> Option<&Dataset> {
        if id.volume != self.volume {
            return None;
        }
        self.datasets.get(&id.anchor).map(|s| &s.dataset)
    }

    /// All datasets, in no particular order.
    pub fn datasets(&self) -> impl Iterator<Item = &Dataset> + '_ {
        self.datasets.values().map(|s| &s.dataset)
    }

    /// The datasets `frn` is a member of (as anchor or sidecar).
    pub fn datasets_containing(&self, frn: Frn) -> impl Iterator<Item = DatasetId> + '_ {
        self.owners(frn)
            .iter()
            .map(move |&a| DatasetId::new(self.volume, a))
    }

    /// The datasets grouped from the direct children of `dir` (plus `dir` itself if it is a
    /// folder dataset such as a `.gdb`).
    pub fn datasets_in(&self, dir: Frn) -> impl Iterator<Item = DatasetId> + '_ {
        self.units
            .get(&dir)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(move |&a| DatasetId::new(self.volume, a))
    }

    /// Every dataset in the subtree rooted at `dir` (`dir` included), sorted. Use it to refresh
    /// paths after [`GroupDelta::moved_dirs`]. Walks the view; cycles are guarded.
    pub fn datasets_under<V: DirectoryView + ?Sized>(&self, view: &V, dir: Frn) -> Vec<DatasetId> {
        let mut out: Vec<DatasetId> = walk(view, dir)
            .iter()
            .flat_map(|d| self.datasets_in(*d))
            .collect();
        out.sort_unstable();
        out
    }

    /// Groups the whole tree under `root` from scratch and reconciles with the current state:
    /// new datasets are `added`, vanished ones `removed`, and so on. The first scan reports
    /// every dataset as `added`.
    pub fn full_scan<V: DirectoryView + ?Sized>(&mut self, view: &V, root: Frn) -> GroupDelta {
        let units: BTreeSet<Frn> = walk(view, root).into_iter().collect();
        let dropped = self
            .units
            .keys()
            .filter(|u| !units.contains(u))
            .copied()
            .collect();
        self.regroup(view, units, dropped, &[], BTreeSet::new())
    }

    /// Applies one change event. Equivalent to [`Grouper::apply_batch`] with one event.
    pub fn apply<V: DirectoryView + ?Sized>(
        &mut self,
        view: &V,
        event: &SourceEvent,
    ) -> GroupDelta {
        self.apply_batch(view, std::slice::from_ref(event))
    }

    /// Applies a batch of change events and returns what happened to the datasets.
    ///
    /// Precondition: `view` already reflects every event in `events` (the caller updates its
    /// name index first). The grouper then regroups each affected directory once, so the order
    /// of events within the batch does not matter.
    ///
    /// Event handling:
    /// - files: `Created`/`Deleted`/`Renamed` regroup the old and new parent directories when
    ///   the name is relevant ([`is_relevant_name`]) and the directories of any dataset the
    ///   file belonged to; `Modified` marks the datasets the file belongs to as `modified` (or
    ///   regroups the parent if a relevant file is not known yet: a missed `Created`);
    /// - directories: `Created`/`Renamed` regroup the directory itself (its name decides
    ///   whether it is a `.gdb`, and its children may have arrived first); `Renamed` also
    ///   reports it in `moved_dirs`, as does a `Created` for a directory that already holds
    ///   datasets (an unpaired rename); `Deleted` drops its datasets; `Modified` is ignored.
    pub fn apply_batch<V: DirectoryView + ?Sized>(
        &mut self,
        view: &V,
        events: &[SourceEvent],
    ) -> GroupDelta {
        let mut units = BTreeSet::new();
        let mut dropped = BTreeSet::new();
        let mut moved_dirs = BTreeSet::new();
        let mut touched = Vec::new();
        for event in events {
            let rec = event.record();
            if rec.is_dir() {
                match event {
                    SourceEvent::Deleted(_) => {
                        dropped.insert(rec.frn);
                    }
                    SourceEvent::Renamed { .. } => {
                        units.insert(rec.frn);
                        moved_dirs.insert(rec.frn);
                    }
                    SourceEvent::Created(_) => {
                        units.insert(rec.frn);
                        if self.units.contains_key(&rec.frn) {
                            moved_dirs.insert(rec.frn);
                        }
                    }
                    SourceEvent::Modified(_) => {}
                }
                continue;
            }
            let relevant = is_relevant_name(rec.name.as_wtf8());
            match event {
                SourceEvent::Modified(_) => {
                    if self.members.contains_key(&rec.frn) {
                        touched.push(rec.frn);
                    } else if relevant {
                        units.insert(rec.parent);
                    }
                }
                SourceEvent::Created(_) | SourceEvent::Deleted(_) => {
                    self.owner_units(rec.frn, &mut units);
                    if relevant {
                        units.insert(rec.parent);
                    }
                }
                SourceEvent::Renamed {
                    old_parent,
                    old_name,
                    ..
                } => {
                    self.owner_units(rec.frn, &mut units);
                    if relevant {
                        units.insert(rec.parent);
                    }
                    if is_relevant_name(old_name.as_wtf8()) {
                        units.insert(*old_parent);
                    }
                }
            }
        }
        // Delete wins: a deleted directory is not regrouped even if other events point at it.
        units.retain(|u| !dropped.contains(u));
        self.regroup(view, units, dropped, &touched, moved_dirs)
    }

    fn owners(&self, frn: Frn) -> &[Frn] {
        self.members
            .get(&frn)
            .map(Owners::as_slice)
            .unwrap_or_default()
    }

    /// Adds the directories that produced the datasets `frn` belongs to.
    fn owner_units(&self, frn: Frn, units: &mut BTreeSet<Frn>) {
        for anchor in self.owners(frn) {
            if let Some(s) = self.datasets.get(anchor) {
                units.insert(s.unit);
            }
        }
    }

    /// Regroups `units`, drops `dropped`, diffs old against new and updates the state.
    fn regroup<V: DirectoryView + ?Sized>(
        &mut self,
        view: &V,
        units: BTreeSet<Frn>,
        dropped: BTreeSet<Frn>,
        touched: &[Frn],
        moved_dirs: BTreeSet<Frn>,
    ) -> GroupDelta {
        let mut old: HashMap<Frn, Stored> = HashMap::new();
        for unit in units.iter().chain(&dropped) {
            for anchor in self.units.remove(unit).unwrap_or_default() {
                if let Some(stored) = self.datasets.remove(&anchor) {
                    self.unindex(&stored.dataset);
                    old.insert(anchor, stored);
                }
            }
        }

        let mut delta = GroupDelta::default();
        let mut dirty = HashSet::new();
        for &unit in &units {
            for grouped in group_directory(self.volume, view, unit) {
                let Grouped {
                    dataset,
                    parent,
                    name,
                } = grouped;
                let anchor = dataset.anchor();
                if self.datasets.contains_key(&anchor) {
                    // The same anchor FRN listed in two directories (hard link): first wins.
                    tracing::debug!(%anchor, %unit, "anchor already grouped elsewhere; skipped");
                    continue;
                }
                let id = dataset.id();
                match old.remove(&anchor) {
                    None => {
                        delta.added.push(id);
                        dirty.insert(anchor);
                    }
                    Some(prev) if prev.dataset != dataset => {
                        delta.changed.push(id);
                        dirty.insert(anchor);
                    }
                    Some(prev) if prev.parent != parent || prev.name != name => {
                        delta.renamed.push(id);
                    }
                    Some(_) => {}
                }
                self.insert(Stored {
                    dataset,
                    unit,
                    parent,
                    name,
                });
            }
        }
        delta.removed = old
            .into_keys()
            .map(|a| DatasetId::new(self.volume, a))
            .collect();

        let mut modified = BTreeSet::new();
        for &frn in touched {
            for &anchor in self.owners(frn) {
                if !dirty.contains(&anchor) {
                    modified.insert(anchor);
                }
            }
        }
        delta.renamed.retain(|id| !modified.contains(&id.anchor));
        delta.modified = modified
            .into_iter()
            .map(|a| DatasetId::new(self.volume, a))
            .collect();
        delta.moved_dirs = moved_dirs.into_iter().collect();
        for list in [
            &mut delta.added,
            &mut delta.changed,
            &mut delta.renamed,
            &mut delta.removed,
        ] {
            list.sort_unstable();
        }
        delta
    }

    fn insert(&mut self, stored: Stored) {
        let anchor = stored.dataset.anchor();
        for member in stored.dataset.members() {
            match self.members.get_mut(&member.frn) {
                None => {
                    self.members.insert(member.frn, Owners::One(anchor));
                }
                Some(Owners::One(a)) => {
                    let first = *a;
                    self.members
                        .insert(member.frn, Owners::Many(vec![first, anchor]));
                }
                Some(Owners::Many(v)) => v.push(anchor),
            }
        }
        self.units.entry(stored.unit).or_default().push(anchor);
        self.datasets.insert(anchor, stored);
    }

    fn unindex(&mut self, dataset: &Dataset) {
        let anchor = dataset.anchor();
        for member in dataset.members() {
            let remove = match self.members.get_mut(&member.frn) {
                Some(Owners::One(a)) => *a == anchor,
                Some(Owners::Many(v)) => {
                    v.retain(|a| *a != anchor);
                    if let [only] = v.as_slice() {
                        let only = *only;
                        self.members.insert(member.frn, Owners::One(only));
                    }
                    false
                }
                None => false,
            };
            if remove {
                self.members.remove(&member.frn);
            }
        }
    }
}

/// Directories reachable from `root` (included), breadth first, each once.
fn walk<V: DirectoryView + ?Sized>(view: &V, root: Frn) -> Vec<Frn> {
    let mut seen = HashSet::from([root]);
    let mut order = vec![root];
    let mut queue = VecDeque::from([root]);
    while let Some(dir) = queue.pop_front() {
        view.for_each_child(dir, &mut |e| {
            if e.is_dir && e.frn != dir && seen.insert(e.frn) {
                order.push(e.frn);
                queue.push_back(e.frn);
            }
        });
    }
    order
}
