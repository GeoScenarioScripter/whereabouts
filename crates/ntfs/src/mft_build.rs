//! Hand-built `$MFT` fixtures: records with update sequence arrays, attributes, runlists, and a
//! whole synthetic volume image around them.
//!
//! Only compiled for this crate's tests or with the `test-fixtures` feature. The encoders follow
//! the on-disk layouts documented in [`crate::mft`], written independently of the parsers (they
//! build bytes from field values), so a parser/builder mismatch shows up as a test failure.

use crate::mft::{
    ATTR_ATTRIBUTE_LIST, ATTR_BITMAP, ATTR_DATA, ATTR_FILE_NAME, ATTR_STANDARD_INFORMATION,
    NAMESPACE_WIN32, NTFS_VOLUME_DATA_SIZE, RECORD_IN_USE, RECORD_IS_DIRECTORY, VolumeGeometry,
};

/// The update sequence number written into every fixture record.
pub const USN: u16 = 0x7A5C;

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// A reference as stored in `$FILE_NAME` / attribute lists: sequence in the top 16 bits.
#[must_use]
pub const fn reference(number: u64, sequence: u16) -> u64 {
    (sequence as u64) << 48 | number
}

/// Encodes a resident attribute.
#[must_use]
pub fn resident_attr(type_code: u32, name: &str, value: &[u8]) -> Vec<u8> {
    let name = utf16le(name);
    let value_off = (0x18 + name.len()).next_multiple_of(8);
    let len = (value_off + value.len()).next_multiple_of(8);
    let mut a = vec![0_u8; len];
    put32(&mut a, 0, type_code);
    put32(&mut a, 4, len as u32);
    a[9] = (name.len() / 2) as u8;
    put16(&mut a, 10, 0x18);
    put32(&mut a, 0x10, value.len() as u32);
    put16(&mut a, 0x14, value_off as u16);
    a[0x18..0x18 + name.len()].copy_from_slice(&name);
    a[value_off..value_off + value.len()].copy_from_slice(value);
    a
}

/// Encodes a non-resident attribute holding `runlist` (already encoded, see [`encode_runs`]).
#[must_use]
pub fn nonresident_attr(
    type_code: u32,
    name: &str,
    start_vcn: u64,
    real_size: u64,
    initialized_size: u64,
    runlist: &[u8],
) -> Vec<u8> {
    let name = utf16le(name);
    let runs_off = (0x40 + name.len()).next_multiple_of(8);
    let len = (runs_off + runlist.len() + 1).next_multiple_of(8);
    let mut a = vec![0_u8; len];
    put32(&mut a, 0, type_code);
    put32(&mut a, 4, len as u32);
    a[8] = 1;
    a[9] = (name.len() / 2) as u8;
    put16(&mut a, 10, 0x40);
    put64(&mut a, 0x10, start_vcn);
    put16(&mut a, 0x20, runs_off as u16);
    put64(&mut a, 0x28, real_size.next_multiple_of(4096));
    put64(&mut a, 0x30, real_size);
    put64(&mut a, 0x38, initialized_size);
    a[0x40..0x40 + name.len()].copy_from_slice(&name);
    a[runs_off..runs_off + runlist.len()].copy_from_slice(runlist);
    a
}

