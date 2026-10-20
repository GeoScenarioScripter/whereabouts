//! ESRI Shapefile reader: header facts from `.shp`, `.shx`, `.dbf`, `.prj` and `.cpg`.
//!
//! Reads per dataset (never the records themselves):
//!
//! | Member | Bytes | Facts |
//! |---|---|---|
//! | `.shp` | 100 (main header) | shape type → geometry type + Z/M, bbox, declared file length |
//! | `.shx` | 100 (same header) | shape count = (file length − 100) / 8: the feature count, as in GDAL |
//! | `.dbf` | 32 + 32 per field (≤ 1,024 fields) | record count (the fallback feature count), fields, LDID |
//! | `.cpg` | ≤ 64 | DBF encoding label (wins over the LDID) |
//! | `.prj` | ≤ 16 KiB | raw CRS definition, ESRI vs OGC WKT1 detected (`crate::prj`) |
//!
//! So a dataset costs at most about 49 KiB of reads, whatever the file sizes; typically < 1 KiB.
//!
//! Outcomes:
//! - The `.shp` header is the one thing the reader cannot do without: missing, short, a wrong file
//!   code, an unknown shape type or a declared length beyond the end of the file (truncated
//!   records) is [`ReadError::Malformed`]. GDAL refuses those files too.
//! - Problems in the other members degrade the result to `partial` with a reason, keeping
//!   whatever is still known (a corrupt `.dbf` loses the fields; a missing `.shx` leaves the
//!   `.dbf` count). Count disagreements between the members are `partial` too.
//! - The extent is the header bbox, except that a `.shp` without records (declared length 100)
//!   reports none: its header bbox is all zeros (true-extent convention, D-33).

use std::path::Path;

use whereabouts_core::{
    Bounds, DatasetMetadata, Format, FormatReader, GeometryKind, GeometryType, LayerMetadata,
    MemberRole, ReadError, ReadLimits, ResolvedDataset,
};

use crate::dbf;
use crate::io::{Budget, SourceFile};
use crate::prj;

/// Size of the `.shp`/`.shx` main header.
const HEADER_LEN: usize = 100;
/// Big-endian file code at offset 0.
const FILE_CODE: i32 = 9994;
/// Size of one `.shx` index record.
const SHX_RECORD_LEN: u64 = 8;
/// ESRI's "no data" for measures: any value below -10^38.
const M_NO_DATA: f64 = -1e38;

/// Reader for ESRI Shapefiles ([`Format::Shapefile`]): one layer named after the `.shp` stem.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShapefileReader;

impl FormatReader for ShapefileReader {
    fn name(&self) -> &'static str {
        "shapefile"
    }

    fn formats(&self) -> &'static [Format] {
        &[Format::Shapefile]
    }

    fn read(
        &self,
        dataset: &ResolvedDataset,
        limits: &ReadLimits,
    ) -> Result<DatasetMetadata, ReadError> {
        if dataset.format != Format::Shapefile {
            return Err(ReadError::Unsupported(format!(
                "shapefile reader cannot read {} datasets",
                dataset.format
            )));
        }
        read_shapefile(dataset, &Budget::new(limits.max_bytes))
    }
}

