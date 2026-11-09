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
    // A file anywhere inside a GDB belongs to that folder dataset.
    let target = requested
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gdb")) && p.is_dir())
        .unwrap_or(&requested);
    let parent = target
        .parent()
        .context("inspect requires a dataset path, not a volume root")?;
    let mut tree = MemoryTree::new();
    let mut paths = HashMap::new();
    let root = Frn(1);
    let mut pending = vec![(parent.to_path_buf(), root)];
    let mut next = 2_u64;
    let mut selected = None;
    while let Some((dir, parent_id)) = pending.pop() {
        for entry in fs::read_dir(&dir).with_context(|| format!("cannot list {}", dir.display()))? {
            let entry = entry?;
            ensure!(
                paths.len() < MAX_ENTRIES,
                "inspect directory exceeds {MAX_ENTRIES} entries"
            );
            let ty = entry.file_type()?;
            // Never recurse through symlinks/junctions. Readers receive only ordinary entries.
            if ty.is_symlink() {
                continue;
            }
            let frn = Frn(next);
            next += 1;
            let entry_path = entry.path();
            if entry_path == target {
                selected = Some(frn);
            }
            let name = FileName::from_wtf8(entry.file_name().as_encoded_bytes())
                .context("filename is not representable as WTF-8")?;
            tree.upsert(&FileRecord {
                frn,
                parent: parent_id,
                name,
                attrs: FileAttributes(if ty.is_dir() {
                    FileAttributes::DIRECTORY
                } else {
                    0
                }),
            });
            // Only the selected folder (and its ordinary descendants) needs discovery.
            if ty.is_dir() && (entry_path == target || dir.starts_with(target)) {
                pending.push((entry_path.clone(), frn));
            }
            paths.insert(frn, entry_path);
        }
    }
    let selected = selected.context("inspect target is not an ordinary file or folder")?;
    let mut grouper = Grouper::new(VolumeId(0));
    grouper.full_scan(&tree, root);
    let mut ids: Vec<_> = grouper.datasets_containing(selected).collect();
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
