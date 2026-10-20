//! Search: case-insensitive substring and wildcard matching over names or full paths, with
//! extension and kind filters, as a parallel linear scan.
//!
//! - **Name mode, with a literal** (a substring, or the longest literal run of a wildcard): one
//!   SIMD `memmem` pass over the folded pool, split into chunks across threads. Each hit is mapped
//!   to its segment by binary search, the scan skips to the next segment, and stale segments
//!   (renamed/removed entries) are ignored.
//! - **Name mode, no literal** (`*`, `???`, filter-only queries): a scan over the entry columns.
//! - **Path mode**: the haystack is the folded path `\dir\…\name`. A per-query memo records, for
//!   each directory, whether its path already contains the needle; an entry matches if its
//!   parent's path does, or if the needle occurs in the last `len − 1` bytes of the parent's path
//!   plus `\name` — so no full path is built unless a wildcard has to be checked.
//!
//! Hits come back in a deterministic, unspecified order (ranking is the query layer's job);
//! `total` always counts every match even when `limit` truncates `hits`.

use std::ops::Range;
use std::sync::atomic::{AtomicU8, Ordering};

use memchr::memmem::Finder;
use rayon::prelude::*;
use whereabouts_core::FileAttributes;

use crate::fold;
use crate::index::{EntryId, Link, MAX_DEPTH, NO_EXT, NONE, NameIndex, SEPARATOR};

/// Minimum number of entries (or segments) per parallel chunk.
const MIN_CHUNK: usize = 16 * 1024;

/// How [`Query::text`] is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatchMode {
    /// The folded text occurs anywhere in the folded name (or path). Empty text matches all.
    #[default]
    Substring,
    /// The whole folded name (or path) matches the pattern: `*` = any run of characters, `?` =
    /// exactly one character. There is no escape for a literal `*`/`?` (NTFS names can't hold
    /// them).
    Wildcard,
}

/// Restricts results to files or directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindFilter {
    /// Files and directories.
    #[default]
    Any,
    /// Entries without `FILE_ATTRIBUTE_DIRECTORY`.
    Files,
    /// Entries with `FILE_ATTRIBUTE_DIRECTORY`.
    Dirs,
}

/// One search request. The query *syntax* (Q-11) is out of scope here: callers build this
/// structurally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Text or pattern, folded with [`crate::fold_str`] before matching. Text containing NUL
    /// matches nothing.
    pub text: String,
    /// How `text` is matched.
    pub mode: MatchMode,
    /// Match against the volume-relative path (`\dir\name`, `\` separators) instead of the
    /// name. Orphans and entries in parent cycles have no path, so they never match in this mode.
    pub match_path: bool,
    /// Keep only entries whose extension (text after the last `.`, case-insensitive) is one of
    /// these; a leading `.` is ignored and `""` selects names without an extension. Empty: any.
    pub extensions: Vec<String>,
    /// Files, directories or both.
    pub kind: KindFilter,
    /// Maximum number of hits returned (`total` still counts all).
    pub limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            text: String::new(),
            mode: MatchMode::Substring,
            match_path: false,
            extensions: Vec::new(),
            kind: KindFilter::Any,
            limit: usize::MAX,
        }
    }
}

impl Query {
    /// A substring query.
    #[must_use]
    pub fn substring(text: impl Into<String>) -> Self {
        Query {
            text: text.into(),
            ..Query::default()
        }
    }

    /// A wildcard query.
    #[must_use]
    pub fn wildcard(pattern: impl Into<String>) -> Self {
        Query {
            text: pattern.into(),
            mode: MatchMode::Wildcard,
            ..Query::default()
        }
    }

    /// Sets [`Query::match_path`].
    #[must_use]
    pub fn in_path(mut self, yes: bool) -> Self {
        self.match_path = yes;
        self
    }

    /// Sets [`Query::extensions`].
    #[must_use]
    pub fn extensions<I, S>(mut self, exts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extensions = exts.into_iter().map(Into::into).collect();
        self
    }

    /// Sets [`Query::kind`].
    #[must_use]
    pub fn kind(mut self, kind: KindFilter) -> Self {
        self.kind = kind;
        self
    }

