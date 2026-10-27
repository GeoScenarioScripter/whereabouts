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

use std::io;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use crate::mft::{
    ATTR_ATTRIBUTE_LIST, ATTR_BITMAP, ATTR_DATA, ATTR_FILE_NAME, ATTR_STANDARD_INFORMATION,
    AttrBody, AttrListEntries, Attrs, MAX_LOADED_STREAM, MAX_RECORDS, MftParseError, NAMESPACE_DOS,
    NAMESPACE_POSIX, Run, StandardInfo, VolumeGeometry, apply_fixups, decode_runlist,
    parse_file_name, parse_record_header, parse_standard_info, reference_record_number,
    runs_clusters,
};
use crate::name::NtfsName;
use crate::record::{FILE_ATTRIBUTE_DIRECTORY, RawRecord};

/// Largest `$ATTRIBUTE_LIST` loaded into memory (a file would need ~1M named streams or links).
const MAX_ATTRIBUTE_LIST: u64 = 16 << 20;
/// Most extension records read for the `$FILE_NAME` attributes of one file.
const MAX_EXTENSIONS_PER_FILE: usize = 64;
/// Most extension records that may extend `$MFT`'s own `$DATA`.
const MAX_MFT_EXTENSIONS: usize = 1024;

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
}

impl Default for MftScanOptions {
    fn default() -> Self {
        Self {
            chunk_bytes: 4 << 20,
            skip_reserved: false,
            list_hard_links: false,
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
    /// Slots without the `FILE` signature (zeroed, `BAAD`, ...) that the bitmap marked in use.
    pub not_file: u64,
    /// Extension records, reached through their base record instead.
    pub extension: u64,
    /// Records whose update sequence did not match (torn writes); each is a missing entry.
    pub torn: u64,
    /// Records with a malformed header, attribute chain or name; each is a missing entry.
    pub malformed: u64,
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
    /// Records that exist on the volume but could not be reported: torn, malformed, no usable
    /// name. A non-zero value means the result is incomplete.
    #[must_use]
    pub fn lost(&self) -> u64 {
        self.torn + self.malformed + self.no_name
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
        let data_bytes = real_size.min(initialized_size).min(covered);
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

    /// Reads the whole `$MFT` and passes every in-use base record to `sink` (in record order).
    /// The sink may return [`ControlFlow::Break`] to stop early.
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
        let mut ext_buf = vec![0_u8; rec_size];
        let mut scratch = Scratch::default();
        let mut stats = MftScanStats::default();

        let mut first = 0_u64;
        while first < self.record_count {
            let n = per_chunk.min(self.record_count - first);
            if !self.bitmap.is_empty() && (first..first + n).all(|r| !self.marked_in_use(r)) {
                stats.chunks_skipped += 1;
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
            stats.read_time += started.elapsed();
            stats.reads += 1;
            stats.bytes += bytes as u64;

            for (i, rec) in buf[..bytes].chunks_exact_mut(rec_size).enumerate() {
                let number = first + i as u64;
                if !self.marked_in_use(number) {
                    stats.unused += 1;
                    continue;
                }
                if options.skip_reserved && number < 16 && number != 5 {
                    continue;
                }
                stats.examined += 1;
                if let Some(raw) = self.process(number, rec, &mut ext_buf, &mut scratch, &mut stats)
                {
                    stats.yielded += 1;
                    if options.list_hard_links && scratch.links > 1 {
                        stats.hard_link_frns.push(raw.frn);
                    }
                    if sink(raw).is_break() {
                        stats.cancelled = true;
                        return Ok(stats);
                    }
                }
            }
            first += n;
        }
        Ok(stats)
    }

    /// Turns one record slot into a [`RawRecord`], or counts why not.
    fn process(
        &self,
        number: u64,
        rec: &mut [u8],
        ext_buf: &mut [u8],
        scratch: &mut Scratch,
        stats: &mut MftScanStats,
    ) -> Option<RawRecord> {
        match apply_fixups(rec) {
            Ok(()) => {}
            Err(MftParseError::NotFileRecord) => {
                stats.not_file += 1;
                return None;
            }
            Err(MftParseError::TornWrite { .. }) => {
                stats.torn += 1;
                tracing::debug!(record = number, "torn $MFT record skipped");
                return None;
            }
            Err(e) => {
                stats.malformed += 1;
                tracing::debug!(record = number, error = %e, "malformed $MFT record skipped");
                return None;
            }
        }
        let header = match parse_record_header(rec) {
            Ok(h) => h,
            Err(e) => {
                stats.malformed += 1;
                tracing::debug!(record = number, error = %e, "malformed $MFT record skipped");
                return None;
            }
        };
        if !header.in_use() {
            stats.unused += 1;
            return None;
        }
        if header.is_extension() {
            stats.extension += 1;
            return None;
        }

        scratch.reset();
        let mut info: Option<StandardInfo> = None;
        for attr in Attrs::new(rec, &header) {
            let attr = match attr {
                Ok(a) => a,
                Err(e) => {
                    stats.malformed += 1;
                    tracing::debug!(record = number, error = %e, "malformed attribute chain");
                    return None;
                }
            };
            match (attr.type_code, attr.body) {
                (ATTR_STANDARD_INFORMATION, AttrBody::Resident { value }) if attr.is_unnamed() => {
                    info = parse_standard_info(value).ok();
                }
                (ATTR_FILE_NAME, AttrBody::Resident { value }) => {
                    if let Ok(name) = parse_file_name(value) {
                        scratch.consider(name.parent, name.namespace, name.name);
                    }
                }
                (ATTR_ATTRIBUTE_LIST, body) => {
                    scratch.list = Stream::from_body(body, self.geom.total_clusters).ok();
                }
                _ => {}
            }
        }

        if let Some(list) = scratch.list.take() {
            self.names_from_extensions(number, &list, ext_buf, scratch, stats);
        }

        if scratch.links > 1 {
            stats.hard_link_files += 1;
        }
        if !scratch.has_name {
            stats.no_name += 1;
            return None;
        }
        let mut attributes = info.map_or(0, |i| i.attributes);
        if header.is_directory() {
            attributes |= FILE_ATTRIBUTE_DIRECTORY;
        }
        Some(RawRecord {
            frn: u64::from(header.sequence) << 48 | number,
            parent_frn: scratch.parent,
            file_attributes: attributes,
            usn: info.map_or(0, |i| i.usn),
            timestamp: info.map_or(0, |i| i.modified as i64),
            name: NtfsName::from_utf16le_bytes(&scratch.name),
        })
    }

    /// Reads the extension records that an `$ATTRIBUTE_LIST` places `$FILE_NAME` attributes in.
    fn names_from_extensions(
        &self,
        number: u64,
        list: &Stream,
        ext_buf: &mut [u8],
        scratch: &mut Scratch,
        stats: &mut MftScanStats,
    ) {
        let rec_size = ext_buf.len() as u64;
        let list = match load_stream(
            self.src,
            &self.geom,
            list,
            MAX_ATTRIBUTE_LIST,
            "$ATTRIBUTE_LIST",
        ) {
            Ok(l) => l,
            Err(e) => {
                stats.extension_errors += 1;
                tracing::debug!(record = number, error = %e, "unreadable $ATTRIBUTE_LIST");
                return;
            }
        };
        let mut wanted: Vec<u64> = Vec::new();
        for entry in AttrListEntries::new(&list) {
            let Ok(entry) = entry else {
                stats.extension_errors += 1;
                break;
            };
            let record = reference_record_number(entry.segment_reference);
            if entry.type_code == ATTR_FILE_NAME
                && record != number
                && record < self.record_count
                && !wanted.contains(&record)
            {
                if wanted.len() >= MAX_EXTENSIONS_PER_FILE {
                    stats.extension_errors += 1;
                    break;
                }
                wanted.push(record);
            }
        }
        for record in wanted {
            let started = Instant::now();
            let read = read_runs(self.src, &self.geom, &self.runs, record * rec_size, ext_buf);
            stats.read_time += started.elapsed();
            stats.reads += 1;
            stats.bytes += rec_size;
            stats.extension_reads += 1;
            let usable = read.is_ok()
                && apply_fixups(ext_buf).is_ok()
                && parse_record_header(ext_buf).is_ok_and(|h| {
                    h.in_use() && reference_record_number(h.base_reference) == number
                });
            let Some(header) = usable
                .then(|| parse_record_header(ext_buf))
                .and_then(Result::ok)
            else {
                stats.extension_errors += 1;
                continue;
            };
            for attr in Attrs::new(ext_buf, &header) {
                let Ok(attr) = attr else {
                    stats.extension_errors += 1;
                    break;
                };
                if let (ATTR_FILE_NAME, AttrBody::Resident { value }) = (attr.type_code, attr.body)
                    && let Ok(name) = parse_file_name(value)
                {
                    scratch.consider(name.parent, name.namespace, name.name);
                }
            }
        }
    }
}

/// Per-record working state, reused across records to avoid allocation.
#[derive(Debug, Default)]
struct Scratch {
    has_name: bool,
    /// Rank of the chosen name: 0 = Win32 / Win32+DOS, 1 = POSIX.
    rank: u8,
    parent: u64,
    name: Vec<u8>,
    /// Non-DOS `$FILE_NAME` attributes seen.
    links: u32,
    list: Option<Stream>,
}

impl Scratch {
    fn reset(&mut self) {
        self.has_name = false;
        self.rank = u8::MAX;
        self.parent = 0;
        self.name.clear();
        self.links = 0;
        self.list = None;
    }

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
