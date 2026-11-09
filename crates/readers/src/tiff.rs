//! Bounded TIFF/BigTIFF metadata only: no strip/tile or codec reads. Stage01/3.2.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use whereabouts_core::{
    Bounds, CrsDefinition, CrsDefinitionKind, DatasetMetadata, Format, FormatReader, LayerMetadata,
    MemberRole, RasterDataType, RasterInfo, ReadError, ReadLimits, ResolvedDataset,
};

use crate::io::{Budget, SourceFile};

const MAX_ENTRIES: u64 = 1024;
const MAX_IFDS: usize = 64;
const MAX_BLOCK: usize = 64 * 1024;
const MAX_TOTAL: u64 = 1024 * 1024;

/// TIFF/BigTIFF header reader, including GeoTIFF and supplied raster sidecars.
#[derive(Debug)]
pub struct TiffReader;

fn bad(reason: impl Into<String>) -> ReadError {
    ReadError::Malformed {
        format: Format::GeoTiff,
        reason: reason.into(),
    }
}

// Impossible ranges declared by file contents are malformed data. An I/O error after this
// check (for example a concurrent truncation) remains an I/O error for the pipeline's retry policy.
fn metadata_read(file: &mut SourceFile<'_>, offset: u64, len: usize) -> Result<Vec<u8>, ReadError> {
    if offset
        .checked_add(len as u64)
        .is_none_or(|end| end > file.len())
    {
        return Err(bad("truncated metadata or offset beyond EOF"));
    }
    file.read_at(offset, len)
}

#[derive(Clone, Copy)]
struct Layout {
    le: bool,
    big: bool,
}

impl Layout {
    fn uint(self, b: &[u8]) -> Result<u64, ReadError> {
        if !matches!(b.len(), 1 | 2 | 4 | 8) {
            return Err(bad("invalid integer width"));
        }
        Ok(if self.le {
            b.iter()
                .enumerate()
                .fold(0, |v, (i, x)| v | (u64::from(*x) << (8 * i)))
        } else {
            b.iter().fold(0, |v, x| (v << 8) | u64::from(*x))
        })
    }
    fn double(self, b: &[u8]) -> Result<f64, ReadError> {
        Ok(f64::from_bits(self.uint(b)?))
    }
}

struct Tag {
    kind: u16,
    data: Vec<u8>,
}
type Tags = BTreeMap<u16, Tag>;

struct Tiff<'a> {
    file: SourceFile<'a>,
    layout: Layout,
    first: u64,
}

impl<'a> Tiff<'a> {
    fn open(path: &'a Path, budget: &'a Budget) -> Result<Self, ReadError> {
        let mut file = SourceFile::open(path, budget)?;
        let h = metadata_read(&mut file, 0, 8)?;
        let le = match h.get(..2) {
            Some(b"II") => true,
            Some(b"MM") => false,
            _ => return Err(bad("TIFF byte order")),
        };
        let mut layout = Layout { le, big: false };
        let magic = layout.uint(&h[2..4])?;
        let first = match magic {
            42 => layout.uint(&h[4..8])?,
            43 => {
                if layout.uint(&h[4..6])? != 8 || layout.uint(&h[6..8])? != 0 {
                    return Err(bad("BigTIFF offset size/reserved"));
                }
                layout.big = true;
                layout.uint(&metadata_read(&mut file, 8, 8)?)?
            }
            _ => return Err(bad("TIFF magic")),
        };
        if first == 0 {
            return Err(bad("missing first IFD"));
        }
        Ok(Self {
            file,
            layout,
            first,
        })
    }