    /// Sets [`Query::limit`].
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// Result of [`NameIndex::search`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    /// Up to `limit` matching entries (never the root).
    pub hits: Vec<EntryId>,
    /// Number of matching entries.
    pub total: usize,
}

enum Pat {
    Never,
    All,
    Substring(Vec<u8>),
    Wildcard { pat: Vec<u8>, literal: Vec<u8> },
}

struct Compiled {
    pat: Pat,
    exts: Vec<Vec<u8>>,
    kind: KindFilter,
    limit: usize,
}

fn compile(q: &Query) -> Compiled {
    let folded = fold::fold_str(&q.text).into_bytes();
    let pat = if folded.contains(&0) {
        Pat::Never
    } else {
        match q.mode {
            MatchMode::Substring if folded.is_empty() => Pat::All,
            MatchMode::Substring => Pat::Substring(folded),
            MatchMode::Wildcard => Pat::Wildcard {
                literal: fold::longest_literal(&folded).to_vec(),
                pat: folded,
            },
        }
    };
    let exts = q
        .extensions
        .iter()
        .map(|e| fold::fold_str(e.strip_prefix('.').unwrap_or(e)).into_bytes())
        .collect();
    Compiled {
        pat,
        exts,
        kind: q.kind,
        limit: q.limit,
    }
}

/// Matches found by one chunk.
struct Hits {
    ids: Vec<u32>,
    count: usize,
    limit: usize,
}

impl Hits {
    fn new(limit: usize) -> Self {
        Hits {
            ids: Vec::new(),
            count: 0,
            limit,
        }
    }

    fn push(&mut self, i: u32) {
        self.count += 1;
        if self.ids.len() < self.limit {
            self.ids.push(i);
        }
    }
}

// Path-mode memo states.
const UNKNOWN: u8 = 0;
const MATCH: u8 = 1;
const NO_MATCH: u8 = 2;
const UNREACHABLE: u8 = 3;

/// Per-query path-mode state shared by all chunks.
struct PathScan<'a> {
    /// Needle searched in path windows; `None` matches every reachable entry.
    finder: Option<Finder<'a>>,
    memo: Vec<AtomicU8>,
}

/// Per-thread scratch buffers for path mode.
#[derive(Default)]
struct PathBufs {
    chain: Vec<u32>,
    pieces: Vec<u32>,
    window: Vec<u8>,
}

impl NameIndex {
    /// Runs `query` over the live entries (never returns the root).
    #[must_use]
    pub fn search(&self, query: &Query) -> SearchResults {
        let c = compile(query);
        if matches!(c.pat, Pat::Never) {
            return SearchResults::default();
        }
        if query.match_path {
            return self.search_paths(&c);
        }
        match &c.pat {
            Pat::Never => SearchResults::default(),
            Pat::All => self.scan_entries(&c, |_| true),
            Pat::Substring(s) => self.scan_pool(&c, s, |_| true),
            Pat::Wildcard { pat, literal } if literal.is_empty() => {
                self.scan_entries(&c, |i| fold::wild_match(pat, self.folded_name(i as usize)))
            }
            Pat::Wildcard { pat, literal } => {
                self.scan_pool(&c, literal, |seg| fold::wild_match(pat, seg))
            }
        }
    }

    /// Kind and extension filters for live, non-root entry `i`.
    fn passes_filters(&self, c: &Compiled, i: usize) -> bool {
        let attrs = FileAttributes(self.attrs.get(i).copied().unwrap_or(0));
        let ok_kind = match c.kind {
            KindFilter::Any => true,
            KindFilter::Files => !attrs.contains(FileAttributes::DIRECTORY),
            KindFilter::Dirs => attrs.contains(FileAttributes::DIRECTORY),
        };
        if !ok_kind {
            return false;
        }
        if c.exts.is_empty() {
            return true;
        }
        let ext = match self.ext.get(i).copied().unwrap_or(NO_EXT) {
            NO_EXT => None,
            e => self.folded_name(i).get(usize::from(e)..),
        };
        c.exts.iter().any(|want| match ext {
            None => want.is_empty(),
            Some(have) => have == want.as_slice(),
        })
    }

