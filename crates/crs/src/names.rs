//! Canonical names: projection methods, projection parameters, units and datums.
//!
//! WKT1 (GDAL), ESRI WKT and WKT2/PROJJSON (EPSG) spell the same things differently —
//! `Transverse_Mercator` / `Transverse Mercator`, `false_easting` / `False_Easting` /
//! `False easting` / `Easting at false origin`, `D_WGS_1984` / `WGS_1984` /
//! `World Geodetic System 1984`. Lookups here compare a *key*: the ASCII-lower-cased
//! alphanumerics of a name (see [`name_key`]), so one alias table covers every spelling.

/// Lower-cased ASCII alphanumerics of `name`, everything else dropped.
///
/// `"Lambert Conic Conformal (2SP)"` and `"Lambert_Conic_Conformal_2SP"` both give
/// `"lambertconicconformal2sp"`.
pub fn name_key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// [`name_key`] after removing ESRI's `GCS_` (geographic CRS) and `D_` (datum) prefixes, for
/// comparing CRS, datum and ellipsoid names across dialects (`GCS_WGS_1984` ↔ `WGS_1984`).
pub fn normalize_name(name: &str) -> String {
    let trimmed = name.trim();
    let stripped = strip_prefix_ignore_case(trimmed, "GCS_")
        .or_else(|| strip_prefix_ignore_case(trimmed, "D_"))
        .unwrap_or(trimmed);
    name_key(stripped)
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// A comparable datum name: the EPSG name for well-known datums whatever the dialect
/// (`D_WGS_1984`, `WGS_1984`, `World Geodetic System 1984 ensemble` → `World Geodetic System
/// 1984`), otherwise the name with ESRI's `D_` prefix removed and underscores turned into spaces.
pub fn canonical_datum_name(name: &str) -> String {
    let key = normalize_name(name);
    if let Some((_, canonical)) = DATUM_ALIASES.iter().find(|(alias, _)| *alias == key) {
        return (*canonical).to_owned();
    }
    let trimmed = name.trim();
    strip_prefix_ignore_case(trimmed, "D_")
        .unwrap_or(trimmed)
        .replace('_', " ")
}

/// Datum name keys (after [`normalize_name`]) → EPSG datum names, for the datums most data uses.
const DATUM_ALIASES: &[(&str, &str)] = &[
    ("wgs1984", "World Geodetic System 1984"),
    ("wgs84", "World Geodetic System 1984"),
    ("worldgeodeticsystem1984", "World Geodetic System 1984"),
    (
        "worldgeodeticsystem1984ensemble",
        "World Geodetic System 1984",
    ),
    ("northamerican1983", "North American Datum 1983"),
    ("northamericandatum1983", "North American Datum 1983"),
    ("nad83", "North American Datum 1983"),
    ("northamerican1927", "North American Datum 1927"),
    ("northamericandatum1927", "North American Datum 1927"),
    ("nad27", "North American Datum 1927"),
    ("etrs1989", "European Terrestrial Reference System 1989"),
    (
        "europeanterrestrialreferencesystem1989",
        "European Terrestrial Reference System 1989",
    ),
    (
        "europeanterrestrialreferencesystem1989ensemble",
        "European Terrestrial Reference System 1989",
    ),
    ("european1950", "European Datum 1950"),
    ("europeandatum1950", "European Datum 1950"),
    ("gda1994", "Geocentric Datum of Australia 1994"),
    (
        "geocentricdatumofaustralia1994",
        "Geocentric Datum of Australia 1994",
    ),
    ("gda2020", "Geocentric Datum of Australia 2020"),
    (
        "geocentricdatumofaustralia2020",
        "Geocentric Datum of Australia 2020",
    ),
    ("nzgd2000", "New Zealand Geodetic Datum 2000"),
    (
        "newzealandgeodeticdatum2000",
        "New Zealand Geodetic Datum 2000",
    ),
    ("osgb1936", "Ordnance Survey of Great Britain 1936"),
    (
        "ordnancesurveyofgreatbritain1936",
        "Ordnance Survey of Great Britain 1936",
    ),
];

/// Canonical projection methods. Names follow the EPSG method names; the doc comment of each
/// variant lists the EPSG method code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProjectionMethod {
    /// Transverse Mercator (EPSG 9807); ESRI `Transverse_Mercator`, `Gauss_Kruger`.
    TransverseMercator,
    /// Transverse Mercator (South Orientated) (EPSG 9808).
    TransverseMercatorSouthOrientated,
    /// Lambert Conic Conformal (1SP) (EPSG 9801).
    LambertConicConformal1Sp,
    /// Lambert Conic Conformal (2SP) (EPSG 9802).
    LambertConicConformal2Sp,
    /// Lambert Conic Conformal (2SP Belgium) (EPSG 9803).
    LambertConicConformal2SpBelgium,
    /// Albers Equal Area (EPSG 9822); ESRI `Albers`, OGC `Albers_Conic_Equal_Area`.
    AlbersEqualArea,
    /// Mercator (variant A) (EPSG 9804); OGC `Mercator_1SP`.
    MercatorVariantA,
    /// Mercator (variant B) (EPSG 9805); OGC `Mercator_2SP`.
    MercatorVariantB,
    /// Popular Visualisation Pseudo Mercator — "Web Mercator" (EPSG 1024); ESRI
    /// `Mercator_Auxiliary_Sphere`.
    PopularVisualisationPseudoMercator,
    /// Polar Stereographic (variant A) (EPSG 9810).
    PolarStereographicVariantA,
    /// Polar Stereographic (variant B) (EPSG 9829); ESRI `Stereographic_North_Pole` /
    /// `Stereographic_South_Pole`.
    PolarStereographicVariantB,
    /// Polar Stereographic (variant C) (EPSG 9830).
    PolarStereographicVariantC,
    /// Oblique Stereographic (EPSG 9809); ESRI `Double_Stereographic`.
    ObliqueStereographic,
    /// Generic (non-polar) Stereographic, PROJ `stere` — ESRI/OGC `Stereographic`.
    Stereographic,
    /// Lambert Azimuthal Equal Area (EPSG 9820, spherical 1027).
    LambertAzimuthalEqualArea,
    /// Azimuthal Equidistant (PROJ `aeqd`).
    AzimuthalEquidistant,
    /// Equidistant Cylindrical (EPSG 1028, spherical 1029); OGC `Equirectangular`, ESRI
    /// `Equidistant_Cylindrical`, `Plate_Carree`.
    EquidistantCylindrical,
    /// Cassini-Soldner (EPSG 9806); ESRI `Cassini`.
    CassiniSoldner,
    /// Hotine Oblique Mercator (variant A) (EPSG 9812); OGC `Hotine_Oblique_Mercator`, ESRI
    /// `Hotine_Oblique_Mercator_Azimuth_Natural_Origin`, `Rectified_Skew_Orthomorphic_Natural_Origin`.
    HotineObliqueMercatorVariantA,
    /// Hotine Oblique Mercator (variant B) (EPSG 9815); OGC `Oblique_Mercator`, ESRI
    /// `Hotine_Oblique_Mercator_Azimuth_Center`, `Rectified_Skew_Orthomorphic_Center`.
    HotineObliqueMercatorVariantB,
    /// Krovak (EPSG 9819).
    Krovak,
    /// Krovak (North Orientated) (EPSG 1041); also ESRI `Krovak` with `X_Scale = -1` and
    /// `XY_Plane_Rotation = 90`.
    KrovakNorthOrientated,
    /// New Zealand Map Grid (EPSG 9811).
    NewZealandMapGrid,
    /// Lambert Cylindrical Equal Area (EPSG 9835, spherical 9834); OGC/ESRI `Cylindrical_Equal_Area`.
    LambertCylindricalEqualArea,
    /// Equidistant Conic (PROJ `eqdc`).
    EquidistantConic,
    /// American Polyconic (EPSG 9818); OGC/ESRI `Polyconic`.
    AmericanPolyconic,
    /// Orthographic (EPSG 9840).
    Orthographic,
    /// Sinusoidal.
    Sinusoidal,
    /// Mollweide.
    Mollweide,
    /// Robinson.
    Robinson,
    /// Equal Earth (EPSG 1078).
    EqualEarth,
    /// Gnomonic.
    Gnomonic,
    /// Miller Cylindrical.
    MillerCylindrical,
    /// Eckert IV.
    EckertIv,
    /// Eckert VI.
    EckertVi,
    /// Van der Grinten (I).
    VanDerGrinten,
    /// Geostationary Satellite View.
    GeostationarySatellite,
    /// Not recognised; see [`crate::Projection::method_name`].
    Other,
}