    fn ifd(&mut self, offset: u64, full: bool) -> Result<(Tags, u64), ReadError> {
        let l = self.layout;
        let (count_size, entry_size, inline) = if l.big { (8, 20, 8) } else { (2, 12, 4) };
        let count = l.uint(&metadata_read(&mut self.file, offset, count_size)?)?;
        if count > MAX_ENTRIES {
            return Err(bad("IFD entry limit exceeded"));
        }
        let size = usize::try_from(count).map_err(|_| bad("IFD count"))? * entry_size + inline;
        let start = offset
            .checked_add(count_size as u64)
            .ok_or_else(|| bad("IFD offset overflow"))?;
        let entries = metadata_read(&mut self.file, start, size)?;
        let mut tags = Tags::new();
        for e in entries[..size - inline].chunks_exact(entry_size) {
            let id = l.uint(&e[..2])? as u16;
            // Only metadata tags are loaded; offsets/counts of compressed imagery are never followed.
            if !(matches!(id, 254 | 256 | 257 | 330)
                || full
                    && matches!(
                        id,
                        258 | 259
                            | 277
                            | 339
                            | 33550
                            | 33922
                            | 34264
                            | 34735
                            | 34736
                            | 34737
                            | 42112
                            | 42113
                    ))
            {
                continue;
            }
            let kind = l.uint(&e[2..4])? as u16;
            let (n, value) = if l.big {
                (l.uint(&e[4..12])?, &e[12..20])
            } else {
                (l.uint(&e[4..8])?, &e[8..12])
            };
            let unit = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 | 16 | 17 | 18 => 8,
                _ => return Err(bad("unknown metadata field type")),
            };
            let bytes =
                n.checked_mul(unit)
                    .filter(|n| *n <= MAX_BLOCK as u64)
                    .ok_or_else(|| bad("metadata tag exceeds 64 KiB"))? as usize;
            let data = if bytes <= inline {
                value[..bytes].to_vec()
            } else {
                metadata_read(&mut self.file, l.uint(value)?, bytes)?
            };
            if tags.insert(id, Tag { kind, data }).is_some() {
                return Err(bad("duplicate metadata tag"));
            }
        }
        Ok((tags, l.uint(&entries[size - inline..])?))
    }

    fn overviews(
        &mut self,
        first: &Tags,
        next: u64,
        start: Instant,
        limits: &ReadLimits,
    ) -> Result<u32, ReadError> {
        let mut pending = integers(first, 330, self.layout)?;
        if next != 0 {
            pending.push(next);
        }
        let mut seen = BTreeSet::from([self.first]);
        let mut count = 0;
        while let Some(offset) = pending.pop() {
            if offset == 0 {
                continue;
            }
            if seen.len() >= MAX_IFDS {
                return Err(bad("IFD traversal limit exceeded"));
            }
            if !seen.insert(offset) {
                return Err(bad("cyclic/duplicate IFD reference"));
            }
            if start.elapsed() > limits.time_budget {
                return Err(bad("TIFF time budget exhausted"));
            }
            let (tags, next) = self.ifd(offset, false)?;
            let reduced = scalar(&tags, 254, self.layout)?.unwrap_or(0);
            if reduced & 1 != 0 && reduced & 4 == 0 {
                count += 1;
            }
            pending.extend(integers(&tags, 330, self.layout)?);
            if next != 0 {
                pending.push(next);
            }
        }
        Ok(count)
    }
}

fn integers(tags: &Tags, id: u16, l: Layout) -> Result<Vec<u64>, ReadError> {
    let Some(t) = tags.get(&id) else {
        return Ok(Vec::new());
    };
    let unit = match t.kind {
        1 => 1,
        3 => 2,
        4 | 13 => 4,
        16 | 18 => 8,
        _ => return Err(bad(format!("tag {id} needs unsigned integers"))),
    };
    t.data.chunks_exact(unit).map(|v| l.uint(v)).collect()
}
fn scalar(tags: &Tags, id: u16, l: Layout) -> Result<Option<u64>, ReadError> {
    let v = integers(tags, id, l)?;
    if v.len() > 1 {
        return Err(bad(format!("tag {id} is not scalar")));
    }
    Ok(v.first().copied())
}
fn doubles(tags: &Tags, id: u16, l: Layout) -> Result<Vec<f64>, ReadError> {
    let Some(t) = tags.get(&id) else {
        return Ok(Vec::new());
    };
    if t.kind != 12 {
        return Err(bad(format!("tag {id} needs doubles")));
    }
    t.data
        .as_chunks::<8>()
        .0
        .iter()
        .map(|v| {
            l.double(v).and_then(|v| {
                if v.is_finite() {
                    Ok(v)
                } else {
                    Err(bad("non-finite metadata double"))
                }
            })
        })
        .collect()
}
fn text(tags: &Tags, id: u16) -> Result<Option<String>, ReadError> {
    tags.get(&id)
        .map(|t| {
            if t.kind != 2 {
                return Err(bad(format!("tag {id} needs ASCII")));
            }
            std::str::from_utf8(&t.data)
                .map(|s| s.trim_end_matches('\0').to_owned())
                .map_err(|_| bad("invalid ASCII/UTF-8 metadata"))
        })
        .transpose()
}

