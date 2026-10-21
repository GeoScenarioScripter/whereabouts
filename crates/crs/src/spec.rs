//! [`ProjSpec`]: a CRS reduced to what transforming coordinates needs — a PROJ-style projection
//! with canonical parameters, the ellipsoid, prime meridian, unit, axis directions and datum shift.
//!
//! Every dialect (and the embedded EPSG table) goes through [`ProjSpec::from_crs`], so two CRSs
//! that describe the same projection end up with the same spec whatever their spelling: ESRI's
//! `Lambert_Conformal_Conic` with equal standard parallels and EPSG's LCC (1SP), Mercator variant A
//! and B, ESRI's negative azimuths and EPSG's 0–360° ones, Swiss Oblique Mercator as Hotine
//! variant B with 90° angles. EPSG parameter matching ([`crate::identify`]) compares specs with
//! [`ProjSpec::equivalent`]; the transform ([`crate::transform`]) builds a `proj4rs` projection or
//! one of this crate's native inverses from it.

use crate::model::{Crs, CrsKind};
use crate::names::{ParamName, ProjectionMethod};
use crate::transform::TransformError;

/// The projection a [`ProjSpec`] uses: a `proj4rs` projection name or a native inverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Method {
    /// Geographic coordinates (no projection).
    LongLat,
    /// Transverse Mercator (`proj4rs` `etmerc`, the exact Poder/Engsager series PROJ uses).
    Tmerc,
    /// Lambert Conic Conformal (1SP and 2SP).
    Lcc,
    /// Lambert Conic Conformal (2SP Belgium); transformed as `lcc` (≈ 30 m difference).
    LccBelgium,
    /// Albers Equal Area.
    Aea,
    /// Mercator (variants A and B, as a scale factor at the equator).
    Merc,
    /// Popular Visualisation Pseudo Mercator: spherical Mercator on the semi-major axis.
    WebMerc,
    /// Polar and oblique stereographic (PROJ `stere`).
    Stere,
    /// Oblique Stereographic (EPSG 9809, PROJ `sterea`).
    Sterea,
    /// Lambert Azimuthal Equal Area.
    Laea,
    /// Equidistant Cylindrical (native inverse; ellipsoidal as EPSG 1028 and PROJ 9).
    Eqc,
    /// Lambert Cylindrical Equal Area.
    Cea,
    /// Krovak (both orientations; `proj4rs` fixes Bessel and the standard cone parameters).
    Krovak,
    /// Swiss Oblique Mercator (Hotine variant B with 90° azimuth and grid angle).
    Somerc,
    /// Mollweide.
    Moll,
    /// Miller Cylindrical.
    Mill,
    /// Geostationary Satellite View.
    Geos,
    /// Cassini-Soldner (native inverse).
    Cass,
    /// Hotine Oblique Mercator variants A and B (native inverse).
    Omerc,
    /// New Zealand Map Grid (native inverse).
    Nzmg,
    /// Equal Earth (native inverse).
    EqEarth,
    /// Robinson (native inverse).
    Robin,
    /// Sinusoidal (native inverse).
    Sinu,
}

/// Canonical projection parameters (PROJ meanings). Angles in degrees, lengths in metres. Unused
/// parameters keep their defaults (0, scale 1), so equal projections compare equal field by field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Params {
    pub lat_0: f64,
    pub lon_0: f64,
    pub k_0: f64,
    pub x_0: f64,
    pub y_0: f64,
    pub lat_1: f64,
    pub lat_2: f64,
    pub lat_ts: f64,
    pub alpha: f64,
    pub gamma: f64,
    /// Satellite height (geostationary).
    pub h: f64,
    /// Hotine variant A: false origin at the natural origin (PROJ `+no_uoff`).
    pub no_uoff: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            lat_0: 0.0,
            lon_0: 0.0,
            k_0: 1.0,
            x_0: 0.0,
            y_0: 0.0,
            lat_1: 0.0,
            lat_2: 0.0,
            lat_ts: 0.0,
            alpha: 0.0,
            gamma: 0.0,
            h: 0.0,
            no_uoff: false,
        }
    }
}