/// Method name keys → canonical method. Some keys are ambiguous on their own (ESRI
/// `Lambert_Conformal_Conic`, `Mercator`, `Stereographic`, OGC `Polar_Stereographic`) and are
/// refined from the parameters by [`refine_method`].
const METHOD_ALIASES: &[(&str, ProjectionMethod)] = {
    use ProjectionMethod as M;
    &[
        ("transversemercator", M::TransverseMercator),
        ("gausskruger", M::TransverseMercator),
        (
            "transversemercatorsouthorientated",
            M::TransverseMercatorSouthOrientated,
        ),
        (
            "transversemercatorsouthoriented",
            M::TransverseMercatorSouthOrientated,
        ),
        ("lambertconformalconic1sp", M::LambertConicConformal1Sp),
        ("lambertconicconformal1sp", M::LambertConicConformal1Sp),
        ("lambertconformalconic2sp", M::LambertConicConformal2Sp),
        ("lambertconicconformal2sp", M::LambertConicConformal2Sp),
        ("lambertconformalconic", M::LambertConicConformal2Sp),
        (
            "lambertconformalconic2spbelgium",
            M::LambertConicConformal2SpBelgium,
        ),
        (
            "lambertconicconformal2spbelgium",
            M::LambertConicConformal2SpBelgium,
        ),
        ("albersconicequalarea", M::AlbersEqualArea),
        ("albersequalarea", M::AlbersEqualArea),
        ("albers", M::AlbersEqualArea),
        ("mercator1sp", M::MercatorVariantA),
        ("mercatorvarianta", M::MercatorVariantA),
        ("mercator", M::MercatorVariantA),
        ("mercator2sp", M::MercatorVariantB),
        ("mercatorvariantb", M::MercatorVariantB),
        (
            "popularvisualisationpseudomercator",
            M::PopularVisualisationPseudoMercator,
        ),
        (
            "popularvisualizationpseudomercator",
            M::PopularVisualisationPseudoMercator,
        ),
        (
            "mercatorauxiliarysphere",
            M::PopularVisualisationPseudoMercator,
        ),
        ("pseudomercator", M::PopularVisualisationPseudoMercator),
        ("polarstereographic", M::PolarStereographicVariantA),
        ("polarstereographicvarianta", M::PolarStereographicVariantA),
        ("polarstereographicvariantb", M::PolarStereographicVariantB),
        ("stereographicnorthpole", M::PolarStereographicVariantB),
        ("stereographicsouthpole", M::PolarStereographicVariantB),
        ("polarstereographicvariantc", M::PolarStereographicVariantC),
        ("obliquestereographic", M::ObliqueStereographic),
        ("doublestereographic", M::ObliqueStereographic),
        ("stereographic", M::Stereographic),
        ("lambertazimuthalequalarea", M::LambertAzimuthalEqualArea),
        (
            "lambertazimuthalequalareaspherical",
            M::LambertAzimuthalEqualArea,
        ),
        ("azimuthalequidistant", M::AzimuthalEquidistant),
        ("equirectangular", M::EquidistantCylindrical),
        ("equidistantcylindrical", M::EquidistantCylindrical),
        ("equidistantcylindricalspherical", M::EquidistantCylindrical),
        ("platecarree", M::EquidistantCylindrical),
        ("cassinisoldner", M::CassiniSoldner),
        ("cassini", M::CassiniSoldner),
        ("hotineobliquemercator", M::HotineObliqueMercatorVariantA),
        (
            "hotineobliquemercatorvarianta",
            M::HotineObliqueMercatorVariantA,
        ),
        (
            "hotineobliquemercatorazimuthnaturalorigin",
            M::HotineObliqueMercatorVariantA,
        ),
        (
            "rectifiedskeworthomorphicnaturalorigin",
            M::HotineObliqueMercatorVariantA,
        ),
        ("obliquemercator", M::HotineObliqueMercatorVariantB),
        (
            "hotineobliquemercatorvariantb",
            M::HotineObliqueMercatorVariantB,
        ),
        (
            "hotineobliquemercatorazimuthcenter",
            M::HotineObliqueMercatorVariantB,
        ),
        (
            "rectifiedskeworthomorphiccenter",
            M::HotineObliqueMercatorVariantB,
        ),
        ("krovak", M::Krovak),
        ("krovaknorthorientated", M::KrovakNorthOrientated),
        ("krovaknorthoriented", M::KrovakNorthOrientated),
        ("newzealandmapgrid", M::NewZealandMapGrid),
        ("cylindricalequalarea", M::LambertCylindricalEqualArea),
        // ESRI: cylindrical equal area with a standard parallel of 30° (written or implied).
        ("behrmann", M::LambertCylindricalEqualArea),
        (
            "lambertcylindricalequalarea",
            M::LambertCylindricalEqualArea,
        ),
        (
            "lambertcylindricalequalareaspherical",
            M::LambertCylindricalEqualArea,
        ),
        ("equidistantconic", M::EquidistantConic),
        ("polyconic", M::AmericanPolyconic),
        ("americanpolyconic", M::AmericanPolyconic),
        ("orthographic", M::Orthographic),
        ("sinusoidal", M::Sinusoidal),
        ("mollweide", M::Mollweide),
        ("robinson", M::Robinson),
        ("equalearth", M::EqualEarth),
        ("gnomonic", M::Gnomonic),
        ("millercylindrical", M::MillerCylindrical),
        ("eckertiv", M::EckertIv),
        ("eckertvi", M::EckertVi),
        ("vandergrinten", M::VanDerGrinten),
        ("vandergrinteni", M::VanDerGrinten),
        ("geostationarysatellite", M::GeostationarySatellite),
        ("geostationarysatelliteview", M::GeostationarySatellite),
    ]
};

