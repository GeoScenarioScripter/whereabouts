//! The index itself: entry columns, string pools, FRN map, mutation, path reconstruction.

use std::collections::HashMap;

use whereabouts_core::{FileAttributes, FileRecord, Frn};
use whereabouts_source::SourceEvent;

use crate::error::{IndexError, PathError};
use crate::fold;
use crate::frnmap::FrnMap;

/// Sentinel for "no entry" (unresolved parent, stale segment, empty FRN-map slot).
pub(crate) const NONE: u32 = u32::MAX;
/// Sentinel for "no extension" in the `ext` column.
pub(crate) const NO_EXT: u16 = u16::MAX;
/// Path separator used in reconstructed paths and in path-mode matching.
pub const SEPARATOR: u8 = b'\\';
/// Longest accepted name in WTF-8 bytes. NTFS allows 255 UTF-16 units (≤ 765 bytes); the margin
/// covers other sources. Also bounds the folded name below `u16::MAX`.
pub const MAX_NAME_BYTES: usize = 4096;
/// Deepest parent chain that path reconstruction follows. NTFS paths are at most 32,767 UTF-16
/// units, so no real chain has more than 16,384 components; a longer one is reported as a cycle.
pub const MAX_DEPTH: usize = 16_384;

/// Index of one entry in a [`NameIndex`].
///
/// Valid until the next [`NameIndex::compact`] (inserts and removals never move entries).
/// [`EntryId::ROOT`] is the volume root, seeded at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryId(pub(crate) u32);

impl EntryId {
    /// The volume root.
    pub const ROOT: EntryId = EntryId(0);

    /// The entry's position in the index's columns.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Where an entry's parent link leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Link {
    Entry(u32),
    Broken,
}

/// Compact in-memory filename index for one volume (see the crate docs for the layout).
#[derive(Debug, Clone)]
pub struct NameIndex {
    pub(crate) root_frn: Frn,
    // Entry columns (struct of arrays), all the same length.
    pub(crate) frn: Vec<Frn>,
    /// Parent entry, or `NONE` while the parent FRN is unknown (see `unresolved`).
    pub(crate) parent: Vec<u32>,
    pub(crate) name_off: Vec<u32>,
    pub(crate) name_len: Vec<u16>,
    /// Current segment of the folded pool.
    pub(crate) seg: Vec<u32>,
    /// Offset of the extension (after the last `.`) within the folded name, or `NO_EXT`.
    pub(crate) ext: Vec<u16>,
    pub(crate) attrs: Vec<u32>,
    pub(crate) alive: Vec<bool>,
    /// Original names, WTF-8, back to back.
    pub(crate) names: Vec<u8>,
    /// Folded names, each followed by a NUL separator, in segment order.
    pub(crate) folded: Vec<u8>,
    /// Start offset of each folded segment (ascending).
    pub(crate) seg_start: Vec<u32>,
    /// Entry owning each segment, or `NONE` once the entry is renamed or removed.
    pub(crate) seg_owner: Vec<u32>,
    map: FrnMap,
    /// Entries whose parent FRN is not in the index yet → that FRN.
    unresolved: HashMap<u32, Frn>,
    /// Parent FRN → entries waiting for it (may hold stale ids; `unresolved` is authoritative).
    pending: HashMap<Frn, Vec<u32>>,
    live: usize,
    dead_name_bytes: usize,
    dead_fold_bytes: usize,
    scratch: Vec<u8>,
}

/// Size and memory figures (see [`NameIndex::stats`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStats {
    /// Live entries, excluding the root.
    pub entries: usize,
    /// Entry slots including the root and tombstones.
    pub slots: usize,
    /// Tombstoned slots (reclaimed by [`NameIndex::compact`]).
    pub dead_slots: usize,
    /// Entries whose parent FRN is not in the index (orphans waiting for their parent).
    pub unresolved: usize,
    /// Bytes in the original-name pool, including dead names.
    pub name_bytes: usize,
    /// Bytes in the folded pool, including separators and dead names.
    pub folded_bytes: usize,
    /// Heap bytes held by the index (allocated capacity of every buffer; hash maps estimated).
    pub heap_bytes: usize,
}

