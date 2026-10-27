//! Raw `$MFT` scan: enumerate a volume by reading its master file table in large sequential
//! chunks instead of asking `FSCTL_ENUM_USN_DATA` (stage01 session 2.7, Q-16).
//!
//! The scan is generic over a [`ByteSource`] (the raw volume on Windows, an in-memory image in
//! tests), so everything except the Windows volume handle runs and is tested on every platform.
//!
//! How it works:
//!
//! 1. [`MftScanner::open`] reads `$MFT`'s own record (record 0, at `MftStartLcn`), undoes its
//!    fixups and decodes its `$DATA` runlist (following an `$ATTRIBUTE_LIST` into extension
//!    records when the `$MFT` itself is fragmented) and its `$BITMAP`.
//! 2. [`MftScanner::scan`] reads the `$MFT` stream in multi-MiB chunks, skips chunks the bitmap
//!    says are entirely free, undoes the fixups of each record, and turns every in-use *base*
//!    record into a [`RawRecord`]: the same type `FSCTL_ENUM_USN_DATA` yields.
//!
//! Choices that matter for equivalence with the USN API:
//!
//! - **One name per file record.** The first `$FILE_NAME` in the Win32 or Win32+DOS namespace
//!   wins, else the first POSIX one. DOS-only (8.3) names are never reported. Hard links beyond
//!   the chosen one are counted ([`MftScanStats::hard_link_files`]) but not reported, as with the
//!   USN API (documented M0 limitation).
//! - **`$ATTRIBUTE_LIST` is followed** only to find `$FILE_NAME` attributes that live in
//!   extension records; those are read with one extra random read each.
//! - `file_attributes` comes from `$STANDARD_INFORMATION` (plus the directory flag of the record
//!   header); `usn` is its last-USN field and `timestamp` its last-write time. The last two have
//!   a different meaning than in a USN record (change time), so equivalence is defined on FRN,
//!   parent FRN, name and the directory bit.
//!
//! Safety against hostile bytes: see [`crate::mft`]. In addition, all memory is sized from the
//! validated geometry, from [`MftScanOptions`] clamps, or from limits in [`crate::mft`], never
//! from a length read out of an attribute without a limit.

use std::collections::HashMap;
use std::io;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use crate::mft::{
    ATTR_ATTRIBUTE_LIST, ATTR_BITMAP, ATTR_DATA, ATTR_FILE_NAME, ATTR_STANDARD_INFORMATION,
    AttrBody, AttrListEntries, Attrs, MAX_LOADED_STREAM, MAX_RECORDS, MftParseError, NAMESPACE_DOS,
    NAMESPACE_POSIX, RecordHeader, Run, StandardInfo, VolumeGeometry, apply_fixups, decode_runlist,
    parse_file_name, parse_record_header, parse_standard_info, reference_record_number,
    runs_clusters,
};
use crate::name::NtfsName;
use crate::record::{FILE_ATTRIBUTE_DIRECTORY, RawRecord};

/// Largest `$ATTRIBUTE_LIST` loaded into memory (1 MiB holds about 30,000 entries).
const MAX_ATTRIBUTE_LIST: u64 = 1 << 20;
/// Most extension records read for the `$FILE_NAME` attributes of one file.
const MAX_EXTENSIONS_PER_FILE: usize = 4096;
/// Most extension records that may extend `$MFT`'s own `$DATA`.
const MAX_MFT_EXTENSIONS: usize = 1024;
/// Most bytes of files held back for extension records further down the table; beyond it a file
/// is finished at once with direct reads.
const MAX_PENDING_BYTES: usize = 64 << 20;
/// Most torn record numbers remembered for the retry (more stay counted as lost).
const MAX_TORN_RETRY: usize = 1 << 16;

/// Random-access byte reads from a volume (or a test image).
pub trait ByteSource {
    /// Fills `buf` with the bytes at `offset`; a short read is an error.
    ///
    /// # Errors
    /// Any I/O error, or [`io::ErrorKind::UnexpectedEof`] past the end.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

impl ByteSource for [u8] {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let eof = || io::Error::from(io::ErrorKind::UnexpectedEof);
        let start = usize::try_from(offset).map_err(|_| eof())?;
        let end = start.checked_add(buf.len()).ok_or_else(eof)?;
        buf.copy_from_slice(self.get(start..end).ok_or_else(eof)?);
        Ok(())
    }
}

impl ByteSource for Vec<u8> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.as_slice().read_at(offset, buf)
    }
}