fn affine(tags: &Tags, l: Layout, point: bool) -> Result<Option<[f64; 6]>, ReadError> {
    let m = doubles(tags, 34264, l)?;
    let scale = doubles(tags, 33550, l)?;
    let tie = doubles(tags, 33922, l)?;
    let mut g = if !scale.is_empty() && !tie.is_empty() {
        if scale.len() != 3
            || tie.len() < 6
            || tie.len() % 6 != 0
            || scale[0] == 0.0
            || scale[1] == 0.0
        {
            return Err(bad("invalid scale/tiepoint"));
        }
        [
            tie[3] - tie[0] * scale[0],
            scale[0],
            0.0,
            tie[4] + tie[1] * scale[1],
            0.0,
            -scale[1],
        ]
    } else if !m.is_empty() {
        if m.len() != 16 || m[12..16] != [0.0, 0.0, 0.0, 1.0] {
            return Err(bad("invalid affine transformation matrix"));
        }
        [m[3], m[0], m[1], m[7], m[4], m[5]]
    } else {
        return Ok(None);
    };
    if point {
        g[0] -= (g[1] + g[2]) * 0.5;
        g[3] -= (g[4] + g[5]) * 0.5;
    }
    validate_affine(g).map(Some)
}
fn validate_affine(g: [f64; 6]) -> Result<[f64; 6], ReadError> {
    if g.iter().any(|v| !v.is_finite()) || g[1] * g[5] - g[2] * g[4] == 0.0 {
        return Err(bad("invalid/singular geotransform"));
    }
    Ok(g)
}
fn bounds(g: [f64; 6], w: u64, h: u64) -> Result<Bounds, ReadError> {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for (x, y) in [
        (0.0, 0.0),
        (w as f64, 0.0),
        (0.0, h as f64),
        (w as f64, h as f64),
    ] {
        let (x, y) = (g[0] + x * g[1] + y * g[2], g[3] + x * g[4] + y * g[5]);
        if !x.is_finite() || !y.is_finite() {
            return Err(bad("non-finite transformed raster corner"));
        }
        b[0] = b[0].min(x);
        b[1] = b[1].min(y);
        b[2] = b[2].max(x);
        b[3] = b[3].max(y);
    }
    Bounds::new(b[0], b[1], b[2], b[3]).map_err(|e| bad(e.to_string()))
}

fn read_text(path: &Path, budget: &Budget) -> Result<String, ReadError> {
    let mut f = SourceFile::open(path, budget)?;
    if f.len() > MAX_BLOCK as u64 {
        return Err(bad("sidecar exceeds 64 KiB"));
    }
    String::from_utf8(f.read_prefix(MAX_BLOCK)?).map_err(|_| bad("sidecar is not UTF-8"))
}

#[derive(Default)]
struct Pam {
    crs: Option<CrsDefinition>,
    affine: Option<[f64; 6]>,
    nodata: Option<String>,
}
fn pam(xml: &str) -> Result<Pam, ReadError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| bad(format!("PAM XML: {e}")))?;
    if doc.root_element().tag_name().name() != "PAMDataset" {
        return Err(bad("PAMDataset root missing"));
    }
    let mut p = Pam::default();
    for n in doc.root_element().children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "SRS" => {
                if let Some(s) = n.text().filter(|s| !s.trim().is_empty()) {
                    p.crs = Some(crate::prj::parse_prj(s.as_bytes()).map_err(bad)?);
                }
            }
            "GeoTransform" => {
                let values = numbers(n.text().unwrap_or_default(), ',')?;
                let g: [f64; 6] = values
                    .try_into()
                    .map_err(|_| bad("PAM geotransform needs six numbers"))?;
                p.affine = Some(validate_affine(g)?);
            }
            "PAMRasterBand" if n.attribute("band") == Some("1") => {
                p.nodata = n
                    .children()
                    .find(|n| n.has_tag_name("NoDataValue"))
                    .and_then(|n| n.text())
                    .map(|s| s.trim().to_owned());
            }
            _ => {}
        }
    }
    Ok(p)
}
fn numbers(s: &str, sep: char) -> Result<Vec<f64>, ReadError> {
    s.split(sep)
        .map(|s| {
            s.trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or_else(|| bad("invalid affine number"))
        })
        .collect()
}
fn world(s: &str) -> Result<[f64; 6], ReadError> {
    let v: Vec<f64> = s
        .split_whitespace()
        .map(|s| {
            s.parse::<f64>()
                .map_err(|_| bad("invalid world file number"))
        })
        .collect::<Result<_, _>>()?;
    let [a, d, b, e, c, f]: [f64; 6] = v
        .try_into()
        .map_err(|_| bad("world file needs six numbers"))?;
    validate_affine([c - (a + b) * 0.5, a, b, f - (d + e) * 0.5, d, e])
}