impl NameIndex {
    /// An empty index for a volume whose root directory has FRN `root`.
    ///
    /// The root is seeded as [`EntryId::ROOT`] with an empty name: NTFS enumeration never returns
    /// it (D-31), so take `root` from `Source::root()` / `Volume::root_frn()`.
    #[must_use]
    pub fn new(root: Frn) -> Self {
        Self::with_capacity(root, 0, 0)
    }

    /// Like [`NameIndex::new`], pre-allocating for `entries` entries and `name_bytes` bytes of
    /// names (both are hints; the index grows as needed).
    #[must_use]
    pub fn with_capacity(root: Frn, entries: usize, name_bytes: usize) -> Self {
        let n = entries.saturating_add(1);
        let mut index = NameIndex {
            root_frn: root,
            frn: Vec::with_capacity(n),
            parent: Vec::with_capacity(n),
            name_off: Vec::with_capacity(n),
            name_len: Vec::with_capacity(n),
            seg: Vec::with_capacity(n),
            ext: Vec::with_capacity(n),
            attrs: Vec::with_capacity(n),
            alive: Vec::with_capacity(n),
            names: Vec::with_capacity(name_bytes),
            folded: Vec::with_capacity(name_bytes.saturating_add(n)),
            seg_start: Vec::with_capacity(n),
            seg_owner: Vec::with_capacity(n),
            map: FrnMap::default(),
            unresolved: HashMap::new(),
            pending: HashMap::new(),
            live: 0,
            dead_name_bytes: 0,
            dead_fold_bytes: 0,
            scratch: Vec::new(),
        };
        index.frn.push(root);
        index.parent.push(0);
        index.name_off.push(0);
        index.name_len.push(0);
        index.seg.push(0);
        index.ext.push(NO_EXT);
        index.attrs.push(FileAttributes::DIRECTORY);
        index.alive.push(true);
        index.seg_start.push(0);
        index.seg_owner.push(0);
        index.folded.push(0);
        index.map.insert(root, 0, &index.frn, 1);
        index
    }

    /// FRN of the volume root.
    #[must_use]
    pub fn root_frn(&self) -> Frn {
        self.root_frn
    }

    /// Number of live entries, excluding the root.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    /// `true` if the index holds nothing but the root.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Inserts or updates (upserts) one record. Re-inserting a known FRN updates its name, parent
    /// and attributes in place, so replayed events are idempotent. A record for the root FRN only
    /// updates the root's attributes.
    ///
    /// # Errors
    /// [`IndexError::NameTooLong`], [`IndexError::CapacityExceeded`].
    pub fn insert(&mut self, record: &FileRecord) -> Result<EntryId, IndexError> {
        // `FileName` is validated WTF-8 by construction.
        self.upsert(
            record.frn,
            record.parent,
            record.name.as_wtf8(),
            record.attrs,
        )
    }

    /// [`NameIndex::insert`] from borrowed parts, with the name as WTF-8 (validated).
    ///
    /// # Errors
    /// [`IndexError::InvalidName`] for ill-formed WTF-8, plus those of [`NameIndex::insert`].
    pub fn insert_wtf8(
        &mut self,
        frn: Frn,
        parent: Frn,
        name: &[u8],
        attrs: FileAttributes,
    ) -> Result<EntryId, IndexError> {
        if !fold::is_wtf8(name) {
            return Err(IndexError::InvalidName);
        }
        self.upsert(frn, parent, name, attrs)
    }

    /// [`NameIndex::insert`] from borrowed parts, with the name as raw little-endian UTF-16 — the
    /// zero-copy path from `whereabouts_ntfs::UsnRecord::name_utf16le()` (`Enumerator::next_batch`).
    ///
    /// # Errors
    /// [`IndexError::InvalidName`] for an odd byte length, plus those of [`NameIndex::insert`].
    pub fn insert_utf16le(
        &mut self,
        frn: Frn,
        parent: Frn,
        name_utf16le: &[u8],
        attrs: FileAttributes,
    ) -> Result<EntryId, IndexError> {
        let mut name = std::mem::take(&mut self.scratch);
        name.clear();
        let result = if fold::utf16le_to_wtf8(name_utf16le, &mut name) {
            self.upsert(frn, parent, &name, attrs)
        } else {
            Err(IndexError::InvalidName)
        };
        self.scratch = name;
        result
    }

