//! Pure parsers for the on-disk structures of the NTFS master file table (`$MFT`).
//!
//! Everything here works on plain byte slices, so it is unit-tested on every platform from
//! hand-built fixtures (`crate::mft_build`, test-only). The `$MFT` bytes come straight off a disk and are
//! **untrusted**: every length, offset and count is validated before use, nothing is allocated
//! from a size the data declares (only from a count bounded by the buffer being parsed or by a
//! constant here), and malformed input becomes a [`MftParseError`], never a panic.
//!
//! Layouts (all little-endian):
//!
//! | Structure | Notes |
//! |---|---|
//! | `NTFS_VOLUME_DATA_BUFFER` | 96 bytes, output of `FSCTL_GET_NTFS_VOLUME_DATA` ([`parse_volume_data`]) |
//! | `FILE` record | header, update sequence array, attributes ([`apply_fixups`], [`parse_record_header`]) |
//! | attribute | resident or non-resident header, then value or runlist ([`Attrs`]) |
//! | runlist | variable-length (length, signed LCN delta) pairs ([`decode_runlist`]) |
//! | `$STANDARD_INFORMATION` 0x10 | times, attributes, USN ([`parse_standard_info`]) |
//! | `$FILE_NAME` 0x30 | parent reference, namespace, UTF-16 name ([`parse_file_name`]) |
//! | `$ATTRIBUTE_LIST` 0x20 | where each attribute lives ([`AttrListEntries`]) |

/// `$STANDARD_INFORMATION`.
pub const ATTR_STANDARD_INFORMATION: u32 = 0x10;
/// `$ATTRIBUTE_LIST`.
pub const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
/// `$FILE_NAME`.
pub const ATTR_FILE_NAME: u32 = 0x30;
/// `$DATA`.
pub const ATTR_DATA: u32 = 0x80;
/// `$BITMAP`.
pub const ATTR_BITMAP: u32 = 0xB0;
/// End-of-attributes marker.
pub const ATTR_END: u32 = 0xFFFF_FFFF;

/// `$FILE_NAME` namespace: POSIX (any characters, case-sensitive).
pub const NAMESPACE_POSIX: u8 = 0;
/// `$FILE_NAME` namespace: Win32.
pub const NAMESPACE_WIN32: u8 = 1;
/// `$FILE_NAME` namespace: DOS 8.3 only.
pub const NAMESPACE_DOS: u8 = 2;
/// `$FILE_NAME` namespace: one name valid as both Win32 and DOS.
pub const NAMESPACE_WIN32_AND_DOS: u8 = 3;

/// `FILE` record header flag: the record is in use.
pub const RECORD_IN_USE: u16 = 0x0001;
/// `FILE` record header flag: the record is a directory.
pub const RECORD_IS_DIRECTORY: u16 = 0x0002;

/// Bytes covered by one update-sequence slot. Fixed at 512 by the NTFS format, whatever the
/// device's sector size.
pub const FIXUP_STRIDE: usize = 512;

/// Size of `NTFS_VOLUME_DATA_BUFFER` (the part we read).
pub const NTFS_VOLUME_DATA_SIZE: usize = 96;

/// Most runs accepted for one stream (16 bytes each once decoded, so 16 MiB). A real `$MFT` has
/// at most a few thousand; the cap stops runlist bombs.
pub const MAX_RUNS: usize = 1 << 20;

/// Largest in-use bitmap or attribute list we load into memory (256 MiB covers a `$MFT` of
/// about 2 billion records; an attribute list is far smaller in practice).
pub const MAX_LOADED_STREAM: u64 = 256 << 20;

/// Most `FILE` records a volume may declare (record numbers are 48-bit; 2^32 records is 4 TiB of
/// `$MFT` at 1 KiB each).
pub const MAX_RECORDS: u64 = 1 << 32;

