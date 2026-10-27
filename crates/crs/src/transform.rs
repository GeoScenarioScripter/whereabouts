//! Native bounds → WGS84 extent (D-09): densified edges, axis-order aware, antimeridian split,
//! poles, out-of-domain points → partial.
//!
//! Pipeline per point: file axis order → projection axes (signs, false origin, unit) → inverse
//! projection (`proj4rs`, or a native inverse from [`crate::native`]) → longitude on the source
//! datum relative to the CRS's central meridian → Helmert shift to WGS 84 when the datum has
//! one (`TOWGS84`/`BOUNDCRS`, or the EPSG table's preferred transformation). Datum grids are
//! never used: metres of error are acceptable for a search index.
//!
//! Longitudes are tracked *relative to the central meridian* while the extent is built. A
//! projection's domain is contiguous in that frame, so a box east of 180° (Fiji, TM at 178.75°E)
//! simply comes out with an east edge above 180° and is split there; no wrap-around guessing.

use std::f64::consts::PI;

use whereabouts_core::{Bounds, Wgs84Extent};

use crate::model::{Crs, CrsKind};
use crate::native::Native;
use crate::spec::{Method, ProjSpec, normalize_lon};

/// Points per bbox edge between the corners, as GDAL's `OCTTransformBounds` recommends and the
/// oracle goldens use.
pub const DEFAULT_DENSIFY: u32 = 21;

/// Lower bound on [`TransformOptions::densify`]. The longitude walk takes each ring step as the
/// shorter way round, so steps must stay well under 180°: with 8 points per edge a box as wide
/// as the world still has steps of 40°. Smaller requests are raised to this.
pub const MIN_DENSIFY: u32 = 8;

/// Upper bound on [`TransformOptions::densify`] (keeps a call's work bounded).
pub const MAX_DENSIFY: u32 = 1000;

/// How the two coordinates of the native bounds map to the CRS axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisPolicy {
    /// x is the east/west axis and y the north/south axis whatever the CRS declares
    /// (longitude/latitude, easting/northing). This is how Shapefile, GeoPackage, GeoTIFF and
    /// GeoParquet store coordinates and what GDAL assumes for file data
    /// (`OAMS_TRADITIONAL_GIS_ORDER`). The default.
    #[default]
    TraditionalGis,
    /// x is the CRS's first declared axis: swapped when the CRS declares north/east order
    /// (EPSG:4326 in WKT2, EPSG:3035). For sources that follow the authority's axis order.
    Authority,
}

/// Options for [`transform_bounds_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransformOptions {
    /// Points added along each edge between the corners (clamped to
    /// [`MIN_DENSIFY`]..=[`MAX_DENSIFY`]).
    pub densify: u32,
    /// Axis interpretation of the input bounds.
    pub axis_policy: AxisPolicy,
}

impl Default for TransformOptions {
    fn default() -> Self {
        Self {
            densify: DEFAULT_DENSIFY,
            axis_policy: AxisPolicy::TraditionalGis,
        }
    }
}

/// Why bounds could not be transformed to WGS84.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum TransformError {
    /// The CRS has no horizontal geographic/projected component (geocentric, vertical,
    /// engineering, unknown).
    #[error("{0:?} CRS has no geographic placement")]
    NotHorizontal(CrsKind),
    /// The projection method has no implementation here (see `Method` coverage in the crate
    /// docs).
    #[error("unsupported projection method `{0}`")]
    UnsupportedMethod(String),
    /// The parameters are inconsistent, out of range or rejected by the projection.
    #[error("invalid CRS parameters: {0}")]
    InvalidParameters(String),
    /// No sampled point of the bounds could be transformed (all outside the projection's
    /// domain).
    #[error("bounds are outside the projection's domain")]
    OutOfDomain,
}

impl TransformError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidParameters(message.into())
    }
}