    /// Removes the entry with this FRN; returns `false` if it is unknown (or is the root, which is
    /// never removed). Children of a removed directory keep their link: their paths report
    /// [`PathError::Orphan`] until an entry with the same FRN appears again.
    pub fn remove(&mut self, frn: Frn) -> bool {
        if frn == self.root_frn {
            return false;
        }
        let Some(i) = self.map.get(frn, &self.frn) else {
            return false;
        };
        self.map.remove(frn, i);
        let iu = i as usize;
        if let Some(a) = self.alive.get_mut(iu) {
            *a = false;
        }
        self.retire_segment(iu);
        self.dead_name_bytes += self.name_len.get(iu).map_or(0, |&l| usize::from(l));
        self.unresolved.remove(&i);
        self.live -= 1;
        true
    }

    /// Applies one change event (`Created`/`Modified`/`Renamed` upsert, `Deleted` removes).
    ///
    /// # Errors
    /// As [`NameIndex::insert`].
    pub fn apply(&mut self, event: &SourceEvent) -> Result<(), IndexError> {
        match event {
            SourceEvent::Created(r) | SourceEvent::Modified(r) => self.insert(r).map(|_| ()),
            SourceEvent::Renamed { record, .. } => self.insert(record).map(|_| ()),
            SourceEvent::Deleted(r) => {
                self.remove(r.frn);
                Ok(())
            }
        }
    }

    /// The live entry with this FRN.
    #[must_use]
    pub fn lookup(&self, frn: Frn) -> Option<EntryId> {
        self.map.get(frn, &self.frn).map(EntryId)
    }

    /// `true` if `id` names a live entry.
    #[must_use]
    pub fn contains(&self, id: EntryId) -> bool {
        self.alive.get(id.index()).copied().unwrap_or(false)
    }

    /// The entry's FRN.
    #[must_use]
    pub fn frn(&self, id: EntryId) -> Option<Frn> {
        if !self.contains(id) {
            return None;
        }
        self.frn.get(id.index()).copied()
    }

    /// The entry's name as WTF-8 (`FileName::from_wtf8` turns it into a `FileName`). The root's
    /// name is empty.
    #[must_use]
    pub fn name(&self, id: EntryId) -> Option<&[u8]> {
        self.contains(id).then(|| self.name_bytes(id.index()))
    }

    /// The entry's attributes.
    #[must_use]
    pub fn attrs(&self, id: EntryId) -> Option<FileAttributes> {
        if !self.contains(id) {
            return None;
        }
        self.attrs.get(id.index()).map(|&a| FileAttributes(a))
    }

    /// The entry's live parent: `None` for the root, unknown entries and orphans.
    #[must_use]
    pub fn parent(&self, id: EntryId) -> Option<EntryId> {
        if id == EntryId::ROOT || !self.contains(id) {
            return None;
        }
        match self.link(id.0) {
            Link::Entry(p) => Some(EntryId(p)),
            Link::Broken => None,
        }
    }