/// A `$MFT` structure did not have the documented layout.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MftParseError {
    /// A structure runs past the end of its buffer.
    #[error("{what} is truncated")]
    Truncated {
        /// The structure.
        what: &'static str,
    },
    /// The record does not start with the `FILE` signature (unused slots are often zeroed;
    /// `BAAD` marks a record chkdsk found damaged).
    #[error("not a FILE record")]
    NotFileRecord,
    /// The last two bytes of a 512-byte sector do not match the update sequence number: the
    /// record was only partly written (or the disk returned garbage).
    #[error("update sequence mismatch in sector {sector} (torn write)")]
    TornWrite {
        /// Zero-based 512-byte sector within the record.
        sector: usize,
    },
    /// A field has a value the format forbids.
    #[error("malformed {what}: {why}")]
    Malformed {
        /// The structure.
        what: &'static str,
        /// What is wrong.
        why: &'static str,
    },
    /// A count or size exceeds this reader's safety limit.
    #[error("{what} exceeds the limit of {limit}")]
    Limit {
        /// What was too large.
        what: &'static str,
        /// The limit.
        limit: u64,
    },
}

impl MftParseError {
    pub(crate) const fn truncated(what: &'static str) -> Self {
        Self::Truncated { what }
    }
    pub(crate) const fn malformed(what: &'static str, why: &'static str) -> Self {
        Self::Malformed { what, why }
    }
}

type Result<T> = std::result::Result<T, MftParseError>;

// ---- little-endian readers (bounds-checked; never panic) -------------------------------------

fn bytes_at<const N: usize>(b: &[u8], offset: usize) -> Option<[u8; N]> {
    b.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

pub(crate) fn u16_at(b: &[u8], offset: usize) -> Option<u16> {
    bytes_at(b, offset).map(u16::from_le_bytes)
}

pub(crate) fn u32_at(b: &[u8], offset: usize) -> Option<u32> {
    bytes_at(b, offset).map(u32::from_le_bytes)
}

pub(crate) fn u64_at(b: &[u8], offset: usize) -> Option<u64> {
    bytes_at(b, offset).map(u64::from_le_bytes)
}

// ---- volume geometry -------------------------------------------------------------------------

/// What `FSCTL_GET_NTFS_VOLUME_DATA` says about the volume, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeGeometry {
    /// Volume serial number.
    pub serial_number: u64,
    /// Bytes per sector (power of two, 256 ..= 4096).
    pub bytes_per_sector: u32,
    /// Bytes per cluster (power of two, at least a sector, at most 64 MiB).
    pub bytes_per_cluster: u32,
    /// Bytes per `FILE` record (power of two, 512 ..= 64 KiB).
    pub bytes_per_record: u32,
    /// Clusters on the volume.
    pub total_clusters: u64,
    /// `$MFT`'s valid data length in bytes, as the file system reports it.
    pub mft_valid_data_length: u64,
    /// First cluster of `$MFT`.
    pub mft_start_lcn: u64,
}

impl VolumeGeometry {
    /// Size of the volume in bytes (validated not to overflow).
    #[must_use]
    pub fn volume_bytes(&self) -> u64 {
        self.total_clusters
            .saturating_mul(u64::from(self.bytes_per_cluster))
    }

    /// Byte offset of `$MFT`'s first record on the volume.
    #[must_use]
    pub fn mft_start_byte(&self) -> u64 {
        self.mft_start_lcn
            .saturating_mul(u64::from(self.bytes_per_cluster))
    }
}

