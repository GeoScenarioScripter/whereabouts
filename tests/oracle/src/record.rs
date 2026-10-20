//! Oracle records (what GDAL said, from `tests/corpus/expected/<id>.json`) and native records (what
//! a Whereabouts reader said, produced by an adapter).
//!
//! The oracle types mirror `tests/corpus/scripts/golden.py` exactly (`deny_unknown_fields`), so a
//! drift between the generator and this crate fails loudly. Bump [`SCHEMA`] in both on any
//! incompatible change. Field semantics are documented in `tests/corpus/README.md`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Oracle record schema version understood by this crate (`"schema"` in every golden).
pub const SCHEMA: u32 = 1;

/// `[min_x, min_y, max_x, max_y]`. For WGS84 extents: `[west, south, east, north]` in degrees,
/// with `west > east` when the extent crosses the antimeridian.
pub type Bbox = [f64; 4];

/// One golden file: GDAL's view of one corpus case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleRecord {
    /// Record schema version ([`SCHEMA`]).
    pub schema: u32,
    /// Case id (matches the manifest).
    pub case: String,
    /// Format directory name (matches the manifest).
    pub format: String,
    /// GDAL/PROJ versions that produced the record.
    pub oracle: OracleVersion,
    /// Whether GDAL could open and read the dataset.
    pub status: OracleStatus,
    /// GDAL's error message when `status` is `error` (paths stripped).
    pub error: Option<String>,
    /// GDAL warnings/errors emitted while reading (paths stripped, de-duplicated).
    pub messages: Vec<String>,
    /// One entry per layer (GeoPackage tables, or the single layer of SHP/GeoTIFF).
    pub layers: Vec<LayerRecord>,
}

/// Versions of the oracle toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleVersion {
    /// GDAL release, e.g. `3.13.3`.
    pub gdal: String,
    /// PROJ release, e.g. `9.8.1`.
    pub proj: String,
}

/// GDAL's verdict on a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OracleStatus {
    /// Opened and read (possibly with warnings in `messages`).
    Ok,
    /// GDAL refused the dataset.
    Error,
}

/// GDAL's view of one layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerRecord {
    /// Layer name: GPKG table name; file stem for SHP/GeoTIFF.
    pub name: String,
    /// `vector`, `attributes`, `raster`, `tiles`, or `other:<gpkg data_type>`.
    pub kind: String,
    /// Flattened geometry type (`Point`, `LineString`, `Polygon`, `MultiPoint`,
    /// `MultiLineString`, `MultiPolygon`, `GeometryCollection`, `Unknown`); `None` for
    /// non-spatial and raster layers.
    pub geometry_type: Option<String>,
    /// Geometry type has Z.
    pub has_z: bool,
    /// Geometry type has M.
    pub has_m: bool,
    /// Exact feature count (vector/attributes layers).
    pub feature_count: Option<u64>,
    /// True native-CRS extent: union of geometry envelopes (vector), or the bbox of the four
    /// pixel-corner positions of the geotransform (raster, PixelIsArea convention). `None` when
    /// empty / not georeferenced.
    pub extent: Option<Bbox>,
    /// Extent as the file declares it (SHP header, `gpkg_contents`), which may be stale or zero.
    pub declared_extent: Option<Bbox>,
    /// Raster facts (raster and tiles layers).
    pub raster: Option<RasterRecord>,
    /// Attribute fields (vector/attributes layers).
    pub fields: Option<Vec<FieldRecord>>,
    /// DBF encoding signals (shapefile only).
    pub encoding: Option<EncodingRecord>,
    /// CRS: raw definition, WKT2, EPSG identification.
    pub crs: CrsRecord,
    /// WGS84 extent via `OCTTransformBounds` with 21-point edge densification.
    pub wgs84_extent: Option<Bbox>,
    /// `wgs84_extent` crosses the antimeridian (`west > east`).
    pub crosses_antimeridian: bool,
    /// Why the WGS84 transform failed, if it did.
    pub wgs84_error: Option<String>,
}

impl LayerRecord {
    /// Raster or tile-pyramid layer.
    pub fn is_raster(&self) -> bool {
        self.kind == "raster" || self.kind == "tiles"
    }

    /// Non-spatial attribute table.
    pub fn is_attributes(&self) -> bool {
        self.kind == "attributes"
    }
}

