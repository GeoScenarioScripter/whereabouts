//! Parent-chain resolution statistics: how many enumerated records reach the volume root by
//! following `parent_frn` links. Used by the elevated enumeration test and the `enum_volume`
//! example as a completeness check of an enumeration (pure; tested cross-platform).

use std::collections::HashMap;

/// Outcome of walking every record's parent chain towards the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParentChainReport {
    /// Records examined (duplicate FRNs counted once).
    pub records: usize,
    /// Records whose chain reaches the root (the root itself included).
    pub resolved: usize,
    /// Records whose chain hits a parent that was not enumerated.
    pub orphaned: usize,
    /// Records whose chain loops without reaching the root.
    pub cyclic: usize,
    /// `true` if the root FRN was itself among the records.
    pub root_enumerated: bool,
    /// Records whose parent is the root.
    pub root_children: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Visiting,
    Resolved,
    Orphaned,
    Cyclic,
}

impl ParentChainReport {
    /// Walks the `(frn, parent_frn)` links. Runs in O(n): each record is resolved once and the
    /// result is memoised for every record on its chain. Never recurses.
    #[must_use]
    pub fn compute(links: &[(u64, u64)], root_frn: u64) -> Self {
        let parents: HashMap<u64, u64> = links.iter().copied().collect();
        let mut state: HashMap<u64, State> = HashMap::with_capacity(parents.len());
        let mut path = Vec::new();
        for &start in parents.keys() {
            let mut cur = start;
            let outcome = loop {
                if cur == root_frn {
                    break State::Resolved;
                }
                match state.get(&cur) {
                    Some(State::Visiting) => break State::Cyclic,
                    Some(&done) => break done,
                    None => {}
                }
                let Some(&parent) = parents.get(&cur) else {
                    break State::Orphaned;
                };
                state.insert(cur, State::Visiting);
                path.push(cur);
                cur = parent;
            };
            for frn in path.drain(..) {
                state.insert(frn, outcome);
            }
        }

        let mut report = Self {
            records: parents.len(),
            root_enumerated: parents.contains_key(&root_frn),
            root_children: parents
                .iter()
                .filter(|&(&frn, &parent)| parent == root_frn && frn != root_frn)
                .count(),
            ..Self::default()
        };
        for frn in parents.keys() {
            match state.get(frn) {
                Some(State::Orphaned) => report.orphaned += 1,
                Some(State::Cyclic) => report.cyclic += 1,
                // The root itself never enters `state`; it trivially resolves.
                _ => report.resolved += 1,
            }
        }
        report
    }

    /// Fraction of records that resolve to the root (1.0 for an empty input).
    #[must_use]
    pub fn resolved_ratio(&self) -> f64 {
        if self.records == 0 {
            1.0
        } else {
            self.resolved as f64 / self.records as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: u64 = 0x0005_0000_0000_0005;

    #[test]
    fn simple_tree_resolves() {
        let links = [(ROOT, ROOT), (10, ROOT), (11, 10), (12, 11), (13, ROOT)];
        let r = ParentChainReport::compute(&links, ROOT);
        assert_eq!(r.records, 5);
        assert_eq!(r.resolved, 5);
        assert_eq!((r.orphaned, r.cyclic), (0, 0));
        assert!(r.root_enumerated);
        assert_eq!(r.root_children, 2);
        assert!((r.resolved_ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn root_not_enumerated_still_resolves_children() {
        let links = [(10, ROOT), (11, 10)];
        let r = ParentChainReport::compute(&links, ROOT);
        assert_eq!(r.resolved, 2);
        assert!(!r.root_enumerated);
    }

    #[test]
    fn orphans_and_cycles_are_counted() {
        let links = [
            (10, ROOT),
            (20, 999), // parent missing
            (21, 20),  // inherits the orphan
            (30, 31),  // 30 <-> 31 cycle
            (31, 30),
            (32, 30), // hangs off the cycle
        ];
        let r = ParentChainReport::compute(&links, ROOT);
        assert_eq!(r.records, 6);
        assert_eq!(r.resolved, 1);
        assert_eq!(r.orphaned, 2);
        assert_eq!(r.cyclic, 3);
    }

    #[test]
    fn stale_sequence_number_is_an_orphan() {
        // Parent reference carries sequence 2 but the directory now has sequence 3.
        let links = [(0x0003_0000_0000_0040, ROOT), (77, 0x0002_0000_0000_0040)];
        let r = ParentChainReport::compute(&links, ROOT);
        assert_eq!(r.orphaned, 1);
    }

    #[test]
    fn deep_chain_does_not_recurse() {
        let links: Vec<(u64, u64)> = (0..200_000_u64)
            .map(|i| (100 + i, if i == 0 { ROOT } else { 99 + i }))
            .collect();
        let r = ParentChainReport::compute(&links, ROOT);
        assert_eq!(r.resolved, 200_000);
    }

    #[test]
    fn empty_input() {
        let r = ParentChainReport::compute(&[], ROOT);
        assert_eq!(r.records, 0);
        assert!((r.resolved_ratio() - 1.0).abs() < f64::EPSILON);
    }
}