/// Encodes a runlist from `(clusters, Some(absolute LCN) | None for sparse)` pairs.
#[must_use]
pub fn encode_runs(runs: &[(u64, Option<u64>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut prev = 0_i64;
    for &(len, lcn) in runs {
        let len_bytes = len.to_le_bytes();
        let len_size = 8 - len_bytes
            .iter()
            .rev()
            .take_while(|&&b| b == 0)
            .count()
            .min(7);
        match lcn {
            None => {
                out.push(len_size as u8);
                out.extend_from_slice(&len_bytes[..len_size]);
            }
            Some(lcn) => {
                let delta = lcn as i64 - prev;
                prev = lcn as i64;
                let delta_bytes = delta.to_le_bytes();
                // Shortest two's-complement encoding.
                let mut off_size = 8;
                while off_size > 1 {
                    let top = delta_bytes[off_size - 1];
                    let below = delta_bytes[off_size - 2];
                    if (top == 0 && below & 0x80 == 0) || (top == 0xFF && below & 0x80 != 0) {
                        off_size -= 1;
                    } else {
                        break;
                    }
                }
                out.push((off_size as u8) << 4 | len_size as u8);
                out.extend_from_slice(&len_bytes[..len_size]);
                out.extend_from_slice(&delta_bytes[..off_size]);
            }
        }
    }
    out.push(0);
    out
}

/// `$STANDARD_INFORMATION` value (the 72-byte NTFS 3.0 form).
#[must_use]
pub fn standard_info(modified: u64, attributes: u32, usn: u64) -> Vec<u8> {
    let mut v = vec![0_u8; 72];
    put64(&mut v, 0, modified.wrapping_sub(1));
    put64(&mut v, 8, modified);
    put32(&mut v, 0x20, attributes);
    put64(&mut v, 0x40, usn);
    v
}

/// `$FILE_NAME` value.
#[must_use]
pub fn file_name_value(parent: u64, namespace: u8, name: &str) -> Vec<u8> {
    let name = utf16le(name);
    let mut v = vec![0_u8; 0x42 + name.len()];
    put64(&mut v, 0, parent);
    v[0x40] = (name.len() / 2) as u8;
    v[0x41] = namespace;
    v[0x42..].copy_from_slice(&name);
    v
}

/// One `$ATTRIBUTE_LIST` entry.
#[must_use]
pub fn attr_list_entry(type_code: u32, start_vcn: u64, segment_reference: u64) -> Vec<u8> {
    let mut e = vec![0_u8; 0x20];
    put32(&mut e, 0, type_code);
    put16(&mut e, 4, 0x20);
    put64(&mut e, 8, start_vcn);
    put64(&mut e, 0x10, segment_reference);
    e
}

/// Encodes a `FILE` record of `size` bytes with its update sequence array applied.
///
/// # Panics
/// If the attributes do not fit (fixtures only).
#[must_use]
pub fn record(size: usize, sequence: u16, flags: u16, base: u64, attrs: &[Vec<u8>]) -> Vec<u8> {
    let sectors = size / 512;
    let usa_count = sectors + 1;
    let first = (0x30 + usa_count * 2).next_multiple_of(8);
    let mut r = vec![0_u8; size];
    r[..4].copy_from_slice(b"FILE");
    put16(&mut r, 4, 0x30);
    put16(&mut r, 6, usa_count as u16);
    put16(&mut r, 0x10, sequence);
    put16(&mut r, 0x12, 1);
    put16(&mut r, 0x14, first as u16);
    put16(&mut r, 0x16, flags);
    put32(&mut r, 0x1C, size as u32);
    put64(&mut r, 0x20, base);
    let mut pos = first;
    for a in attrs {
        assert!(
            pos + a.len() + 8 <= size,
            "attributes do not fit the record"
        );
        r[pos..pos + a.len()].copy_from_slice(a);
        pos += a.len();
    }
    put32(&mut r, pos, 0xFFFF_FFFF);
    put32(&mut r, 0x18, (pos + 8) as u32);
    seal(&mut r);
    r
}

/// Applies the fixup transformation: saves each sector's last two bytes in the array and
/// overwrites them with [`USN`].
pub fn seal(r: &mut [u8]) {
    put16(r, 0x30, USN);
    for s in 1..=r.len() / 512 {
        let slot = s * 512 - 2;
        let saved = [r[slot], r[slot + 1]];
        r[0x30 + s * 2..0x32 + s * 2].copy_from_slice(&saved);
        put16(r, slot, USN);
    }
}

/// A plain in-use file or directory record with one Win32 name.
#[must_use]
pub fn simple_file(
    size: usize,
    sequence: u16,
    directory: bool,
    parent: u64,
    name: &str,
    attributes: u32,
) -> Vec<u8> {
    record(
        size,
        sequence,
        RECORD_IN_USE | if directory { RECORD_IS_DIRECTORY } else { 0 },
        0,
        &[
            resident_attr(
                ATTR_STANDARD_INFORMATION,
                "",
                &standard_info(132_000_000_000_000_000, attributes, 0),
            ),
            resident_attr(
                ATTR_FILE_NAME,
                "",
                &file_name_value(parent, NAMESPACE_WIN32, name),
            ),
        ],
    )
}

/// `NTFS_VOLUME_DATA_BUFFER` for `geometry`.
#[must_use]
pub fn volume_data(g: &VolumeGeometry) -> Vec<u8> {
    let mut v = vec![0_u8; NTFS_VOLUME_DATA_SIZE];
    put64(&mut v, 0, g.serial_number);
    put64(
        &mut v,
        8,
        g.total_clusters * u64::from(g.bytes_per_cluster) / 512,
    );
    put64(&mut v, 16, g.total_clusters);
    put32(&mut v, 40, g.bytes_per_sector);
    put32(&mut v, 44, g.bytes_per_cluster);
    put32(&mut v, 48, g.bytes_per_record);
    put32(
        &mut v,
        52,
        (g.bytes_per_record / g.bytes_per_cluster).max(1),
    );
    put64(&mut v, 56, g.mft_valid_data_length);
    put64(&mut v, 64, g.mft_start_lcn);
    v
}

/// Builds a synthetic volume image: `$MFT` records spread over the given extents, a bitmap, and
/// record 0 describing both.
#[derive(Debug, Clone)]
pub struct ImageBuilder {
    /// Bytes per cluster (sector size is fixed at 512).
    pub cluster: u32,
    /// Bytes per record.
    pub record: u32,
    /// Clusters on the volume.
    pub total_clusters: u64,
    /// `$MFT` extents `(first LCN, clusters)` in VCN order.
    pub extents: Vec<(u64, u64)>,
    /// LCN of the `$MFT` bitmap (one cluster); `None` for no `$BITMAP` attribute.
    pub bitmap_lcn: Option<u64>,
    /// How many of `extents` are described in record 0; the rest go into the extension record
    /// `data_extension_record` through an `$ATTRIBUTE_LIST`. `None` = all in record 0.
    pub extents_in_record0: Option<usize>,
    /// Record number holding the `$DATA` continuation (used with `extents_in_record0`).
    pub data_extension_record: u64,
    records: Vec<(u64, Vec<u8>)>,
    unmarked: Vec<u64>,
}

impl ImageBuilder {
    /// A volume of `total_clusters` with the `$MFT` in one extent at LCN 4.
    #[must_use]
    pub fn new(cluster: u32, record: u32, total_clusters: u64, mft_clusters: u64) -> Self {
        Self {
            cluster,
            record,
            total_clusters,
            extents: vec![(4, mft_clusters)],
            bitmap_lcn: Some(2),
            extents_in_record0: None,
            data_extension_record: 0,
            records: Vec::new(),
            unmarked: Vec::new(),
        }
    }

    /// Adds (or replaces) record `number` with already-encoded bytes.
    pub fn put(&mut self, number: u64, bytes: Vec<u8>) -> &mut Self {
        self.records.retain(|(n, _)| *n != number);
        self.records.push((number, bytes));
        self
    }

    /// Clears the bitmap bit of `number` although a record is stored there.
    pub fn unmark(&mut self, number: u64) -> &mut Self {
        self.unmarked.push(number);
        self
    }

    /// Geometry of the image.
    #[must_use]
    pub fn geometry(&self) -> VolumeGeometry {
        let clusters: u64 = self.extents.iter().map(|e| e.1).sum();
        VolumeGeometry {
            serial_number: 0x1234_5678_9ABC_DEF0,
            bytes_per_sector: 512,
            bytes_per_cluster: self.cluster,
            bytes_per_record: self.record,
            total_clusters: self.total_clusters,
            mft_valid_data_length: clusters * u64::from(self.cluster),
            mft_start_lcn: self.extents[0].0,
        }
    }

    /// Writes the image. Record 0 is generated unless `put(0, ..)` was called.
    ///
    /// # Panics
    /// If the layout is inconsistent (fixtures only).
    #[must_use]
    pub fn build(&self) -> Vec<u8> {
        let cluster = u64::from(self.cluster);
        let rec = u64::from(self.record);
        let mut img = vec![0_u8; (self.total_clusters * cluster) as usize];
        let mft_bytes: u64 = self.extents.iter().map(|e| e.1 * cluster).sum();
        let slots = mft_bytes / rec;

        let in_zero = self.extents_in_record0.unwrap_or(self.extents.len());
        let runs = |from: usize, to: usize| {
            encode_runs(
                &self.extents[from..to]
                    .iter()
                    .map(|&(lcn, len)| (len, Some(lcn)))
                    .collect::<Vec<_>>(),
            )
        };
        let mut bitmap = vec![0_u8; self.cluster as usize];
        let mut used: Vec<u64> = self.records.iter().map(|r| r.0).collect();
        used.push(0);
        if in_zero < self.extents.len() {
            used.push(self.data_extension_record);
        }
        for n in used {
            if !self.unmarked.contains(&n) {
                bitmap[(n / 8) as usize] |= 1 << (n % 8);
            }
        }

        let mut record0 = self.records.iter().find(|r| r.0 == 0).map(|r| r.1.clone());
        let mut extension = None;
        if record0.is_none() {
            let mut attrs = vec![
                resident_attr(ATTR_STANDARD_INFORMATION, "", &standard_info(1, 6, 0)),
                resident_attr(
                    ATTR_FILE_NAME,
                    "",
                    &file_name_value(reference(5, 5), NAMESPACE_WIN32, "$MFT"),
                ),
            ];
            if in_zero < self.extents.len() {
                // runs in record 0 ...
                let first_clusters: u64 = self.extents[..in_zero].iter().map(|e| e.1).sum();
                let mut list = attr_list_entry(ATTR_DATA, 0, reference(0, 1));
                list.extend(attr_list_entry(
                    ATTR_DATA,
                    first_clusters,
                    reference(self.data_extension_record, 1),
                ));
                attrs.push(resident_attr(ATTR_ATTRIBUTE_LIST, "", &list));
                attrs.push(nonresident_attr(
                    ATTR_DATA,
                    "",
                    0,
                    mft_bytes,
                    mft_bytes,
                    &runs(0, in_zero),
                ));
                // ... and the rest in the extension record.
                extension = Some(record(
                    self.record as usize,
                    1,
                    RECORD_IN_USE,
                    reference(0, 1),
                    &[nonresident_attr(
                        ATTR_DATA,
                        "",
                        first_clusters,
                        0,
                        0,
                        &runs_rebased(&self.extents[in_zero..]),
                    )],
                ));
            } else {
                attrs.push(nonresident_attr(
                    ATTR_DATA,
                    "",
                    0,
                    mft_bytes,
                    mft_bytes,
                    &runs(0, self.extents.len()),
                ));
            }
            if let Some(lcn) = self.bitmap_lcn {
                attrs.push(nonresident_attr(
                    ATTR_BITMAP,
                    "",
                    0,
                    slots.div_ceil(8),
                    slots.div_ceil(8),
                    &encode_runs(&[(1, Some(lcn))]),
                ));
            }
            record0 = Some(record(self.record as usize, 1, RECORD_IN_USE, 0, &attrs));
        }

        let put_virtual = |img: &mut [u8], number: u64, bytes: &[u8]| {
            let mut off = number * rec;
            let mut remaining = bytes;
            while !remaining.is_empty() {
                // Locate the extent holding virtual offset `off`.
                let mut base = 0_u64;
                let mut placed = false;
                for &(lcn, len) in &self.extents {
                    let span = len * cluster;
                    if off < base + span {
                        let within = off - base;
                        let n = remaining.len().min((span - within) as usize);
                        let at = (lcn * cluster + within) as usize;
                        img[at..at + n].copy_from_slice(&remaining[..n]);
                        remaining = &remaining[n..];
                        off += n as u64;
                        placed = true;
                        break;
                    }
                    base += span;
                }
                assert!(placed, "record {number} lies outside the $MFT extents");
            }
        };
        put_virtual(&mut img, 0, &record0.unwrap_or_default());
        if let Some(ext) = &extension {
            put_virtual(&mut img, self.data_extension_record, ext);
        }
        for (n, bytes) in &self.records {
            if *n != 0 {
                put_virtual(&mut img, *n, bytes);
            }
        }
        if let Some(lcn) = self.bitmap_lcn {
            let at = (lcn * cluster) as usize;
            img[at..at + bitmap.len()].copy_from_slice(&bitmap);
        }
        img
    }
}

/// Encodes extents as a runlist whose first LCN delta is relative to 0 (as in any runlist,
/// continuation pieces start from LCN 0 again).
fn runs_rebased(extents: &[(u64, u64)]) -> Vec<u8> {
    encode_runs(
        &extents
            .iter()
            .map(|&(lcn, len)| (len, Some(lcn)))
            .collect::<Vec<_>>(),
    )
}