/// Raster facts for one raster/tiles layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RasterRecord {
    /// Width in pixels.
    pub width: u64,
    /// Height in pixels.
    pub height: u64,
    /// Number of bands.
    pub band_count: u32,
    /// GDAL data type name per band (`Byte`, `UInt16`, `Int16`, `Float32`, …).
    pub data_types: Vec<String>,
    /// Nodata per band.
    pub nodata: Vec<Option<NoData>>,
    /// GDAL geotransform (PixelIsArea convention, even for PixelIsPoint files).
    pub geotransform: Option<[f64; 6]>,
    /// Pixel size `[x, y]` (lengths of the geotransform's column/row vectors; positive).
    pub resolution: Option<[f64; 2]>,
    /// Where GDAL took the geotransform from: `geotiff_tags`, `aux_xml`, `worldfile`,
    /// `gpkg_tile_matrix`, or `None` when not georeferenced.
    pub georef_source: Option<String>,
    /// `AREA_OR_POINT=Point`.
    pub pixel_is_point: bool,
    /// Number of overview levels on band 1.
    pub overview_count: u32,
    /// File is BigTIFF.
    pub bigtiff: bool,
    /// `IMAGE_STRUCTURE` `LAYOUT` (e.g. `COG`).
    pub layout: Option<String>,
    /// `IMAGE_STRUCTURE` `COMPRESSION`.
    pub compression: Option<String>,
}

/// A nodata value. JSON can't carry NaN/±inf, so goldens write those as strings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoData {
    /// A finite value.
    Value(f64),
    /// NaN.
    NaN,
    /// +inf.
    PosInf,
    /// -inf.
    NegInf,
}

impl NoData {
    /// Classify an `f64` (NaN and infinities become their own variants).
    pub fn from_f64(v: f64) -> Self {
        if v.is_nan() {
            Self::NaN
        } else if v == f64::INFINITY {
            Self::PosInf
        } else if v == f64::NEG_INFINITY {
            Self::NegInf
        } else {
            Self::Value(v)
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum NoDataRepr {
    Value(f64),
    Special(String),
}

impl<'de> Deserialize<'de> for NoData {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match NoDataRepr::deserialize(d)? {
            NoDataRepr::Value(v) => Ok(Self::Value(v)),
            NoDataRepr::Special(s) => match s.as_str() {
                "nan" => Ok(Self::NaN),
                "inf" => Ok(Self::PosInf),
                "-inf" => Ok(Self::NegInf),
                other => Err(serde::de::Error::custom(format!("bad nodata {other:?}"))),
            },
        }
    }
}

impl Serialize for NoData {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::Value(v) => NoDataRepr::Value(v),
            Self::NaN => NoDataRepr::Special("nan".into()),
            Self::PosInf => NoDataRepr::Special("inf".into()),
            Self::NegInf => NoDataRepr::Special("-inf".into()),
        }
        .serialize(s)
    }
}

/// One attribute field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldRecord {
    /// Field name (decoded with the layer's encoding).
    pub name: String,
    /// OGR field type name (`Integer`, `Integer64`, `Real`, `String`, `Date`, …).
    #[serde(rename = "type")]
    pub field_type: String,
    /// Declared width (0 when unspecified).
    pub width: i64,
}

/// How GDAL decided a shapefile's DBF encoding (`.cpg` wins over the LDID byte).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncodingRecord {
    /// Encoding GDAL decodes the DBF with (`None`: undetermined/raw).
    pub source_encoding: Option<String>,
    /// Raw `.cpg` content.
    pub cpg: Option<String>,
    /// Encoding derived from the `.cpg`.
    pub encoding_from_cpg: Option<String>,
    /// DBF language driver id (header byte 29), when non-zero.
    pub ldid: Option<u32>,
    /// Encoding derived from the LDID.
    pub encoding_from_ldid: Option<String>,
}

/// GDAL's view of a layer's CRS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrsRecord {
    /// Identification outcome.
    pub status: CrsStatus,
    /// Where the definition came from: `prj`, `gpkg_srs`, `geokeys`, `aux_xml`, `none`.
    pub source: String,
    /// Flavour of `raw`: `wkt1`, `wkt1_esri`, `wkt2`, `projjson`, `geokeys`, `other`.
    pub raw_kind: Option<String>,
    /// The definition text exactly as the file carries it (`.prj` bytes, GPKG `definition`,
    /// `.aux.xml` `<SRS>`); `None` for GeoKeys (binary) and when absent.
    pub raw: Option<String>,
    /// Authority the file itself declares (GPKG `organization` / `organization_coordsys_id`).
    pub declared_authority: Option<DeclaredAuthority>,
    /// GDAL's WKT2:2019 rendering of the CRS.
    pub wkt2: Option<String>,
    /// Identified EPSG code.
    pub epsg: Option<u32>,
    /// How it was identified: `authority`, `auto_identify`, `find_matches`.
    pub epsg_method: Option<String>,
    /// PROJ confidence (0–100) of the identification / best candidate.
    pub epsg_confidence: Option<u32>,
    /// Top `FindMatches` candidates (when identification needed matching).
    pub candidates: Vec<CrsCandidate>,
}