impl ProjectionMethod {
    /// Looks a method up by any known spelling (OGC WKT1, ESRI, EPSG/WKT2, PROJJSON).
    /// Ambiguous ESRI/OGC names (`Lambert_Conformal_Conic`, `Mercator`, `Stereographic`,
    /// `Polar_Stereographic`, ESRI `Krovak`) resolve to their most common meaning here; the
    /// parsers then refine the method from the parameters, the way GDAL does.
    pub fn from_name(name: &str) -> Self {
        let key = name_key(name);
        METHOD_ALIASES
            .iter()
            .find(|(alias, _)| *alias == key)
            .map_or(Self::Other, |(_, m)| *m)
    }

    /// Looks a method up by its EPSG method code.
    pub fn from_epsg(code: u32) -> Option<Self> {
        use ProjectionMethod as M;
        Some(match code {
            9807 | 1111 => M::TransverseMercator, // 1111: Transverse Mercator 3D
            9808 => M::TransverseMercatorSouthOrientated,
            9801 => M::LambertConicConformal1Sp,
            9802 => M::LambertConicConformal2Sp,
            9803 => M::LambertConicConformal2SpBelgium,
            9822 => M::AlbersEqualArea,
            9804 => M::MercatorVariantA,
            9805 => M::MercatorVariantB,
            1024 => M::PopularVisualisationPseudoMercator,
            9810 => M::PolarStereographicVariantA,
            9829 => M::PolarStereographicVariantB,
            9830 => M::PolarStereographicVariantC,
            9809 => M::ObliqueStereographic,
            9820 | 1027 => M::LambertAzimuthalEqualArea,
            1028 | 1029 => M::EquidistantCylindrical,
            9806 => M::CassiniSoldner,
            9812 => M::HotineObliqueMercatorVariantA,
            9815 => M::HotineObliqueMercatorVariantB,
            9819 => M::Krovak,
            1041 => M::KrovakNorthOrientated,
            9811 => M::NewZealandMapGrid,
            9834 | 9835 => M::LambertCylindricalEqualArea,
            9818 => M::AmericanPolyconic,
            9840 => M::Orthographic,
            1078 => M::EqualEarth,
            _ => return None,
        })
    }
}