/// A CRS reduced to its coordinate-transformation essentials.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProjSpec {
    pub method: Method,
    pub params: Params,
    /// Semi-major axis, metres.
    pub a: f64,
    /// Inverse flattening, 0 for a sphere.
    pub rf: f64,
    /// Prime meridian, degrees east of Greenwich.
    pub pm: f64,
    /// Metres per coordinate unit (projected) or degrees per coordinate unit (geographic).
    pub unit: f64,
    /// Directions of the two file coordinates in traditional GIS order (`e`/`w`/`n`/`s`):
    /// the CRS's declared axes, or the method's convention when none are declared (`ws` for
    /// south-orientated TM, `sw` for classic Krovak, else `en`). A declared north-first pair
    /// (`ne`, EPSG:3035, EPSG:4326) is swapped to `en`, as GDAL does for file data
    /// (`OAMS_TRADITIONAL_GIS_ORDER`); south-first pairs (classic Krovak, `sw`) are kept, as
    /// GDAL keeps them.
    pub axes: [u8; 2],
    /// The declared axes were north-first and swapped for GIS order ([`crate::AxisPolicy`]).
    pub gis_swap: bool,
    /// EPSG "(Spherical)" method variants: use the authalic sphere (PROJ `+R_A`).
    pub spherical: bool,
    /// Helmert parameters to WGS 84 (position vector; m, arc-seconds, ppm), if known.
    pub to_wgs84: Option<[f64; 7]>,
}

/// Tolerances for [`ProjSpec::equivalent`]: about a millimetre, generous enough for the
/// 13–15 significant digits ESRI writes, tight enough to separate real CRS variants.
const ANGLE_TOL: f64 = 1e-7;
const LENGTH_TOL: f64 = 1e-3;
const SCALE_TOL: f64 = 1e-8;
const RELATIVE_TOL: f64 = 1e-9;

/// Krovak parameters `proj4rs` hard-codes (co-latitude of the cone axis, pseudo standard parallel).
const KROVAK_ALPHA: f64 = 30.288_139_752_777_78;
const KROVAK_PSEUDO_PARALLEL: f64 = 78.5;
const BESSEL_A: f64 = 6_377_397.155;

impl ProjSpec {
    /// Reduces a parsed CRS (its horizontal component, for compound CRSs) to a spec.
    pub(crate) fn from_crs(crs: &Crs) -> Result<Self, TransformError> {
        let h = crs
            .horizontal()
            .ok_or(TransformError::NotHorizontal(crs.kind))?;
        let datum = h
            .datum
            .as_ref()
            .ok_or_else(|| TransformError::invalid("CRS has no datum"))?;
        let ellipsoid = datum
            .ellipsoid
            .as_ref()
            .ok_or_else(|| TransformError::invalid("datum has no ellipsoid"))?;
        let mut spec = ProjSpec {
            method: Method::LongLat,
            params: Params::default(),
            a: ellipsoid.semi_major_axis,
            rf: ellipsoid.inverse_flattening,
            pm: datum.prime_meridian.longitude,
            unit: 1.0,
            axes: *b"en",
            gis_swap: false,
            spherical: false,
            to_wgs84: datum.to_wgs84,
        };
        if !(spec.pm.is_finite() && spec.pm.abs() <= 360.0) {
            return Err(TransformError::invalid(format!(
                "prime meridian {}",
                spec.pm
            )));
        }
        let declared = declared_letters(&h.axes);
        match h.kind {
            CrsKind::Geographic => {
                spec.gis_swap = declared.is_some_and(|d| matches!(d[0], b'n' | b's'));
                spec.unit = match &h.angular_unit {
                    Some(u) => u
                        .to_degrees(1.0)
                        .ok_or_else(|| TransformError::invalid("angular unit is not an angle"))?,
                    None => 1.0,
                };
            }
            CrsKind::Projected => {
                spec.unit = match &h.linear_unit {
                    Some(u) => u
                        .to_metres(1.0)
                        .ok_or_else(|| TransformError::invalid("linear unit is not a length"))?,
                    None => 1.0,
                };
                let projection = h
                    .projection
                    .as_ref()
                    .ok_or_else(|| TransformError::invalid("projected CRS has no projection"))?;
                spec.set_projection(projection, declared)?;
            }
            kind => return Err(TransformError::NotHorizontal(kind)),
        }
        if !(spec.unit.is_finite() && spec.unit > 0.0) {
            return Err(TransformError::invalid(format!(
                "unit factor {}",
                spec.unit
            )));
        }
        Ok(spec)
    }