/// Parses `NTFS_VOLUME_DATA_BUFFER` (the reply to `FSCTL_GET_NTFS_VOLUME_DATA`; trailing
/// `NTFS_EXTENDED_VOLUME_DATA` is ignored).
///
/// # Errors
/// [`MftParseError`] when the reply is short or the geometry is impossible.
pub fn parse_volume_data(buf: &[u8]) -> Result<VolumeGeometry> {
    const WHAT: &str = "NTFS volume data";
    if buf.len() < NTFS_VOLUME_DATA_SIZE {
        return Err(MftParseError::truncated(WHAT));
    }
    let rd32 = |o| u32_at(buf, o).ok_or(MftParseError::truncated(WHAT));
    let rd64 = |o| u64_at(buf, o).ok_or(MftParseError::truncated(WHAT));
    let geometry = VolumeGeometry {
        serial_number: rd64(0)?,
        total_clusters: rd64(16)?,
        bytes_per_sector: rd32(40)?,
        bytes_per_cluster: rd32(44)?,
        bytes_per_record: rd32(48)?,
        mft_valid_data_length: rd64(56)?,
        mft_start_lcn: rd64(64)?,
    };
    let pow2 = |v: u32, lo: u32, hi: u32| v.is_power_of_two() && (lo..=hi).contains(&v);
    if !pow2(geometry.bytes_per_sector, 256, 4096) {
        return Err(MftParseError::malformed(WHAT, "bad bytes per sector"));
    }
    if !pow2(
        geometry.bytes_per_cluster,
        geometry.bytes_per_sector,
        64 << 20,
    ) {
        return Err(MftParseError::malformed(WHAT, "bad bytes per cluster"));
    }
    if !pow2(geometry.bytes_per_record, 512, 64 << 10) {
        return Err(MftParseError::malformed(WHAT, "bad bytes per file record"));
    }
    if geometry.total_clusters == 0
        || geometry
            .total_clusters
            .checked_mul(u64::from(geometry.bytes_per_cluster))
            .is_none()
    {
        return Err(MftParseError::malformed(WHAT, "bad cluster count"));
    }
    if geometry.mft_start_lcn >= geometry.total_clusters {
        return Err(MftParseError::malformed(
            WHAT,
            "$MFT starts outside the volume",
        ));
    }
    Ok(geometry)
}

// ---- FILE record: fixups and header ----------------------------------------------------------

/// Validates the `FILE` signature and update sequence array of one record and undoes the
/// fixups in place: each 512-byte sector's last two bytes were replaced by the update sequence
/// number when the record was written; the originals are stored in the array.
///
/// `rec` must be exactly one record (a multiple of 512 bytes).
///
/// # Errors
/// [`MftParseError::NotFileRecord`], [`MftParseError::TornWrite`] or a malformed array. On error
/// `rec` may be partly modified and must be discarded.
pub fn apply_fixups(rec: &mut [u8]) -> Result<()> {
    const WHAT: &str = "FILE record";
    if rec.len() < FIXUP_STRIDE || !rec.len().is_multiple_of(FIXUP_STRIDE) {
        return Err(MftParseError::malformed(
            WHAT,
            "length is not a multiple of 512",
        ));
    }
    if rec.get(..4) != Some(b"FILE") {
        return Err(MftParseError::NotFileRecord);
    }
    let usa_offset = usize::from(u16_at(rec, 4).ok_or(MftParseError::truncated(WHAT))?);
    let usa_count = usize::from(u16_at(rec, 6).ok_or(MftParseError::truncated(WHAT))?);
    let sectors = rec.len() / FIXUP_STRIDE;
    if usa_count != sectors + 1 {
        return Err(MftParseError::malformed(
            WHAT,
            "update sequence count does not match size",
        ));
    }
    // The array (1 + sectors u16 values) must sit after the fixed header and inside the first
    // sector's data, before that sector's own fixup slot.
    if usa_offset < 0x2A || usa_offset + usa_count * 2 > FIXUP_STRIDE - 2 {
        return Err(MftParseError::malformed(
            WHAT,
            "update sequence array out of bounds",
        ));
    }
    let usn: [u8; 2] = bytes_at(rec, usa_offset).ok_or(MftParseError::truncated(WHAT))?;
    for sector in 1..=sectors {
        let slot = sector * FIXUP_STRIDE - 2;
        let saved: [u8; 2] =
            bytes_at(rec, usa_offset + sector * 2).ok_or(MftParseError::truncated(WHAT))?;
        let at: [u8; 2] = bytes_at(rec, slot).ok_or(MftParseError::truncated(WHAT))?;
        if at != usn {
            return Err(MftParseError::TornWrite { sector: sector - 1 });
        }
        rec[slot..slot + 2].copy_from_slice(&saved);
    }
    Ok(())
}