/// The WGS84 extent of some native bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformedBounds {
    /// The extent, split at the antimeridian when it crosses it.
    pub extent: Wgs84Extent,
    /// `Some(reason)` when part of the boundary could not be transformed (outside the
    /// projection's domain; catalog status `partial`). The extent then covers the points that did
    /// transform plus the places where the boundary leaves the domain (found by bisection), so
    /// it reaches the domain edge along the box but not beyond. Latitudes that exceed ±90° by
    /// rounding noise (1e-7°) are clamped silently and do not make an extent partial.
    pub partial: Option<String>,
}

/// Transforms native bounds (in `crs`'s units, x = east/west axis) to a WGS84 extent with the
/// default options (21 points per edge, traditional GIS axis order).
pub fn transform_bounds(bounds: &Bounds, crs: &Crs) -> Result<TransformedBounds, TransformError> {
    transform_bounds_with(bounds, crs, &TransformOptions::default())
}

/// [`transform_bounds`] with explicit options.
pub fn transform_bounds_with(
    bounds: &Bounds,
    crs: &Crs,
    options: &TransformOptions,
) -> Result<TransformedBounds, TransformError> {
    Transformer::new(crs, options.axis_policy)?.transform_bounds(bounds, options.densify)
}

/// A prepared coordinate transformation from a CRS to WGS 84 longitude/latitude. Build it once
/// per CRS when transforming many bounds.
#[derive(Debug)]
pub struct Transformer {
    spec: ProjSpec,
    engine: Engine,
    /// Swap the input coordinates before anything else.
    swap: bool,
    /// Longitude (degrees, Greenwich) that relative longitudes are measured from.
    center: f64,
}

#[derive(Debug)]
enum Engine {
    LongLat,
    Proj4(Box<proj4rs::Proj>, Box<proj4rs::Proj>),
    Native(Native),
}

impl Transformer {
    /// Prepares the transformation for `crs` (its horizontal component for compound CRSs).
    pub fn new(crs: &Crs, axis_policy: AxisPolicy) -> Result<Self, TransformError> {
        let spec = ProjSpec::from_crs(crs)?;
        let swap = axis_policy == AxisPolicy::Authority && spec.gis_swap;
        let engine = match spec.method {
            Method::LongLat => Engine::LongLat,
            _ => match Native::new(&spec) {
                Some(native) => Engine::Native(native.map_err(TransformError::invalid)?),
                None => {
                    let (src, dst) = proj4_pair(&spec)?;
                    Engine::Proj4(Box::new(src), Box::new(dst))
                }
            },
        };
        let center = if spec.method == Method::LongLat {
            spec.pm
        } else {
            spec.params.lon_0 + spec.pm
        };
        Ok(Self {
            spec,
            engine,
            swap,
            center,
        })
    }

    /// Transforms one native point to WGS 84 `(longitude, latitude)` in degrees, longitude in
    /// [-180, 180]. `None` when the point is outside the projection's domain.
    pub fn to_wgs84(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (rel, lat) = self.relative(x, y)?;
        Some((normalize_lon(self.center + rel), lat))
    }

    /// `(longitude relative to the central meridian, latitude)` in degrees, after the datum shift.
    fn relative(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (x, y) = if self.swap { (y, x) } else { (x, y) };
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        let s = &self.spec;
        let (rel, lat) = match &self.engine {
            Engine::LongLat => {
                let lat = y * s.unit;
                if lat.abs() > 90.0 + 1e-7 {
                    return None;
                }
                (x * s.unit, lat.clamp(-90.0, 90.0))
            }
            Engine::Proj4(src, dst) => {
                let (e, n) = self.projected(x, y);
                let mut point = (e, n, 0.0);
                proj4rs::transform::transform(src, dst, &mut point).ok()?;
                (point.0.to_degrees(), point.1.to_degrees())
            }
            Engine::Native(native) => {
                let (e, n) = self.projected(x, y);
                let (lam, phi) = native.inverse(e / s.a, n / s.a)?;
                (lam.to_degrees(), phi.to_degrees())
            }
        };
        if !(rel.is_finite() && lat.is_finite()) || lat.abs() > 90.0 + 1e-7 {
            return None;
        }
        // A valid inverse stays within half a turn of the central meridian; more means the point
        // is beyond the edge of the world (e.g. x > π·a in Mercator).
        if s.method != Method::LongLat && rel.abs() > 180.0 + 1e-7 {
            return None;
        }
        let lat = lat.clamp(-90.0, 90.0);
        match s.to_wgs84 {
            Some(h) if h.iter().any(|v| *v != 0.0) => {
                let lon = self.center + rel;
                let (lon2, lat2) = helmert(lon, lat, s.a, s.es(), &h)?;
                let d = (lon2 - lon + 180.0).rem_euclid(360.0) - 180.0;
                Some((rel + d, lat2))
            }
            _ => Some((rel, lat)),
        }
    }