/// Failure of an `$MFT` scan.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MftError {
    /// The `$MFT`'s own structures are malformed (per-record problems are counted instead).
    #[error("malformed $MFT: {0}")]
    Parse(#[from] MftParseError),
    /// Reading the volume failed.
    #[error("reading the volume failed: {0}")]
    Io(#[from] io::Error),
}

/// Tuning for [`MftScanner::scan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MftScanOptions {
    /// Bytes per sequential read, clamped to 64 KiB ..= 64 MiB and rounded down to a multiple of
    /// the cluster and record size. Default 4 MiB.
    pub chunk_bytes: usize,
    /// Skip the reserved system records 0 ..= 15 (`$MFT`, `$Bitmap`, ... `$Extend`) other than
    /// the root directory (record 5). Default `false`.
    pub skip_reserved: bool,
    /// Collect the FRNs of hard-linked files in [`MftScanStats::hard_link_frns`] (for
    /// comparisons; 8 bytes per such file). Default `false`.
    pub list_hard_links: bool,
    /// Most bytes of extension-record names remembered during the sequential pass (the rest are
    /// read again on demand with one random read each). Default 128 MiB.
    pub ext_cache_bytes: usize,
}

impl Default for MftScanOptions {
    fn default() -> Self {
        Self {
            chunk_bytes: 4 << 20,
            skip_reserved: false,
            list_hard_links: false,
            ext_cache_bytes: 128 << 20,
        }
    }
}

/// Counters from one scan.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MftScanStats {
    /// Record slots read and examined (free chunks skipped by the bitmap are not counted).
    pub examined: u64,
    /// Records handed to the sink.
    pub yielded: u64,
    /// Slots that are not in use (bitmap clear or header flag clear).
    pub unused: u64,
    /// Examined slots without the `FILE` signature (zeroed, `BAAD`, ...), whether or not the
    /// bitmap vouches for them.
    pub not_file: u64,
    /// The part of `not_file` that the bitmap marks in use (where the bitmap covers the slot):
    /// each is a missing entry.
    pub not_file_in_use: u64,
    /// Extension records, reached through their base record instead.
    pub extension: u64,
    /// Records still torn (update sequence mismatch) after the retry; each is a missing entry.
    pub torn: u64,
    /// Torn records read a second time at the end of the pass.
    pub torn_retried: u64,
    /// Of those, how many were whole the second time.
    pub torn_recovered: u64,
    /// Records or attributes with a malformed header, attribute chain, `$FILE_NAME` or
    /// `$STANDARD_INFORMATION`; each is a missing entry or a damaged one.
    pub malformed: u64,
    /// `$MFT` slots beyond its initialized size (not read; can be legitimate).
    pub uninitialized_slots: u64,
    /// Files held back for a later extension record (diagnostic).
    pub deferred: u64,
    /// In-use base records without a reportable name (DOS-only or none).
    pub no_name: u64,
    /// Files with more than one non-DOS `$FILE_NAME` (hard links). One name is reported.
    pub hard_link_files: u64,
    /// FRNs of the hard-linked files yielded, when [`MftScanOptions::list_hard_links`] is set.
    pub hard_link_frns: Vec<u64>,
    /// Extension records read through an `$ATTRIBUTE_LIST`.
    pub extension_reads: u64,
    /// Extension records that could not be used (unreadable or inconsistent).
    pub extension_errors: u64,
    /// Chunks skipped because the bitmap marks all their records free.
    pub chunks_skipped: u64,
    /// Read calls issued (chunks plus random extension reads).
    pub reads: u64,
    /// Bytes read.
    pub bytes: u64,
    /// Time spent in [`ByteSource::read_at`].
    pub read_time: Duration,
    /// `true` when the sink stopped the scan early.
    pub cancelled: bool,
}

impl MftScanStats {
    /// Records or names that exist on the volume but could not be reported: torn after the
    /// retry, malformed, in use per the bitmap but not a `FILE` record, or unusable extension
    /// records (including the per-file cap and `$ATTRIBUTE_LIST` problems). A non-zero value
    /// means the result is incomplete. `no_name` is **not** included: a record without a
    /// reportable name (DOS-only) is legitimate and is not reported by the USN API either.
    #[must_use]
    pub fn lost(&self) -> u64 {
        self.torn + self.malformed + self.not_file_in_use + self.extension_errors
    }
}

/// A stream stored in the record (resident) or on disk (non-resident).
#[derive(Debug)]
enum Stream {
    Resident(Vec<u8>),
    NonResident { runs: Vec<Run>, real_size: u64 },
}

impl Stream {
    fn from_body(body: AttrBody<'_>, total_clusters: u64) -> Result<Self, MftParseError> {
        match body {
            AttrBody::Resident { value } => Ok(Self::Resident(value.to_vec())),
            AttrBody::NonResident {
                start_vcn,
                real_size,
                runlist,
                ..
            } => {
                if start_vcn != 0 {
                    return Err(MftParseError::malformed(
                        "stream",
                        "first extent does not start at VCN 0",
                    ));
                }
                let mut runs = Vec::new();
                decode_runlist(runlist, 0, total_clusters, &mut runs)?;
                Ok(Self::NonResident { runs, real_size })
            }
        }
    }
}