/// The fixed part of a `FILE` record header, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHeader {
    /// Sequence number (incremented each time the record is reused).
    pub sequence: u16,
    /// Hard-link count as recorded in the header.
    pub link_count: u16,
    /// Offset of the first attribute.
    pub first_attribute: u16,
    /// `RECORD_*` flags.
    pub flags: u16,
    /// Bytes of the record in use (attributes end here).
    pub used_size: u32,
    /// Base record reference (0 for a base record; otherwise this is an extension record).
    pub base_reference: u64,
}

impl RecordHeader {
    /// `true` when the in-use flag is set.
    #[must_use]
    pub fn in_use(&self) -> bool {
        self.flags & RECORD_IN_USE != 0
    }

    /// `true` when the directory flag is set.
    #[must_use]
    pub fn is_directory(&self) -> bool {
        self.flags & RECORD_IS_DIRECTORY != 0
    }

    /// `true` for an extension record (it belongs to another base record).
    #[must_use]
    pub fn is_extension(&self) -> bool {
        self.base_reference != 0
    }
}

/// Parses the header of a record whose fixups have been applied.
///
/// # Errors
/// [`MftParseError`] when the header does not fit the record or points outside it.
pub fn parse_record_header(rec: &[u8]) -> Result<RecordHeader> {
    const WHAT: &str = "FILE record header";
    let rd16 = |o| u16_at(rec, o).ok_or(MftParseError::truncated(WHAT));
    let rd32 = |o| u32_at(rec, o).ok_or(MftParseError::truncated(WHAT));
    let header = RecordHeader {
        sequence: rd16(0x10)?,
        link_count: rd16(0x12)?,
        first_attribute: rd16(0x14)?,
        flags: rd16(0x16)?,
        used_size: rd32(0x18)?,
        base_reference: u64_at(rec, 0x20).ok_or(MftParseError::truncated(WHAT))?,
    };
    let used = header.used_size as usize;
    if used > rec.len() {
        return Err(MftParseError::malformed(
            WHAT,
            "used size exceeds the record",
        ));
    }
    let first = usize::from(header.first_attribute);
    if first < 0x30 || !first.is_multiple_of(8) || first >= used {
        return Err(MftParseError::malformed(
            WHAT,
            "first attribute offset out of bounds",
        ));
    }
    Ok(header)
}

// ---- attributes ------------------------------------------------------------------------------

/// One attribute of a `FILE` record, borrowed from the record's bytes.
#[derive(Debug, Clone, Copy)]
pub struct Attr<'a> {
    /// Attribute type code (`ATTR_*`).
    pub type_code: u32,
    /// The attribute's name as UTF-16LE bytes; empty for the unnamed attribute.
    pub name: &'a [u8],
    /// Where the content lives.
    pub body: AttrBody<'a>,
}

impl Attr<'_> {
    /// `true` for the unnamed attribute (`$DATA` without a stream name, ...).
    #[must_use]
    pub fn is_unnamed(&self) -> bool {
        self.name.is_empty()
    }
}

/// An attribute's content.
#[derive(Debug, Clone, Copy)]
pub enum AttrBody<'a> {
    /// The value is stored in the record.
    Resident {
        /// The value bytes.
        value: &'a [u8],
    },
    /// The value is stored in clusters described by a runlist.
    NonResident {
        /// First VCN this attribute record covers (non-zero for continuation extents).
        start_vcn: u64,
        /// Size in bytes of the whole stream (meaningful in the first extent only).
        real_size: u64,
        /// Bytes initialised on disk.
        initialized_size: u64,
        /// The encoded runlist ([`decode_runlist`]).
        runlist: &'a [u8],
    },
}