/// Resolves ambiguous method names using the parameters, the way GDAL does when importing
/// ESRI/OGC WKT1:
///
/// - ESRI `Lambert_Conformal_Conic`: 2SP if a second standard parallel is given, else 1SP.
/// - ESRI `Mercator`: variant B if a standard parallel is given, else variant A.
/// - ESRI/OGC `Stereographic` with latitude of origin ±90°: Polar Stereographic variant A.
/// - OGC `Polar_Stereographic`: variant A if the latitude of origin is ±90°, else variant B
///   (whose latitude of origin is then really the latitude of true scale).
/// - ESRI `Krovak` with `X_Scale = -1` and `XY_Plane_Rotation = 90`: Krovak (North Orientated).
/// - OGC `Mercator_1SP` with a PROJ4 extension on a 6378137 m sphere (GDAL's EPSG:3857 WKT1):
///   Popular Visualisation Pseudo Mercator.
pub(crate) fn refine_method(
    method_name: &str,
    method: ProjectionMethod,
    params: &[crate::Parameter],
    proj4_extension: Option<&str>,
) -> ProjectionMethod {
    use ProjectionMethod as M;
    let get = |n: ParamName| {
        params
            .iter()
            .find(|p| p.name == n)
            .map(crate::Parameter::normalized)
    };
    let is_polar = |v: Option<f64>| v.is_some_and(|lat| (lat.abs() - 90.0).abs() < 1e-9);
    match name_key(method_name).as_str() {
        "lambertconformalconic" if get(ParamName::StandardParallel2).is_none() => {
            M::LambertConicConformal1Sp
        }
        "mercator" if get(ParamName::StandardParallel1).is_some() => M::MercatorVariantB,
        "stereographic" if is_polar(get(ParamName::LatitudeOfOrigin)) => {
            M::PolarStereographicVariantA
        }
        "polarstereographic" if !is_polar(get(ParamName::LatitudeOfOrigin)) => {
            M::PolarStereographicVariantB
        }
        "krovak"
            if get(ParamName::XScale).is_some_and(|x| (x + 1.0).abs() < 1e-9)
                && get(ParamName::XyPlaneRotation).is_some_and(|r| (r - 90.0).abs() < 1e-9) =>
        {
            M::KrovakNorthOrientated
        }
        "mercator1sp" if proj4_extension.is_some_and(is_web_mercator_proj4) => {
            M::PopularVisualisationPseudoMercator
        }
        _ => method,
    }
}

