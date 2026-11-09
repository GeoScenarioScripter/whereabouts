//! Test fixture: a directory tree built from paths, with event helpers.
#![allow(dead_code, clippy::unwrap_used)]

use std::collections::HashMap;

use whereabouts_core::{Dataset, FileAttributes, FileName, FileRecord, Frn, VolumeId};
use whereabouts_datasets::{DirectoryView, Grouper, MemoryTree};
use whereabouts_source::SourceEvent;

pub const ROOT: Frn = Frn(5);
pub const VOL: VolumeId = VolumeId(1);

/// A tree addressed by `/`-separated paths relative to the root.
pub struct Fx {
    pub tree: MemoryTree,
    paths: HashMap<String, Frn>,
    next: u64,
}

impl Default for Fx {
    fn default() -> Self {
        Self::new()
    }
}

impl Fx {
    pub fn new() -> Self {
        let mut tree = MemoryTree::new();
        tree.upsert(&FileRecord {
            frn: ROOT,
            parent: ROOT,
            name: FileName::from(""),
            attrs: FileAttributes(FileAttributes::DIRECTORY),
        });
        Fx {
            tree,
            paths: HashMap::from([(String::new(), ROOT)]),
            next: 100,
        }
    }

    /// Builds a tree: entries ending in `/` are directories; parents are created as needed.
    pub fn with(paths: &[&str]) -> Self {
        let mut fx = Fx::new();
        for p in paths {
            fx.add(p);
        }
        fx
    }

    /// Adds a path (directory if it ends in `/`) and returns its FRN.
    pub fn add(&mut self, path: &str) -> Frn {
        let (path, dir) = match path.strip_suffix('/') {
            Some(p) => (p, true),
            None => (path, false),
        };
        let (parent, name) = split(path);
        let parent = self.ensure_dir(parent);
        self.add_named(parent, FileName::from(name), dir)
    }

    /// Adds an entry with an arbitrary (possibly non-Unicode) name.
    pub fn add_named(&mut self, parent: Frn, name: FileName, dir: bool) -> Frn {
        let frn = Frn(self.next);
        self.next += 1;
        self.tree.upsert(&record(frn, parent, name, dir));
        self.reindex();
        frn
    }

    pub fn frn(&self, path: &str) -> Frn {
        *self
            .paths
            .get(path.trim_end_matches('/'))
            .unwrap_or_else(|| panic!("no such path {path}"))
    }

    pub fn record(&self, path: &str) -> FileRecord {
        self.record_of(self.frn(path))
    }

    pub fn record_of(&self, frn: Frn) -> FileRecord {
        let e = self.tree.entry(frn).unwrap();
        record(
            frn,
            e.parent,
            FileName::from_wtf8(e.name).unwrap(),
            e.is_dir,
        )
    }

    pub fn path_of(&self, frn: Frn) -> String {
        let mut parts = Vec::new();
        let mut cur = frn;
        for _ in 0..64 {
            if cur == ROOT {
                break;
            }
            let Some(e) = self.tree.entry(cur) else {
                parts.push(format!("<{cur}>"));
                break;
            };
            parts.push(String::from_utf8_lossy(e.name).into_owned());
            cur = e.parent;
        }
        parts.reverse();
        parts.join("/")
    }

    fn ensure_dir(&mut self, path: &str) -> Frn {
        if let Some(&f) = self.paths.get(path) {
            return f;
        }
        let (parent, name) = split(path);
        let parent = self.ensure_dir(parent);
        self.add_named(parent, FileName::from(name), true)
    }

    fn reindex(&mut self) {
        let mut paths = HashMap::from([(String::new(), ROOT)]);
        let mut stack = vec![ROOT];
        while let Some(dir) = stack.pop() {
            let mut kids = Vec::new();
            self.tree
                .for_each_child(dir, &mut |e| kids.push((e.frn, e.is_dir)));
            for (frn, is_dir) in kids {
                paths.insert(self.path_of(frn), frn);
                if is_dir {
                    stack.push(frn);
                }
            }
        }
        self.paths = paths;
    }

    // ---- events (each applies itself to the tree and returns the event) ----

    pub fn create(&mut self, path: &str) -> SourceEvent {
        let frn = self.add(path);
        SourceEvent::Created(self.record_of(frn))
    }

    pub fn delete(&mut self, path: &str) -> SourceEvent {
        let rec = self.record(path);
        let ev = SourceEvent::Deleted(rec);
        self.tree.apply(&ev);
        self.reindex();
        ev
    }

    pub fn rename(&mut self, from: &str, to: &str) -> SourceEvent {
        let old = self.record(from);
        let (parent, name) = split(to.trim_end_matches('/'));
        let parent = self.ensure_dir(parent);
        let new = FileRecord {
            parent,
            name: FileName::from(name),
            ..old.clone()
        };
        let ev = SourceEvent::Renamed {
            record: new,
            old_parent: old.parent,
            old_name: old.name,
        };
        self.tree.apply(&ev);
        self.reindex();
        ev
    }

    pub fn modify(&mut self, path: &str) -> SourceEvent {
        SourceEvent::Modified(self.record(path))
    }

    /// Describes a dataset as `"<format> <anchor> [<role>:<path>, …]"` (members sorted).
    pub fn describe(&self, ds: &Dataset) -> String {
        let mut members: Vec<String> = ds
            .members()
            .iter()
            .filter(|m| m.frn != ds.anchor())
            .map(|m| format!("{}:{}", m.role.code(), self.path_of(m.frn)))
            .collect();
        members.sort();
        format!(
            "{} {} [{}]",
            ds.format().code(),
            self.path_of(ds.anchor()),
            members.join(", ")
        )
    }

    /// All datasets of a grouper, described and sorted.
    pub fn describe_all(&self, g: &Grouper) -> Vec<String> {
        let mut out: Vec<String> = g.datasets().map(|d| self.describe(d)).collect();
        out.sort();
        out
    }

    /// Groups the whole tree from scratch.
    pub fn scan(&self) -> Grouper {
        let mut g = Grouper::new(VOL);
        g.full_scan(&self.tree, ROOT);
        g
    }
}

pub fn record(frn: Frn, parent: Frn, name: FileName, dir: bool) -> FileRecord {
    FileRecord {
        frn,
        parent,
        name,
        attrs: FileAttributes(if dir {
            FileAttributes::DIRECTORY
        } else {
            FileAttributes::ARCHIVE
        }),
    }
}

fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}
