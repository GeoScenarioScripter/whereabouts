//! Dataset query planning over a persisted name index and catalog.
//!
//! Spec: `docs/03-architecture.md` §5, stage01/4.1. Plain substring names only;
//! Q-11 syntax and Q-17 spatial relevance remain deferred.

use rayon::{ThreadPool, ThreadPoolBuilder};
use std::collections::{HashMap, HashSet};
use whereabouts_catalog::{Catalog, CatalogError, DatasetFilter};
use whereabouts_core::{DatasetId, DatasetRecord, Format, VolumeId, Wgs84Extent};
use whereabouts_nameindex::{EntryId, NameIndex, Query, fold_str};

/// A dataset search. The limit applies after deduplication and ranking.
#[derive(Debug, Clone)]
pub struct Request {
    /// Case-insensitive literal substring (empty matches all datasets).
    pub text: String,
    /// Allowed formats; empty means any.
    pub formats: Vec<Format>,
    /// Required EPSG, on the same layer as the spatial filter.
    pub epsg: Option<u32>,
    /// WGS84 search extent, possibly split at the antimeridian.
    pub bbox: Option<Wgs84Extent>,
    /// Maximum datasets returned.
    pub limit: usize,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            text: String::new(),
            formats: Vec::new(),
            epsg: None,
            bbox: None,
            limit: 100,
        }
    }
}

/// Reusable dedicated search pool. Does not share the extraction worker pool.
#[derive(Debug)]
pub struct QueryEngine {
    pool: ThreadPool,
}

impl QueryEngine {
    /// Creates a pool with `threads` workers (zero uses Rayon's available parallelism).
    pub fn new(threads: usize) -> Result<Self, rayon::ThreadPoolBuildError> {
        Ok(Self {
            pool: ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("query-{i}"))
                .build()?,
        })
    }

    /// Scans names, maps member FRNs to datasets, intersects catalog filters and ranks.
    ///
    /// Orphan/cyclic paths are hidden. Exact names (including dataset display stems) rank
    /// before prefixes, then substrings; latest member mtime breaks ties, then dataset ID.
    /// The caller must ensure the snapshot checkpoint matches the catalog's volume.
    pub fn search(
        &self,
        names: &NameIndex,
        catalog: &Catalog,
        volume: VolumeId,
        request: &Request,
    ) -> Result<Vec<DatasetRecord>, CatalogError> {
        if request.limit == 0 {
            return Ok(Vec::new());
        }
        let hits = (!request.text.is_empty()).then(|| {
            self.pool
                .install(|| names.search(&Query::substring(&request.text)))
        });
        let filtered =
            !request.formats.is_empty() || request.epsg.is_some() || request.bbox.is_some();
        let allowed: Option<HashSet<_>> = if filtered {
            Some(
                catalog
                    .find_datasets(&DatasetFilter {
                        volume: Some(volume),
                        formats: request.formats.clone(),
                        epsg: request.epsg,
                        bbox: request.bbox.clone(),
                        limit: None,
                    })?
                    .into_iter()
                    .collect(),
            )
        } else {
            None
        };
        if allowed.as_ref().is_some_and(HashSet::is_empty) {
            return Ok(Vec::new());
        }
        let needle = fold_str(&request.text);
        let mut members = HashMap::new();
        let broad = match &hits {
            None => true,
            Some(hits) if hits.hits.len() > 256 => {
                let candidates = match &allowed {
                    Some(ids) => ids.len() as u64,
                    None => catalog.stats()?.datasets,
                };
                hits.hits.len() as u64 > candidates.saturating_mul(4)
            }
            Some(_) => false,
        };
        for hit in hits
            .into_iter()
            .filter(|_| !broad)
            .flat_map(|hits| hits.hits)
        {
            let Some(frn) = names.frn(hit) else {
                continue;
            };
            members.insert(frn, hit);
        }
        let mut rows = Vec::new();
        // Broad terms should cost O(catalog datasets), not one SQL call for every
        // non-GIS filename. Keep indexed member lookups for selective terms.
        let mut ranks: HashMap<DatasetId, u8> = HashMap::new();
        if broad {
            let ids: Vec<_> = match &allowed {
                Some(ids) => ids.iter().copied().collect(),
                None => catalog.dataset_ids(volume)?,
            };
            for id in ids {
                if let Some(record) = catalog.get_dataset(id)?
                    && let Some(rank) = if request.text.is_empty() {
                        Some(1)
                    } else {
                        record
                            .dataset
                            .members()
                            .iter()
                            .filter_map(|member| {
                                names
                                    .lookup(member.frn)
                                    .and_then(|entry| member_rank(names, entry, &needle))
                            })
                            .min()
                    }
                {
                    rows.push((rank, record));
                }
            }
        } else {
            for (frn, entry) in members {
                let Some(rank) = member_rank(names, entry, &needle) else {
                    continue;
                };
                for (id, _) in catalog.datasets_with_member(volume, frn)? {
                    if allowed.as_ref().is_some_and(|set| !set.contains(&id)) {
                        continue;
                    }
                    ranks
                        .entry(id)
                        .and_modify(|old| *old = (*old).min(rank))
                        .or_insert(rank);
                }
            }
        }
        for (id, rank) in ranks {
            if let Some(record) = catalog.get_dataset(id)? {
                rows.push((rank, record));
            }
        }
        // An accessible sidecar must not make an orphan anchor visible.
        rows.retain(|(_, row)| {
            names
                .lookup(row.dataset.anchor())
                .is_some_and(|entry| names.path(entry).is_ok())
        });
        for (rank, record) in &mut rows {
            *rank = (*rank).min(relevance(&fold_str(&record.name), &needle));
        }
        rows.sort_by(|(ar, a), (br, b)| {
            ar.cmp(br)
                .then_with(|| b.mtime.cmp(&a.mtime))
                .then_with(|| a.dataset.id().cmp(&b.dataset.id()))
        });
        rows.truncate(request.limit);
        Ok(rows.into_iter().map(|(_, row)| row).collect())
    }
}