    /// File coordinates → projection-plane metres relative to the natural origin, east/north up.
    fn projected(&self, x: f64, y: f64) -> (f64, f64) {
        let s = &self.spec;
        let [first, second] = s.axes;
        let ns_first = matches!(first, b'n' | b's');
        // Each file coordinate minus the false origin of its own axis.
        let (fx, fy) = if ns_first {
            (s.params.y_0, s.params.x_0)
        } else {
            (s.params.x_0, s.params.y_0)
        };
        let (u, v) = (x * s.unit - fx, y * s.unit - fy);
        let signed = |letter: u8, value: f64| {
            if matches!(letter, b'w' | b's') {
                -value
            } else {
                value
            }
        };
        if ns_first {
            (signed(second, v), signed(first, u))
        } else {
            (signed(first, u), signed(second, v))
        }
    }

    /// Transforms bounds; `densify` points are added along each edge between the corners.
    pub fn transform_bounds(
        &self,
        bounds: &Bounds,
        densify: u32,
    ) -> Result<TransformedBounds, TransformError> {
        let n = densify.clamp(MIN_DENSIFY, MAX_DENSIFY) as usize + 1; // segments per edge
        let (x0, y0, x1, y1) = (
            bounds.min_x(),
            bounds.min_y(),
            bounds.max_x(),
            bounds.max_y(),
        );
        // The boundary as a closed counter-clockwise ring (corners included once).
        let mut ring = Vec::with_capacity(4 * n);
        let lerp = |a: f64, b: f64, i: usize| a + (b - a) * (i as f64 / n as f64);
        if bounds.width() == 0.0 && bounds.height() == 0.0 {
            ring.push((x0, y0));
        } else {
            for i in 0..n {
                ring.push((lerp(x0, x1, i), y0));
            }
            for i in 0..n {
                ring.push((x1, lerp(y0, y1, i)));
            }
            for i in 0..n {
                ring.push((lerp(x1, x0, i), y1));
            }
            for i in 0..n {
                ring.push((x0, lerp(y1, y0, i)));
            }
        }
        let points: Vec<Option<(f64, f64)>> =
            ring.iter().map(|&(x, y)| self.relative(x, y)).collect();
        let failed = points.iter().filter(|p| p.is_none()).count();
        // Ring-ordered transformed points. Where the ring crosses the edge of the projection's
        // domain (a valid sample next to a failed one) the crossing is found by bisection and
        // added, so a box that runs past the domain (Mercator beyond ±85°) is widened to the
        // domain edge rather than stopping at the last sample.
        let mut valid: Vec<(f64, f64)> = Vec::with_capacity(points.len());
        for i in 0..points.len() {
            let j = (i + 1) % points.len();
            match (points[i], points[j]) {
                (Some(p), None) if i != j => {
                    valid.push(p);
                    valid.extend(self.domain_edge(ring[i], ring[j]));
                }
                (None, Some(_)) if i != j => {
                    valid.extend(self.domain_edge(ring[j], ring[i]));
                }
                (Some(p), _) => valid.push(p),
                _ => {}
            }
        }
        if valid.is_empty() {
            return Err(TransformError::OutOfDomain);
        }
        let partial = (failed > 0).then(|| {
            format!(
                "{failed} of {} boundary points are outside the projection's domain",
                points.len()
            )
        });

        let mut south = valid.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let mut north = valid.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        // Longitude extent: walk the ring, unwrapping each step into [-180, 180]. The relative
        // frame keeps a Fiji box (TM at 178.75°E) contiguous; the walk also handles polar
        // azimuthal projections, whose domain is contiguous across the back meridian.
        let (mut west, mut east) = unwrapped_range(&valid);

        // A pole inside the box: the ring winds once around it. A pole on the boundary: a
        // sampled point sits on it. Either way every longitude is covered.
        // (Not for plain longitude/latitude: a geographic box up to 90° stays as wide as it is.)
        let projected = self.spec.method != Method::LongLat;
        let touches_pole = projected && valid.iter().any(|p| p.1.abs() >= 90.0 - 1e-9);
        let winding = winding_degrees(&valid);
        let mut full_longitude = touches_pole;
        if projected && winding.abs() > 180.0 {
            full_longitude = true;
            // Which pole: the one the box's interior gets closest to. (The boundary alone can
            // mislead: a hemisphere in polar LAEA has corners far into the other hemisphere.)
            let mut interior_lat: Option<f64> = None;
            for i in 1..=3 {
                for j in 1..=3 {
                    let x = x0 + (x1 - x0) * f64::from(i) / 4.0;
                    let y = y0 + (y1 - y0) * f64::from(j) / 4.0;
                    if let Some((_, lat)) = self.relative(x, y)
                        && interior_lat.is_none_or(|best| lat.abs() > best.abs())
                    {
                        interior_lat = Some(lat);
                    }
                }
            }
            // No interior sample transformed: the pole is unknown, so leave the latitudes alone
            // rather than guess north.
            match interior_lat {
                Some(lat) if lat >= 0.0 => north = 90.0,
                Some(_) => south = -90.0,
                None => {}
            }
        }
        if east - west >= 360.0 - 1e-9 {
            full_longitude = true;
        }
        let (w, e) = if full_longitude {
            (-180.0, 180.0)
        } else {
            west = normalize_lon(self.center + west);
            east = normalize_lon(self.center + east);
            // normalize_lon maps +180 to itself and keeps -180; an east edge exactly at the
            // antimeridian after a crossing is +180.
            if east == -180.0 && west > east {
                east = 180.0;
            }
            if west == 180.0 && east < west {
                west = -180.0;
            }
            (west, east)
        };
        let s = south.clamp(-90.0, 90.0);
        let n = north.clamp(-90.0, 90.0);
        let extent = Wgs84Extent::from_west_south_east_north(w, s, e, n)
            .map_err(|err| TransformError::invalid(err.to_string()))?;
        Ok(TransformedBounds { extent, partial })
    }

