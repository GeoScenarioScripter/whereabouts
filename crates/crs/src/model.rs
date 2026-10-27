//! The internal CRS model every input format is parsed into.
//!
//! It keeps what identification and bbox transformation need (session 2.4) and nothing more:
//! datum + ellipsoid, prime meridian, projection method + parameters, units, axes, authority codes.
//! Names are kept as written; canonical forms live in [`ProjectionMethod`], [`ParamName`] and
//! [`crate::names`].

use crate::names::{ParamClass, ParamName, ProjectionMethod};

/// What sort of coordinate reference system this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrsKind {
    /// Latitude/longitude on an ellipsoid (`GEOGCS`, `GEOGCRS`, `GEODCRS` with an ellipsoidal CS).
    Geographic,
    /// A map projection of a geographic CRS (`PROJCS`, `PROJCRS`).
    Projected,
    /// Earth-centred Cartesian X/Y/Z (`GEOCCS`, `GEODCRS` with a Cartesian CS).
    Geocentric,
    /// Horizontal + vertical (`COMPD_CS`, `COMPOUNDCRS`, or ESRI `PROJCS[…],VERTCS[…]`).
    Compound,
    /// Heights/depths only (`VERT_CS`, ESRI `VERTCS`, `VERTCRS`).
    Vertical,
    /// Local engineering / site grid (`LOCAL_CS`, `ENGCRS`).
    Engineering,
    /// Anything else that parsed but has no georeferencing meaning here (`FITTED_CS`,
    /// derived/temporal/parametric WKT2 CRSs).
    Unknown,
}

/// The text format (and dialect) a [`Crs`] was parsed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// OGC 01-009 WKT1 as written by GDAL/PROJ (`AUTHORITY[…]`, lower-case parameter names).
    Wkt1Ogc,
    /// The ESRI flavour of WKT1 found in `.prj` files (`GCS_*`, `D_*`, `False_Easting`, no
    /// authority codes, `VERTCS`).
    Wkt1Esri,
    /// ISO 19162:2015 / OGC 12-063r5.
    Wkt2_2015,
    /// ISO 19162:2019 / OGC 18-010r7 (adds `GEOGCRS`, `USAGE`, `ENSEMBLE`, `BASEGEOGCRS`, …).
    Wkt2_2019,
    /// PROJ's JSON encoding of WKT2:2019.
    ProjJson,
    /// Not parsed from text: built from the embedded EPSG table for a bare EPSG code
    /// ([`crate::epsg_crs`]).
    EpsgTable,
}

/// An authority citation such as `AUTHORITY["EPSG","4326"]` or `ID["EPSG",4326]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Authority {
    /// Authority name as written (`EPSG`, `ESRI`, `IGNF`, …).
    pub name: String,
    /// Code as written (codes are not always numeric).
    pub code: String,
}

impl Authority {
    /// The EPSG code, if this is an `EPSG` citation with a numeric code.
    pub fn epsg(&self) -> Option<u32> {
        if self.name.eq_ignore_ascii_case("EPSG") {
            self.code.trim().parse().ok()
        } else {
            None
        }
    }
}

/// What a [`Unit`] measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitKind {
    /// Length; `factor` converts to metres.
    Linear,
    /// Angle; `factor` converts to radians.
    Angular,
    /// Dimensionless scale; `factor` converts to unity.
    Scale,
    /// Time; `factor` converts to seconds.
    Time,
    /// Parametric (pressure, …); `factor` converts to the parameter's SI unit.
    Parametric,
    /// Unknown kind (WKT2:2015 bare `UNIT` in a context that doesn't say).
    Unknown,
}

/// A unit of measure with its conversion factor to the SI base unit of its kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// Name as written (`metre`, `Meter`, `US survey foot`, `Degree`, `grad`, …).
    pub name: String,
    /// What it measures.
    pub kind: UnitKind,
    /// Multiply a value in this unit by `factor` to get metres / radians / unity / seconds.
    /// Always finite and strictly positive.
    pub factor: f64,
}

impl Unit {
    /// The metre.
    pub fn metre() -> Self {
        Self {
            name: "metre".into(),
            kind: UnitKind::Linear,
            factor: 1.0,
        }
    }

    /// The degree (π/180 radians).
    pub fn degree() -> Self {
        Self {
            name: "degree".into(),
            kind: UnitKind::Angular,
            factor: std::f64::consts::PI / 180.0,
        }
    }

    /// Unity (dimensionless scale 1).
    pub fn unity() -> Self {
        Self {
            name: "unity".into(),
            kind: UnitKind::Scale,
            factor: 1.0,
        }
    }

    /// True for a linear unit of (within 1e-12 relative) one metre.
    pub fn is_metre(&self) -> bool {
        self.kind == UnitKind::Linear && approx(self.factor, 1.0)
    }