fn read_shapefile(ds: &ResolvedDataset, budget: &Budget) -> Result<DatasetMetadata, ReadError> {
    let shp_path = ds.anchor.as_path();
    let header = read_main_header(shp_path, budget, ".shp")
        .and_then(|(header, len)| {
            if header.file_len > len {
                Err(Fault::Invalid(format!(
                    "truncated: the header declares {} bytes but the file has {len}",
                    header.file_len
                )))
            } else {
                Ok(header)
            }
        })
        .map_err(|reason| match reason {
            Fault::Io(e) => e,
            Fault::Invalid(reason) => malformed(shp_path, &reason),
        })?;
    let geometry_type =
        geometry_type(header.shape_type, header.m_range).map_err(|r| malformed(shp_path, &r))?;
    let mut partial = Vec::new();

    let shx_count = match ds.sidecar(MemberRole::Shx) {
        None => {
            partial.push("no .shx member: feature count taken from the .dbf".to_owned());
            None
        }
        Some(p) => read_shx_count(p, budget)
            .map_err(|r| partial.push(r.into_reason(p)))
            .ok(),
    };

    let cpg = ds
        .sidecar(MemberRole::Cpg)
        .and_then(|p| dbf::read_cpg(p, budget).map_err(|r| partial.push(r)).ok());
    let (dbf_count, fields) = match ds.sidecar(MemberRole::Dbf) {
        None => {
            partial.push("no .dbf member: attribute fields unknown".to_owned());
            (None, Vec::new())
        }
        Some(p) => match dbf::read_header(p, budget, Format::Shapefile) {
            Ok(h) => {
                let (fields, problems) = h.field_infos(cpg.as_deref());
                partial.extend(h.problems);
                partial.extend(problems);
                (h.record_count, fields)
            }
            Err(e) => {
                partial.push(e.to_string());
                (None, Vec::new())
            }
        },
    };

    // Like GDAL: the .shx (one entry per shape) is the count, the .dbf the fallback.
    let feature_count = match (shx_count, dbf_count) {
        (Some(s), Some(d)) if s != d => {
            partial.push(format!(".shx indexes {s} shapes but .dbf has {d} records"));
            Some(s)
        }
        (Some(s), _) => Some(s),
        (None, Some(d)) => Some(d),
        (None, None) => {
            partial.push("feature count unknown: no readable .shx or .dbf".to_owned());
            None
        }
    };

    // Whether there are records is decided by the .shp alone, so a sidecar can never erase a
    // valid bbox (or invent one for an empty file, whose header bbox is all zeros).
    let has_records = header.file_len > HEADER_LEN as u64;
    match feature_count {
        Some(n) if n > 0 && !has_records => {
            partial.push(format!(".shp has no records but the feature count is {n}"))
        }
        Some(0) if has_records => {
            partial.push(".shp has records but the feature count is 0".to_owned());
        }
        _ => {}
    }
    let native_bounds = if geometry_type.is_some() && has_records {
        let [min_x, min_y, max_x, max_y] = header.bbox;
        match Bounds::new(min_x, min_y, max_x, max_y) {
            Ok(b) => Some(b),
            Err(e) => {
                partial.push(format!(
                    ".shp header bbox is unusable ({e}); extent unknown"
                ));
                None
            }
        }
    } else {
        None
    };

    let crs = ds
        .sidecar(MemberRole::Prj)
        .and_then(|p| prj::read_prj(p, budget).map_err(|r| partial.push(r)).ok());

    let layer = LayerMetadata {
        name: crate::layer_name(shp_path),
        geometry_type,
        feature_count,
        crs,
        native_bounds,
        fields,
        raster: None,
    };
    Ok(DatasetMetadata {
        layers: vec![layer],
        partial,
    })
}

fn malformed(path: &Path, reason: &str) -> ReadError {
    ReadError::Malformed {
        format: Format::Shapefile,
        reason: format!("{}: {reason}", path.display()),
    }
}

/// Why a header could not be used.
enum Fault {
    /// Could not be read at all.
    Io(ReadError),
    /// Read, but invalid.
    Invalid(String),
}

impl Fault {
    fn into_reason(self, path: &Path) -> String {
        match self {
            Fault::Io(e) => e.to_string(),
            Fault::Invalid(r) => format!("{}: {r}", path.display()),
        }
    }
}

impl From<String> for Fault {
    fn from(reason: String) -> Self {
        Fault::Invalid(reason)
    }
}

/// The 100-byte main header shared by `.shp` and `.shx`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MainHeader {
    /// File length declared by the header, in bytes.
    pub(crate) file_len: u64,
    /// Shape type code.
    pub(crate) shape_type: i32,
    /// `[min_x, min_y, max_x, max_y]`.
    pub(crate) bbox: [f64; 4],
    /// `[min_m, max_m]`.
    pub(crate) m_range: [f64; 2],
}