/// Reads `buf.len()` bytes at stream offset `offset` of a non-resident stream.
fn read_runs<S: ByteSource + ?Sized>(
    src: &S,
    geom: &VolumeGeometry,
    runs: &[Run],
    offset: u64,
    buf: &mut [u8],
) -> Result<(), MftError> {
    let cluster = u64::from(geom.bytes_per_cluster);
    let total = buf.len() as u64;
    if offset.checked_add(total).is_none() {
        return Err(MftParseError::malformed("stream read", "offset overflow").into());
    }
    let mut done = 0_usize;
    while done < buf.len() {
        let pos = offset + done as u64;
        let (vcn, within) = (pos / cluster, pos % cluster);
        let idx = runs.partition_point(|r| r.vcn.saturating_add(r.len) <= vcn);
        let run = runs
            .get(idx)
            .filter(|r| r.vcn <= vcn)
            .ok_or(MftParseError::malformed(
                "stream read",
                "beyond the runlist",
            ))?;
        let into_run = (vcn - run.vcn)
            .saturating_mul(cluster)
            .saturating_add(within);
        let remaining = run
            .len
            .saturating_mul(cluster)
            .checked_sub(into_run)
            .filter(|r| *r > 0)
            .ok_or(MftParseError::malformed(
                "stream read",
                "bad run arithmetic",
            ))?;
        let n = remaining.min((buf.len() - done) as u64) as usize;
        let out = &mut buf[done..done + n];
        match run.lcn {
            Some(lcn) => src.read_at(lcn.saturating_mul(cluster).saturating_add(into_run), out)?,
            None => out.fill(0), // sparse
        }
        done += n;
    }
    Ok(())
}

fn load_stream<S: ByteSource + ?Sized>(
    src: &S,
    geom: &VolumeGeometry,
    stream: &Stream,
    limit: u64,
    what: &'static str,
) -> Result<Vec<u8>, MftError> {
    match stream {
        Stream::Resident(v) => Ok(v.clone()),
        Stream::NonResident { runs, real_size } => {
            if *real_size > limit {
                return Err(MftParseError::Limit { what, limit }.into());
            }
            let covered = runs_clusters(runs).saturating_mul(u64::from(geom.bytes_per_cluster));
            if *real_size > covered {
                return Err(MftParseError::malformed(what, "size exceeds its runlist").into());
            }
            let mut out = vec![0_u8; *real_size as usize];
            read_runs(src, geom, runs, 0, &mut out)?;
            Ok(out)
        }
    }
}

/// One `$DATA` attribute record of `$MFT`: (start VCN, `(real, initialized)` size if it is the
/// first extent, runlist bytes).
type Piece = (u64, Option<(u64, u64)>, Vec<u8>);

/// An opened `$MFT`, ready to scan.
#[derive(Debug)]
pub struct MftScanner<'s, S: ByteSource + ?Sized> {
    src: &'s S,
    geom: VolumeGeometry,
    runs: Vec<Run>,
    record_count: u64,
    uninitialized_slots: u64,
    bitmap: Vec<u8>,
}