fn dtype(bits: u64, sample: u64) -> RasterDataType {
    use RasterDataType::*;
    match (bits, sample) {
        (1..=8, 1) => Byte,
        (16, 1) => UInt16,
        (32, 1) => UInt32,
        (64, 1) => UInt64,
        (8, 2) => Int8,
        (16, 2) => Int16,
        (32, 2) => Int32,
        (64, 2) => Int64,
        (16, 3) => Float16,
        (32, 3) => Float32,
        (64, 3) => Float64,
        (32, 5) => CInt16,
        (64, 5) => CInt32,
        (64, 6) => CFloat32,
        (128, 6) => CFloat64,
        _ => Unknown,
    }
}

impl FormatReader for TiffReader {
    fn name(&self) -> &'static str {
        "tiff"
    }
    fn formats(&self) -> &'static [Format] {
        &[Format::GeoTiff]
    }
    fn read(
        &self,
        ds: &ResolvedDataset,
        limits: &ReadLimits,
    ) -> Result<DatasetMetadata, ReadError> {
        let start = Instant::now();
        let budget = Budget::new(limits.max_bytes.min(MAX_TOTAL));
        let mut t = Tiff::open(&ds.anchor, &budget)?;
        let (tags, next) = t.ifd(t.first, true)?;
        let l = t.layout;
        let width = scalar(&tags, 256, l)?
            .filter(|n| *n > 0)
            .ok_or_else(|| bad("missing/zero width"))?;
        let height = scalar(&tags, 257, l)?
            .filter(|n| *n > 0)
            .ok_or_else(|| bad("missing/zero height"))?;
        let bands = u32::try_from(scalar(&tags, 277, l)?.unwrap_or(1))
            .ok()
            .filter(|n| *n > 0 && *n <= 1024)
            .ok_or_else(|| bad("invalid band count"))?;
        let mut partial = Vec::new();
        let mut overview_count = match t.overviews(&tags, next, start, limits) {
            Ok(n) => n,
            Err(e) => {
                partial.push(e.to_string());
                0
            }
        };
        if let Some(path) = ds.sidecar(MemberRole::Ovr) {
            let ext = (|| {
                let mut t = Tiff::open(path, &budget)?;
                let (tags, next) = t.ifd(t.first, false)?;
                Ok::<_, ReadError>(1 + t.overviews(&tags, next, start, limits)?)
            })();
            match ext {
                Ok(n) => overview_count += n,
                Err(e) => partial.push(e.to_string()),
            }
        }
        let bits = integers(&tags, 258, l)?;
        let sample = integers(&tags, 339, l)?;
        for (id, values) in [(258, &bits), (339, &sample)] {
            if !values.is_empty() && values.len() != 1 && values.len() != bands as usize {
                return Err(bad(format!("tag {id} count differs from samples")));
            }
        }
        let bits0 = bits.first().copied().unwrap_or(1);
        let sample0 = sample.first().copied().unwrap_or(1);
        let dtype = if bits.iter().any(|b| *b != bits0) || sample.iter().any(|s| *s != sample0) {
            RasterDataType::Unknown
        } else {
            dtype(bits0, sample0)
        };
        // Read/validate compression even though metadata extraction does not decode it.
        let _compression = scalar(&tags, 259, l)?;
        let directory = integers(&tags, 34735, l)?;
        let point = if directory.is_empty() {
            false
        } else {
            if tags.get(&34735).is_none_or(|t| t.kind != 3)
                || directory.len() < 4
                || directory[0] != 1
                || directory[1] != 1
                || directory[2] > 1
            {
                return Err(bad("invalid GeoKeyDirectory header/type"));
            }
            let end = 4 + directory[3] as usize * 4;
            let keys = directory
                .get(4..end)
                .ok_or_else(|| bad("truncated GeoKeyDirectory"))?;
            keys.as_chunks::<4>().0.contains(&[1025, 0, 1, 2])
        };
        let mut crs = if directory.is_empty() {
            None
        } else {
            let doubles = doubles(&tags, 34736, l)?;
            let ascii = match tags.get(&34737) {
                Some(tag) if tag.kind == 2 => std::str::from_utf8(&tag.data)
                    .map_err(|_| bad("invalid GeoASCII"))?
                    .to_owned(),
                Some(_) => return Err(bad("GeoASCII needs ASCII type")),
                None => String::new(),
            };
            Some(CrsDefinition::new(
                CrsDefinitionKind::GeoKeys,
                serde_json::json!({"directory":directory,"doubles":doubles,"ascii":ascii})
                    .to_string(),
            ))
        };
        let mut g = affine(&tags, l, point)?;
        let mut nodata = text(&tags, 42113)?;
        if let Some(xml) = text(&tags, 42112)? {
            // GDAL_METADATA is bounded and parsed as XML; the frozen core has no generic metadata field.
            roxmltree::Document::parse(&xml).map_err(|e| bad(format!("GDAL_METADATA: {e}")))?;
        }
        if let Some(path) = ds.sidecar(MemberRole::AuxXml) {
            match read_text(path, &budget).and_then(|s| pam(&s)) {
                Ok(p) => {
                    crs = p.crs.or(crs);
                    g = p.affine.or(g);
                    nodata = p.nodata.or(nodata);
                }
                Err(e) => partial.push(e.to_string()),
            }
        }
        if g.is_none() {
            for path in ds.sidecars_with(MemberRole::WorldFile) {
                match read_text(path, &budget).and_then(|s| world(&s)) {
                    Ok(v) => {
                        g = Some(v);
                        break;
                    }
                    Err(e) => partial.push(e.to_string()),
                }
            }
        }
        if crs.is_none()
            && let Some(path) = ds.sidecar(MemberRole::Prj)
        {
            match crate::prj::read_prj(path, &budget) {
                Ok(v) => crs = Some(v),
                Err(e) => partial.push(e),
            }
        }
        if start.elapsed() > limits.time_budget {
            partial.push("TIFF time budget exhausted".into());
        }
        let native_bounds = g.map(|g| bounds(g, width, height)).transpose()?;
        let raster = RasterInfo {
            width,
            height,
            bands,
            dtype,
            nodata,
            overview_count,
            res_x: g.map(|g| g[1].hypot(g[4])),
            res_y: g.map(|g| g[2].hypot(g[5])),
            geotransform: g,
        };
        Ok(DatasetMetadata {
            layers: vec![LayerMetadata {
                name: crate::layer_name(&ds.anchor),
                crs,
                native_bounds,
                raster: Some(raster),
                ..LayerMetadata::default()
            }],
            partial,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use whereabouts_core::ResolvedMember;

    fn fixture(big: bool, le: bool, extra: &[(u16, u16, u64, Vec<u8>)]) -> Vec<u8> {
        let put = |n: u64, size: usize| -> Vec<u8> {
            if le {
                n.to_le_bytes()[..size].to_vec()
            } else {
                n.to_be_bytes()[8 - size..].to_vec()
            }
        };
        let (head, entry, inline, count_size) = if big { (16, 20, 8, 8) } else { (8, 12, 4, 2) };
        let mut tags = vec![
            (256, 4, 1, put(20, 4)),
            (257, 4, 1, put(10, 4)),
            (258, 3, 1, put(8, 2)),
            (277, 3, 1, put(1, 2)),
        ];
        tags.extend_from_slice(extra);
        tags.sort_by_key(|t| t.0);
        let mut bytes = if le { b"II".to_vec() } else { b"MM".to_vec() };
        bytes.extend(put(if big { 43 } else { 42 }, 2));
        if big {
            bytes.extend(put(8, 2));
            bytes.extend([0, 0]);
        }
        bytes.extend(put(head, inline));
        bytes.extend(put(tags.len() as u64, count_size));
        let mut tail: Vec<u8> = Vec::new();
        for (id, kind, count, value) in &tags {
            bytes.extend(put(u64::from(*id), 2));
            bytes.extend(put(u64::from(*kind), 2));
            bytes.extend(put(*count, if big { 8 } else { 4 }));
            if value.len() <= inline {
                bytes.extend(value);
                bytes.resize(bytes.len() + inline - value.len(), 0);
            } else {
                bytes.extend(put(
                    head + count_size as u64 + (tags.len() * entry + inline + tail.len()) as u64,
                    inline,
                ));
                tail.extend(value);
            }
        }
        bytes.extend(vec![0; inline]);
        bytes.extend(tail);
        bytes
    }
    fn dataset(path: &Path) -> ResolvedDataset {
        ResolvedDataset {
            format: Format::GeoTiff,
            anchor: path.into(),
            sidecars: vec![],
        }
    }

    #[test]
    fn classic_and_bigtiff_both_endians_and_bounded_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.tif");
        for big in [false, true] {
            for le in [false, true] {
                std::fs::write(&path, fixture(big, le, &[])).unwrap();
                let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.set_len(100_000_000).unwrap();
                let (md, stats) =
                    crate::measure(|| TiffReader.read(&dataset(&path), &ReadLimits::default()));
                let md = md.unwrap();
                let r = md.layers[0].raster.as_ref().unwrap();
                assert_eq!((r.width, r.height, r.dtype), (20, 10, RasterDataType::Byte));
                assert!(stats.bytes_read < 160, "{stats:?}");
                assert!(
                    TiffReader
                        .read(
                            &dataset(&path),
                            &ReadLimits {
                                max_bytes: 10,
                                ..ReadLimits::default()
                            }
                        )
                        .is_err()
                );
            }
        }
    }
    #[test]
    fn pixel_scale_signs_preserve_horizontal_and_vertical_flips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flipped.tif");
        let doubles = |v: &[f64]| v.iter().flat_map(|v| v.to_le_bytes()).collect();
        for sx in [-2.0, 2.0] {
            for sy in [-3.0, 3.0] {
                std::fs::write(
                    &path,
                    fixture(
                        false,
                        true,
                        &[
                            (33550, 12, 3, doubles(&[sx, sy, 0.0])),
                            (33922, 12, 6, doubles(&[4.0, 5.0, 0.0, 100.0, 200.0, 0.0])),
                        ],
                    ),
                )
                .unwrap();
                let md = TiffReader
                    .read(&dataset(&path), &ReadLimits::default())
                    .unwrap();
                let r = md.layers[0].raster.as_ref().unwrap();
                assert_eq!(
                    r.geotransform,
                    Some([100.0 - 4.0 * sx, sx, 0.0, 200.0 + 5.0 * sy, 0.0, -sy])
                );
                let b = md.layers[0].native_bounds.unwrap();
                let xs = [100.0 - 4.0 * sx, 100.0 + 16.0 * sx];
                let ys = [200.0 + 5.0 * sy, 200.0 - 5.0 * sy];
                assert_eq!(
                    [b.min_x(), b.min_y(), b.max_x(), b.max_y()],
                    [
                        xs[0].min(xs[1]),
                        ys[0].min(ys[1]),
                        xs[0].max(xs[1]),
                        ys[0].max(ys[1])
                    ]
                );
            }
        }
    }

    #[test]
    fn rotations_point_shift_and_world_file_centres() {
        let g = world("2\n1\n-1\n-2\n10\n20").unwrap();
        assert_eq!(g, [9.5, 2.0, -1.0, 20.5, 1.0, -2.0]);
        let b = bounds(g, 20, 10).unwrap();
        assert_eq!(
            [b.min_x(), b.min_y(), b.max_x(), b.max_y()],
            [-0.5, 0.5, 49.5, 40.5]
        );
        let mut tags = Tags::new();
        let doubles = |values: &[f64]| Tag {
            kind: 12,
            data: values.iter().flat_map(|x| x.to_le_bytes()).collect(),
        };
        tags.insert(
            34264,
            doubles(&[
                2.0, -1.0, 0.0, 10.0, 1.0, -2.0, 0.0, 20.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ]),
        );
        assert_eq!(
            affine(
                &tags,
                Layout {
                    le: true,
                    big: false
                },
                true
            )
            .unwrap(),
            Some(g)
        );
        for s in ["1 2", "nan 0 0 -1 0 0", "0 0 0 0 0 0", "1 0 0 -1 0 inf"] {
            assert!(world(s).is_err());
        }
    }

    #[test]
    fn linked_internal_and_external_overviews_are_counted_without_imagery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.tif");
        let mut bytes = fixture(false, true, &[]);
        let next = bytes.len();
        bytes[next - 4..next].copy_from_slice(&(next as u32).to_le_bytes());
        let reduced = fixture(false, true, &[(254, 4, 1, 1u32.to_le_bytes().to_vec())]);
        bytes.extend_from_slice(&reduced[8..]);
        std::fs::write(&path, bytes).unwrap();
        let ovr = dir.path().join("test.tif.ovr");
        std::fs::write(&ovr, reduced).unwrap();
        let mut ds = dataset(&path);
        ds.sidecars.push(ResolvedMember {
            role: MemberRole::Ovr,
            path: ovr,
        });
        let md = TiffReader.read(&ds, &ReadLimits::default()).unwrap();
        assert!(md.partial.is_empty(), "{:?}", md.partial);
        assert_eq!(md.layers[0].raster.as_ref().unwrap().overview_count, 2);
    }
    #[test]
    fn pam_precedes_internal_and_internal_precedes_world() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.tif");
        let doubles = |v: &[f64]| v.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(
            &path,
            fixture(
                false,
                true,
                &[
                    (33550, 12, 3, doubles(&[2.0, 2.0, 0.0])),
                    (33922, 12, 6, doubles(&[0.0, 0.0, 0.0, 10.0, 20.0, 0.0])),
                ],
            ),
        )
        .unwrap();
        let world = dir.path().join("test.tfw");
        std::fs::write(&world, "5 0 0 -5 100 200").unwrap();
        let aux = dir.path().join("test.tif.aux.xml");
        std::fs::write(
            &aux,
            "<PAMDataset><GeoTransform>1000,10,0,2000,0,-10</GeoTransform></PAMDataset>",
        )
        .unwrap();
        let mut ds = dataset(&path);
        ds.sidecars.push(ResolvedMember {
            role: MemberRole::WorldFile,
            path: world,
        });
        let get = |ds: &ResolvedDataset| {
            TiffReader.read(ds, &ReadLimits::default()).unwrap().layers[0]
                .raster
                .as_ref()
                .unwrap()
                .geotransform
                .unwrap()
        };
        assert_eq!(get(&ds), [10.0, 2.0, 0.0, 20.0, 0.0, -2.0]);
        ds.sidecars.push(ResolvedMember {
            role: MemberRole::AuxXml,
            path: aux.clone(),
        });
        assert_eq!(get(&ds), [1000.0, 10.0, 0.0, 2000.0, 0.0, -10.0]);
        std::fs::write(
            aux,
            "<!DOCTYPE x [<!ENTITY x SYSTEM 'file:///x'>]><PAMDataset><SRS>&x;</SRS></PAMDataset>",
        )
        .unwrap();
        let md = TiffReader.read(&ds, &ReadLimits::default()).unwrap();
        assert!(!md.partial.is_empty());
    }
    #[test]
    fn corrupt_offsets_counts_cycles_and_truncations_never_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.tif");
        let base = fixture(false, true, &[]);
        for len in 0..base.len() {
            std::fs::write(&path, &base[..len]).unwrap();
            assert!(
                TiffReader
                    .read(&dataset(&path), &ReadLimits::default())
                    .is_err(),
                "{len}"
            );
        }
        let mut corrupt = base.clone();
        corrupt[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
        std::fs::write(&path, &corrupt).unwrap();
        assert!(
            TiffReader
                .read(&dataset(&path), &ReadLimits::default())
                .is_err()
        );
        let mut cyclic = base.clone();
        let n = cyclic.len();
        cyclic[n - 4..].copy_from_slice(&8u32.to_le_bytes());
        std::fs::write(&path, cyclic).unwrap();
        assert!(
            !TiffReader
                .read(&dataset(&path), &ReadLimits::default())
                .unwrap()
                .partial
                .is_empty()
        );
        // Byte mutation sweep exercises every header, entry and count without a decoding dependency.
        for pos in 0..base.len() {
            let mut b = base.clone();
            b[pos] = 255;
            std::fs::write(&path, b).unwrap();
            let _ = TiffReader.read(&dataset(&path), &ReadLimits::default());
        }
    }
}