impl MainHeader {
    /// Parses a main header. `bytes` must hold at least [`HEADER_LEN`] bytes.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        let short = || format!("header is {} bytes, expected {HEADER_LEN}", bytes.len());
        let code = be_i32(bytes, 0).ok_or_else(short)?;
        if code != FILE_CODE {
            return Err(format!(
                "not a shapefile header (file code {code}, expected {FILE_CODE})"
            ));
        }
        let words = be_i32(bytes, 24).ok_or_else(short)?;
        // The length counts 16-bit words and includes the header itself.
        let file_len = u64::try_from(words)
            .ok()
            .map(|w| w * 2)
            .filter(|&len| len >= HEADER_LEN as u64)
            .ok_or_else(|| {
                format!("header declares a file length of {words} words, shorter than the header")
            })?;
        let shape_type = le_i32(bytes, 32).ok_or_else(short)?;
        let f = |at| le_f64(bytes, at).ok_or_else(short);
        Ok(MainHeader {
            file_len,
            shape_type,
            bbox: [f(36)?, f(44)?, f(52)?, f(60)?],
            m_range: [f(84)?, f(92)?],
        })
    }
}

/// Opens `path` and parses its main header; also returns the actual file length.
fn read_main_header(path: &Path, budget: &Budget, what: &str) -> Result<(MainHeader, u64), Fault> {
    let mut file = SourceFile::open(path, budget).map_err(Fault::Io)?;
    let len = file.len();
    if len < HEADER_LEN as u64 {
        return Err(Fault::Invalid(format!(
            "{what} is {len} bytes, shorter than its {HEADER_LEN}-byte header"
        )));
    }
    let bytes = file.read_at(0, HEADER_LEN).map_err(Fault::Io)?;
    Ok((MainHeader::parse(&bytes)?, len))
}

/// Shape count from the `.shx` header: (declared length − 100) / 8, checked against the file.
fn read_shx_count(path: &Path, budget: &Budget) -> Result<u64, Fault> {
    let (header, len) = read_main_header(path, budget, ".shx")?;
    let body = header.file_len - HEADER_LEN as u64;
    if !body.is_multiple_of(SHX_RECORD_LEN) {
        return Err(Fault::Invalid(format!(
            ".shx length {} is not a whole number of 8-byte index records",
            header.file_len
        )));
    }
    if header.file_len > len {
        return Err(Fault::Invalid(format!(
            ".shx truncated: the header declares {} bytes but the file has {len}",
            header.file_len
        )));
    }
    Ok(body / SHX_RECORD_LEN)
}

/// Layer geometry type for a shape type code, as GDAL reports it: PolyLine → `LineString`,
/// Polygon → `Polygon` (not multi), MultiPatch → `PolyhedralSurface Z`. `None` for the Null
/// shape type. Z types carry M only if the header's M range is set (not 0/0 and not ESRI
/// no-data), since writers that have no measures leave it so.
pub(crate) fn geometry_type(
    shape_type: i32,
    m_range: [f64; 2],
) -> Result<Option<GeometryType>, String> {
    use GeometryKind as K;
    let (kind, has_z, declared_m) = match shape_type {
        0 => return Ok(None),
        1 => (K::Point, false, false),
        3 => (K::LineString, false, false),
        5 => (K::Polygon, false, false),
        8 => (K::MultiPoint, false, false),
        11 => (K::Point, true, false),
        13 => (K::LineString, true, false),
        15 => (K::Polygon, true, false),
        18 => (K::MultiPoint, true, false),
        21 => (K::Point, false, true),
        23 => (K::LineString, false, true),
        25 => (K::Polygon, false, true),
        28 => (K::MultiPoint, false, true),
        31 => (K::PolyhedralSurface, true, false),
        other => return Err(format!("unknown shape type {other}")),
    };
    let has_m = declared_m || (has_z && header_has_measures(m_range));
    Ok(Some(GeometryType { kind, has_z, has_m }))
}

fn header_has_measures([min, max]: [f64; 2]) -> bool {
    let no_data = |v: f64| !v.is_finite() || v < M_NO_DATA;
    let unset = (no_data(min) && no_data(max)) || (min == 0.0 && max == 0.0);
    !unset
}