    fn set_projection(
        &mut self,
        projection: &crate::Projection,
        declared: Option<[u8; 2]>,
    ) -> Result<(), TransformError> {
        use ProjectionMethod as M;
        let get = |name: ParamName| projection.param(name).map(crate::Parameter::normalized);
        let lat_0 = get(ParamName::LatitudeOfOrigin).unwrap_or(0.0);
        let lon_0 = get(ParamName::LongitudeOfOrigin).unwrap_or(0.0);
        let k_0 = get(ParamName::ScaleFactor).unwrap_or(1.0);
        let x_0 = get(ParamName::FalseEasting).unwrap_or(0.0);
        let y_0 = get(ParamName::FalseNorthing).unwrap_or(0.0);
        let sp1 = get(ParamName::StandardParallel1);
        let sp2 = get(ParamName::StandardParallel2);
        let mut p = Params {
            lat_0,
            lon_0: normalize_lon(lon_0),
            k_0,
            x_0,
            y_0,
            ..Params::default()
        };
        self.method = match projection.method {
            M::TransverseMercator => Method::Tmerc,
            M::TransverseMercatorSouthOrientated => {
                self.axes = *b"ws";
                Method::Tmerc
            }
            M::LambertConicConformal1Sp => {
                p.lat_1 = lat_0;
                p.lat_2 = lat_0;
                Method::Lcc
            }
            M::LambertConicConformal2Sp | M::LambertConicConformal2SpBelgium => {
                p.lat_1 = sp1.unwrap_or(lat_0);
                p.lat_2 = sp2.unwrap_or(p.lat_1);
                if projection.method == M::LambertConicConformal2SpBelgium {
                    Method::LccBelgium
                } else {
                    Method::Lcc
                }
            }
            M::AlbersEqualArea => {
                p.lat_1 = sp1.unwrap_or(lat_0);
                p.lat_2 = sp2.unwrap_or(p.lat_1);
                p.k_0 = 1.0;
                Method::Aea
            }
            M::MercatorVariantA | M::MercatorVariantB => {
                // Both as a scale factor at the equator: variant B's standard parallel φ is
                // variant A with k0 = cos φ / sqrt(1 − e² sin² φ).
                if let Some(lat_ts) = sp1.filter(|_| projection.method == M::MercatorVariantB) {
                    let es = self.es();
                    let s = lat_ts.to_radians().sin();
                    p.k_0 = lat_ts.to_radians().cos() / (1.0 - es * s * s).sqrt();
                }
                p.lat_0 = 0.0;
                Method::Merc
            }
            M::PopularVisualisationPseudoMercator => {
                p.lat_0 = 0.0;
                p.k_0 = 1.0;
                Method::WebMerc
            }
            M::PolarStereographicVariantA => {
                if (lat_0.abs() - 90.0).abs() > ANGLE_TOL {
                    return Err(TransformError::invalid(format!(
                        "polar stereographic latitude of origin {lat_0}"
                    )));
                }
                p.lat_0 = 90f64.copysign(lat_0);
                p.lat_ts = p.lat_0;
                Method::Stere
            }
            M::PolarStereographicVariantB => {
                let lat_ts = sp1.unwrap_or(lat_0);
                p.lat_0 = 90f64.copysign(lat_ts);
                p.lat_ts = lat_ts;
                p.k_0 = 1.0;
                Method::Stere
            }
            M::Stereographic => {
                p.lat_ts = lat_0;
                Method::Stere
            }
            M::ObliqueStereographic => Method::Sterea,
            M::LambertAzimuthalEqualArea => {
                p.k_0 = 1.0;
                Method::Laea
            }
            M::EquidistantCylindrical => {
                p.lat_ts = sp1.unwrap_or(0.0);
                p.k_0 = 1.0;
                Method::Eqc
            }
            M::LambertCylindricalEqualArea => {
                let behrmann = crate::names::name_key(&projection.method_name) == "behrmann";
                p.lat_ts = sp1.unwrap_or(if behrmann { 30.0 } else { 0.0 });
                p.k_0 = 1.0;
                Method::Cea
            }
            M::CassiniSoldner => Method::Cass,
            M::HotineObliqueMercatorVariantA | M::HotineObliqueMercatorVariantB => {
                let alpha = get(ParamName::Azimuth)
                    .ok_or_else(|| TransformError::invalid("oblique Mercator without azimuth"))?;
                let gamma = get(ParamName::RectifiedGridAngle)
                    .or_else(|| get(ParamName::XyPlaneRotation))
                    .unwrap_or(alpha);
                p.alpha = normalize_angle(alpha);
                p.gamma = normalize_angle(gamma);
                p.no_uoff = projection.method == M::HotineObliqueMercatorVariantA;
                if !p.no_uoff
                    && angle_close(p.alpha.abs(), 90.0)
                    && angle_close(p.gamma.abs(), 90.0)
                {
                    // PROJ does the same for EPSG:2056 and friends: omerc rejects a 90° azimuth.
                    p.alpha = 0.0;
                    p.gamma = 0.0;
                    p.no_uoff = false;
                    Method::Somerc
                } else {
                    Method::Omerc
                }
            }
            M::Krovak | M::KrovakNorthOrientated => {
                let alpha = get(ParamName::ColatitudeOfConeAxis).unwrap_or(KROVAK_ALPHA);
                let lat_1 =
                    get(ParamName::PseudoStandardParallel).unwrap_or(KROVAK_PSEUDO_PARALLEL);
                if !angle_close(alpha, KROVAK_ALPHA)
                    || !angle_close(lat_1, KROVAK_PSEUDO_PARALLEL)
                    || (self.a - BESSEL_A).abs() > 1.0
                {
                    return Err(TransformError::UnsupportedMethod(format!(
                        "{} with non-standard cone parameters or ellipsoid",
                        projection.method_name
                    )));
                }
                p.alpha = KROVAK_ALPHA;
                p.lat_1 = KROVAK_PSEUDO_PARALLEL;
                // proj4rs computes the North Orientated (negative east/north) plane. Classic
                // Krovak is south/west orientated (southing, westing). GDAL's WKT1 has no "North
                // Orientated" method name and writes EPSG:5514 as `Krovak` with east/north axes,
                // so declared axes win over the method below.
                if projection.method == M::Krovak {
                    self.axes = *b"sw";
                }
                Method::Krovak
            }
            M::NewZealandMapGrid => Method::Nzmg,
            M::Mollweide => Method::Moll,
            M::MillerCylindrical => Method::Mill,
            M::EqualEarth => Method::EqEarth,
            M::Robinson => Method::Robin,
            M::Sinusoidal => Method::Sinu,
            M::GeostationarySatellite => {
                p.h = get(ParamName::SatelliteHeight)
                    .ok_or_else(|| TransformError::invalid("geostationary without height"))?;
                Method::Geos
            }
            _ => {
                return Err(TransformError::UnsupportedMethod(
                    projection.method_name.clone(),
                ));
            }
        };
        if let Some(d) = declared {
            self.axes = d;
        }
        if self.axes[0] == b'n' {
            self.axes = [self.axes[1], self.axes[0]];
            self.gis_swap = true;
        }
        // EPSG 1027 / 9834: LAEA and Lambert cylindrical equal area on the authalic sphere.
        self.spherical = matches!(self.method, Method::Laea | Method::Cea)
            && (matches!(
                projection
                    .method_authority
                    .as_ref()
                    .and_then(crate::Authority::epsg),
                Some(1027 | 9834)
            ) || crate::names::name_key(&projection.method_name).ends_with("spherical"));
        // Methods without a scale factor: an ESRI `Scale_Factor` of 1 is noise, anything else is
        // not something the method can express.
        if matches!(
            self.method,
            Method::Cass
                | Method::Nzmg
                | Method::Moll
                | Method::Mill
                | Method::EqEarth
                | Method::Robin
                | Method::Sinu
                | Method::Geos
                | Method::WebMerc
        ) {
            p.k_0 = 1.0;
        }
        let finite = [
            p.lat_0, p.lon_0, p.k_0, p.x_0, p.y_0, p.lat_1, p.lat_2, p.lat_ts, p.alpha, p.gamma,
            p.h,
        ]
        .iter()
        .all(|v| v.is_finite());
        if !finite || p.k_0 <= 0.0 {
            return Err(TransformError::invalid(
                "non-finite or non-positive parameter",
            ));
        }
        for lat in [p.lat_0, p.lat_1, p.lat_2, p.lat_ts] {
            if lat.abs() > 90.0 + ANGLE_TOL {
                return Err(TransformError::invalid(format!(
                    "latitude {lat} out of range"
                )));
            }
        }
        self.params = p;
        Ok(())
    }