/// Renames parameters whose meaning depends on the method so every dialect ends up with the same
/// canonical names (GDAL applies the same mapping on import):
///
/// - Polar Stereographic variant B: OGC WKT1 writes the latitude of true scale as
///   `latitude_of_origin` → [`ParamName::StandardParallel1`] (as EPSG's "Latitude of standard
///   parallel").
/// - Krovak: OGC and ESRI write the co-latitude of the cone axis as `azimuth` →
///   [`ParamName::ColatitudeOfConeAxis`].
pub(crate) fn harmonize_params(method: ProjectionMethod, params: &mut [crate::Parameter]) {
    use ProjectionMethod as M;
    let rename = |params: &mut [crate::Parameter], from: ParamName, to: ParamName| {
        if params.iter().all(|p| p.name != to) {
            for p in params.iter_mut().filter(|p| p.name == from) {
                p.name = to;
            }
        }
    };
    match method {
        M::PolarStereographicVariantB => {
            rename(
                params,
                ParamName::LatitudeOfOrigin,
                ParamName::StandardParallel1,
            );
        }
        M::Krovak | M::KrovakNorthOrientated => {
            rename(params, ParamName::Azimuth, ParamName::ColatitudeOfConeAxis);
        }
        _ => {}
    }
}

fn is_web_mercator_proj4(proj4: &str) -> bool {
    let has = |token: &str| proj4.split_whitespace().any(|t| t == token);
    has("+proj=merc") && has("+a=6378137") && has("+b=6378137")
}