    /// Scans the entry columns; `pred` gets live, non-root entries that pass the filters.
    fn scan_entries<P>(&self, c: &Compiled, pred: P) -> SearchResults
    where
        P: Fn(u32) -> bool + Sync,
    {
        let ranges = chunk_ranges(1, self.frn.len());
        run(&ranges, c.limit, |range, hits| {
            for i in range {
                if self.alive.get(i).copied().unwrap_or(false)
                    && self.passes_filters(c, i)
                    && pred(i as u32)
                {
                    hits.push(i as u32);
                }
            }
        })
    }

    /// `memmem` over the folded pool; `verify` gets the folded name of each live segment that
    /// contains `needle` and whose owner passes the filters.
    fn scan_pool<V>(&self, c: &Compiled, needle: &[u8], verify: V) -> SearchResults
    where
        V: Fn(&[u8]) -> bool + Sync,
    {
        let finder = Finder::new(needle);
        let segs = self.seg_start.len();
        let ranges = chunk_ranges(0, segs);
        run(&ranges, c.limit, |range, hits| {
            let (a, b) = (range.start, range.end);
            let Some(starts) = self.seg_start.get(a..b) else {
                return;
            };
            let Some(&first) = starts.first() else {
                return;
            };
            let end = self
                .seg_start
                .get(b)
                .map_or(self.folded.len(), |&s| s as usize);
            let mut pos = first as usize;
            while pos < end {
                let Some(hay) = self.folded.get(pos..end) else {
                    break;
                };
                let Some(off) = finder.find(hay) else {
                    break;
                };
                let hit = pos + off;
                let s = a + starts
                    .partition_point(|&x| x as usize <= hit)
                    .saturating_sub(1);
                let seg = self.segment(s);
                pos = self.seg_start.get(s).map_or(end, |&x| x as usize) + seg.len() + 1;
                let owner = self.seg_owner.get(s).copied().unwrap_or(NONE);
                if owner == NONE || owner == 0 {
                    continue;
                }
                if self.passes_filters(c, owner as usize) && verify(seg) {
                    hits.push(owner);
                }
            }
        })
    }

    fn search_paths(&self, c: &Compiled) -> SearchResults {
        let (needle, wildcard): (&[u8], Option<&[u8]>) = match &c.pat {
            Pat::Never => return SearchResults::default(),
            Pat::All => (&[], None),
            Pat::Substring(s) => (s, None),
            Pat::Wildcard { pat, literal } => (literal, Some(pat)),
        };
        let scan = PathScan {
            finder: (!needle.is_empty()).then(|| Finder::new(needle)),
            memo: (0..self.frn.len())
                .map(|_| AtomicU8::new(UNKNOWN))
                .collect(),
        };
        let ranges = chunk_ranges(1, self.frn.len());
        run(&ranges, c.limit, |range, hits| {
            let mut bufs = PathBufs::default();
            let mut full = Vec::new();
            for i in range {
                if !self.alive.get(i).copied().unwrap_or(false) || !self.passes_filters(c, i) {
                    continue;
                }
                if self.path_state(&scan, i as u32, &mut bufs) != MATCH {
                    continue;
                }
                if let Some(pat) = wildcard {
                    if !self.folded_path(i as u32, &mut full, &mut bufs.chain) {
                        continue;
                    }
                    if !fold::wild_match(pat, &full) {
                        continue;
                    }
                }
                hits.push(i as u32);
            }
        })
    }

    /// Memoised "does the folded path of `x` contain the needle" (`x` live, non-root).
    fn path_state(&self, scan: &PathScan<'_>, x: u32, bufs: &mut PathBufs) -> u8 {
        let state = |i: u32| {
            scan.memo
                .get(i as usize)
                .map_or(UNREACHABLE, |m| m.load(Ordering::Relaxed))
        };
        bufs.chain.clear();
        let mut cur = x;
        let base = loop {
            if cur == 0 {
                break NO_MATCH;
            }
            let s = state(cur);
            if s != UNKNOWN {
                break s;
            }
            if bufs.chain.len() >= MAX_DEPTH {
                break UNREACHABLE;
            }
            bufs.chain.push(cur);
            match self.link(cur) {
                Link::Entry(p) => cur = p,
                Link::Broken => break UNREACHABLE,
            }
        };
        let mut st = base;
        let chain = std::mem::take(&mut bufs.chain);
        for &e in chain.iter().rev() {
            st = match st {
                UNREACHABLE | MATCH => st,
                _ if self.window_matches(scan, e, bufs) => MATCH,
                _ => NO_MATCH,
            };
            if let Some(m) = scan.memo.get(e as usize) {
                m.store(st, Ordering::Relaxed);
            }
        }
        bufs.chain = chain;
        st
    }