    /// The last point that transforms on the segment from `good` (transforms) to `bad` (does
    /// not), by bisection: the boundary of the projection's domain along the segment.
    fn domain_edge(&self, good: (f64, f64), bad: (f64, f64)) -> Option<(f64, f64)> {
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        let mut found = self.relative(good.0, good.1)?;
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            let (x, y) = (
                good.0 + (bad.0 - good.0) * mid,
                good.1 + (bad.1 - good.1) * mid,
            );
            match self.relative(x, y) {
                Some(p) => {
                    lo = mid;
                    found = p;
                }
                None => hi = mid,
            }
        }
        Some(found)
    }
}

/// Min and max of the ring's longitudes after unwrapping consecutive steps (each step taken
/// as the shorter way round). Starts from the first point's relative longitude.
fn unwrapped_range(points: &[(f64, f64)]) -> (f64, f64) {
    let Some(&(first, _)) = points.first() else {
        return (0.0, 0.0);
    };
    let (mut lo, mut hi, mut u) = (first, first, first);
    for &(rel, _) in &points[1..] {
        // rel shifted by whole turns to land within half a turn of the previous point (exact
        // when no shift is needed, so plain boxes keep their exact edges).
        let turns = ((u - rel) / 360.0).round();
        u = rel + 360.0 * turns;
        lo = lo.min(u);
        hi = hi.max(u);
    }
    (lo, hi)
}