    /// First eccentricity squared.
    pub(crate) fn es(&self) -> f64 {
        if self.rf == 0.0 {
            0.0
        } else {
            let f = 1.0 / self.rf;
            f * (2.0 - f)
        }
    }

    /// Whether two specs describe the same coordinate conversion (method, parameters, ellipsoid,
    /// prime meridian, unit, axis directions), within about a millimetre. Longitudes and azimuths
    /// are compared modulo 360°. Not compared: the datum shift (datums are matched by name) and
    /// the EPSG "(Spherical)" flag, which ESRI WKT cannot express.
    pub(crate) fn equivalent(&self, other: &ProjSpec) -> bool {
        let (p, q) = (&self.params, &other.params);
        self.method == other.method
            && self.axes == other.axes
            && rel_close(self.a, other.a)
            && rel_close(self.rf, other.rf)
            && angle_close(self.pm, other.pm)
            && rel_close(self.unit, other.unit)
            && angle_close(p.lat_0, q.lat_0)
            && angle_close(p.lon_0, q.lon_0)
            && (p.k_0 - q.k_0).abs() <= SCALE_TOL
            && length_close(p.x_0, q.x_0)
            && length_close(p.y_0, q.y_0)
            && angle_close(p.lat_1, q.lat_1)
            && angle_close(p.lat_2, q.lat_2)
            && angle_close(p.lat_ts, q.lat_ts)
            && angle_close(p.alpha, q.alpha)
            && angle_close(p.gamma, q.gamma)
            && length_close(p.h, q.h)
            && p.no_uoff == q.no_uoff
    }
}