    /// True for an angular unit of (within 1e-12 relative) one degree.
    pub fn is_degree(&self) -> bool {
        self.kind == UnitKind::Angular && approx(self.factor, std::f64::consts::PI / 180.0)
    }

    /// Converts `value` in this unit to degrees (angular units only).
    ///
    /// Degrees and grads written with truncated factors (`0.0174532925199433`) convert exactly,
    /// so `15` stays `15.0` rather than `15.000000000000002`.
    pub fn to_degrees(&self, value: f64) -> Option<f64> {
        if self.kind != UnitKind::Angular {
            None
        } else if self.is_degree() {
            Some(value)
        } else if approx(self.factor, std::f64::consts::PI / 200.0) {
            Some(value * 0.9)
        } else {
            Some((value * self.factor).to_degrees())
        }
    }

    /// Converts `value` in this unit to metres (linear units only).
    pub fn to_metres(&self, value: f64) -> Option<f64> {
        (self.kind == UnitKind::Linear).then_some(value * self.factor)
    }
}

/// `value` if it is finite, else an [`crate::CrsError::Invalid`] naming `element`. Used after
/// every value × unit-factor product: finite inputs can still overflow (1e300 × 1e300).
pub(crate) fn finite(element: &'static str, value: f64) -> Result<f64, crate::CrsError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(crate::CrsError::invalid(
            element,
            format!("value {value} is not finite"),
        ))
    }
}

pub(crate) fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0)
}

/// A reference ellipsoid (`SPHEROID` / `ELLIPSOID`).
#[derive(Debug, Clone, PartialEq)]
pub struct Ellipsoid {
    /// Name as written (`WGS 84`, `WGS_1984`, `GRS 1980`, …).
    pub name: String,
    /// Semi-major axis in metres (finite, > 0).
    pub semi_major_axis: f64,
    /// Inverse flattening (finite, ≥ 0); `0` means a sphere, as in WKT.
    pub inverse_flattening: f64,
    /// Authority citation, if any.
    pub authority: Option<Authority>,
}

impl Ellipsoid {
    /// Builds an ellipsoid after validating it: the semi-major axis must be finite and positive,
    /// the inverse flattening `0` (sphere) or finite and greater than 1.
    pub(crate) fn checked(
        name: String,
        semi_major_axis: f64,
        inverse_flattening: f64,
        authority: Option<Authority>,
    ) -> Result<Self, crate::CrsError> {
        if !(semi_major_axis.is_finite() && semi_major_axis > 0.0) {
            return Err(crate::CrsError::invalid(
                "ellipsoid",
                format!("`{name}` semi-major axis {semi_major_axis}"),
            ));
        }
        if !(inverse_flattening == 0.0
            || (inverse_flattening.is_finite() && inverse_flattening > 1.0))
        {
            return Err(crate::CrsError::invalid(
                "ellipsoid",
                format!("`{name}` inverse flattening {inverse_flattening}"),
            ));
        }
        Ok(Self {
            name,
            semi_major_axis,
            inverse_flattening,
            authority,
        })
    }

    /// True if this ellipsoid is a sphere (inverse flattening 0).
    pub fn is_sphere(&self) -> bool {
        self.inverse_flattening == 0.0
    }

    /// Semi-minor axis in metres.
    pub fn semi_minor_axis(&self) -> f64 {
        if self.is_sphere() {
            self.semi_major_axis
        } else {
            self.semi_major_axis * (1.0 - 1.0 / self.inverse_flattening)
        }
    }
}

/// A prime meridian. Longitude is always normalised to degrees east of Greenwich.
#[derive(Debug, Clone, PartialEq)]
pub struct PrimeMeridian {
    /// Name as written (`Greenwich`, `Paris`, …).
    pub name: String,
    /// Longitude in degrees east of Greenwich.
    pub longitude: f64,
    /// Authority citation, if any.
    pub authority: Option<Authority>,
}

impl Default for PrimeMeridian {
    fn default() -> Self {
        Self {
            name: "Greenwich".into(),
            longitude: 0.0,
            authority: None,
        }
    }
}

/// A geodetic datum (or datum ensemble), or the datum of a vertical/engineering CRS.
#[derive(Debug, Clone, PartialEq)]
pub struct Datum {
    /// Name as written (`WGS_1984`, `D_WGS_1984`, `World Geodetic System 1984 ensemble`, …).
    /// See [`crate::names::canonical_datum_name`] for a comparable form.
    pub name: String,
    /// The ellipsoid; `None` for vertical and engineering datums.
    pub ellipsoid: Option<Ellipsoid>,
    /// Prime meridian (Greenwich unless stated).
    pub prime_meridian: PrimeMeridian,
    /// WKT1 `TOWGS84` (or a WKT2/PROJJSON `BOUNDCRS` Helmert to WGS 84): dx, dy, dz in metres,
    /// rx, ry, rz in arc-seconds, ds in ppm — padded with zeros when only 3 values are given.
    pub to_wgs84: Option<[f64; 7]>,
    /// Authority citation, if any.
    pub authority: Option<Authority>,
}