/// Iterator over the attributes of a record. Yields one `Err` and then stops if the attribute
/// chain is malformed.
#[derive(Debug, Clone)]
pub struct Attrs<'a> {
    rec: &'a [u8],
    pos: usize,
    end: usize,
    done: bool,
}

impl<'a> Attrs<'a> {
    /// Walks the attributes of `rec` (fixups applied) as described by `header`.
    #[must_use]
    pub fn new(rec: &'a [u8], header: &RecordHeader) -> Self {
        Self {
            rec,
            pos: usize::from(header.first_attribute),
            end: (header.used_size as usize).min(rec.len()),
            done: false,
        }
    }

    fn parse_one(&self) -> Result<Option<(Attr<'a>, usize)>> {
        const WHAT: &str = "attribute";
        let pos = self.pos;
        let type_code = u32_at(self.rec, pos).ok_or(MftParseError::truncated(WHAT))?;
        if type_code == ATTR_END {
            return Ok(None);
        }
        let len = u32_at(self.rec, pos + 4).ok_or(MftParseError::truncated(WHAT))? as usize;
        if len < 0x18 || !len.is_multiple_of(8) || len > self.end - pos {
            return Err(MftParseError::malformed(WHAT, "bad attribute length"));
        }
        let attr = &self.rec[pos..pos + len];
        let non_resident = attr[8] != 0;
        let name_len = usize::from(attr[9]) * 2;
        let name_off = usize::from(u16_at(attr, 10).ok_or(MftParseError::truncated(WHAT))?);
        let name = if name_len == 0 {
            &attr[..0]
        } else {
            if name_off < 0x10 || name_off.saturating_add(name_len) > len {
                return Err(MftParseError::malformed(WHAT, "name out of bounds"));
            }
            &attr[name_off..name_off + name_len]
        };
        let body = if non_resident {
            if len < 0x40 {
                return Err(MftParseError::malformed(
                    WHAT,
                    "non-resident header too short",
                ));
            }
            let start_vcn = u64_at(attr, 0x10).ok_or(MftParseError::truncated(WHAT))?;
            let runs_off = usize::from(u16_at(attr, 0x20).ok_or(MftParseError::truncated(WHAT))?);
            if runs_off < 0x40 || runs_off > len {
                return Err(MftParseError::malformed(
                    WHAT,
                    "runlist offset out of bounds",
                ));
            }
            AttrBody::NonResident {
                start_vcn,
                real_size: u64_at(attr, 0x30).ok_or(MftParseError::truncated(WHAT))?,
                initialized_size: u64_at(attr, 0x38).ok_or(MftParseError::truncated(WHAT))?,
                runlist: &attr[runs_off..],
            }
        } else {
            let value_len = u32_at(attr, 0x10).ok_or(MftParseError::truncated(WHAT))? as usize;
            let value_off = usize::from(u16_at(attr, 0x14).ok_or(MftParseError::truncated(WHAT))?);
            if value_off < 0x18 || value_off.saturating_add(value_len) > len {
                return Err(MftParseError::malformed(
                    WHAT,
                    "resident value out of bounds",
                ));
            }
            AttrBody::Resident {
                value: &attr[value_off..value_off + value_len],
            }
        };
        Ok(Some((
            Attr {
                type_code,
                name,
                body,
            },
            len,
        )))
    }
}