/// Canonical projection parameter names, following PROJ's semantics (`+lat_0`, `+lon_0`, `+k_0`,
/// `+x_0`, …) so that synonyms across dialects collapse: `latitude_of_origin`,
/// `Latitude of natural origin`, `Latitude of false origin`, `latitude_of_center` and
/// `Latitude of projection centre` are all [`ParamName::LatitudeOfOrigin`].
///
/// [`ParamName::from_name`] maps purely by name. The parsers then fix the few spellings whose
/// meaning depends on the method (OGC `Polar_Stereographic` variant B's `latitude_of_origin` is
/// the standard parallel; Krovak's `azimuth` is the co-latitude of the cone axis), so a parsed
/// [`crate::Projection`] uses the same names whatever dialect it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ParamName {
    /// Latitude of natural/false origin or projection centre (PROJ `lat_0`).
    LatitudeOfOrigin,
    /// Central meridian, longitude of natural/false origin or projection centre (PROJ `lon_0`).
    LongitudeOfOrigin,
    /// Scale factor at natural origin / on initial line / on pseudo standard parallel (`k_0`).
    ScaleFactor,
    /// False easting, easting at false origin / projection centre (`x_0`).
    FalseEasting,
    /// False northing, northing at false origin / projection centre (`y_0`).
    FalseNorthing,
    /// First standard parallel, or *the* standard parallel (`lat_1` / `lat_ts`).
    StandardParallel1,
    /// Second standard parallel (`lat_2`).
    StandardParallel2,
    /// Azimuth of the initial line (`alpha`).
    Azimuth,
    /// Angle from rectified to skew grid (`gamma`).
    RectifiedGridAngle,
    /// Krovak latitude of pseudo standard parallel.
    PseudoStandardParallel,
    /// Krovak co-latitude of cone axis.
    ColatitudeOfConeAxis,
    /// ESRI `Auxiliary_Sphere_Type` (Web Mercator).
    AuxiliarySphereType,
    /// Satellite height (geostationary).
    SatelliteHeight,
    /// ESRI `X_Scale` (Krovak axis flip).
    XScale,
    /// ESRI `Y_Scale` (Krovak axis flip).
    YScale,
    /// ESRI `XY_Plane_Rotation` (Krovak).
    XyPlaneRotation,
    /// Not recognised; see [`crate::Parameter::original_name`].
    Other,
}

/// Whether a parameter is an angle, a length or a scale — decides which CRS unit a WKT1
/// parameter inherits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamClass {
    /// Angle (in the geographic CRS's angular unit in WKT1).
    Angular,
    /// Length (in the projected CRS's linear unit in WKT1).
    Linear,
    /// Dimensionless scale.
    Scale,
    /// Unknown or unitless code.
    Unknown,
}

impl ParamClass {
    /// The unit kind a parameter of this class is measured in (the kind a generic WKT `UNIT`
    /// on the parameter takes).
    pub fn unit_kind(self) -> crate::UnitKind {
        match self {
            Self::Angular => crate::UnitKind::Angular,
            Self::Linear => crate::UnitKind::Linear,
            Self::Scale => crate::UnitKind::Scale,
            Self::Unknown => crate::UnitKind::Unknown,
        }
    }

    /// The unit a parameter of this class inherits when none is written: the geographic
    /// (base) CRS's angular unit, the projected CRS's linear unit, or unity.
    pub(crate) fn default_unit(
        self,
        angular: &crate::Unit,
        linear: Option<&crate::Unit>,
    ) -> Option<crate::Unit> {
        match self {
            Self::Angular => Some(angular.clone()),
            Self::Linear => linear.cloned(),
            Self::Scale => Some(crate::Unit::unity()),
            Self::Unknown => None,
        }
    }
}