/// CRS identification outcome (maps to the catalog's `crs_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrsStatus {
    /// An EPSG code was identified.
    Identified,
    /// A usable CRS without an EPSG match.
    Unidentified,
    /// No usable CRS (absent, or engineering/"undefined").
    Missing,
}

/// The authority a GeoPackage declares for a layer's `srs_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredAuthority {
    /// `organization` (e.g. `EPSG`, `NONE`).
    pub name: Option<String>,
    /// `organization_coordsys_id` as text.
    pub code: String,
    /// GPKG `srs_id`.
    pub srs_id: i64,
}

/// One `FindMatches` candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrsCandidate {
    /// Authority name.
    pub authority: Option<String>,
    /// Authority code.
    pub code: Option<String>,
    /// PROJ confidence 0–100.
    pub confidence: u32,
}

// ------------------------------------------------------------------------------------------------
// Native side
// ------------------------------------------------------------------------------------------------

/// What a Whereabouts reader reported for one case, in oracle terms. Built by an adapter.
///
/// Every field an adapter's declared [`Check`](crate::Check)s cover must be filled in: for a
/// declared check, `None` means "the reader found nothing", which disagrees with an oracle value.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeRecord {
    /// Extraction status.
    pub status: NativeStatus,
    /// Layers found.
    pub layers: Vec<NativeLayer>,
}

impl NativeRecord {
    /// A record with `status` and no layers.
    pub fn new(status: NativeStatus) -> Self {
        Self {
            status,
            layers: Vec::new(),
        }
    }
}

/// Native extraction status (mirrors `whereabouts_core::ExtractionStatus` without the reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStatus {
    /// Fully read.
    Ok,
    /// Read with some facts missing.
    Partial,
    /// Format not handled.
    Unsupported,
    /// Could not read.
    Error,
}

/// A native reader's view of one layer.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NativeLayer {
    /// Layer name (same convention as [`LayerRecord::name`]).
    pub name: String,
    /// Flattened geometry type name (same vocabulary as [`LayerRecord::geometry_type`]).
    pub geometry_type: Option<String>,
    /// Has Z.
    pub has_z: bool,
    /// Has M.
    pub has_m: bool,
    /// Feature count.
    pub feature_count: Option<u64>,
    /// Native-CRS extent (see [`LayerRecord::extent`] for the convention).
    pub extent: Option<Bbox>,
    /// Raster facts.
    pub raster: Option<NativeRaster>,
    /// Identified EPSG code.
    pub epsg: Option<u32>,
    /// WGS84 extent (`west > east` when crossing the antimeridian).
    pub wgs84_extent: Option<Bbox>,
}

/// A native reader's raster facts. Size and band count are always compared; the optional fields
/// only when the reader provides them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NativeRaster {
    /// Width in pixels.
    pub width: u64,
    /// Height in pixels.
    pub height: u64,
    /// Number of bands.
    pub band_count: u32,
    /// GDAL-style data type names per band.
    pub data_types: Option<Vec<String>>,
    /// Nodata per band.
    pub nodata: Option<Vec<Option<NoData>>>,
    /// Pixel size `[x, y]`.
    pub resolution: Option<[f64; 2]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodata_round_trips_specials() {
        let v: Vec<Option<NoData>> =
            serde_json::from_str(r#"[null, -9999.0, "nan", "inf", "-inf"]"#).expect("parse");
        assert_eq!(v[0], None);
        assert_eq!(v[1], Some(NoData::Value(-9999.0)));
        assert!(matches!(v[2], Some(NoData::NaN)));
        assert_eq!(v[3], Some(NoData::PosInf));
        assert_eq!(v[4], Some(NoData::NegInf));
        assert_eq!(
            serde_json::to_string(&v).expect("serialize"),
            r#"[null,-9999.0,"nan","inf","-inf"]"#
        );
        assert!(serde_json::from_str::<NoData>(r#""bogus""#).is_err());
        assert!(matches!(NoData::from_f64(f64::NAN), NoData::NaN));
        assert_eq!(NoData::from_f64(0.0), NoData::Value(0.0));
    }
}
