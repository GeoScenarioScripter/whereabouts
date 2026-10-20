//! dBase (`.dbf`) table headers: record count, field list, language driver — plus [`DbfReader`]
//! for lone `.dbf` tables (non-spatial, [`Format::Dbf`]).
//!
//! Layout (dBase III+, as written by shapefile tools): a 32-byte header (`[4..8]` record count
//! u32 LE, `[8..10]` header length u16 LE, `[10..12]` record length u16 LE, `[29]` language driver
//! ID), then 32-byte field descriptors (`[0..11]` NUL-padded name, `[11]` type, `[16]` width,
//! `[17]` decimals) up to a `0x0D` terminator. Only the header is read, never the records.

use std::path::Path;

use whereabouts_core::{
    DatasetMetadata, FieldInfo, FieldKind, Format, FormatReader, LayerMetadata, MemberRole,
    ReadError, ReadLimits, ResolvedDataset,
};

use crate::io::{Budget, SourceFile};

/// Fixed header size.
const HEADER_LEN: usize = 32;
/// Size of one field descriptor.
const DESCRIPTOR_LEN: usize = 32;
/// Field descriptor terminator.
const TERMINATOR: u8 = 0x0D;
/// Most field descriptors read. dBase allows 255 and GDAL writes up to 2046 (the 16-bit header
/// length limit); 1,024 keeps the header read at ≤ 32 KiB, so a whole shapefile stays < 64 KiB.
pub(crate) const MAX_FIELDS: usize = 1024;
/// Largest `.cpg` read (its content is a short label such as `UTF-8`).
pub(crate) const MAX_CPG_BYTES: usize = 64;

/// A field descriptor, name still undecoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawField {
    /// Name bytes: up to the first NUL, trailing spaces trimmed.
    pub(crate) name: Vec<u8>,
    /// Type character (`C`, `N`, `F`, `D`, `L`, …).
    pub(crate) kind: u8,
    /// Width in bytes.
    pub(crate) width: u32,
    /// Decimal count.
    pub(crate) decimals: u32,
}

impl RawField {
    /// The OGR field type GDAL's shapefile driver reports for this descriptor.
    pub(crate) fn field_kind(&self) -> FieldKind {
        match self.kind {
            b'C' => FieldKind::String,
            // Shapelib reads N/F as integer below width 10 and without decimals; OGR widens
            // integers of 10..=18 digits to Integer64 and keeps wider ones as Real.
            b'N' | b'F' if self.decimals > 0 => FieldKind::Real,
            b'N' | b'F' if self.width < 10 => FieldKind::Integer,
            b'N' | b'F' if self.width < 19 => FieldKind::Integer64,
            b'N' | b'F' => FieldKind::Real,
            b'D' => FieldKind::Date,
            b'L' => FieldKind::Boolean,
            _ => FieldKind::Other,
        }
    }
}

/// What a DBF header says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DbfHeader {
    /// Record count, or `None` if the header's count does not fit in the file.
    pub(crate) record_count: Option<u64>,
    /// Language driver ID (0 = not set).
    pub(crate) ldid: u8,
    /// Field descriptors in declaration order.
    pub(crate) fields: Vec<RawField>,
    /// Inconsistencies that degrade the result (for [`DatasetMetadata::partial`]).
    pub(crate) problems: Vec<String>,
}

impl DbfHeader {
    /// Decoded fields, using the table's encoding (`.cpg` content wins over the LDID).
    pub(crate) fn field_infos(&self, cpg: Option<&str>) -> Vec<FieldInfo> {
        let encoding = crate::encoding::resolve(cpg, self.ldid);
        self.fields
            .iter()
            .map(|f| FieldInfo {
                name: crate::encoding::decode(&f.name, encoding),
                kind: f.field_kind(),
                width: Some(f.width),
                precision: Some(f.decimals),
            })
            .collect()
    }
}

fn malformed(format: Format, path: &Path, reason: impl std::fmt::Display) -> ReadError {
    ReadError::Malformed {
        format,
        reason: format!("{}: {reason}", path.display()),
    }
}