impl Datum {
    /// The datum name in a comparable, canonical form (see [`crate::names::canonical_datum_name`]).
    pub fn canonical_name(&self) -> String {
        crate::names::canonical_datum_name(&self.name)
    }
}

/// One projection parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    /// Canonical parameter name ([`ParamName::Other`] when not recognised).
    pub name: ParamName,
    /// Name as written (`False_Easting`, `false_easting`, `False easting`, …).
    pub original_name: String,
    /// Value as written, in [`Self::unit`].
    pub value: f64,
    /// The unit `value` is expressed in: explicit in WKT2/PROJJSON; in WKT1 inherited from the
    /// CRS (angles in the geographic CRS's angular unit, lengths in the projected CRS's linear
    /// unit, scales in unity). `None` when it can't be determined (unknown parameters).
    pub unit: Option<Unit>,
    /// Authority citation (WKT2 `ID`), if any.
    pub authority: Option<Authority>,
}

impl Parameter {
    /// The value in degrees, if this is an angular parameter with a known unit.
    pub fn degrees(&self) -> Option<f64> {
        self.unit.as_ref()?.to_degrees(self.value)
    }

    /// The value in metres, if this is a linear parameter with a known unit.
    pub fn metres(&self) -> Option<f64> {
        self.unit.as_ref()?.to_metres(self.value)
    }

    /// The value with angles in degrees, lengths in metres and scales as plain numbers — the
    /// conventions `proj4rs` expects. Falls back to the raw value when the unit is unknown.
    pub fn normalized(&self) -> f64 {
        match &self.unit {
            Some(u) => u.to_degrees(self.value).unwrap_or(self.value * u.factor),
            None => self.value,
        }
    }

    /// Whether this parameter is an angle, a length, a scale, or unknown.
    pub fn class(&self) -> ParamClass {
        self.name.class()
    }

    /// Checks that the value and its unit conversions ([`Self::normalized`], hence
    /// [`Self::degrees`] / [`Self::metres`]) are finite, so a parsed CRS never carries ±inf.
    pub(crate) fn validated(self) -> Result<Self, crate::CrsError> {
        finite("parameter value", self.value)?;
        finite("parameter value", self.normalized())?;
        Ok(self)
    }
}

/// A map projection (WKT1 `PROJECTION` + `PARAMETER`s, WKT2 `CONVERSION`).
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    /// Canonical method ([`ProjectionMethod::Other`] when not recognised).
    pub method: ProjectionMethod,
    /// Method name as written (`Transverse_Mercator`, `Transverse Mercator`, …).
    pub method_name: String,
    /// Method authority citation (WKT2 `METHOD[…, ID["EPSG",9807]]`), if any.
    pub method_authority: Option<Authority>,
    /// Parameters in the order written.
    pub parameters: Vec<Parameter>,
}

impl Projection {
    /// The first parameter with this canonical name.
    pub fn param(&self, name: ParamName) -> Option<&Parameter> {
        self.parameters.iter().find(|p| p.name == name)
    }
}

/// Direction of a coordinate-system axis.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AxisDirection {
    /// North.
    North,
    /// South.
    South,
    /// East.
    East,
    /// West.
    West,
    /// Up (heights).
    Up,
    /// Down (depths).
    Down,
    /// Geocentric X (towards 0°E on the equator).
    GeocentricX,
    /// Geocentric Y (towards 90°E on the equator).
    GeocentricY,
    /// Geocentric Z (towards the north pole).
    GeocentricZ,
    /// Anything else, lower-cased as written (`other`, `northnortheast`, `unspecified`, …).
    Other(String),
}

impl AxisDirection {
    /// Parses a WKT/PROJJSON direction word, case-insensitively.
    pub fn from_word(word: &str) -> Self {
        match word.to_ascii_lowercase().as_str() {
            "north" => Self::North,
            "south" => Self::South,
            "east" => Self::East,
            "west" => Self::West,
            "up" => Self::Up,
            "down" => Self::Down,
            "geocentricx" => Self::GeocentricX,
            "geocentricy" => Self::GeocentricY,
            "geocentricz" => Self::GeocentricZ,
            other => Self::Other(other.to_owned()),
        }
    }

    fn is_east_west(&self) -> bool {
        matches!(self, Self::East | Self::West)
    }

    fn is_north_south(&self) -> bool {
        matches!(self, Self::North | Self::South)
    }
}