fn be_i32(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le_i32(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le_f64(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::PathBuf;

    use whereabouts_core::{CrsDefinitionKind, ExtractionStatus, ResolvedMember};

    use super::*;
    use crate::dbf::tests::dbf_bytes;
    use crate::io::measure;

    const PRJ: &str = r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;

    /// A main header for `shape_type` declaring `total_len` bytes, padded with zeros to
    /// `total_len` (so it doubles as a `.shx` with `(total_len - 100) / 8` entries).
    fn main_header(shape_type: i32, bbox: [f64; 4], m: [f64; 2], total_len: usize) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&FILE_CODE.to_be_bytes());
        let words = i32::try_from(total_len / 2).unwrap();
        b[24..28].copy_from_slice(&words.to_be_bytes());
        b[28..32].copy_from_slice(&1000i32.to_le_bytes());
        b[32..36].copy_from_slice(&shape_type.to_le_bytes());
        for (i, v) in bbox.iter().enumerate() {
            b[36 + 8 * i..44 + 8 * i].copy_from_slice(&v.to_le_bytes());
        }
        b[84..92].copy_from_slice(&m[0].to_le_bytes());
        b[92..100].copy_from_slice(&m[1].to_le_bytes());
        b.resize(total_len.max(HEADER_LEN), 0);
        b
    }

    const BBOX: [f64; 4] = [10.0, -5.0, 20.5, 7.25];

    struct Files {
        shp: Option<Vec<u8>>,
        shx: Option<Vec<u8>>,
        dbf: Option<Vec<u8>>,
        prj: Option<Vec<u8>>,
        cpg: Option<Vec<u8>>,
    }

    impl Files {
        /// A consistent polygon shapefile with `n` records.
        fn polygons(n: u32) -> Self {
            Files {
                shp: Some(main_header(5, BBOX, [0.0; 2], 100 + 136 * n as usize)),
                shx: Some(main_header(5, BBOX, [0.0; 2], 100 + 8 * n as usize)),
                dbf: Some(dbf_bytes(
                    &[(b"id", b'N', 10, 0), (b"name", b'C', 32, 0)],
                    n,
                    87,
                )),
                prj: Some(PRJ.as_bytes().to_vec()),
                cpg: Some(b"UTF-8".to_vec()),
            }
        }

        fn write(&self, dir: &Path, stem: &str) -> ResolvedDataset {
            let mut sidecars = Vec::new();
            let mut put = |ext: &str, bytes: &Option<Vec<u8>>, role: Option<MemberRole>| {
                let path = dir.join(format!("{stem}.{ext}"));
                if let Some(bytes) = bytes {
                    std::fs::write(&path, bytes).unwrap();
                    if let Some(role) = role {
                        sidecars.push(ResolvedMember { role, path });
                    }
                }
                dir.join(format!("{stem}.{ext}"))
            };
            let anchor: PathBuf = put("shp", &self.shp, None);
            put("shx", &self.shx, Some(MemberRole::Shx));
            put("dbf", &self.dbf, Some(MemberRole::Dbf));
            put("prj", &self.prj, Some(MemberRole::Prj));
            put("cpg", &self.cpg, Some(MemberRole::Cpg));
            ResolvedDataset {
                format: Format::Shapefile,
                anchor,
                sidecars,
            }
        }

        fn read(&self) -> Result<DatasetMetadata, ReadError> {
            self.read_with(&ReadLimits::default())
        }

        fn read_with(&self, limits: &ReadLimits) -> Result<DatasetMetadata, ReadError> {
            let dir = tempfile::tempdir().unwrap();
            let ds = self.write(dir.path(), "layer");
            ShapefileReader.read(&ds, limits)
        }
    }

    fn only_layer(md: &DatasetMetadata) -> &LayerMetadata {
        assert_eq!(md.layers.len(), 1);
        &md.layers[0]
    }

    #[test]
    fn reads_a_complete_shapefile() {
        let md = Files::polygons(3).read().unwrap();
        assert_eq!(md.status(), ExtractionStatus::Ok, "{:?}", md.partial);
        let l = only_layer(&md);
        assert_eq!(l.name, "layer");
        assert_eq!(
            l.geometry_type,
            Some(GeometryType::xy(GeometryKind::Polygon))
        );
        assert_eq!(l.feature_count, Some(3));
        let b = l.native_bounds.unwrap();
        assert_eq!([b.min_x(), b.min_y(), b.max_x(), b.max_y()], BBOX);
        let crs = l.crs.as_ref().unwrap();
        assert_eq!(crs.kind, CrsDefinitionKind::Wkt1Esri);
        assert_eq!(crs.text, PRJ);
        let names: Vec<_> = l.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["id", "name"]);
        assert!(l.raster.is_none());
    }

    #[test]
    fn layer_name_keeps_inner_dots() {
        let dir = tempfile::tempdir().unwrap();
        let ds = Files::polygons(1).write(dir.path(), "roads.v2.final");
        let md = ShapefileReader.read(&ds, &ReadLimits::default()).unwrap();
        assert_eq!(only_layer(&md).name, "roads.v2.final");
    }

    #[test]
    fn shape_types_map_like_gdal() {
        use GeometryKind as K;
        let none = [0.0, 0.0];
        let some = [0.0, 8.0];
        let no_data = [-1e39, -1e39];
        type Case = (i32, [f64; 2], Option<(K, bool, bool)>);
        let cases: &[Case] = &[
            (0, none, None),
            (1, none, Some((K::Point, false, false))),
            (3, none, Some((K::LineString, false, false))),
            (5, none, Some((K::Polygon, false, false))),
            (8, none, Some((K::MultiPoint, false, false))),
            (11, some, Some((K::Point, true, true))),
            (11, none, Some((K::Point, true, false))),
            (11, no_data, Some((K::Point, true, false))),
            (11, [f64::NAN, f64::NAN], Some((K::Point, true, false))),
            (13, some, Some((K::LineString, true, true))),
            (15, none, Some((K::Polygon, true, false))),
            (18, some, Some((K::MultiPoint, true, true))),
            (21, none, Some((K::Point, false, true))),
            (23, none, Some((K::LineString, false, true))),
            (25, none, Some((K::Polygon, false, true))),
            (28, none, Some((K::MultiPoint, false, true))),
            (31, none, Some((K::PolyhedralSurface, true, false))),
        ];
        for &(code, m, expected) in cases {
            let expected = expected.map(|(kind, has_z, has_m)| GeometryType { kind, has_z, has_m });
            assert_eq!(
                geometry_type(code, m).unwrap(),
                expected,
                "type {code}, m {m:?}"
            );
        }
        for bad in [2, 4, 9, 30, 32, -1, i32::MAX] {
            assert!(geometry_type(bad, none).is_err(), "type {bad}");
        }
    }

    #[test]
    fn empty_shapefile_has_no_extent() {
        let mut files = Files::polygons(0);
        files.shp = Some(main_header(5, [0.0; 4], [0.0; 2], 100));
        let md = files.read().unwrap();
        assert_eq!(md.status(), ExtractionStatus::Ok, "{:?}", md.partial);
        let l = only_layer(&md);
        assert_eq!(l.feature_count, Some(0));
        assert_eq!(l.native_bounds, None);
        assert_eq!(
            l.geometry_type,
            Some(GeometryType::xy(GeometryKind::Polygon))
        );
    }

    #[test]
    fn null_shape_type_has_no_geometry_or_extent() {
        let mut files = Files::polygons(2);
        files.shp = Some(main_header(0, BBOX, [0.0; 2], 124));
        let md = files.read().unwrap();
        let l = only_layer(&md);
        assert_eq!(l.geometry_type, None);
        assert_eq!(l.native_bounds, None);
        assert_eq!(l.feature_count, Some(2));
    }

    #[test]
    fn corrupt_shp_headers_are_errors() {
        let check = |shp: Vec<u8>, needle: &str| {
            let mut files = Files::polygons(1);
            files.shp = Some(shp);
            match files.read() {
                Err(ReadError::Malformed { format, reason }) => {
                    assert_eq!(format, Format::Shapefile);
                    assert!(reason.contains(needle), "{reason:?} lacks {needle:?}");
                }
                other => panic!("expected Malformed({needle}), got {other:?}"),
            }
        };
        let good = main_header(5, BBOX, [0.0; 2], 236);
        check(good[..60].to_vec(), "shorter than its 100-byte header");
        check(Vec::new(), "shorter than its 100-byte header");
        let mut bad_code = good.clone();
        bad_code[0..4].copy_from_slice(&9995i32.to_be_bytes());
        check(bad_code, "file code");
        let mut truncated = good.clone();
        truncated.truncate(200);
        check(truncated, "truncated");
        let mut negative = good.clone();
        negative[24..28].copy_from_slice(&(-4i32).to_be_bytes());
        check(negative, "words");
        let mut tiny = good.clone();
        tiny[24..28].copy_from_slice(&10i32.to_be_bytes());
        check(tiny, "words");
        let mut unknown = good;
        unknown[32..36].copy_from_slice(&7i32.to_le_bytes());
        check(unknown, "unknown shape type 7");
    }

    #[test]
    fn missing_shp_is_an_io_error() {
        let mut files = Files::polygons(1);
        files.shp = None;
        assert!(matches!(files.read(), Err(ReadError::Io { .. })));
    }

    #[test]
    fn longer_shp_than_declared_is_accepted() {
        let mut files = Files::polygons(1);
        let mut shp = main_header(5, BBOX, [0.0; 2], 236);
        shp.extend_from_slice(&[0u8; 50]);
        files.shp = Some(shp);
        assert_eq!(files.read().unwrap().status(), ExtractionStatus::Ok);
    }

    #[test]
    fn unusable_bbox_is_partial() {
        for bbox in [
            [f64::NAN, 0.0, 1.0, 1.0],
            [5.0, 0.0, 1.0, 1.0],
            [0.0, 0.0, f64::INFINITY, 1.0],
        ] {
            let mut files = Files::polygons(1);
            files.shp = Some(main_header(5, bbox, [0.0; 2], 236));
            let md = files.read().unwrap();
            assert!(matches!(md.status(), ExtractionStatus::Partial(_)));
            assert!(md.partial[0].contains("bbox"));
            assert_eq!(only_layer(&md).native_bounds, None);
            assert_eq!(only_layer(&md).feature_count, Some(1));
        }
    }

    #[test]
    fn count_prefers_the_shx_and_flags_gaps() {
        // No .shx: the .dbf count, flagged partial like a missing .dbf.
        let mut files = Files::polygons(4);
        files.shx = None;
        let md = files.read().unwrap();
        assert!(
            md.partial.iter().any(|p| p.contains("no .shx member")),
            "{:?}",
            md.partial
        );
        assert_eq!(only_layer(&md).feature_count, Some(4));

        // No .dbf: count from the .shx, fields unknown.
        let mut files = Files::polygons(4);
        files.dbf = None;
        let md = files.read().unwrap();
        assert!(md.partial.iter().any(|p| p.contains("no .dbf")));
        assert_eq!(only_layer(&md).feature_count, Some(4));
        assert!(only_layer(&md).fields.is_empty());

        // Neither: count unknown, extent still from the .shp.
        files.shx = None;
        let md = files.read().unwrap();
        assert!(
            md.partial
                .iter()
                .any(|p| p.contains("feature count unknown"))
        );
        assert_eq!(only_layer(&md).feature_count, None);
        assert!(only_layer(&md).native_bounds.is_some());

        // Disagreement: the .shx count is reported (as GDAL does), flagged partial.
        let mut files = Files::polygons(4);
        files.shx = Some(main_header(5, BBOX, [0.0; 2], 100 + 8 * 3));
        let md = files.read().unwrap();
        assert!(
            md.partial
                .iter()
                .any(|p| p.contains("indexes 3 shapes but .dbf has 4 records"))
        );
        assert_eq!(only_layer(&md).feature_count, Some(3));
    }

    #[test]
    fn records_are_decided_by_the_shp_alone() {
        // A .dbf (and .shx) claiming 0 records can't erase the bbox of a .shp that has records.
        let mut files = Files::polygons(0);
        files.shp = Some(main_header(5, BBOX, [0.0; 2], 236));
        let md = files.read().unwrap();
        assert!(
            md.partial
                .iter()
                .any(|p| p.contains("has records but the feature count is 0")),
            "{:?}",
            md.partial
        );
        let b = only_layer(&md).native_bounds.unwrap();
        assert_eq!([b.min_x(), b.min_y(), b.max_x(), b.max_y()], BBOX);

        // The reverse: an empty .shp gets no extent even if the sidecars count features.
        let mut files = Files::polygons(2);
        files.shp = Some(main_header(5, BBOX, [0.0; 2], 100));
        let md = files.read().unwrap();
        assert!(
            md.partial
                .iter()
                .any(|p| p.contains("no records but the feature count is 2")),
            "{:?}",
            md.partial
        );
        assert_eq!(only_layer(&md).native_bounds, None);
        assert_eq!(only_layer(&md).feature_count, Some(2));
    }

    #[test]
    fn dbf_of_unknown_version_drops_fields_only() {
        let mut files = Files::polygons(3);
        if let Some(dbf) = files.dbf.as_mut() {
            dbf[0] = 0x04;
        }
        let md = files.read().unwrap();
        assert!(
            md.partial.iter().any(|p| p.contains("version byte 0x04")),
            "{:?}",
            md.partial
        );
        let l = only_layer(&md);
        assert!(l.fields.is_empty());
        assert_eq!(l.feature_count, Some(3));
        assert!(l.native_bounds.is_some());
    }

    #[test]
    fn unreadable_cpg_is_partial() {
        let dir = tempfile::tempdir().unwrap();
        let mut ds = Files::polygons(1).write(dir.path(), "c");
        for m in &mut ds.sidecars {
            if m.role == MemberRole::Cpg {
                m.path = dir.path().join("missing.cpg");
            }
        }
        let md = ShapefileReader.read(&ds, &ReadLimits::default()).unwrap();
        assert!(
            md.partial.iter().any(|p| p.contains(".cpg ignored")),
            "{:?}",
            md.partial
        );
    }

    #[test]
    fn corrupt_dbf_keeps_geometry_facts() {
        // Mirrors corpus case shp_corrupt_bad_dbf: absurd record count, header past EOF.
        let mut files = Files::polygons(3);
        let mut dbf = files.dbf.take().unwrap();
        dbf[4..8].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
        dbf[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        files.dbf = Some(dbf);
        let md = files.read().unwrap();
        assert!(matches!(md.status(), ExtractionStatus::Partial(_)));
        assert!(md.partial.iter().any(|p| p.contains("beyond the end")));
        let l = only_layer(&md);
        assert_eq!(l.feature_count, Some(3));
        assert!(l.fields.is_empty());
        assert!(l.native_bounds.is_some());
        assert!(l.crs.is_some());
    }

    #[test]
    fn bad_shx_is_partial() {
        for shx in [
            vec![0u8; 40],
            main_header(5, BBOX, [0.0; 2], 100 + 12),
            main_header(5, BBOX, [0.0; 2], 100 + 24)[..110].to_vec(),
        ] {
            let mut files = Files::polygons(3);
            files.shx = Some(shx);
            let md = files.read().unwrap();
            assert!(
                md.partial.iter().any(|p| p.contains(".shx")),
                "{:?}",
                md.partial
            );
            assert_eq!(only_layer(&md).feature_count, Some(3));
        }
    }

    #[test]
    fn prj_problems_are_partial_and_missing_prj_is_fine() {
        let mut files = Files::polygons(1);
        files.prj = None;
        let md = files.read().unwrap();
        assert_eq!(md.status(), ExtractionStatus::Ok);
        assert_eq!(only_layer(&md).crs, None);

        for prj in [Vec::new(), b"Projection UTM".to_vec()] {
            files.prj = Some(prj);
            let md = files.read().unwrap();
            assert!(matches!(md.status(), ExtractionStatus::Partial(_)));
            assert_eq!(only_layer(&md).crs, None);
        }
    }

    #[test]
    fn cpg_selects_the_field_name_encoding() {
        let mut files = Files::polygons(1);
        // "名称" in GBK.
        files.dbf = Some(dbf_bytes(&[(&[0xC3, 0xFB, 0xB3, 0xC6], b'C', 10, 0)], 1, 0));
        files.cpg = Some(b"936\r\n".to_vec());
        let md = files.read().unwrap();
        assert_eq!(only_layer(&md).fields[0].name, "名称");
    }

    #[test]
    fn wrong_format_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let mut ds = Files::polygons(1).write(dir.path(), "x");
        ds.format = Format::GeoPackage;
        assert!(matches!(
            ShapefileReader.read(&ds, &ReadLimits::default()),
            Err(ReadError::Unsupported(_))
        ));
    }

    #[test]
    fn read_budget_is_respected() {
        let files = Files::polygons(2);
        let limits = |max_bytes| ReadLimits {
            max_bytes,
            ..ReadLimits::default()
        };
        // Not even the .shp header fits.
        assert!(matches!(
            files.read_with(&limits(99)),
            Err(ReadError::Io { .. })
        ));
        // Only the .shp header fits: everything else degrades.
        let (md, stats) = measure(|| files.read_with(&limits(100)).unwrap());
        assert_eq!(stats.bytes_read, 100);
        assert!(md.partial.iter().any(|p| p.contains("budget")));
        assert_eq!(only_layer(&md).feature_count, None);
        assert!(only_layer(&md).native_bounds.is_some());
    }

    /// The bounded-read guarantee: whatever the member sizes, a dataset costs < 64 KiB of reads.
    #[test]
    fn reads_stay_bounded_for_huge_members() {
        let n = 500;
        let names: Vec<Vec<u8>> = (0..2000).map(|i| format!("f{i}").into_bytes()).collect();
        let fields: Vec<(&[u8], u8, u8, u8)> =
            names.iter().map(|n| (n.as_slice(), b'C', 1, 0)).collect();
        let mut prj = PRJ.as_bytes().to_vec();
        prj.resize(prj::MAX_PRJ_BYTES, b' ');
        let files = Files {
            shp: Some(main_header(5, BBOX, [0.0; 2], 100 + 136 * n)),
            shx: Some(main_header(5, BBOX, [0.0; 2], 100 + 8 * n)),
            dbf: Some(dbf_bytes(&fields, u32::try_from(n).unwrap(), 87)),
            prj: Some(prj),
            cpg: Some(vec![b' '; 1 << 20]),
        };
        let (md, stats) = measure(|| files.read().unwrap());
        assert!(
            stats.bytes_read < 64 * 1024,
            "read {} bytes",
            stats.bytes_read
        );
        assert_eq!(stats.files_opened, 5);
        let l = only_layer(&md);
        assert_eq!(l.feature_count, Some(n as u64));
        assert_eq!(l.fields.len(), dbf::MAX_FIELDS);
        assert!(l.crs.is_some());
    }

    /// Deterministic xorshift, so the fuzz test needs no dependency and is reproducible.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// Truncations and byte mutations of every member never panic, and a broken `.shp` header
    /// never yields `Ok`.
    #[test]
    fn truncated_and_mutated_members_never_panic() {
        let base = Files::polygons(3);
        let dir = tempfile::tempdir().unwrap();
        let ds = base.write(dir.path(), "fz");
        let paths: Vec<PathBuf> = std::iter::once(ds.anchor.clone())
            .chain(ds.sidecars.iter().map(|m| m.path.clone()))
            .collect();
        let originals: Vec<Vec<u8>> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();
        let limits = ReadLimits::default();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);

        for (i, path) in paths.iter().enumerate() {
            let original = &originals[i];
            for cut in 0..original.len().min(400) {
                std::fs::write(path, &original[..cut]).unwrap();
                let result = ShapefileReader.read(&ds, &limits);
                if i == 0 && cut < HEADER_LEN {
                    assert!(result.is_err(), "{cut}-byte .shp read as {result:?}");
                }
            }
            for _ in 0..150 {
                let mut bytes = original.clone();
                for _ in 0..=rng.below(4) {
                    let at = rng.below(bytes.len().min(160));
                    bytes[at] = rng.next() as u8;
                }
                std::fs::write(path, &bytes).unwrap();
                let _ = ShapefileReader.read(&ds, &limits);
            }
            std::fs::write(path, original).unwrap();
        }
        assert!(ShapefileReader.read(&ds, &limits).is_ok());
    }
}
