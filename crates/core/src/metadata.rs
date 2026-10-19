//! What a reader returns: per-layer facts (geometry type, counts, CRS definition, native bounds,
//! fields, raster facts).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::code::code_enum;
use crate::{Bounds, CoreError, CrsDefinition, ExtractionStatus};

code_enum! {
    /// Base geometry type. The canonical code is the mixed-case ISO/OGC WKT name (`"MultiPolygon"`,
    /// `"TIN"`), which `code()`, `from_code()`, `FromStr` and serde use exactly.
    ///
    /// Formats spell these names differently — GeoPackage's `gpkg_geometry_columns` stores them
    /// upper case (`'MULTIPOLYGON'`, `'GEOMETRY'`) — so readers parse format-provided names with
    /// [`GeometryKind::parse_lenient`].
    pub enum GeometryKind {
        /// Any geometry (unconstrained column, mixed types).
        Geometry => "Geometry",
        /// Point.
        Point => "Point",
        /// LineString (shapefile PolyLine without parts reports as this, like GDAL).
        LineString => "LineString",
        /// Polygon.
        Polygon => "Polygon",
        /// MultiPoint.
        MultiPoint => "MultiPoint",
        /// MultiLineString.
        MultiLineString => "MultiLineString",
        /// MultiPolygon.
        MultiPolygon => "MultiPolygon",
        /// GeometryCollection.
        GeometryCollection => "GeometryCollection",
        /// CircularString.
        CircularString => "CircularString",
        /// CompoundCurve.
        CompoundCurve => "CompoundCurve",
        /// CurvePolygon.
        CurvePolygon => "CurvePolygon",
        /// MultiCurve.
        MultiCurve => "MultiCurve",
        /// MultiSurface.
        MultiSurface => "MultiSurface",
        /// Curve (abstract).
        Curve => "Curve",
        /// Surface (abstract).
        Surface => "Surface",
        /// PolyhedralSurface (e.g. shapefile MultiPatch).
        PolyhedralSurface => "PolyhedralSurface",
        /// TIN.
        Tin => "TIN",
        /// Triangle.
        Triangle => "Triangle",
    }
}

impl GeometryKind {
    /// Parses a geometry type name from a file, ignoring ASCII case (`"POINT"`, `"MultiPolygon"`,
    /// `"tin"`). Use for format-provided names; canonical codes go through `from_code`.
    ///
    /// Case-insensitivity is specific to this enum: the other code enums hold codes that only
    /// Whereabouts writes, so their strict parsing catches corruption instead of masking it.
    pub fn parse_lenient(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.code().eq_ignore_ascii_case(name))
    }
}

/// A layer's geometry type: base kind plus Z/M dimensions.
///
/// String form (catalog `layers.geometry_type` and serde): the kind code, then `" Z"`, `" M"` or
/// `" ZM"` when present — e.g. `"MultiPolygon"`, `"Point Z"`, `"LineString ZM"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct GeometryType {
    /// Base type.
    pub kind: GeometryKind,
    /// Has a Z coordinate.
    pub has_z: bool,
    /// Has an M (measure) coordinate.
    pub has_m: bool,
}

impl GeometryType {
    /// A 2D geometry type.
    pub const fn xy(kind: GeometryKind) -> Self {
        GeometryType {
            kind,
            has_z: false,
            has_m: false,
        }
    }
}

impl fmt::Display for GeometryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind.code())?;
        match (self.has_z, self.has_m) {
            (false, false) => Ok(()),
            (true, false) => f.write_str(" Z"),
            (false, true) => f.write_str(" M"),
            (true, true) => f.write_str(" ZM"),
        }
    }
}

impl FromStr for GeometryType {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, CoreError> {
        let (base, dims) = match s.split_once(' ') {
            Some((base, dims)) => (base, Some(dims)),
            None => (s, None),
        };
        let (has_z, has_m) = match dims {
            None => (false, false),
            Some("Z") => (true, false),
            Some("M") => (false, true),
            Some("ZM") => (true, true),
            _ => {
                return Err(CoreError::UnknownCode {
                    kind: "GeometryType",
                    code: s.to_owned(),
                });
            }
        };
        let kind = base.parse::<GeometryKind>()?;
        Ok(GeometryType { kind, has_z, has_m })
    }
}

impl From<GeometryType> for String {
    fn from(g: GeometryType) -> String {
        g.to_string()
    }
}

impl TryFrom<String> for GeometryType {
    type Error = CoreError;

    fn try_from(s: String) -> Result<Self, CoreError> {
        s.parse()
    }
}