fn member_rank(names: &NameIndex, entry: EntryId, needle: &str) -> Option<u8> {
    names.path(entry).ok()?;
    let folded = fold_name(names.name(entry)?);
    let needle = needle.as_bytes();
    if needle.is_empty() || folded.windows(needle.len()).any(|part| part == needle) {
        Some(if folded == needle {
            0
        } else if folded.starts_with(needle) {
            1
        } else {
            2
        })
    } else {
        None
    }
}

// Preserve unpaired WTF-8 surrogates: lossy conversion would invent U+FFFD hits
// on the broad planner path that the name index correctly does not match.
fn fold_name(mut name: &[u8]) -> Vec<u8> {
    let mut folded = Vec::with_capacity(name.len());
    while !name.is_empty() {
        match std::str::from_utf8(name) {
            Ok(text) => {
                folded.extend_from_slice(fold_str(text).as_bytes());
                break;
            }
            Err(error) => {
                let (valid, rest) = name.split_at(error.valid_up_to());
                if let Ok(text) = std::str::from_utf8(valid) {
                    folded.extend_from_slice(fold_str(text).as_bytes());
                }
                let (invalid, rest) = rest.split_at(error.error_len().unwrap_or(rest.len()));
                folded.extend_from_slice(invalid);
                name = rest;
            }
        }
    }
    folded
}

fn relevance(name: &str, needle: &str) -> u8 {
    if name == needle {
        0
    } else if name.starts_with(needle) {
        1
    } else {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_keeps_surrogates_distinct_from_replacement_characters() {
        assert_eq!(fold_name(b"RO\xed\xa0\x80AD"), b"ro\xed\xa0\x80ad");
        assert_ne!(
            fold_name(b"RO\xed\xa0\x80AD"),
            fold_name("RO�AD".as_bytes())
        );
        assert_eq!(
            fold_name("ΣTRASSE".as_bytes()),
            fold_name("ςtrasse".as_bytes())
        );
    }
}