/// Total signed longitude change walking the ring (closing it), with each step wrapped into
/// [-180, 180]: ±360 when the ring encircles a pole, ≈0 otherwise.
fn winding_degrees(points: &[(f64, f64)]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..points.len() {
        let a = points[i].0;
        let b = points[(i + 1) % points.len()].0;
        sum += (b - a + 180.0).rem_euclid(360.0) - 180.0;
    }
    sum
}

/// The `proj4rs` source projection (`lon_0 = x_0 = y_0 = 0`; those are applied by
/// [`Transformer`]) and a geographic target on the same ellipsoid, without datum, so `proj4rs`
/// does no datum shift.
fn proj4_pair(spec: &ProjSpec) -> Result<(proj4rs::Proj, proj4rs::Proj), TransformError> {
    let p = &spec.params;
    let projection = match spec.method {
        Method::Tmerc => format!("+proj=etmerc +lat_0={} +k_0={}", p.lat_0, p.k_0),
        Method::Lcc | Method::LccBelgium => format!(
            "+proj=lcc +lat_0={} +lat_1={} +lat_2={} +k_0={}",
            p.lat_0, p.lat_1, p.lat_2, p.k_0
        ),
        Method::Aea => format!(
            "+proj=aea +lat_0={} +lat_1={} +lat_2={}",
            p.lat_0, p.lat_1, p.lat_2
        ),
        Method::Merc => format!("+proj=merc +k_0={}", p.k_0),
        Method::WebMerc => "+proj=merc +k_0=1".to_owned(),
        Method::Stere if (p.lat_0.abs() - 90.0).abs() < 1e-9 => format!(
            "+proj=stere +lat_0={} +lat_ts={} +k_0={}",
            p.lat_0, p.lat_ts, p.k_0
        ),
        Method::Stere => format!("+proj=stere +lat_0={} +k_0={}", p.lat_0, p.k_0),
        Method::Sterea => format!("+proj=sterea +lat_0={} +k_0={}", p.lat_0, p.k_0),
        Method::Laea => format!("+proj=laea +lat_0={}", p.lat_0),
        Method::Cea => format!("+proj=cea +lat_ts={}", p.lat_ts),
        Method::Krovak => format!("+proj=krovak +lat_0={} +k_0={}", p.lat_0, p.k_0),
        Method::Somerc => format!("+proj=somerc +lat_0={} +k_0={}", p.lat_0, p.k_0),
        Method::Moll => "+proj=moll".to_owned(),
        Method::Mill => "+proj=mill".to_owned(),
        Method::Geos => format!("+proj=geos +h={}", p.h),
        other => {
            return Err(TransformError::UnsupportedMethod(format!("{other:?}")));
        }
    };
    let ellipsoid = if spec.rf == 0.0 || spec.method == Method::WebMerc {
        format!("+R={}", spec.a)
    } else {
        format!(
            "+a={} +rf={}{}",
            spec.a,
            spec.rf,
            if spec.spherical { " +R_A" } else { "" }
        )
    };
    let src = format!("{projection} +lon_0=0 +x_0=0 +y_0=0 {ellipsoid} +over +no_defs");
    let src = proj4rs::Proj::from_proj_string(&src)
        .map_err(|e| TransformError::invalid(format!("{}: {e}", spec_name(spec))))?;
    let dst = proj4rs::Proj::from_proj_string("+proj=longlat +ellps=WGS84 +over +no_defs")
        .map_err(|e| TransformError::invalid(e.to_string()))?;
    if !src.has_inverse() {
        return Err(TransformError::UnsupportedMethod(spec_name(spec)));
    }
    Ok((src, dst))
}

fn spec_name(spec: &ProjSpec) -> String {
    format!("{:?}", spec.method)
}