const PARAM_ALIASES: &[(&str, ParamName)] = {
    use ParamName as P;
    &[
        ("latitudeoforigin", P::LatitudeOfOrigin),
        ("latitudeofnaturalorigin", P::LatitudeOfOrigin),
        ("latitudeoffalseorigin", P::LatitudeOfOrigin),
        ("latitudeofcenter", P::LatitudeOfOrigin),
        ("latitudeofcentre", P::LatitudeOfOrigin),
        ("latitudeofprojectioncentre", P::LatitudeOfOrigin),
        ("latitudeofprojectioncenter", P::LatitudeOfOrigin),
        ("centralmeridian", P::LongitudeOfOrigin),
        ("longitudeofnaturalorigin", P::LongitudeOfOrigin),
        ("longitudeoffalseorigin", P::LongitudeOfOrigin),
        ("longitudeoforigin", P::LongitudeOfOrigin),
        ("longitudeofcenter", P::LongitudeOfOrigin),
        ("longitudeofcentre", P::LongitudeOfOrigin),
        ("longitudeofprojectioncentre", P::LongitudeOfOrigin),
        ("longitudeofprojectioncenter", P::LongitudeOfOrigin),
        ("scalefactor", P::ScaleFactor),
        ("scalefactoratnaturalorigin", P::ScaleFactor),
        ("scalefactoroninitialline", P::ScaleFactor),
        ("scalefactoratprojectioncentre", P::ScaleFactor),
        ("scalefactoronpseudostandardparallel", P::ScaleFactor),
        ("falseeasting", P::FalseEasting),
        ("eastingatfalseorigin", P::FalseEasting),
        ("eastingatprojectioncentre", P::FalseEasting),
        ("falsenorthing", P::FalseNorthing),
        ("northingatfalseorigin", P::FalseNorthing),
        ("northingatprojectioncentre", P::FalseNorthing),
        ("standardparallel1", P::StandardParallel1),
        ("latitudeof1ststandardparallel", P::StandardParallel1),
        ("latitudeofstandardparallel", P::StandardParallel1),
        ("standardparallel2", P::StandardParallel2),
        ("latitudeof2ndstandardparallel", P::StandardParallel2),
        ("azimuth", P::Azimuth),
        ("azimuthofinitialline", P::Azimuth),
        ("azimuthatprojectioncentre", P::Azimuth),
        ("rectifiedgridangle", P::RectifiedGridAngle),
        ("anglefromrectifiedtoskewgrid", P::RectifiedGridAngle),
        ("pseudostandardparallel1", P::PseudoStandardParallel),
        (
            "latitudeofpseudostandardparallel",
            P::PseudoStandardParallel,
        ),
        ("colatitudeofconeaxis", P::ColatitudeOfConeAxis),
        ("auxiliaryspheretype", P::AuxiliarySphereType),
        ("satelliteheight", P::SatelliteHeight),
        ("xscale", P::XScale),
        ("yscale", P::YScale),
        ("xyplanerotation", P::XyPlaneRotation),
    ]
};

impl ParamName {
    /// Looks a parameter up by any known spelling.
    pub fn from_name(name: &str) -> Self {
        let key = name_key(name);
        PARAM_ALIASES
            .iter()
            .find(|(alias, _)| *alias == key)
            .map_or(Self::Other, |(_, p)| *p)
    }

    /// Looks a parameter up by its EPSG parameter code.
    pub fn from_epsg(code: u32) -> Option<Self> {
        use ParamName as P;
        Some(match code {
            8801 | 8811 | 8821 => P::LatitudeOfOrigin,
            8802 | 8812 | 8822 | 8833 => P::LongitudeOfOrigin,
            8805 | 8815 | 8819 => P::ScaleFactor,
            8806 | 8816 | 8826 => P::FalseEasting,
            8807 | 8817 | 8827 => P::FalseNorthing,
            8823 | 8832 => P::StandardParallel1,
            8824 => P::StandardParallel2,
            8813 => P::Azimuth,
            8814 => P::RectifiedGridAngle,
            8818 => P::PseudoStandardParallel,
            1036 => P::ColatitudeOfConeAxis,
            _ => return None,
        })
    }

    /// Whether this parameter is an angle, a length or a scale.
    pub fn class(self) -> ParamClass {
        use ParamName as P;
        match self {
            P::LatitudeOfOrigin
            | P::LongitudeOfOrigin
            | P::StandardParallel1
            | P::StandardParallel2
            | P::Azimuth
            | P::RectifiedGridAngle
            | P::PseudoStandardParallel
            | P::ColatitudeOfConeAxis
            | P::XyPlaneRotation => ParamClass::Angular,
            P::FalseEasting | P::FalseNorthing | P::SatelliteHeight => ParamClass::Linear,
            P::ScaleFactor | P::XScale | P::YScale => ParamClass::Scale,
            P::AuxiliarySphereType | P::Other => ParamClass::Unknown,
        }
    }
}

