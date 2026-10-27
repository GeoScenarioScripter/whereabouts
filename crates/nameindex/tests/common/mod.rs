//! A deliberately naive reference model of the name index: a map FRN → (parent FRN, UTF-16
//! name, attributes), paths by walking the map with a visited set, search by filtering every
//! record over code points. It shares nothing with the index's byte-level implementation except
//! the per-character fold function.

#![allow(dead_code, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet, HashSet};

use whereabouts_core::{FileAttributes, FileName, Frn};
use whereabouts_nameindex::{KindFilter, MatchMode, PathError, Query, fold_char};

#[derive(Debug, Clone)]
pub struct Model {
    pub root: Frn,
    pub recs: BTreeMap<Frn, (Frn, Vec<u16>, u32)>,
}

/// Code points of UTF-16 units, unpaired surrogates kept as their own values.
pub fn code_points(units: &[u16]) -> Vec<u32> {
    char::decode_utf16(units.iter().copied())
        .map(|r| match r {
            Ok(c) => c as u32,
            Err(e) => u32::from(e.unpaired_surrogate()),
        })
        .collect()
}

pub fn fold_cps(cps: &[u32]) -> Vec<u32> {
    cps.iter()
        .map(|&cp| char::from_u32(cp).map_or(cp, |c| fold_char(c) as u32))
        .collect()
}

pub fn wtf8(units: &[u16]) -> Vec<u8> {
    FileName::from_utf16(units).as_wtf8().to_vec()
}

/// Anchored wildcard match by dynamic programming over code points.
pub fn wild(p: &[u32], t: &[u32]) -> bool {
    let (star, one) = ('*' as u32, '?' as u32);
    let mut dp = vec![vec![false; t.len() + 1]; p.len() + 1];
    dp[0][0] = true;
    for i in 1..=p.len() {
        if p[i - 1] == star {
            dp[i][0] = dp[i - 1][0];
        }
        for j in 1..=t.len() {
            dp[i][j] = if p[i - 1] == star {
                dp[i - 1][j] || dp[i][j - 1]
            } else {
                dp[i - 1][j - 1] && (p[i - 1] == one || p[i - 1] == t[j - 1])
            };
        }
    }
    dp[p.len()][t.len()]
}

fn contains(hay: &[u32], needle: &[u32]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

impl Model {
    pub fn new(root: Frn) -> Self {
        Model {
            root,
            recs: BTreeMap::new(),
        }
    }

    pub fn upsert(&mut self, frn: Frn, parent: Frn, name: Vec<u16>, attrs: u32) {
        if frn != self.root {
            self.recs.insert(frn, (parent, name, attrs));
        }
    }

    pub fn remove(&mut self, frn: Frn) {
        self.recs.remove(&frn);
    }

    /// Path components (UTF-16 names, root first) or the reason there is no path.
    pub fn components(&self, frn: Frn) -> Result<Vec<&[u16]>, PathError> {
        if frn != self.root && !self.recs.contains_key(&frn) {
            return Err(PathError::NotFound);
        }
        let mut seen = HashSet::new();
        let mut parts = Vec::new();
        let mut cur = frn;
        while cur != self.root {
            if !seen.insert(cur) {
                return Err(PathError::Cycle);
            }
            let Some((parent, name, _)) = self.recs.get(&cur) else {
                return Err(PathError::Orphan);
            };
            parts.push(name.as_slice());
            cur = *parent;
        }
        parts.reverse();
        Ok(parts)
    }

    pub fn path(&self, frn: Frn) -> Result<Vec<u8>, PathError> {
        let parts = self.components(frn)?;
        if parts.is_empty() {
            return Ok(b"\\".to_vec());
        }
        let mut out = Vec::new();
        for p in parts {
            out.push(b'\\');
            out.extend_from_slice(&wtf8(p));
        }
        Ok(out)
    }

    pub fn search(&self, q: &Query) -> BTreeSet<Frn> {
        let text = fold_cps(&q.text.chars().map(|c| c as u32).collect::<Vec<_>>());
        if text.contains(&0) {
            return BTreeSet::new();
        }
        let exts: Vec<Vec<u32>> = q
            .extensions
            .iter()
            .map(|e| {
                fold_cps(
                    &e.strip_prefix('.')
                        .unwrap_or(e)
                        .chars()
                        .map(|c| c as u32)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let mut out = BTreeSet::new();
        for (&frn, (_, name, attrs)) in &self.recs {
            let is_dir = FileAttributes(*attrs).contains(FileAttributes::DIRECTORY);
            match q.kind {
                KindFilter::Files if is_dir => continue,
                KindFilter::Dirs if !is_dir => continue,
                _ => {}
            }
            let folded = fold_cps(&code_points(name));
            if !exts.is_empty() {
                let dot = folded.iter().rposition(|&c| c == '.' as u32);
                let ext = match dot {
                    Some(d) if d + 1 < folded.len() => Some(&folded[d + 1..]),
                    _ => None,
                };
                let ok = exts.iter().any(|want| match ext {
                    None => want.is_empty(),
                    Some(have) => have == want.as_slice(),
                });
                if !ok {
                    continue;
                }
            }
            let hay = if q.match_path {
                let Ok(parts) = self.components(frn) else {
                    continue;
                };
                let mut hay = Vec::new();
                for p in parts {
                    hay.push('\\' as u32);
                    hay.extend(fold_cps(&code_points(p)));
                }
                hay
            } else {
                folded
            };
            let hit = match q.mode {
                MatchMode::Substring => contains(&hay, &text),
                MatchMode::Wildcard => wild(&text, &hay),
            };
            if hit {
                out.insert(frn);
            }
        }
        out
    }
}
