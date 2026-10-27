//! The directory view the grouper reads, and an in-memory implementation.

use std::collections::{BTreeSet, HashMap};

use whereabouts_core::{FileName, FileRecord, Frn};
use whereabouts_source::SourceEvent;

/// One directory entry as the grouper sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The entry's FRN.
    pub frn: Frn,
    /// FRN of the containing directory.
    pub parent: Frn,
    /// The entry's name as WTF-8 bytes (D-28; [`FileName::as_wtf8`]).
    pub name: &'a [u8],
    /// `true` for directories.
    pub is_dir: bool,
}

/// Read access to a volume's directory tree, keyed by FRN.
///
/// The grouper never touches the file system: it reads names through this trait, so it can be
/// tested without a volume and fed by the name index in the pipeline.
///
/// Requirements:
/// - `for_each_child(dir)` visits every direct child of `dir` once, files and directories. It
///   may skip files for which [`crate::is_relevant_name`] is `false`: grouping ignores them.
///   It must not report `dir` as its own child (the NTFS root is its own parent).
/// - Children are keyed by their parent FRN even if `dir` itself is not (yet) known, so a file
///   moved into a directory whose creation arrives later in the same batch is still listed.
/// - `entry(frn)` returns the entry's current parent and name (used for folder datasets such
///   as `*.gdb`).
pub trait DirectoryView {
    /// Calls `visit` for each direct child of `dir`, in any order.
    fn for_each_child(&self, dir: Frn, visit: &mut dyn FnMut(Entry<'_>));

    /// The entry for `frn`, or `None` if unknown.
    fn entry(&self, frn: Frn) -> Option<Entry<'_>>;
}

impl<V: DirectoryView + ?Sized> DirectoryView for &V {
    fn for_each_child(&self, dir: Frn, visit: &mut dyn FnMut(Entry<'_>)) {
        (**self).for_each_child(dir, visit);
    }

    fn entry(&self, frn: Frn) -> Option<Entry<'_>> {
        (**self).entry(frn)
    }
}

#[derive(Debug, Clone)]
struct Node {
    parent: Frn,
    name: FileName,
    is_dir: bool,
}

/// A simple in-memory [`DirectoryView`]: FRN → entry, plus a child set per parent FRN.
///
/// Used by tests and usable by callers that have no name index. It applies [`SourceEvent`]s
/// idempotently: `Created`, `Renamed` and `Modified` upsert (moving the entry if its parent or
/// name changed) and `Deleted` removes. Children are keyed by parent FRN, so entries whose
/// parent is not (yet) known are kept and listed. Hard links are not modelled: one FRN has one
/// parent and name (the last one applied).
#[derive(Debug, Clone, Default)]
pub struct MemoryTree {
    nodes: HashMap<Frn, Node>,
    children: HashMap<Frn, BTreeSet<Frn>>,
}

impl MemoryTree {
    /// An empty tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// `true` if the tree has no entries.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Inserts or updates an entry (moving it if its parent changed).
    pub fn upsert(&mut self, record: &FileRecord) {
        let node = Node {
            parent: record.parent,
            name: record.name.clone(),
            is_dir: record.is_dir(),
        };
        if let Some(old) = self.nodes.insert(record.frn, node) {
            self.unlink(record.frn, old.parent);
        }
        if record.parent != record.frn {
            self.children
                .entry(record.parent)
                .or_default()
                .insert(record.frn);
        }
    }

    /// Removes an entry. Its children (if any) stay listed under its FRN.
    pub fn remove(&mut self, frn: Frn) -> bool {
        match self.nodes.remove(&frn) {
            Some(old) => {
                self.unlink(frn, old.parent);
                true
            }
            None => false,
        }
    }

    /// Applies one change event (see the type docs for the semantics).
    pub fn apply(&mut self, event: &SourceEvent) {
        match event {
            SourceEvent::Deleted(r) => {
                self.remove(r.frn);
            }
            SourceEvent::Created(r)
            | SourceEvent::Modified(r)
            | SourceEvent::Renamed { record: r, .. } => self.upsert(r),
        }
    }

    fn unlink(&mut self, frn: Frn, parent: Frn) {
        if let Some(set) = self.children.get_mut(&parent) {
            set.remove(&frn);
            if set.is_empty() {
                self.children.remove(&parent);
            }
        }
    }
}

impl DirectoryView for MemoryTree {
    fn for_each_child(&self, dir: Frn, visit: &mut dyn FnMut(Entry<'_>)) {
        let Some(kids) = self.children.get(&dir) else {
            return;
        };
        for &frn in kids {
            if frn == dir {
                continue;
            }
            if let Some(node) = self.nodes.get(&frn) {
                visit(Entry {
                    frn,
                    parent: node.parent,
                    name: node.name.as_wtf8(),
                    is_dir: node.is_dir,
                });
            }
        }
    }

    fn entry(&self, frn: Frn) -> Option<Entry<'_>> {
        self.nodes.get(&frn).map(|node| Entry {
            frn,
            parent: node.parent,
            name: node.name.as_wtf8(),
            is_dir: node.is_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whereabouts_core::FileAttributes;

    fn rec(frn: u64, parent: u64, name: &str, dir: bool) -> FileRecord {
        FileRecord {
            frn: Frn(frn),
            parent: Frn(parent),
            name: FileName::from(name),
            attrs: FileAttributes(if dir { FileAttributes::DIRECTORY } else { 0 }),
        }
    }

    fn names(t: &MemoryTree, dir: u64) -> Vec<String> {
        let mut out = Vec::new();
        t.for_each_child(Frn(dir), &mut |e| {
            out.push(String::from_utf8_lossy(e.name).into_owned())
        });
        out
    }

    #[test]
    fn upsert_moves_and_remove_unlinks() {
        let mut t = MemoryTree::new();
        t.upsert(&rec(5, 5, "", true));
        t.upsert(&rec(10, 5, "d", true));
        t.upsert(&rec(11, 10, "a.shp", false));
        assert_eq!(names(&t, 5), ["d"], "root is not its own child");
        assert_eq!(names(&t, 10), ["a.shp"]);
        // Out-of-order: a child of a directory that does not exist yet.
        t.upsert(&rec(12, 20, "b.shp", false));
        assert_eq!(names(&t, 20), ["b.shp"]);
        t.apply(&SourceEvent::Created(rec(11, 20, "c.shp", false)));
        assert_eq!(names(&t, 10), Vec::<String>::new());
        assert_eq!(names(&t, 20), ["c.shp", "b.shp"], "FRN order");
        t.apply(&SourceEvent::Deleted(rec(11, 20, "c.shp", false)));
        assert_eq!(names(&t, 20), ["b.shp"]);
        assert!(!t.remove(Frn(11)));
        assert_eq!(t.entry(Frn(10)).map(|e| e.is_dir), Some(true));
        assert_eq!(t.len(), 3);
    }
}
