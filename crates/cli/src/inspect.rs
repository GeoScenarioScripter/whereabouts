//! Single-path grouping and shared extraction without a catalog.
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, ensure};
use whereabouts_core::{
    DatasetRecord, FileAttributes, FileName, FileRecord, Frn, MemberRole, ReadLimits,
    ResolvedDataset, ResolvedMember, Timestamp, VolumeId,
};
use whereabouts_datasets::{Grouper, MemoryTree};

// Bound directory discovery independently of the readers' content budgets.
const MAX_ENTRIES: usize = 100_000;

pub fn inspect(path: &Path) -> anyhow::Result<Vec<DatasetRecord>> {
    let requested = fs::canonicalize(path).context("cannot resolve inspect path")?;
    // Group the containing directory exactly as the indexer does. Seed its own
    // entry too: folder datasets are anchored there (GDB, Esri Grid signatures).
    let is_dir = requested.is_dir();
    let directory = if is_dir {
        requested.as_path()
    } else {
        requested
            .parent()
            .context("inspect path has no containing directory")?
    };
    let root = Frn(1);
    let folder = Frn(2);
    let mut tree = MemoryTree::new();
    tree.upsert(&FileRecord {
        frn: folder,
        parent: root,
        name: FileName::from_wtf8(directory.file_name().unwrap_or_default().as_encoded_bytes())?,
        attrs: FileAttributes(FileAttributes::DIRECTORY),
    });
    let mut paths = HashMap::from([(folder, directory.to_path_buf())]);
    let mut selected = is_dir.then_some(folder);
    for (i, entry) in fs::read_dir(directory)
        .with_context(|| format!("cannot list {}", directory.display()))?
        .enumerate()
    {
        ensure!(
            i < MAX_ENTRIES,
            "inspect directory exceeds {MAX_ENTRIES} entries"
        );
        let entry = entry?;
        let ty = entry.file_type()?;
        // Skipped symlinks still consume the discovery budget. No recursive walk.
        if ty.is_symlink() {
            continue;
        }
        let frn = Frn(i as u64 + 3);
        let entry_path = entry.path();
        if entry_path == requested {
            selected = Some(frn);
        }
        tree.upsert(&FileRecord {
            frn,
            parent: folder,
            name: FileName::from_wtf8(entry.file_name().as_encoded_bytes())
                .context("filename is not representable as WTF-8")?,
            attrs: FileAttributes(if ty.is_dir() {
                FileAttributes::DIRECTORY
            } else {
                0
            }),
        });
        paths.insert(frn, entry_path);
    }
    let selected = selected.context("inspect target is not an ordinary file or folder")?;
    let mut grouper = Grouper::new(VolumeId(0));
    grouper.full_scan(&tree, root);
    // A recognized folder owns even an ignored internal filename. A directory
    // merely named *.gdb without a signature keeps normal per-file grouping.
    let mut ids: Vec<_> = grouper.datasets_containing(folder).collect();
    if ids.is_empty() {
        ids.extend(grouper.datasets_containing(selected));
    }
    ids.sort();
    ensure!(
        !ids.is_empty(),
        "path is not a recognized dataset or dataset member"
    );
    let mut records = Vec::new();
    for id in ids {
        let dataset = grouper
            .dataset(id)
            .context("grouped dataset disappeared")?
            .clone();
        let anchor = paths
            .get(&id.anchor)
            .context("missing dataset anchor")?
            .clone();
        let mut sidecars = Vec::new();
        let mut total_size = Some(0_u64);
        let mut mtime = None;
        for member in dataset.members() {
            let member_path = paths.get(&member.frn).context("missing dataset member")?;
            let metadata = fs::metadata(member_path)?;
            if metadata.is_file() {
                total_size = total_size.and_then(|n| n.checked_add(metadata.len()));
            }
            mtime = mtime.max(
                metadata
                    .modified()
                    .ok()
                    .and_then(Timestamp::from_system_time),
            );
            if member.role != MemberRole::Main {
                sidecars.push(ResolvedMember {
                    role: member.role,
                    path: member_path.clone(),
                });
            }
        }
        let resolved = ResolvedDataset {
            format: dataset.format(),
            anchor: anchor.clone(),
            sidecars,
        };
        let extracted = whereabouts_indexer::extract(&resolved, &ReadLimits::default());
        records.push(DatasetRecord {
            dataset,
            name: anchor
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            primary_path: anchor.display().to_string(),
            total_size,
            mtime,
            status: extracted.status,
            extracted_at: Timestamp::from_system_time(SystemTime::now()),
            layers: extracted.layers,
        });
    }
    Ok(records)
}

pub fn snapshot_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_os_string();
    name.push(".names");
    name.into()
}