/// One coordinate-system axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    /// Name/abbreviation as written (`Easting`, `(E)`, `geodetic latitude (Lat)`, `Lat`, …).
    pub name: String,
    /// Axis direction.
    pub direction: AxisDirection,
    /// Axis unit, when given per axis (WKT2/PROJJSON); WKT1 axes inherit the CRS unit.
    pub unit: Option<Unit>,
}

/// Order of the first two (horizontal) axes, which decides whether coordinates are x/y or y/x.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxisOrder {
    /// First axis points east or west, second north or south (lon/lat, easting/northing).
    EastNorth,
    /// First axis points north or south, second east or west (lat/lon as in EPSG:4326).
    NorthEast,
    /// Axes are given but are not a horizontal east/north pair (geocentric, polar grids, …).
    Other,
    /// No axes were given (common in WKT1 and universal in ESRI WKT). The WKT1 default — and
    /// what GIS software assumes for data in files — is east/north.
    Unspecified,
}

impl AxisOrder {
    /// Derives the order from a list of axes.
    pub fn from_axes(axes: &[Axis]) -> Self {
        match axes {
            [] => Self::Unspecified,
            [first, second, ..] => {
                if first.direction.is_east_west() && second.direction.is_north_south() {
                    Self::EastNorth
                } else if first.direction.is_north_south() && second.direction.is_east_west() {
                    Self::NorthEast
                } else {
                    Self::Other
                }
            }
            [_] => Self::Other,
        }
    }
}

/// The geographic CRS a projected CRS is based on (its datum, prime meridian and angular unit
/// are copied onto the projected [`Crs`] itself).
#[derive(Debug, Clone, PartialEq)]
pub struct BaseCrs {
    /// Name as written (`WGS 84`, `GCS_WGS_1984`, …).
    pub name: String,
    /// Authority citation, if any.
    pub authority: Option<Authority>,
}

/// A parsed coordinate reference system.
///
/// Field use by kind:
///
/// | kind | datum | projection | linear_unit | angular_unit | base | components |
/// |---|---|---|---|---|---|---|
/// | Geographic | ✓ | – | – | ✓ | – | – |
/// | Projected | ✓ (of the base) | ✓ | ✓ | ✓ (of the base) | ✓ | – |
/// | Geocentric | ✓ | – | ✓ | – | – | – |
/// | Vertical / Engineering | ✓ (no ellipsoid) | – | ✓ | – | – | – |
/// | Compound | – | – | – | – | – | ✓ |
#[derive(Debug, Clone, PartialEq)]
pub struct Crs {
    /// What sort of CRS this is.
    pub kind: CrsKind,
    /// The format/dialect it was parsed from.
    pub dialect: Dialect,
    /// Name as written.
    pub name: String,
    /// The CRS's own authority citation (`AUTHORITY["EPSG","32633"]`, `ID["EPSG",32633]`).
    pub authority: Option<Authority>,
    /// For projected CRSs, the geographic base CRS.
    pub base: Option<BaseCrs>,
    /// Datum (for projected CRSs, the base CRS's datum).
    pub datum: Option<Datum>,
    /// Map projection (projected CRSs only).
    pub projection: Option<Projection>,
    /// Linear unit of the coordinates (projected, geocentric, vertical, engineering).
    pub linear_unit: Option<Unit>,
    /// Angular unit (geographic CRSs; for projected CRSs the base's angular unit).
    pub angular_unit: Option<Unit>,
    /// Axes as written (empty when the definition gives none).
    pub axes: Vec<Axis>,
    /// Order of the horizontal axes, derived from `axes`.
    pub axis_order: AxisOrder,
    /// Components of a compound CRS, horizontal first.
    pub components: Vec<Crs>,
    /// WKT1 `EXTENSION["PROJ4", "…"]` text, if present (GDAL writes one for EPSG:3857).
    pub proj4_extension: Option<String>,
}

impl Crs {
    /// The EPSG code this definition cites for itself, if any. (Identifying CRSs that don't cite
    /// one is session 2.4's job.)
    pub fn epsg(&self) -> Option<u32> {
        self.authority.as_ref().and_then(Authority::epsg)
    }

    /// The horizontal part: `self` for geographic/projected CRSs, the first geographic or
    /// projected component for compound CRSs, `None` otherwise.
    pub fn horizontal(&self) -> Option<&Crs> {
        match self.kind {
            CrsKind::Geographic | CrsKind::Projected => Some(self),
            CrsKind::Compound => self.components.iter().find_map(Crs::horizontal),
            _ => None,
        }
    }

    pub(crate) fn empty(kind: CrsKind, dialect: Dialect, name: String) -> Self {
        Self {
            kind,
            dialect,
            name,
            authority: None,
            base: None,
            datum: None,
            projection: None,
            linear_unit: None,
            angular_unit: None,
            axes: Vec::new(),
            axis_order: AxisOrder::Unspecified,
            components: Vec::new(),
            proj4_extension: None,
        }
    }
}