/// Letters (`e`/`w`/`n`/`s`) of the first two declared axes, `None` unless both are plain
/// compass directions (polar "North along 90°E" axes, geocentric, missing axes).
fn declared_letters(axes: &[crate::Axis]) -> Option<[u8; 2]> {
    use crate::AxisDirection as D;
    let letter = |a: &crate::Axis| match a.direction {
        D::East => Some(b'e'),
        D::West => Some(b'w'),
        D::North => Some(b'n'),
        D::South => Some(b's'),
        _ => None,
    };
    match axes {
        [a, b, ..] => {
            let (a, b) = (letter(a)?, letter(b)?);
            // One east/west and one north/south axis.
            let ew = |c: u8| c == b'e' || c == b'w';
            (ew(a) != ew(b)).then_some([a, b])
        }
        _ => None,
    }
}

fn rel_close(a: f64, b: f64) -> bool {
    (a - b).abs() <= RELATIVE_TOL * a.abs().max(b.abs())
}

fn length_close(a: f64, b: f64) -> bool {
    (a - b).abs() <= LENGTH_TOL.max(RELATIVE_TOL * a.abs().max(b.abs()))
}

/// Angles equal modulo 360°.
pub(crate) fn angle_close(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d) <= ANGLE_TOL
}

/// Longitude into [-180, 180].
pub(crate) fn normalize_lon(lon: f64) -> f64 {
    if (-180.0..=180.0).contains(&lon) {
        lon
    } else {
        let l = (lon + 180.0).rem_euclid(360.0) - 180.0;
        if l == -180.0 && lon > 0.0 { 180.0 } else { l }
    }
}

/// Azimuth-like angle into (-180, 180].
fn normalize_angle(a: f64) -> f64 {
    let l = normalize_lon(a);
    if l == -180.0 { 180.0 } else { l }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn angles_compare_modulo_360() {
        assert!(angle_close(323.025_796_466_666_7, -36.974_203_533_333_4));
        assert!(angle_close(180.0, -180.0));
        assert!(!angle_close(10.0, 10.001));
        assert_eq!(normalize_lon(190.0), -170.0);
        assert_eq!(normalize_lon(540.0), 180.0);
        assert_eq!(normalize_lon(-180.0), -180.0);
        assert_eq!(normalize_angle(323.0), -37.0);
    }
}