/// Position-vector Helmert transformation of a point on the source ellipsoid (height 0) to WGS 84.
/// `h` = dx, dy, dz (m), rx, ry, rz (arc-seconds), ds (ppm). Degrees in and out.
fn helmert(lon: f64, lat: f64, a: f64, es: f64, h: &[f64; 7]) -> Option<(f64, f64)> {
    const SEC: f64 = PI / (180.0 * 3600.0);
    let (lam, phi) = (lon.to_radians(), lat.to_radians());
    let n = a / (1.0 - es * phi.sin() * phi.sin()).sqrt();
    let x = n * phi.cos() * lam.cos();
    let y = n * phi.cos() * lam.sin();
    let z = n * (1.0 - es) * phi.sin();
    let (rx, ry, rz) = (h[3] * SEC, h[4] * SEC, h[5] * SEC);
    let m = 1.0 + h[6] * 1e-6;
    let x2 = h[0] + m * (x - rz * y + ry * z);
    let y2 = h[1] + m * (rz * x + y - rx * z);
    let z2 = h[2] + m * (-ry * x + rx * y + z);
    // ECEF → geodetic on WGS 84.
    let a84 = 6_378_137.0;
    let f84 = 1.0 / 298.257_223_563;
    let es84 = f84 * (2.0 - f84);
    let p = (x2 * x2 + y2 * y2).sqrt();
    let lam2 = y2.atan2(x2);
    let mut phi2 = z2.atan2(p * (1.0 - es84));
    for _ in 0..10 {
        let n2 = a84 / (1.0 - es84 * phi2.sin() * phi2.sin()).sqrt();
        let next = (z2 + es84 * n2 * phi2.sin()).atan2(p);
        let done = (next - phi2).abs() < 1e-14;
        phi2 = next;
        if done {
            break;
        }
    }
    let (lon2, lat2) = (lam2.to_degrees(), phi2.to_degrees());
    (lon2.is_finite() && lat2.is_finite()).then_some((lon2, lat2.clamp(-90.0, 90.0)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn b(x0: f64, y0: f64, x1: f64, y1: f64) -> Bounds {
        Bounds::new(x0, y0, x1, y1).unwrap()
    }

    fn parts(t: &TransformedBounds) -> Vec<[f64; 4]> {
        t.extent
            .parts()
            .iter()
            .map(|p| [p.min_x(), p.min_y(), p.max_x(), p.max_y()])
            .collect()
    }

    #[test]
    fn geographic_identity_and_wrap() {
        let crs = crate::epsg_crs(4326).unwrap();
        let t = transform_bounds(&b(10.0, 50.0, 12.0, 52.0), &crs).unwrap();
        assert_eq!(parts(&t), vec![[10.0, 50.0, 12.0, 52.0]]);
        assert!(t.partial.is_none());
        // 0..360 data covers the globe.
        let t = transform_bounds(&b(0.0, -90.0, 360.0, 90.0), &crs).unwrap();
        assert_eq!(parts(&t), vec![[-180.0, -90.0, 180.0, 90.0]]);
        // 170..190 crosses the antimeridian.
        let t = transform_bounds(&b(170.0, -10.0, 190.0, 10.0), &crs).unwrap();
        assert_eq!(t.extent.parts().len(), 2);
        // Authority order swaps for EPSG:4326 (lat/lon).
        let t = transform_bounds_with(
            &b(50.0, 10.0, 52.0, 12.0),
            &crs,
            &TransformOptions {
                axis_policy: AxisPolicy::Authority,
                ..TransformOptions::default()
            },
        )
        .unwrap();
        assert_eq!(parts(&t), vec![[10.0, 50.0, 12.0, 52.0]]);
    }

    #[test]
    fn helmert_zero_is_identity() {
        let (lon, lat) = helmert(10.0, 50.0, 6_378_137.0, 0.006_694_379_990_14, &[0.0; 7]).unwrap();
        assert!((lon - 10.0).abs() < 1e-9 && (lat - 50.0).abs() < 1e-9);
    }

    #[test]
    fn utm_point() {
        let crs = crate::epsg_crs(32633).unwrap();
        let t = Transformer::new(&crs, AxisPolicy::TraditionalGis).unwrap();
        let (lon, lat) = t.to_wgs84(500_000.0, 5_000_000.0).unwrap();
        assert!((lon - 15.0).abs() < 1e-9, "{lon}");
        assert!((lat - 45.153_477).abs() < 1e-5, "{lat}");
    }

    #[test]
    fn geographic_box_touching_a_pole_keeps_its_longitudes() {
        let crs = crate::epsg_crs(4326).unwrap();
        for (bx, expected) in [
            (b(10.0, 50.0, 12.0, 90.0), [10.0, 50.0, 12.0, 90.0]),
            (b(10.0, -90.0, 12.0, -50.0), [10.0, -90.0, 12.0, -50.0]),
            (
                b(10.0, 50.0, 12.0, 89.999_999_999_9),
                [10.0, 50.0, 12.0, 89.999_999_999_9],
            ),
        ] {
            let t = transform_bounds(&bx, &crs).unwrap();
            assert_eq!(parts(&t), vec![expected]);
            assert!(t.partial.is_none());
        }
    }

    #[test]
    fn low_densify_does_not_split_a_wide_geographic_box() {
        let crs = crate::epsg_crs(4326).unwrap();
        for densify in [0, 1, 3] {
            let t = transform_bounds_with(
                &b(-170.0, -10.0, 170.0, 10.0),
                &crs,
                &TransformOptions {
                    densify,
                    ..TransformOptions::default()
                },
            )
            .unwrap();
            assert_eq!(parts(&t), vec![[-170.0, -10.0, 170.0, 10.0]], "{densify}");
        }
    }

    #[test]
    fn polar_box_gains_the_pole() {
        // The no-interior-sample fallback (no pole adjustment) cannot be reached with a real CRS;
        // here, the normal path: a box around the north pole gains latitude 90.
        let crs = crate::epsg_crs(3413).unwrap(); // NSIDC Sea Ice Polar Stereographic North
        let t = transform_bounds(&b(-500_000.0, -500_000.0, 500_000.0, 500_000.0), &crs).unwrap();
        assert_eq!(
            parts(&t),
            vec![[-180.0, t.extent.parts()[0].min_y(), 180.0, 90.0]]
        );
    }

    #[test]
    fn box_past_the_domain_edge_is_widened_to_it_and_partial() {
        // Web Mercator, x from 0 to twice the half-circumference: samples beyond ±π·a are out of
        // domain. The coarse samples alone would stop near 170°E; bisection reaches the edge.
        let crs = crate::epsg_crs(3857).unwrap();
        let t = transform_bounds(&b(0.0, 0.0, 40_000_000.0, 1_000_000.0), &crs).unwrap();
        let p = &t.extent.parts()[0];
        assert!(t.partial.is_some());
        assert!(p.min_x() >= -1e-9 && p.max_x() > 179.999, "{p:?}");
    }

    #[test]
    fn swapped_standard_parallels_transform_identically() {
        let wkt = |sp1: &str, sp2: &str| {
            format!(
                r#"PROJCS["x",GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Lambert_Conformal_Conic"],PARAMETER["False_Easting",6561666.666666666],PARAMETER["False_Northing",1640416.666666667],PARAMETER["Central_Meridian",-120.5],PARAMETER["Standard_Parallel_1",{sp1}],PARAMETER["Standard_Parallel_2",{sp2}],PARAMETER["Latitude_Of_Origin",36.5],UNIT["Foot_US",0.3048006096012192]]"#
            )
        };
        let a = crate::parse(&wkt("37.06666666666667", "38.43333333333333")).unwrap();
        let c = crate::parse(&wkt("38.43333333333333", "37.06666666666667")).unwrap();
        let bx = b(6_000_000.0, 1_500_000.0, 6_900_000.0, 2_000_000.0);
        let (ta, tc) = (
            transform_bounds(&bx, &a).unwrap(),
            transform_bounds(&bx, &c).unwrap(),
        );
        assert_eq!(ta, tc);
        let p = &ta.extent.parts()[0];
        assert!(
            p.min_x() < -121.0 && p.max_x() > -119.5 && p.min_y() > 36.0 && p.max_y() < 39.0,
            "{p:?}"
        );
    }
}