impl<'s, S: ByteSource + ?Sized> MftScanner<'s, S> {
    /// Reads `$MFT`'s own record and prepares the scan.
    ///
    /// # Errors
    /// [`MftError`] if record 0 is unreadable or malformed, its runlist is inconsistent, or a
    /// limit is exceeded.
    pub fn open(src: &'s S, geom: VolumeGeometry) -> Result<Self, MftError> {
        let rec_size = geom.bytes_per_record as usize;
        let mut rec = vec![0_u8; rec_size];
        src.read_at(geom.mft_start_byte(), &mut rec)?;
        apply_fixups(&mut rec)?;
        let header = parse_record_header(&rec)?;
        if !header.in_use() || header.is_extension() {
            return Err(
                MftParseError::malformed("$MFT record", "not an in-use base record").into(),
            );
        }

        // (start VCN, size if this is the first extent, runlist bytes)
        let mut pieces: Vec<Piece> = Vec::new();
        let mut bitmap_stream = None;
        let mut list_stream = None;
        for attr in Attrs::new(&rec, &header) {
            let attr = attr?;
            if !attr.is_unnamed() && attr.type_code != ATTR_ATTRIBUTE_LIST {
                continue;
            }
            match attr.type_code {
                ATTR_DATA => match attr.body {
                    AttrBody::NonResident {
                        start_vcn,
                        real_size,
                        initialized_size,
                        runlist,
                    } => pieces.push((
                        start_vcn,
                        (start_vcn == 0).then_some((real_size, initialized_size)),
                        runlist.to_vec(),
                    )),
                    AttrBody::Resident { .. } => {
                        return Err(MftParseError::malformed("$MFT", "$DATA is resident").into());
                    }
                },
                ATTR_BITMAP => {
                    bitmap_stream = Some(Stream::from_body(attr.body, geom.total_clusters)?);
                }
                ATTR_ATTRIBUTE_LIST => {
                    list_stream = Some(Stream::from_body(attr.body, geom.total_clusters)?);
                }
                _ => {}
            }
        }

        // Decode the pieces held in record 0, then follow the attribute list to the extension
        // records that hold further extents of $DATA (a heavily fragmented $MFT). Each extension
        // record is located through the extents decoded so far.
        pieces.sort_by_key(|p| p.0);
        let mut runs = Vec::new();
        let mut sizes = None;
        let mut next_vcn = 0_u64;
        for (start_vcn, size, runlist) in &pieces {
            if *start_vcn != next_vcn {
                return Err(MftParseError::malformed("$MFT $DATA", "gap in the runlist").into());
            }
            if size.is_some() {
                sizes = *size;
            }
            next_vcn += decode_runlist(runlist, *start_vcn, geom.total_clusters, &mut runs)?;
        }
        let (real_size, initialized_size) = sizes.ok_or(MftParseError::malformed(
            "$MFT",
            "no $DATA starting at VCN 0",
        ))?;

        if let Some(list) = &list_stream {
            let list = load_stream(src, &geom, list, MAX_ATTRIBUTE_LIST, "$MFT $ATTRIBUTE_LIST")?;
            let mut extents: Vec<(u64, u64)> = Vec::new(); // (start VCN, record number)
            for entry in AttrListEntries::new(&list) {
                let entry = entry?;
                let record = reference_record_number(entry.segment_reference);
                if entry.type_code == ATTR_DATA && !entry.named && record != 0 {
                    if extents.len() >= MAX_MFT_EXTENSIONS {
                        return Err(MftParseError::Limit {
                            what: "$MFT extension records",
                            limit: MAX_MFT_EXTENSIONS as u64,
                        }
                        .into());
                    }
                    extents.push((entry.start_vcn, record));
                }
            }
            extents.sort_unstable();
            extents.dedup();
            for (start_vcn, record) in extents {
                if start_vcn != next_vcn {
                    return Err(MftParseError::malformed("$MFT $DATA", "gap in the runlist").into());
                }
                let mut ext = vec![0_u8; rec_size];
                read_runs(
                    src,
                    &geom,
                    &runs,
                    record.saturating_mul(rec_size as u64),
                    &mut ext,
                )?;
                apply_fixups(&mut ext)?;
                let ext_header = parse_record_header(&ext)?;
                if !ext_header.in_use() || reference_record_number(ext_header.base_reference) != 0 {
                    return Err(MftParseError::malformed(
                        "$MFT extension record",
                        "does not belong to $MFT",
                    )
                    .into());
                }
                let mut found = false;
                for attr in Attrs::new(&ext, &ext_header) {
                    let attr = attr?;
                    if let (
                        ATTR_DATA,
                        true,
                        AttrBody::NonResident {
                            start_vcn: s,
                            runlist,
                            ..
                        },
                    ) = (attr.type_code, attr.is_unnamed(), attr.body)
                        && s == start_vcn
                    {
                        next_vcn +=
                            decode_runlist(runlist, start_vcn, geom.total_clusters, &mut runs)?;
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(MftParseError::malformed(
                        "$MFT extension record",
                        "extent not found",
                    )
                    .into());
                }
            }
        }

        let covered = next_vcn.saturating_mul(u64::from(geom.bytes_per_cluster));
        // A runlist that stops short of the declared size means part of the table is missing.
        if covered < real_size {
            return Err(MftParseError::malformed(
                "$MFT $DATA",
                "runlist covers less than the declared size",
            )
            .into());
        }
        let data_bytes = real_size.min(initialized_size);
        let uninitialized_slots = (real_size - data_bytes) / u64::from(geom.bytes_per_record);
        if uninitialized_slots > 0 {
            tracing::warn!(
                uninitialized_slots,
                "$MFT is larger than its initialized size; the tail is not scanned"
            );
        }
        let record_count = data_bytes / u64::from(geom.bytes_per_record);
        if record_count == 0 {
            return Err(MftParseError::malformed("$MFT", "holds no records").into());
        }
        if record_count > MAX_RECORDS {
            return Err(MftParseError::Limit {
                what: "$MFT records",
                limit: MAX_RECORDS,
            }
            .into());
        }
        // Records past the end of a short bitmap count as in use (the record header flag
        // decides), so only the bytes that exist are loaded.
        let bitmap = match bitmap_stream {
            Some(Stream::NonResident { runs: b, real_size }) => {
                let wanted = real_size.min(record_count.div_ceil(8));
                let covered = runs_clusters(&b)
                    .saturating_mul(u64::from(geom.bytes_per_cluster))
                    .min(wanted);
                if covered > MAX_LOADED_STREAM {
                    return Err(MftParseError::Limit {
                        what: "$MFT $BITMAP",
                        limit: MAX_LOADED_STREAM,
                    }
                    .into());
                }
                let mut out = vec![0_u8; covered as usize];
                read_runs(src, &geom, &b, 0, &mut out)?;
                out
            }
            Some(Stream::Resident(v)) => v,
            None => Vec::new(),
        };
        Ok(Self {
            src,
            geom,
            runs,
            record_count,
            uninitialized_slots,
            bitmap,
        })
    }

    /// Number of record slots in `$MFT` (in use or not).
    #[must_use]
    pub fn record_count(&self) -> u64 {
        self.record_count
    }

    /// The geometry the scanner was opened with.
    #[must_use]
    pub fn geometry(&self) -> &VolumeGeometry {
        &self.geom
    }

    fn marked_in_use(&self, number: u64) -> bool {
        // A missing bitmap byte means "unknown": let the record header decide.
        self.bitmap
            .get((number / 8) as usize)
            .is_none_or(|b| b >> (number % 8) & 1 == 1)
    }

    /// `true` when the bitmap says something about this slot (so "not in use" and "in use" are
    /// known facts rather than defaults).
    fn bitmap_covers(&self, number: u64) -> bool {
        ((number / 8) as usize) < self.bitmap.len()
    }

    /// Reads the whole `$MFT` and passes every in-use base record to `sink`. The sink may return
    /// [`ControlFlow::Break`] to stop early.
    ///
    /// Torn records (a write in flight while the disk was read) are read once more at the end of
    /// the pass. The caller must check [`MftScanStats::lost`]: a non-zero value means records
    /// are missing and the result must not be used as a complete index. An `Err` or a non-zero
    /// `lost()` comes *after* the sink has received records; the caller discards them.
    ///
    /// Records come in `$MFT` order, except files whose `$FILE_NAME` attributes live in
    /// extension records that come *later* in the table: those are held back and delivered after
    /// the sequential pass, when their extension records have been seen (no random reads).
    ///
    /// Per-record problems (torn writes, malformed attributes) are counted in the returned
    /// stats and logged at `debug`; they do not fail the scan.
    ///
    /// # Errors
    /// [`MftError::Io`] if a read fails, or a parse error from the `$MFT` runlist.
    pub fn scan(
        &self,
        options: &MftScanOptions,
        sink: &mut dyn FnMut(RawRecord) -> ControlFlow<()>,
    ) -> Result<MftScanStats, MftError> {
        let rec_size = self.geom.bytes_per_record as usize;
        let unit = rec_size.max(self.geom.bytes_per_cluster as usize);
        let chunk = options.chunk_bytes.clamp(64 << 10, 64 << 20);
        let chunk = (chunk / unit).max(1) * unit;
        let per_chunk = (chunk / rec_size) as u64;
        let mut buf = vec![0_u8; chunk.min(self.record_count as usize * rec_size)];
        let mut ctx = Ctx {
            ext_buf: vec![0_u8; rec_size],
            scratch: Scratch::default(),
            ext_names: HashMap::new(),
            ext_bytes: 0,
            ext_cache_limit: options.ext_cache_bytes,
            skip_extend_children: options.skip_reserved,
            torn_list: Vec::new(),
            pending_bytes: 0,
            stats: MftScanStats {
                uninitialized_slots: self.uninitialized_slots,
                ..MftScanStats::default()
            },
        };
        let mut pending: Vec<Pending> = Vec::new();

        let mut first = 0_u64;
        while first < self.record_count {
            let n = per_chunk.min(self.record_count - first);
            if !self.bitmap.is_empty() && (first..first + n).all(|r| !self.marked_in_use(r)) {
                ctx.stats.chunks_skipped += 1;
                first += n;
                continue;
            }
            let bytes = n as usize * rec_size;
            let started = Instant::now();
            read_runs(
                self.src,
                &self.geom,
                &self.runs,
                first * rec_size as u64,
                &mut buf[..bytes],
            )?;
            ctx.stats.read_time += started.elapsed();
            ctx.stats.reads += 1;
            ctx.stats.bytes += bytes as u64;

            for (i, rec) in buf[..bytes].chunks_exact_mut(rec_size).enumerate() {
                let number = first + i as u64;
                if !self.marked_in_use(number) {
                    ctx.stats.unused += 1;
                    continue;
                }
                if options.skip_reserved && number < 16 && number != 5 {
                    continue;
                }
                ctx.stats.examined += 1;
                match self.process(number, rec, &mut ctx) {
                    Outcome::Skip => {}
                    Outcome::Done(raw, multi) => {
                        if emit(options, sink, &mut ctx.stats, raw, multi).is_break() {
                            return Ok(ctx.stats);
                        }
                    }
                    Outcome::Defer(p) => pending.push(p),
                }
            }
            first += n;
        }

        // Torn records: a write was in flight when the disk was read. Read each once more.
        let torn_list = std::mem::take(&mut ctx.torn_list);
        let mut rec = vec![0_u8; rec_size];
        for number in torn_list {
            ctx.stats.torn_retried += 1;
            ctx.stats.torn -= 1; // counted again by `process` if it is still torn
            let started = Instant::now();
            let read = read_runs(
                self.src,
                &self.geom,
                &self.runs,
                number * rec_size as u64,
                &mut rec,
            );
            ctx.stats.read_time += started.elapsed();
            ctx.stats.reads += 1;
            ctx.stats.bytes += rec_size as u64;
            read?;
            let before = ctx.stats.torn;
            match self.process(number, &mut rec, &mut ctx) {
                Outcome::Skip => {}
                Outcome::Done(raw, multi) => {
                    if emit(options, sink, &mut ctx.stats, raw, multi).is_break() {
                        return Ok(ctx.stats);
                    }
                }
                Outcome::Defer(p) => pending.push(p),
            }
            if ctx.stats.torn == before {
                ctx.stats.torn_recovered += 1;
            }
            // A second tear is final: do not retry again.
            ctx.torn_list.clear();
        }

        // Files whose names are partly in extension records that came after them.
        for p in pending {
            let mut choice = p.choice;
            for ext in p.wanted {
                self.apply_extension(
                    p.number,
                    ext,
                    &ctx.ext_names,
                    &mut ctx.ext_buf,
                    &mut choice,
                    &mut ctx.stats,
                );
            }
            if let Some((raw, multi)) = finalize(
                p.number,
                p.sequence,
                p.directory,
                p.info,
                &choice,
                ctx.skip_extend_children,
                &mut ctx.stats,
            ) && emit(options, sink, &mut ctx.stats, raw, multi).is_break()
            {
                break;
            }
        }
        Ok(ctx.stats)
    }

    /// Turns one record slot into a [`RawRecord`], or counts why not.
    fn process(&self, number: u64, rec: &mut [u8], ctx: &mut Ctx) -> Outcome {
        match apply_fixups(rec) {
            Ok(()) => {}
            Err(MftParseError::NotFileRecord) => {
                ctx.stats.not_file += 1;
                if self.bitmap_covers(number) {
                    // The bitmap vouches that this slot holds a file.
                    ctx.stats.not_file_in_use += 1;
                }
                return Outcome::Skip;
            }
            Err(MftParseError::TornWrite { .. }) => {
                ctx.stats.torn += 1;
                if ctx.torn_list.len() < MAX_TORN_RETRY {
                    ctx.torn_list.push(number);
                }
                tracing::debug!(record = number, "torn $MFT record skipped");
                return Outcome::Skip;
            }
            Err(e) => {
                ctx.stats.malformed += 1;
                tracing::debug!(record = number, error = %e, "malformed $MFT record skipped");
                return Outcome::Skip;
            }
        }
        let header = match parse_record_header(rec) {
            Ok(h) => h,
            Err(e) => {
                ctx.stats.malformed += 1;
                tracing::debug!(record = number, error = %e, "malformed $MFT record skipped");
                return Outcome::Skip;
            }
        };
        if !header.in_use() {
            ctx.stats.unused += 1;
            return Outcome::Skip;
        }
        if header.is_extension() {
            ctx.stats.extension += 1;
            ctx.remember_extension(number, rec, &header);
            return Outcome::Skip;
        }

        ctx.scratch.reset();
        let mut info: Option<StandardInfo> = None;
        for attr in Attrs::new(rec, &header) {
            let attr = match attr {
                Ok(a) => a,
                Err(e) => {
                    ctx.stats.malformed += 1;
                    tracing::debug!(record = number, error = %e, "malformed attribute chain");
                    return Outcome::Skip;
                }
            };
            match (attr.type_code, attr.body) {
                (ATTR_STANDARD_INFORMATION, AttrBody::Resident { value }) if attr.is_unnamed() => {
                    info = parse_standard_info(value).ok();
                    if info.is_none() {
                        // Keep the record (attributes default to none), but do not hide it.
                        ctx.stats.malformed += 1;
                        tracing::debug!(record = number, "malformed $STANDARD_INFORMATION");
                    }
                }
                (ATTR_FILE_NAME, AttrBody::Resident { value }) => match parse_file_name(value) {
                    Ok(name) => {
                        ctx.scratch
                            .choice
                            .consider(name.parent, name.namespace, name.name);
                    }
                    Err(e) => {
                        ctx.stats.malformed += 1;
                        tracing::debug!(record = number, error = %e, "malformed $FILE_NAME");
                    }
                },
                // A second extent of a non-resident list must not replace the first (the first
                // extent's size covers the whole list; `extensions_wanted` reports it as an
                // error if the runs fall short).
                (ATTR_ATTRIBUTE_LIST, body) if ctx.scratch.list.is_none() => {
                    match Stream::from_body(body, self.geom.total_clusters) {
                        Ok(list) => ctx.scratch.list = Some(list),
                        Err(e) => {
                            ctx.stats.extension_errors += 1;
                            tracing::debug!(record = number, error = %e, "bad $ATTRIBUTE_LIST");
                        }
                    }
                }
                _ => {}
            }
        }

        let wanted = match ctx.scratch.list.take() {
            Some(list) => self.extensions_wanted(number, &list, &mut ctx.stats),
            None => Vec::new(),
        };
        // An extension record further down the table has not been read yet: hold this file back.
        // (Bounded: past the budget the file is finished at once, with direct reads.)
        let cost = 128 + ctx.scratch.choice.name.len() + 8 * wanted.len();
        if wanted.iter().any(|&ext| ext > number) && ctx.pending_bytes + cost <= MAX_PENDING_BYTES {
            ctx.pending_bytes += cost;
            ctx.stats.deferred += 1;
            return Outcome::Defer(Pending {
                number,
                sequence: header.sequence,
                directory: header.is_directory(),
                info,
                choice: std::mem::take(&mut ctx.scratch.choice),
                wanted,
            });
        }
        for ext in wanted {
            self.apply_extension(
                number,
                ext,
                &ctx.ext_names,
                &mut ctx.ext_buf,
                &mut ctx.scratch.choice,
                &mut ctx.stats,
            );
        }
        match finalize(
            number,
            header.sequence,
            header.is_directory(),
            info,
            &ctx.scratch.choice,
            ctx.skip_extend_children,
            &mut ctx.stats,
        ) {
            Some((raw, multi)) => Outcome::Done(raw, multi),
            None => Outcome::Skip,
        }
    }

    /// The extension records an `$ATTRIBUTE_LIST` places `$FILE_NAME` attributes in.
    fn extensions_wanted(&self, number: u64, list: &Stream, stats: &mut MftScanStats) -> Vec<u64> {
        if let Stream::NonResident { real_size, .. } = list {
            stats.reads += 1;
            stats.bytes += real_size;
        }
        let started = Instant::now();
        let loaded = load_stream(
            self.src,
            &self.geom,
            list,
            MAX_ATTRIBUTE_LIST,
            "$ATTRIBUTE_LIST",
        );
        stats.read_time += started.elapsed();
        let list = match loaded {
            Ok(l) => l,
            Err(e) => {
                stats.extension_errors += 1;
                tracing::debug!(record = number, error = %e, "unreadable $ATTRIBUTE_LIST");
                return Vec::new();
            }
        };
        let mut wanted: Vec<u64> = Vec::new();
        for entry in AttrListEntries::new(&list) {
            let Ok(entry) = entry else {
                stats.extension_errors += 1;
                break;
            };
            let record = reference_record_number(entry.segment_reference);
            if entry.type_code == ATTR_FILE_NAME && record != number && record >= self.record_count
            {
                // A name that lives outside the table we scanned: it cannot be recovered.
                stats.extension_errors += 1;
                continue;
            }
            if entry.type_code == ATTR_FILE_NAME && record != number && !wanted.contains(&record) {
                if wanted.len() >= MAX_EXTENSIONS_PER_FILE {
                    stats.extension_errors += 1;
                    break;
                }
                wanted.push(record);
            }
        }
        wanted
    }

    /// Adds the `$FILE_NAME` attributes of extension record `ext` (of file `base`) to `choice`:
    /// from the names remembered during the sequential pass, else with one random read.
    fn apply_extension(
        &self,
        base: u64,
        ext: u64,
        ext_names: &HashMap<u64, ExtNames>,
        ext_buf: &mut [u8],
        choice: &mut NameChoice,
        stats: &mut MftScanStats,
    ) {
        if let Some(known) = ext_names.get(&ext) {
            if known.base == base {
                for (parent, namespace, name) in &known.names {
                    choice.consider(*parent, *namespace, name);
                }
            } else {
                stats.extension_errors += 1; // claims another base record
            }
            return;
        }
        let rec_size = ext_buf.len() as u64;
        let started = Instant::now();
        let read = read_runs(self.src, &self.geom, &self.runs, ext * rec_size, ext_buf);
        stats.read_time += started.elapsed();
        stats.reads += 1;
        stats.bytes += rec_size;
        stats.extension_reads += 1;
        let header = read
            .ok()
            .and_then(|()| apply_fixups(ext_buf).ok())
            .and_then(|()| parse_record_header(ext_buf).ok())
            .filter(|h| h.in_use() && reference_record_number(h.base_reference) == base);
        let Some(header) = header else {
            stats.extension_errors += 1;
            return;
        };
        for attr in Attrs::new(ext_buf, &header) {
            let Ok(attr) = attr else {
                stats.extension_errors += 1;
                break;
            };
            if let (ATTR_FILE_NAME, AttrBody::Resident { value }) = (attr.type_code, attr.body) {
                match parse_file_name(value) {
                    Ok(name) => choice.consider(name.parent, name.namespace, name.name),
                    Err(_) => stats.extension_errors += 1,
                }
            }
        }
    }
}

/// The `$FILE_NAME` attributes of one extension record, remembered while scanning past it.
#[derive(Debug)]
struct ExtNames {
    /// Record number of the base file the extension record belongs to.
    base: u64,
    /// `(parent, namespace, name UTF-16LE)` of the non-DOS names.
    names: Vec<(u64, u8, Vec<u8>)>,
}

/// A file held back until the extension records after it have been scanned.
#[derive(Debug)]
struct Pending {
    number: u64,
    sequence: u16,
    directory: bool,
    info: Option<StandardInfo>,
    choice: NameChoice,
    wanted: Vec<u64>,
}

enum Outcome {
    Skip,
    /// The record, and whether the file has several hard links.
    Done(RawRecord, bool),
    Defer(Pending),
}

/// State of one scan.
struct Ctx {
    ext_buf: Vec<u8>,
    scratch: Scratch,
    ext_names: HashMap<u64, ExtNames>,
    ext_bytes: usize,
    ext_cache_limit: usize,
    skip_extend_children: bool,
    /// Record numbers of torn records, for the retry at the end of the pass.
    torn_list: Vec<u64>,
    /// Approximate bytes held by deferred files.
    pending_bytes: usize,
    stats: MftScanStats,
}

impl Ctx {
    /// Remembers the names in extension record `number` so its base file need not read it again.
    fn remember_extension(&mut self, number: u64, rec: &[u8], header: &RecordHeader) {
        if self.ext_bytes >= self.ext_cache_limit {
            return;
        }
        let mut names = Vec::new();
        let mut bytes = 64;
        for attr in Attrs::new(rec, header) {
            let Ok(attr) = attr else {
                return; // malformed: let the base file's on-demand read count it
            };
            if let (ATTR_FILE_NAME, AttrBody::Resident { value }) = (attr.type_code, attr.body) {
                let Ok(name) = parse_file_name(value) else {
                    return; // malformed: let the base file's on-demand read count it
                };
                if name.namespace != NAMESPACE_DOS {
                    bytes += name.name.len() + 48;
                    names.push((name.parent, name.namespace, name.name.to_vec()));
                }
            }
        }
        self.ext_bytes += bytes;
        self.ext_names.insert(
            number,
            ExtNames {
                base: reference_record_number(header.base_reference),
                names,
            },
        );
    }
}

/// Hands a record to the sink and updates the counters. `Break` means the sink cancelled.
fn emit(
    options: &MftScanOptions,
    sink: &mut dyn FnMut(RawRecord) -> ControlFlow<()>,
    stats: &mut MftScanStats,
    raw: RawRecord,
    multi_link: bool,
) -> ControlFlow<()> {
    stats.yielded += 1;
    if options.list_hard_links && multi_link {
        stats.hard_link_frns.push(raw.frn);
    }
    let flow = sink(raw);
    if flow.is_break() {
        stats.cancelled = true;
    }
    flow
}

/// The attribute bits NTFS stores in `$STANDARD_INFORMATION` beyond the Win32 ones (directory
/// index flags in the top nibble, and reparse/recall bits) are not part of what the USN API
/// reports; only the low 16 bits are kept.
const ATTRIBUTE_MASK: u32 = 0x0000_FFFF;

/// `FILE_ATTRIBUTE_NORMAL`.
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// Builds the record for a file whose names are all known, or counts why not.
fn finalize(
    number: u64,
    sequence: u16,
    directory: bool,
    info: Option<StandardInfo>,
    choice: &NameChoice,
    skip_extend_children: bool,
    stats: &mut MftScanStats,
) -> Option<(RawRecord, bool)> {
    if choice.links > 1 {
        stats.hard_link_files += 1;
    }
    if !choice.has_name {
        stats.no_name += 1;
        return None;
    }
    // The USN API leaves out the metadata files under `$Extend` (record 11).
    if skip_extend_children && reference_record_number(choice.parent) == 11 {
        return None;
    }
    let mut attributes = info.map_or(0, |i| i.attributes) & ATTRIBUTE_MASK;
    if directory {
        attributes |= FILE_ATTRIBUTE_DIRECTORY;
    }
    // The Win32 view of "no attribute bits" is FILE_ATTRIBUTE_NORMAL, which the USN API reports.
    if attributes == 0 {
        attributes = FILE_ATTRIBUTE_NORMAL;
    }
    Some((
        RawRecord {
            frn: u64::from(sequence) << 48 | number,
            parent_frn: choice.parent,
            file_attributes: attributes,
            usn: info.map_or(0, |i| i.usn),
            timestamp: info.map_or(0, |i| i.modified as i64),
            name: NtfsName::from_utf16le_bytes(&choice.name),
        },
        choice.links > 1,
    ))
}

/// The name chosen so far for one file.
#[derive(Debug, Default)]
struct NameChoice {
    has_name: bool,
    /// Rank of the chosen name: 0 = Win32 / Win32+DOS, 1 = POSIX.
    rank: u8,
    parent: u64,
    name: Vec<u8>,
    /// Non-DOS `$FILE_NAME` attributes seen.
    links: u32,
}

impl NameChoice {
    /// Considers one `$FILE_NAME`: DOS-only names are ignored; Win32 beats POSIX; the first of
    /// equal rank wins.
    fn consider(&mut self, parent: u64, namespace: u8, name: &[u8]) {
        if namespace == NAMESPACE_DOS {
            return;
        }
        self.links += 1;
        let rank = u8::from(namespace == NAMESPACE_POSIX);
        if !self.has_name || rank < self.rank {
            self.has_name = true;
            self.rank = rank;
            self.parent = parent;
            self.name.clear();
            self.name.extend_from_slice(name);
        }
    }
}

/// Per-record working state, reused across records to avoid allocation.
#[derive(Debug, Default)]
struct Scratch {
    choice: NameChoice,
    list: Option<Stream>,
}

impl Scratch {
    fn reset(&mut self) {
        self.choice.has_name = false;
        self.choice.rank = u8::MAX;
        self.choice.parent = 0;
        self.choice.name.clear();
        self.choice.links = 0;
        self.list = None;
    }
}