    /// Whether the needle occurs in (last `len − 1` bytes of the parent's path) + `\` + name.
    fn window_matches(&self, scan: &PathScan<'_>, e: u32, bufs: &mut PathBufs) -> bool {
        let Some(finder) = &scan.finder else {
            return true;
        };
        let keep = finder.needle().len().saturating_sub(1);
        bufs.window.clear();
        if let Link::Entry(p) = self.link(e) {
            self.push_path_suffix(p, keep, &mut bufs.window, &mut bufs.pieces);
        }
        bufs.window.push(SEPARATOR);
        bufs.window.extend_from_slice(self.folded_name(e as usize));
        finder.find(&bufs.window).is_some()
    }

    /// Appends the last `keep` bytes of the folded path of `p` to `out`.
    fn push_path_suffix(&self, p: u32, keep: usize, out: &mut Vec<u8>, pieces: &mut Vec<u32>) {
        pieces.clear();
        let mut need = keep;
        let mut cur = p;
        while need > 0 && cur != 0 && pieces.len() < MAX_DEPTH {
            pieces.push(cur);
            let len = self.folded_name(cur as usize).len() + 1;
            if len >= need {
                break;
            }
            need -= len;
            match self.link(cur) {
                Link::Entry(q) => cur = q,
                Link::Broken => break,
            }
        }
        let start = out.len();
        for &q in pieces.iter().rev() {
            out.push(SEPARATOR);
            out.extend_from_slice(self.folded_name(q as usize));
        }
        let total = out.len() - start;
        if total > keep {
            out.drain(start..start + (total - keep));
        }
    }

    /// Builds the full folded path of `e` into `out`; `false` for orphans and cycles.
    fn folded_path(&self, e: u32, out: &mut Vec<u8>, chain: &mut Vec<u32>) -> bool {
        chain.clear();
        let mut cur = e;
        while cur != 0 {
            if chain.len() >= MAX_DEPTH {
                return false;
            }
            chain.push(cur);
            match self.link(cur) {
                Link::Entry(p) => cur = p,
                Link::Broken => return false,
            }
        }
        out.clear();
        for &q in chain.iter().rev() {
            out.push(SEPARATOR);
            out.extend_from_slice(self.folded_name(q as usize));
        }
        true
    }
}

/// Splits `lo..hi` into roughly `4 × threads` chunks of at least [`MIN_CHUNK`].
fn chunk_ranges(lo: usize, hi: usize) -> Vec<Range<usize>> {
    let n = hi.saturating_sub(lo);
    if n == 0 {
        return Vec::new();
    }
    let threads = rayon::current_num_threads().max(1);
    let size = n.div_ceil(threads * 4).max(MIN_CHUNK);
    (lo..hi)
        .step_by(size)
        .map(|s| s..(s + size).min(hi))
        .collect()
}

/// Runs `scan` over each chunk (in parallel when there is more than one) and merges the hits in
/// chunk order, so results are deterministic.
fn run<F>(ranges: &[Range<usize>], limit: usize, scan: F) -> SearchResults
where
    F: Fn(Range<usize>, &mut Hits) + Sync,
{
    let one = |r: &Range<usize>| {
        let mut hits = Hits::new(limit);
        scan(r.clone(), &mut hits);
        hits
    };
    let parts: Vec<Hits> = if ranges.len() <= 1 {
        ranges.iter().map(one).collect()
    } else {
        ranges.par_iter().map(one).collect()
    };
    let total: usize = parts.iter().map(|h| h.count).sum();
    let mut hits = Vec::with_capacity(total.min(limit));
    for part in parts {
        let room = limit - hits.len();
        hits.extend(part.ids.into_iter().take(room).map(EntryId));
    }
    SearchResults { hits, total }
}