/// A well-known unit by name (`metre`, `Meter`, `US survey foot`, `Foot_US`, `degree`, `grad`,
/// `unity`, …): its kind and factor to the SI base unit. Used where a format names a unit
/// without a conversion factor (PROJJSON string units).
pub fn known_unit(name: &str) -> Option<crate::Unit> {
    use crate::UnitKind as K;
    let deg = std::f64::consts::PI / 180.0;
    let (kind, factor) = match name_key(name).as_str() {
        "metre" | "meter" | "metres" | "meters" | "m" => (K::Linear, 1.0),
        "kilometre" | "kilometer" | "km" => (K::Linear, 1000.0),
        "foot" | "feet" | "ft" | "internationalfoot" | "footinternational" => (K::Linear, 0.3048),
        "ussurveyfoot" | "footus" | "usfoot" | "ftus" | "ussurveyfeet" => {
            (K::Linear, 1200.0 / 3937.0)
        }
        "degree" | "degrees" | "deg" | "degreesupplierdefinedrepresentation" => (K::Angular, deg),
        "grad" | "gon" | "grads" => (K::Angular, std::f64::consts::PI / 200.0),
        "radian" | "radians" | "rad" => (K::Angular, 1.0),
        "arcsecond" | "arcseconds" => (K::Angular, deg / 3600.0),
        "arcminute" | "arcminutes" => (K::Angular, deg / 60.0),
        "unity" | "unitless" | "none" => (K::Scale, 1.0),
        "partspermillion" | "ppm" => (K::Scale, 1e-6),
        "second" | "seconds" | "s" => (K::Time, 1.0),
        _ => return None,
    };
    Some(crate::Unit {
        name: name.to_owned(),
        kind,
        factor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_case_and_punctuation() {
        assert_eq!(
            name_key("Lambert Conic Conformal (2SP)"),
            "lambertconicconformal2sp"
        );
        assert_eq!(normalize_name("GCS_WGS_1984"), "wgs1984");
        assert_eq!(normalize_name("D_WGS_1984"), "wgs1984");
        assert_eq!(normalize_name("WGS_1984"), "wgs1984");
        assert_eq!(normalize_name("ü"), "");
    }

    #[test]
    fn canonical_datums() {
        assert_eq!(
            canonical_datum_name("D_WGS_1984"),
            "World Geodetic System 1984"
        );
        assert_eq!(
            canonical_datum_name("World Geodetic System 1984 ensemble"),
            "World Geodetic System 1984"
        );
        assert_eq!(
            canonical_datum_name("D_North_American_1983"),
            "North American Datum 1983"
        );
        assert_eq!(
            canonical_datum_name("D_Some_Local_Datum"),
            "Some Local Datum"
        );
    }

    #[test]
    fn methods_by_any_spelling() {
        use ProjectionMethod as M;
        for (name, m) in [
            ("Transverse_Mercator", M::TransverseMercator),
            ("Transverse Mercator", M::TransverseMercator),
            ("Gauss_Kruger", M::TransverseMercator),
            ("Lambert_Conformal_Conic_2SP", M::LambertConicConformal2Sp),
            ("Lambert Conic Conformal (1SP)", M::LambertConicConformal1Sp),
            (
                "Mercator_Auxiliary_Sphere",
                M::PopularVisualisationPseudoMercator,
            ),
            ("Albers", M::AlbersEqualArea),
            (
                "Hotine_Oblique_Mercator_Azimuth_Center",
                M::HotineObliqueMercatorVariantB,
            ),
            ("Something else", M::Other),
        ] {
            assert_eq!(ProjectionMethod::from_name(name), m, "{name}");
        }
        assert_eq!(
            ProjectionMethod::from_epsg(9807),
            Some(M::TransverseMercator)
        );
        assert_eq!(ProjectionMethod::from_epsg(1), None);
    }

    #[test]
    fn params_by_any_spelling() {
        use ParamName as P;
        for (name, p) in [
            ("false_easting", P::FalseEasting),
            ("False_Easting", P::FalseEasting),
            ("Easting at false origin", P::FalseEasting),
            ("Latitude of natural origin", P::LatitudeOfOrigin),
            ("latitude_of_center", P::LatitudeOfOrigin),
            ("Central_Meridian", P::LongitudeOfOrigin),
            ("Scale factor at natural origin", P::ScaleFactor),
            ("Latitude of standard parallel", P::StandardParallel1),
            ("Vertical_Shift", P::Other),
        ] {
            assert_eq!(ParamName::from_name(name), p, "{name}");
        }
        assert_eq!(ParamName::from_epsg(8806), Some(P::FalseEasting));
    }

    #[test]
    fn units() {
        let ft = known_unit("US survey foot").expect("known");
        assert!((ft.factor - 0.304_800_609_601_219_2).abs() < 1e-15);
        assert!(known_unit("metre").expect("known").is_metre());
        assert!(known_unit("degree").expect("known").is_degree());
        assert!(known_unit("furlong").is_none());
    }
}