impl<'a> Iterator for Attrs<'a> {
    type Item = Result<Attr<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // Fewer than a type code left: the chain ended without a terminator.
        if self.pos + 4 > self.end {
            self.done = true;
            return None;
        }
        match self.parse_one() {
            Ok(Some((attr, len))) => {
                self.pos += len; // len >= 0x18, so this always makes progress
                Some(Ok(attr))
            }
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

// ---- runlists --------------------------------------------------------------------------------

/// One extent of a non-resident stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// First virtual cluster number of the extent.
    pub vcn: u64,
    /// Length in clusters (never 0).
    pub len: u64,
    /// First logical cluster on the volume, or `None` for a sparse (all-zero) extent.
    pub lcn: Option<u64>,
}

/// Decodes a runlist, appending to `out`, and returns the clusters added. `start_vcn` is the VCN
/// of the first run; `total_clusters` bounds every LCN; `out` is never grown past [`MAX_RUNS`].
///
/// A runlist that simply ends at the end of `bytes` (no terminating zero byte) is accepted.
///
/// # Errors
/// [`MftParseError`] for oversized fields, zero-length runs, negative or out-of-volume LCNs,
/// arithmetic overflow or too many runs.
pub fn decode_runlist(
    bytes: &[u8],
    start_vcn: u64,
    total_clusters: u64,
    out: &mut Vec<Run>,
) -> Result<u64> {
    const WHAT: &str = "runlist";
    let mut pos = 0_usize;
    let mut vcn = start_vcn;
    let mut lcn = 0_i64;
    let mut added = 0_u64;
    while let Some(&header) = bytes.get(pos) {
        if header == 0 {
            break;
        }
        let len_size = usize::from(header & 0x0F);
        let off_size = usize::from(header >> 4);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return Err(MftParseError::malformed(WHAT, "bad field sizes"));
        }
        let fields = bytes
            .get(pos + 1..pos + 1 + len_size + off_size)
            .ok_or(MftParseError::truncated(WHAT))?;
        pos += 1 + len_size + off_size;
        let (len_bytes, off_bytes) = fields.split_at(len_size);
        let mut len = 0_u64;
        for (i, &b) in len_bytes.iter().enumerate() {
            len |= u64::from(b) << (8 * i);
        }
        if len == 0 {
            return Err(MftParseError::malformed(WHAT, "zero-length run"));
        }
        let run_lcn = if off_size == 0 {
            None
        } else {
            let mut delta = 0_u64;
            for (i, &b) in off_bytes.iter().enumerate() {
                delta |= u64::from(b) << (8 * i);
            }
            // Sign-extend from `off_size` bytes.
            let shift = 64 - 8 * off_size as u32;
            let delta = ((delta << shift) as i64) >> shift;
            lcn = lcn
                .checked_add(delta)
                .filter(|l| *l >= 0)
                .ok_or(MftParseError::malformed(WHAT, "LCN out of range"))?;
            let start = lcn as u64;
            if start
                .checked_add(len)
                .is_none_or(|end| end > total_clusters)
            {
                return Err(MftParseError::malformed(
                    WHAT,
                    "extent lies outside the volume",
                ));
            }
            Some(start)
        };
        if out.len() >= MAX_RUNS {
            return Err(MftParseError::Limit {
                what: "runs in a stream",
                limit: MAX_RUNS as u64,
            });
        }
        out.push(Run {
            vcn,
            len,
            lcn: run_lcn,
        });
        vcn = vcn
            .checked_add(len)
            .ok_or(MftParseError::malformed(WHAT, "VCN overflow"))?;
        added += len; // cannot overflow: bounded by the checked VCN sum
    }
    Ok(added)
}

/// Total clusters covered by `runs` (they are contiguous from VCN 0 by construction).
#[must_use]
pub fn runs_clusters(runs: &[Run]) -> u64 {
    runs.last().map_or(0, |r| r.vcn.saturating_add(r.len))
}

// ---- attribute values ------------------------------------------------------------------------

/// The parts of `$STANDARD_INFORMATION` the reader uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardInfo {
    /// Creation time (`FILETIME`).
    pub created: u64,
    /// Last write time (`FILETIME`).
    pub modified: u64,
    /// `FILE_ATTRIBUTE_*` flags.
    pub attributes: u32,
    /// Last USN (0 for the short, pre-3.0 form of the attribute).
    pub usn: i64,
}

