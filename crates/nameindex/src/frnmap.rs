//! FRN → entry map: a dense table indexed by MFT record number, with a hash-map overflow.
//!
//! NTFS record numbers are dense (they index the MFT), so a `Vec<u32>` keyed by record number
//! costs 4 bytes per MFT record instead of ~16–24 for a hash map. Each slot holds the entry index;
//! the full FRN (with sequence number) is verified against the index's own `frn` column. FRNs that
//! don't fit the dense table — a record number far beyond the current size (synthetic IDs of other
//! sources), or a second live FRN sharing a record number (only possible with inconsistent event
//! streams) — go to the overflow map, so the map is correct for any 64-bit FRN.

use std::collections::HashMap;

use whereabouts_core::Frn;

use crate::index::NONE;

/// Slots beyond `GROWTH_SLACK + GROWTH_FACTOR × entries` are not allocated densely.
const GROWTH_SLACK: u64 = 1 << 16;
const GROWTH_FACTOR: u64 = 8;

#[derive(Debug, Clone, Default)]
pub(crate) struct FrnMap {
    dense: Vec<u32>,
    overflow: HashMap<Frn, u32>,
}

impl FrnMap {
    pub(crate) fn get(&self, frn: Frn, frns: &[Frn]) -> Option<u32> {
        if let Ok(rn) = usize::try_from(frn.record_number())
            && let Some(&i) = self.dense.get(rn)
            && i != NONE
            && frns.get(i as usize) == Some(&frn)
        {
            return Some(i);
        }
        if self.overflow.is_empty() {
            return None;
        }
        self.overflow.get(&frn).copied()
    }

    /// Maps `frn` to entry `i`. The caller guarantees `frn` is not currently mapped and that
    /// `frns[i] == frn`. `entries` is the current number of entry slots (bounds dense growth).
    pub(crate) fn insert(&mut self, frn: Frn, i: u32, frns: &[Frn], entries: usize) {
        let rn = frn.record_number();
        let limit = GROWTH_SLACK.saturating_add(GROWTH_FACTOR.saturating_mul(entries as u64));
        if let Ok(slot) = usize::try_from(rn) {
            if slot >= self.dense.len() && rn <= limit {
                self.dense.resize(slot + 1, NONE);
            }
            if let Some(cell) = self.dense.get_mut(slot) {
                let occupied = *cell != NONE && frns.get(*cell as usize).is_some_and(|f| *f != frn);
                if !occupied {
                    *cell = i;
                    return;
                }
            }
        }
        self.overflow.insert(frn, i);
    }

    /// Unmaps `frn`, which the caller knows maps to entry `i`.
    pub(crate) fn remove(&mut self, frn: Frn, i: u32) {
        if let Ok(slot) = usize::try_from(frn.record_number())
            && let Some(cell) = self.dense.get_mut(slot)
            && *cell == i
        {
            *cell = NONE;
            return;
        }
        self.overflow.remove(&frn);
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.dense.capacity() * 4 + self.overflow.capacity() * (size_of::<(Frn, u32)>() + 1)
    }
}