/// Reads and validates a DBF header (at most 32 + 32 × [`MAX_FIELDS`] bytes). Header lengths that
/// point past the end of the file and zero record lengths are [`ReadError::Malformed`] (tagged
/// with `format`); a record count the file cannot hold yields `record_count: None` plus a problem.
pub(crate) fn read_header(
    path: &Path,
    budget: &Budget,
    format: Format,
) -> Result<DbfHeader, ReadError> {
    let mut file = SourceFile::open(path, budget)?;
    let file_len = file.len();
    if file_len < HEADER_LEN as u64 {
        return Err(malformed(
            format,
            path,
            format!("dbf is {file_len} bytes, shorter than its {HEADER_LEN}-byte header"),
        ));
    }
    let head = file.read_at(0, HEADER_LEN)?;
    let (Some(count), Some(header_len), Some(record_len), Some(&ldid)) = (
        le_u32(&head, 4),
        le_u16(&head, 8),
        le_u16(&head, 10),
        head.get(29),
    ) else {
        return Err(malformed(format, path, "short dbf header"));
    };
    let header_len = u64::from(header_len);
    if header_len < HEADER_LEN as u64 {
        return Err(malformed(
            format,
            path,
            format!("dbf header length {header_len} is shorter than {HEADER_LEN} bytes"),
        ));
    }
    if header_len > file_len {
        return Err(malformed(
            format,
            path,
            format!(
                "dbf header length {header_len} is beyond the end of the file ({file_len} bytes)"
            ),
        ));
    }
    if record_len == 0 {
        return Err(malformed(format, path, "dbf record length is 0"));
    }

    let mut problems = Vec::new();
    let slots = usize::try_from((header_len - HEADER_LEN as u64) / DESCRIPTOR_LEN as u64)
        .unwrap_or(usize::MAX);
    let to_read = slots.min(MAX_FIELDS);
    let descriptors = file.read_at(HEADER_LEN as u64, to_read * DESCRIPTOR_LEN)?;
    let mut fields = Vec::new();
    let mut terminated = false;
    for d in descriptors.as_chunks::<DESCRIPTOR_LEN>().0 {
        // Some writers end the list with NUL instead of 0x0D.
        if d.first().is_none_or(|&b| b == TERMINATOR || b == 0) {
            terminated = true;
            break;
        }
        fields.push(parse_descriptor(d));
    }
    if !terminated && slots > MAX_FIELDS {
        problems.push(format!("dbf field list truncated at {MAX_FIELDS} fields"));
    }

    let widths: u64 = fields.iter().map(|f| u64::from(f.width)).sum();
    if widths + 1 > u64::from(record_len) {
        problems.push(format!(
            "dbf field widths ({widths} bytes + 1) exceed the record length {record_len}"
        ));
    }

    let records_end = u64::from(count)
        .checked_mul(u64::from(record_len))
        .and_then(|n| n.checked_add(header_len));
    let record_count = match records_end {
        Some(end) if end <= file_len => Some(u64::from(count)),
        _ => {
            problems.push(format!(
                "dbf claims {count} records of {record_len} bytes but the file is {file_len} bytes"
            ));
            None
        }
    };

    Ok(DbfHeader {
        record_count,
        ldid,
        fields,
        problems,
    })
}