/// Parses a resident `$STANDARD_INFORMATION` value (48 bytes, or 72 with the NTFS 3.0 tail).
///
/// # Errors
/// [`MftParseError::Truncated`] when the value is shorter than 48 bytes.
pub fn parse_standard_info(value: &[u8]) -> Result<StandardInfo> {
    const WHAT: &str = "$STANDARD_INFORMATION";
    let rd64 = |o| u64_at(value, o).ok_or(MftParseError::truncated(WHAT));
    Ok(StandardInfo {
        created: rd64(0)?,
        modified: rd64(8)?,
        attributes: u32_at(value, 0x20).ok_or(MftParseError::truncated(WHAT))?,
        usn: u64_at(value, 0x40).map_or(0, |v| v as i64),
    })
}

/// A parsed `$FILE_NAME` value.
#[derive(Debug, Clone, Copy)]
pub struct FileNameAttr<'a> {
    /// Parent directory reference (record number and sequence number).
    pub parent: u64,
    /// `FILE_ATTRIBUTE_*` copy stored with the name (may lag behind the real attributes).
    pub flags: u32,
    /// `NAMESPACE_*`.
    pub namespace: u8,
    /// The name as UTF-16LE bytes.
    pub name: &'a [u8],
}

/// Parses a resident `$FILE_NAME` value.
///
/// # Errors
/// [`MftParseError`] when the value is shorter than its declared name.
pub fn parse_file_name(value: &[u8]) -> Result<FileNameAttr<'_>> {
    const WHAT: &str = "$FILE_NAME";
    let name_len = usize::from(*value.get(0x40).ok_or(MftParseError::truncated(WHAT))?) * 2;
    let namespace = *value.get(0x41).ok_or(MftParseError::truncated(WHAT))?;
    let name = value
        .get(0x42..0x42 + name_len)
        .ok_or(MftParseError::truncated(WHAT))?;
    Ok(FileNameAttr {
        parent: u64_at(value, 0).ok_or(MftParseError::truncated(WHAT))?,
        flags: u32_at(value, 0x38).ok_or(MftParseError::truncated(WHAT))?,
        namespace,
        name,
    })
}

/// One entry of an `$ATTRIBUTE_LIST`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttrListEntry {
    /// Attribute type code.
    pub type_code: u32,
    /// `true` when the attribute has a name (so it is not the main stream).
    pub named: bool,
    /// First VCN of this piece of a non-resident attribute.
    pub start_vcn: u64,
    /// Reference of the `FILE` record holding the attribute (record number in the low 48 bits).
    pub segment_reference: u64,
}

/// Iterator over `$ATTRIBUTE_LIST` entries. Yields one `Err` and stops on a malformed entry.
#[derive(Debug, Clone)]
pub struct AttrListEntries<'a> {
    buf: &'a [u8],
    pos: usize,
    done: bool,
}

impl<'a> AttrListEntries<'a> {
    /// Walks the entries in `buf` (a resident value, or a non-resident one read into memory).
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            pos: 0,
            done: false,
        }
    }
}

impl Iterator for AttrListEntries<'_> {
    type Item = Result<AttrListEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        const WHAT: &str = "$ATTRIBUTE_LIST entry";
        const FIXED: usize = 0x1A;
        if self.done {
            return None;
        }
        let rest = self.buf.get(self.pos..).unwrap_or_default();
        // Trailing bytes shorter than an entry are padding.
        if rest.len() < FIXED {
            self.done = true;
            return None;
        }
        let type_code = u32_at(rest, 0)?;
        if type_code == 0 {
            self.done = true;
            return None;
        }
        let rec_len = usize::from(u16_at(rest, 4)?);
        if rec_len < FIXED || rec_len > rest.len() {
            self.done = true;
            return Some(Err(MftParseError::malformed(WHAT, "bad entry length")));
        }
        let entry = AttrListEntry {
            type_code,
            named: rest[6] != 0,
            start_vcn: u64_at(rest, 8)?,
            segment_reference: u64_at(rest, 0x10)?,
        };
        self.pos += rec_len; // rec_len >= FIXED > 0
        Some(Ok(entry))
    }
}

/// The record-number part (low 48 bits) of a file reference.
#[must_use]
pub const fn reference_record_number(reference: u64) -> u64 {
    reference & 0x0000_FFFF_FFFF_FFFF
}