code_enum! {
    /// Raster band data type, using GDAL's names (as reported by `gdalinfo`).
    pub enum RasterDataType {
        /// Unsigned 8-bit.
        Byte => "Byte",
        /// Signed 8-bit.
        Int8 => "Int8",
        /// Unsigned 16-bit.
        UInt16 => "UInt16",
        /// Signed 16-bit.
        Int16 => "Int16",
        /// Unsigned 32-bit.
        UInt32 => "UInt32",
        /// Signed 32-bit.
        Int32 => "Int32",
        /// Unsigned 64-bit.
        UInt64 => "UInt64",
        /// Signed 64-bit.
        Int64 => "Int64",
        /// 16-bit float.
        Float16 => "Float16",
        /// 32-bit float.
        Float32 => "Float32",
        /// 64-bit float.
        Float64 => "Float64",
        /// Complex, 16-bit integer parts.
        CInt16 => "CInt16",
        /// Complex, 32-bit integer parts.
        CInt32 => "CInt32",
        /// Complex, 32-bit float parts.
        CFloat32 => "CFloat32",
        /// Complex, 64-bit float parts.
        CFloat64 => "CFloat64",
        /// Not determinable / mixed.
        Unknown => "Unknown",
    }
}

/// Facts about a raster layer (catalog `rasters` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RasterInfo {
    /// Width in pixels.
    pub width: u64,
    /// Height in pixels.
    pub height: u64,
    /// Number of bands.
    pub bands: u32,
    /// Data type of the first band (bands of differing types: `Unknown`).
    pub dtype: RasterDataType,
    /// Pixel width in CRS units (positive), if georeferenced.
    pub res_x: Option<f64>,
    /// Pixel height in CRS units (positive), if georeferenced.
    pub res_y: Option<f64>,
    /// Nodata value of the first band, verbatim as stored (e.g. `"-9999"`, `"nan"`).
    pub nodata: Option<String>,
    /// Number of overview levels (internal + external `.ovr`).
    pub overview_count: u32,
    /// GDAL-style affine geotransform `[x0, dx, rx, y0, ry, dy]` (pixel-corner convention),
    /// if georeferenced.
    pub geotransform: Option<[f64; 6]>,
}

code_enum! {
    /// Attribute field type, normalised across formats (OGR field type names).
    pub enum FieldKind {
        /// Text.
        String => "String",
        /// 32-bit (or narrower) integer.
        Integer => "Integer",
        /// 64-bit integer.
        Integer64 => "Integer64",
        /// Floating point / decimal.
        Real => "Real",
        /// Boolean.
        Boolean => "Boolean",
        /// Date.
        Date => "Date",
        /// Time of day.
        Time => "Time",
        /// Date and time.
        DateTime => "DateTime",
        /// Binary blob.
        Binary => "Binary",
        /// Anything else.
        Other => "Other",
    }
}

/// One attribute field of a layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldInfo {
    /// Field name (decoded with the layer's encoding; lossy if undecodable).
    pub name: String,
    /// Normalised type.
    pub kind: FieldKind,
    /// Declared width, where the format has one (DBF).
    pub width: Option<u32>,
    /// Declared precision/decimal count, where the format has one (DBF).
    pub precision: Option<u32>,
}

/// What a reader extracts for one layer. Every fact is optional: readers fill what they can and
/// report degradation through [`DatasetMetadata::partial`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LayerMetadata {
    /// Layer name (GPKG table name; file stem for single-layer formats).
    pub name: String,
    /// Geometry type; `None` for rasters and attribute-only tables.
    pub geometry_type: Option<GeometryType>,
    /// Feature count (vectors) or point count (point clouds).
    pub feature_count: Option<u64>,
    /// The raw CRS definition, uninterpreted.
    pub crs: Option<CrsDefinition>,
    /// Extent in the layer's native CRS.
    pub native_bounds: Option<Bounds>,
    /// Attribute fields, in declaration order.
    pub fields: Vec<FieldInfo>,
    /// Raster facts; `Some` iff the layer is a raster.
    pub raster: Option<RasterInfo>,
}

/// What a [`FormatReader`](crate::reader::FormatReader) returns for a dataset.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DatasetMetadata {
    /// Layers in a deterministic order (their index is `LayerId::ordinal`).
    pub layers: Vec<LayerMetadata>,
    /// Reasons the result is incomplete or degraded (stale bounds, count fallback, time budget
    /// hit, …). Empty means a complete read.
    pub partial: Vec<String>,
}