/// Reads a `.cpg` file's content (at most [`MAX_CPG_BYTES`]); `None` if unreadable.
pub(crate) fn read_cpg(path: &Path, budget: &Budget) -> Option<String> {
    let mut file = SourceFile::open(path, budget).ok()?;
    let bytes = file.read_prefix(MAX_CPG_BYTES).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_descriptor(d: &[u8]) -> RawField {
    let raw_name = d.get(..11).unwrap_or_default();
    let name_end = raw_name
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(raw_name.len());
    let name = raw_name
        .get(..name_end)
        .unwrap_or_default()
        .trim_ascii_end()
        .to_vec();
    let kind = d.get(11).copied().unwrap_or(0);
    let width = d.get(16).copied().unwrap_or(0);
    let decimals = d.get(17).copied().unwrap_or(0);
    // Clipper/FoxPro extension (also read by shapelib): character fields wider than 255 bytes
    // keep the high byte of the width in the decimals byte.
    let (width, decimals) = if kind == b'C' {
        (u32::from(width) + u32::from(decimals) * 256, 0)
    } else {
        (u32::from(width), u32::from(decimals))
    };
    RawField {
        name,
        kind,
        width,
        decimals,
    }
}

fn le_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Reader for lone dBase tables ([`Format::Dbf`]): one non-spatial layer named after the file
/// stem, with record count and fields. Shapefiles' `.dbf` members are read by the
/// [`ShapefileReader`](crate::ShapefileReader) instead.
#[derive(Debug, Clone, Copy, Default)]
pub struct DbfReader;

impl FormatReader for DbfReader {
    fn name(&self) -> &'static str {
        "dbf"
    }

    fn formats(&self) -> &'static [Format] {
        &[Format::Dbf]
    }

    fn read(
        &self,
        dataset: &ResolvedDataset,
        limits: &ReadLimits,
    ) -> Result<DatasetMetadata, ReadError> {
        if dataset.format != Format::Dbf {
            return Err(ReadError::Unsupported(format!(
                "dbf reader cannot read {} datasets",
                dataset.format
            )));
        }
        let budget = Budget::new(limits.max_bytes);
        let header = read_header(&dataset.anchor, &budget, Format::Dbf)?;
        let cpg = dataset
            .sidecar(MemberRole::Cpg)
            .and_then(|p| read_cpg(p, &budget));
        let layer = LayerMetadata {
            name: crate::layer_name(&dataset.anchor),
            feature_count: header.record_count,
            fields: header.field_infos(cpg.as_deref()),
            ..LayerMetadata::default()
        };
        Ok(DatasetMetadata {
            layers: vec![layer],
            partial: header.problems,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use whereabouts_core::{ExtractionStatus, ResolvedMember};

    /// Builds a DBF header (no records) for `fields` = (name, type, width, decimals), followed
    /// by `count` blank records.
    pub(crate) fn dbf_bytes(fields: &[(&[u8], u8, u8, u8)], count: u32, ldid: u8) -> Vec<u8> {
        let header_len = 32 + 32 * fields.len() + 1;
        let record_len = 1 + fields.iter().map(|f| usize::from(f.2)).sum::<usize>();
        let mut b = vec![0u8; 32];
        b[0] = 0x03;
        b[4..8].copy_from_slice(&count.to_le_bytes());
        b[8..10].copy_from_slice(&u16::try_from(header_len).unwrap().to_le_bytes());
        b[10..12].copy_from_slice(&u16::try_from(record_len).unwrap().to_le_bytes());
        b[29] = ldid;
        for (name, kind, width, decimals) in fields {
            let mut d = [0u8; 32];
            d[..name.len()].copy_from_slice(name);
            d[11] = *kind;
            d[16] = *width;
            d[17] = *decimals;
            b.extend_from_slice(&d);
        }
        b.push(TERMINATOR);
        b.extend(std::iter::repeat_n(b' ', record_len * count as usize));
        b.push(0x1A);
        b
    }

    fn header_of(bytes: &[u8]) -> Result<DbfHeader, ReadError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.dbf");
        std::fs::write(&path, bytes).unwrap();
        read_header(&path, &Budget::new(1 << 20), Format::Dbf)
    }

    #[test]
    fn parses_fields_with_ogr_types() {
        let bytes = dbf_bytes(
            &[
                (b"name", b'C', 40, 0),
                (b"small", b'N', 9, 0),
                (b"id", b'N', 10, 0),
                (b"big", b'N', 18, 0),
                (b"huge", b'N', 19, 0),
                (b"area", b'N', 12, 3),
                (b"ratio", b'F', 8, 2),
                (b"when", b'D', 8, 0),
                (b"ok", b'L', 1, 0),
                (b"memo", b'M', 10, 0),
                (b"FULLNAME_X", b'C', 254, 0),
            ],
            3,
            87,
        );
        let h = header_of(&bytes).unwrap();
        assert_eq!(h.record_count, Some(3));
        assert_eq!(h.ldid, 87);
        assert!(h.problems.is_empty(), "{:?}", h.problems);
        let kinds: Vec<_> = h
            .field_infos(None)
            .into_iter()
            .map(|f| (f.name, f.kind, f.width))
            .collect();
        use FieldKind as K;
        assert_eq!(
            kinds,
            vec![
                ("name".into(), K::String, Some(40)),
                ("small".into(), K::Integer, Some(9)),
                ("id".into(), K::Integer64, Some(10)),
                ("big".into(), K::Integer64, Some(18)),
                ("huge".into(), K::Real, Some(19)),
                ("area".into(), K::Real, Some(12)),
                ("ratio".into(), K::Real, Some(8)),
                ("when".into(), K::Date, Some(8)),
                ("ok".into(), K::Boolean, Some(1)),
                ("memo".into(), K::Other, Some(10)),
                ("FULLNAME_X".into(), K::String, Some(254)),
            ]
        );
    }

    #[test]
    fn names_are_trimmed_and_decoded_with_the_table_encoding() {
        // "名称" in GBK, padded with spaces; then an 11-byte name with no NUL.
        let gbk_name: &[u8] = &[0xC3, 0xFB, 0xB3, 0xC6, b' ', b' '];
        let bytes = dbf_bytes(
            &[(gbk_name, b'C', 10, 0), (b"ELEVENCHARS", b'N', 5, 0)],
            0,
            77,
        );
        let h = header_of(&bytes).unwrap();
        let names: Vec<_> = h.field_infos(None).into_iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["名称".to_owned(), "ELEVENCHARS".to_owned()]);
        // A .cpg overrides the LDID.
        let names: Vec<_> = h
            .field_infos(Some("1251"))
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_ne!(names[0], "名称");
    }

    #[test]
    fn wide_character_fields_use_the_decimals_byte() {
        let mut bytes = dbf_bytes(&[(b"long", b'C', 0x2C, 0)], 0, 0);
        bytes[32 + 17] = 1; // width = 0x2C + 256
        bytes[10..12].copy_from_slice(&301u16.to_le_bytes());
        let h = header_of(&bytes).unwrap();
        assert_eq!(h.fields[0].width, 300);
        assert_eq!(h.fields[0].decimals, 0);
    }

    #[test]
    fn invalid_headers_are_malformed() {
        assert!(matches!(
            header_of(&[3u8; 20]),
            Err(ReadError::Malformed { .. })
        ));
        let good = dbf_bytes(&[(b"id", b'N', 10, 0)], 2, 87);
        let mut beyond = good.clone();
        beyond[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let err = header_of(&beyond).unwrap_err().to_string();
        assert!(err.contains("beyond the end"), "{err}");
        let mut short = good.clone();
        short[8..10].copy_from_slice(&10u16.to_le_bytes());
        assert!(matches!(
            header_of(&short),
            Err(ReadError::Malformed { .. })
        ));
        let mut zero_len = good;
        zero_len[10..12].copy_from_slice(&0u16.to_le_bytes());
        assert!(matches!(
            header_of(&zero_len),
            Err(ReadError::Malformed { .. })
        ));
    }

    #[test]
    fn impossible_record_counts_are_dropped_not_trusted() {
        let mut bytes = dbf_bytes(&[(b"id", b'N', 10, 0)], 2, 87);
        bytes[4..8].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
        let h = header_of(&bytes).unwrap();
        assert_eq!(h.record_count, None);
        assert_eq!(h.problems.len(), 1);
        assert_eq!(h.fields.len(), 1);
        // u32::MAX records × u16::MAX bytes must not overflow.
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[10..12].copy_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(header_of(&bytes).unwrap().record_count, None);
    }

    #[test]
    fn field_widths_exceeding_the_record_are_flagged() {
        let mut bytes = dbf_bytes(&[(b"id", b'N', 10, 0)], 0, 0);
        bytes[10..12].copy_from_slice(&5u16.to_le_bytes());
        let h = header_of(&bytes).unwrap();
        assert!(h.problems.iter().any(|p| p.contains("exceed")));
    }

    #[test]
    fn nul_terminated_and_unterminated_field_lists() {
        // Header length with room for 2 descriptors but a NUL where the second starts.
        let mut bytes = dbf_bytes(&[(b"a", b'C', 1, 0), (b"b", b'C', 1, 0)], 0, 0);
        bytes[64] = 0;
        assert_eq!(header_of(&bytes).unwrap().fields.len(), 1);
        // No terminator at all within the header length: the descriptors in range count.
        let mut bytes = dbf_bytes(&[(b"a", b'C', 1, 0), (b"b", b'C', 1, 0)], 0, 0);
        bytes[8..10].copy_from_slice(&96u16.to_le_bytes());
        assert_eq!(header_of(&bytes).unwrap().fields.len(), 2);
    }

    #[test]
    fn huge_field_lists_are_capped() {
        let names: Vec<Vec<u8>> = (0..2000).map(|i| format!("f{i}").into_bytes()).collect();
        let fields: Vec<(&[u8], u8, u8, u8)> =
            names.iter().map(|n| (n.as_slice(), b'C', 1, 0)).collect();
        let bytes = dbf_bytes(&fields, 0, 0);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.dbf");
        std::fs::write(&path, &bytes).unwrap();
        let budget = Budget::new(1 << 20);
        let (h, stats) = crate::io::measure(|| read_header(&path, &budget, Format::Dbf).unwrap());
        assert_eq!(h.fields.len(), MAX_FIELDS);
        assert!(h.problems.iter().any(|p| p.contains("truncated")));
        assert_eq!(stats.bytes_read, (32 + 32 * MAX_FIELDS) as u64);
    }

    #[test]
    fn lone_dbf_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Owners.DBF");
        std::fs::write(&path, dbf_bytes(&[(b"name", b'C', 20, 0)], 4, 0)).unwrap();
        let cpg = dir.path().join("Owners.cpg");
        std::fs::write(&cpg, "UTF-8\n").unwrap();
        let ds = ResolvedDataset {
            format: Format::Dbf,
            anchor: path,
            sidecars: vec![ResolvedMember {
                role: MemberRole::Cpg,
                path: cpg,
            }],
        };
        let md = DbfReader.read(&ds, &ReadLimits::default()).unwrap();
        assert_eq!(md.status(), ExtractionStatus::Ok);
        let layer = &md.layers[0];
        assert_eq!(layer.name, "Owners");
        assert_eq!(layer.feature_count, Some(4));
        assert_eq!(layer.geometry_type, None);
        assert_eq!(layer.native_bounds, None);
        assert_eq!(layer.fields.len(), 1);

        let shp = ResolvedDataset {
            format: Format::Shapefile,
            ..ds
        };
        assert!(matches!(
            DbfReader.read(&shp, &ReadLimits::default()),
            Err(ReadError::Unsupported(_))
        ));
    }
}