    /// Iterates the live entries (excluding the root) in index order.
    pub fn entries(&self) -> impl Iterator<Item = EntryId> + '_ {
        self.alive
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, a)| **a)
            .map(|(i, _)| EntryId(i as u32))
    }

    /// Reconstructs the entry's path relative to the volume root, as WTF-8 with `\` separators and
    /// a leading `\` (`\data\roads.shp`; the root is `\`). Prefix the drive (`C:`) to open it.
    ///
    /// # Errors
    /// [`PathError::NotFound`] for an unknown entry, [`PathError::Orphan`] when an ancestor is
    /// missing (e.g. NTFS metadata under `$Extend`, which enumeration doesn't return),
    /// [`PathError::Cycle`] for a parent cycle or a chain deeper than [`MAX_DEPTH`].
    pub fn path(&self, id: EntryId) -> Result<Vec<u8>, PathError> {
        if !self.contains(id) {
            return Err(PathError::NotFound);
        }
        let mut chain = Vec::new();
        let mut cur = id.0;
        while cur != 0 {
            if chain.len() >= MAX_DEPTH {
                return Err(PathError::Cycle);
            }
            chain.push(cur);
            match self.link(cur) {
                Link::Entry(p) => cur = p,
                Link::Broken => return Err(PathError::Orphan),
            }
        }
        if chain.is_empty() {
            return Ok(vec![SEPARATOR]);
        }
        let mut out = Vec::with_capacity(chain.len() * 16);
        for &e in chain.iter().rev() {
            out.push(SEPARATOR);
            out.extend_from_slice(self.name_bytes(e as usize));
        }
        Ok(out)
    }

    /// [`NameIndex::path`] for display: unpaired surrogates become U+FFFD.
    ///
    /// # Errors
    /// As [`NameIndex::path`].
    pub fn path_lossy(&self, id: EntryId) -> Result<String, PathError> {
        let bytes = self.path(id)?;
        Ok(match whereabouts_core::FileName::from_wtf8(&bytes) {
            Ok(name) => name.to_string_lossy().into_owned(),
            Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
        })
    }

    /// `true` when tombstones hold more than a quarter of the slots or pool bytes.
    #[must_use]
    pub fn needs_compaction(&self) -> bool {
        let dead_slots = self.frn.len() - 1 - self.live;
        dead_slots > 1024 && dead_slots > self.live / 4
            || self.dead_fold_bytes > (1 << 16) && self.dead_fold_bytes > self.folded.len() / 4
    }

    /// Drops tombstones and dead pool bytes, rebuilding the columns in index order.
    /// **Invalidates every [`EntryId`]** except [`EntryId::ROOT`]; FRNs are unaffected.
    pub fn compact(&mut self) {
        let n = self.frn.len();
        let mut remap = vec![NONE; n];
        let mut next = 0_u32;
        for (i, &a) in self.alive.iter().enumerate() {
            if a {
                if let Some(r) = remap.get_mut(i) {
                    *r = next;
                }
                next += 1;
            }
        }
        let live = next as usize;
        let mut out = NameIndex::with_capacity(
            self.root_frn,
            live.saturating_sub(1),
            self.names.len() - self.dead_name_bytes.min(self.names.len()),
        );
        out.attrs[0] = self
            .attrs
            .first()
            .copied()
            .unwrap_or(FileAttributes::DIRECTORY);
        for i in 1..n {
            if !self.alive.get(i).copied().unwrap_or(false) {
                continue;
            }
            let new = out.frn.len() as u32;
            let frn = self.frn.get(i).copied().unwrap_or(self.root_frn);
            let name = self.name_bytes(i);
            out.name_off.push(out.names.len() as u32);
            out.name_len.push(name.len() as u16);
            out.names.extend_from_slice(name);
            out.seg_start.push(out.folded.len() as u32);
            out.seg_owner.push(new);
            out.seg.push(out.seg_start.len() as u32 - 1);
            out.folded.extend_from_slice(self.folded_name(i));
            out.folded.push(0);
            out.ext.push(self.ext.get(i).copied().unwrap_or(NO_EXT));
            out.attrs.push(self.attrs.get(i).copied().unwrap_or(0));
            out.alive.push(true);
            out.frn.push(frn);
            out.parent.push(NONE);
            let slots = out.frn.len();
            out.map.insert(frn, new, &out.frn, slots);
        }
        out.live = live.saturating_sub(1);
        // Re-link parents: live parents map directly; a dead parent forwards to a live entry with
        // the same FRN if there is one; anything else waits for its parent FRN.
        for i in 1..n {
            let Some(&new) = remap.get(i) else { continue };
            if new == NONE {
                continue;
            }
            let wanted = match self.parent.get(i).copied().unwrap_or(NONE) {
                NONE => self.unresolved.get(&(i as u32)).copied(),
                p => match self.link(i as u32) {
                    Link::Entry(q) => {
                        if let (Some(slot), Some(&np)) =
                            (out.parent.get_mut(new as usize), remap.get(q as usize))
                        {
                            *slot = np;
                        }
                        None
                    }
                    Link::Broken => self.frn.get(p as usize).copied(),
                },
            };
            if let Some(f) = wanted {
                out.unresolved.insert(new, f);
                out.pending.entry(f).or_default().push(new);
            }
        }
        out.scratch = std::mem::take(&mut self.scratch);
        *self = out;
    }

    /// Size and memory figures.
    #[must_use]
    pub fn stats(&self) -> IndexStats {
        fn cap<T>(v: &Vec<T>) -> usize {
            v.capacity() * size_of::<T>()
        }
        let pending: usize = self
            .pending
            .values()
            .map(|v| cap(v) + size_of::<(Frn, Vec<u32>)>() + 1)
            .sum();
        let heap = cap(&self.frn)
            + cap(&self.parent)
            + cap(&self.name_off)
            + cap(&self.name_len)
            + cap(&self.seg)
            + cap(&self.ext)
            + cap(&self.attrs)
            + cap(&self.alive)
            + cap(&self.names)
            + cap(&self.folded)
            + cap(&self.seg_start)
            + cap(&self.seg_owner)
            + cap(&self.scratch)
            + self.map.heap_bytes()
            + self.unresolved.capacity() * (size_of::<(u32, Frn)>() + 1)
            + pending;
        IndexStats {
            entries: self.live,
            slots: self.frn.len(),
            dead_slots: self.frn.len() - 1 - self.live,
            unresolved: self.unresolved.len(),
            name_bytes: self.names.len(),
            folded_bytes: self.folded.len(),
            heap_bytes: heap,
        }
    }

    // ---- internals ------------------------------------------------------------------------

    pub(crate) fn upsert(
        &mut self,
        frn: Frn,
        parent: Frn,
        name: &[u8],
        attrs: FileAttributes,
    ) -> Result<EntryId, IndexError> {
        if name.len() > MAX_NAME_BYTES {
            return Err(IndexError::NameTooLong(name.len()));
        }
        if frn == self.root_frn {
            if let Some(a) = self.attrs.first_mut() {
                *a = attrs.0;
            }
            return Ok(EntryId::ROOT);
        }
        if let Some(i) = self.map.get(frn, &self.frn) {
            let iu = i as usize;
            if self.name_bytes(iu) != name {
                self.check_room(name.len())?;
                self.retire_segment(iu);
                self.dead_name_bytes += self.name_bytes(iu).len();
                let (off, seg, ext) = self.push_name(i, name);
                if let (Some(o), Some(l), Some(s), Some(e)) = (
                    self.name_off.get_mut(iu),
                    self.name_len.get_mut(iu),
                    self.seg.get_mut(iu),
                    self.ext.get_mut(iu),
                ) {
                    *o = off;
                    *l = name.len() as u16;
                    *s = seg;
                    *e = ext;
                }
            }
            self.set_parent(i, parent);
            if let Some(a) = self.attrs.get_mut(iu) {
                *a = attrs.0;
            }
            return Ok(EntryId(i));
        }
        if self.frn.len() >= (NONE - 1) as usize {
            return Err(IndexError::CapacityExceeded);
        }
        self.check_room(name.len())?;
        let i = self.frn.len() as u32;
        let (off, seg, ext) = self.push_name(i, name);
        self.frn.push(frn);
        self.parent.push(NONE);
        self.name_off.push(off);
        self.name_len.push(name.len() as u16);
        self.seg.push(seg);
        self.ext.push(ext);
        self.attrs.push(attrs.0);
        self.alive.push(true);
        let slots = self.frn.len();
        self.map.insert(frn, i, &self.frn, slots);
        self.live += 1;
        self.set_parent(i, parent);
        if let Some(kids) = self.pending.remove(&frn) {
            for k in kids {
                if self.unresolved.get(&k) == Some(&frn) {
                    self.unresolved.remove(&k);
                    if let Some(p) = self.parent.get_mut(k as usize) {
                        *p = i;
                    }
                }
            }
        }
        Ok(EntryId(i))
    }

    /// Fails if appending a name of `len` bytes could overflow the `u32` pool offsets.
    fn check_room(&self, len: usize) -> Result<(), IndexError> {
        let max = u32::MAX as usize;
        // A folded name is at most twice as long as the original (1:1 code points), plus NUL.
        if self.names.len() + len > max || self.folded.len() + 2 * len + 1 > max {
            return Err(IndexError::CapacityExceeded);
        }
        Ok(())
    }

    /// Appends `name` to both pools as a new segment owned by `owner`:
    /// `(name offset, segment, extension offset)`.
    fn push_name(&mut self, owner: u32, name: &[u8]) -> (u32, u32, u16) {
        let off = self.names.len() as u32;
        self.names.extend_from_slice(name);
        let start = self.folded.len();
        fold::fold_wtf8_into(name, &mut self.folded);
        let folded = self.folded.get(start..).unwrap_or_default();
        let ext = match memchr::memrchr(b'.', folded) {
            Some(dot) if dot + 1 < folded.len() => (dot + 1) as u16,
            _ => NO_EXT,
        };
        self.folded.push(0);
        self.seg_start.push(start as u32);
        self.seg_owner.push(owner);
        (off, self.seg_start.len() as u32 - 1, ext)
    }

    fn retire_segment(&mut self, i: usize) {
        let seg = self.seg.get(i).copied().unwrap_or(NONE) as usize;
        self.dead_fold_bytes += self.folded_name(i).len() + 1;
        if let Some(o) = self.seg_owner.get_mut(seg) {
            *o = NONE;
        }
    }

    fn set_parent(&mut self, i: u32, parent: Frn) {
        let resolved = self.map.get(parent, &self.frn);
        if self.parent.get(i as usize) == Some(&NONE) {
            if resolved.is_none() && self.unresolved.get(&i) == Some(&parent) {
                return;
            }
            self.unresolved.remove(&i);
        }
        let slot = match resolved {
            Some(p) => p,
            None => {
                self.unresolved.insert(i, parent);
                self.pending.entry(parent).or_default().push(i);
                NONE
            }
        };
        if let Some(p) = self.parent.get_mut(i as usize) {
            *p = slot;
        }
    }

    /// Follows entry `i`'s parent link (`i` must not be the root). A tombstoned parent forwards
    /// to the live entry with the same FRN, if any.
    pub(crate) fn link(&self, i: u32) -> Link {
        let p = self.parent.get(i as usize).copied().unwrap_or(NONE);
        if p == NONE {
            return Link::Broken;
        }
        if self.alive.get(p as usize).copied().unwrap_or(false) {
            return Link::Entry(p);
        }
        match self
            .frn
            .get(p as usize)
            .and_then(|&f| self.map.get(f, &self.frn))
        {
            Some(q) => Link::Entry(q),
            None => Link::Broken,
        }
    }

    pub(crate) fn name_bytes(&self, i: usize) -> &[u8] {
        let off = self.name_off.get(i).copied().unwrap_or(0) as usize;
        let len = self.name_len.get(i).copied().unwrap_or(0) as usize;
        self.names.get(off..off + len).unwrap_or_default()
    }

    /// Folded name of entry `i` (its current segment, without the NUL).
    pub(crate) fn folded_name(&self, i: usize) -> &[u8] {
        let seg = self.seg.get(i).copied().unwrap_or(NONE) as usize;
        self.segment(seg)
    }

    /// Bytes of segment `seg`, without the NUL separator.
    pub(crate) fn segment(&self, seg: usize) -> &[u8] {
        let Some(&start) = self.seg_start.get(seg) else {
            return &[];
        };
        let end = self
            .seg_start
            .get(seg + 1)
            .map_or(self.folded.len(), |&s| s as usize)
            .saturating_sub(1);
        self.folded.get(start as usize..end).unwrap_or_default()
    }

    /// The parent FRN the entry was last given (persisted in snapshots).
    pub(crate) fn parent_frn(&self, i: usize) -> Frn {
        match self.parent.get(i).copied().unwrap_or(NONE) {
            NONE => self
                .unresolved
                .get(&(i as u32))
                .copied()
                .unwrap_or(self.root_frn),
            p => self.frn.get(p as usize).copied().unwrap_or(self.root_frn),
        }
    }
}