impl DatasetMetadata {
    /// `Ok` if `partial` is empty, else `Partial` with the reasons joined by `"; "`.
    pub fn status(&self) -> ExtractionStatus {
        if self.partial.is_empty() {
            ExtractionStatus::Ok
        } else {
            ExtractionStatus::Partial(self.partial.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::CrsDefinitionKind;

    #[test]
    fn geometry_type_strings() {
        for k in GeometryKind::ALL {
            for (z, m) in [(false, false), (true, false), (false, true), (true, true)] {
                let g = GeometryType {
                    kind: *k,
                    has_z: z,
                    has_m: m,
                };
                assert_eq!(g.to_string().parse::<GeometryType>().unwrap(), g);
            }
        }
        assert_eq!(
            "MultiPolygon ZM".parse::<GeometryType>().unwrap(),
            GeometryType {
                kind: GeometryKind::MultiPolygon,
                has_z: true,
                has_m: true
            }
        );
        for bad in ["", "Polygon ", "Polygon XYZ", "polygon", "Polygon Z M"] {
            assert!(bad.parse::<GeometryType>().is_err(), "{bad:?}");
        }
        let json = serde_json::to_string(&GeometryType::xy(GeometryKind::Point)).unwrap();
        assert_eq!(json, "\"Point\"");
    }

    #[test]
    fn geometry_kind_lenient_parsing_accepts_gpkg_names() {
        // Names as stored in gpkg_geometry_columns.geometry_type_name.
        let gpkg = [
            ("GEOMETRY", GeometryKind::Geometry),
            ("POINT", GeometryKind::Point),
            ("LINESTRING", GeometryKind::LineString),
            ("POLYGON", GeometryKind::Polygon),
            ("MULTIPOINT", GeometryKind::MultiPoint),
            ("MULTILINESTRING", GeometryKind::MultiLineString),
            ("MULTIPOLYGON", GeometryKind::MultiPolygon),
            ("GEOMETRYCOLLECTION", GeometryKind::GeometryCollection),
            ("CIRCULARSTRING", GeometryKind::CircularString),
            ("COMPOUNDCURVE", GeometryKind::CompoundCurve),
            ("CURVEPOLYGON", GeometryKind::CurvePolygon),
            ("MULTICURVE", GeometryKind::MultiCurve),
            ("MULTISURFACE", GeometryKind::MultiSurface),
            ("CURVE", GeometryKind::Curve),
            ("SURFACE", GeometryKind::Surface),
        ];
        for (name, kind) in gpkg {
            assert_eq!(GeometryKind::parse_lenient(name), Some(kind), "{name}");
            // The canonical, strict parsers stay exact.
            assert_eq!(GeometryKind::from_code(name), None, "{name}");
        }
        assert_eq!(GeometryKind::parse_lenient("tin"), Some(GeometryKind::Tin));
        assert_eq!(
            GeometryKind::parse_lenient(" Point "),
            Some(GeometryKind::Point)
        );
        assert_eq!(GeometryKind::parse_lenient("POINTZ"), None);
        assert_eq!(GeometryKind::parse_lenient(""), None);
        for k in GeometryKind::ALL {
            assert_eq!(
                GeometryKind::parse_lenient(&k.code().to_uppercase()),
                Some(*k)
            );
        }
    }

    #[test]
    fn layer_and_dataset_metadata_round_trip() {
        let layer = LayerMetadata {
            name: "roads".into(),
            geometry_type: Some(GeometryType::xy(GeometryKind::LineString)),
            feature_count: Some(1234),
            crs: Some(CrsDefinition::new(
                CrsDefinitionKind::Wkt1Esri,
                "PROJCS[...]",
            )),
            native_bounds: Some(Bounds::new(0.0, 0.0, 10.0, 10.0).unwrap()),
            fields: vec![FieldInfo {
                name: "NAME".into(),
                kind: FieldKind::String,
                width: Some(80),
                precision: None,
            }],
            raster: None,
        };
        let raster = LayerMetadata {
            name: "dem".into(),
            raster: Some(RasterInfo {
                width: 4000,
                height: 3000,
                bands: 1,
                dtype: RasterDataType::Float32,
                res_x: Some(8.0),
                res_y: Some(8.0),
                nodata: Some("-3.4028234663852886e+38".into()),
                overview_count: 3,
                geotransform: Some([1_570_000.0, 8.0, 0.0, 5_200_000.0, 0.0, -8.0]),
            }),
            ..LayerMetadata::default()
        };
        let mut md = DatasetMetadata {
            layers: vec![layer, raster],
            partial: vec![],
        };
        assert_eq!(md.status(), ExtractionStatus::Ok);
        let json = serde_json::to_string(&md).unwrap();
        assert_eq!(serde_json::from_str::<DatasetMetadata>(&json).unwrap(), md);
        md.partial = vec!["count from .shx".into(), "no .prj".into()];
        assert_eq!(
            md.status(),
            ExtractionStatus::Partial("count from .shx; no .prj".into())
        );
    }
}
