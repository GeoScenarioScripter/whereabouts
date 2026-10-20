//! Tolerance-aware comparison of one native record against its oracle record.

use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;

use crate::manifest::Case;
use crate::record::{
    Bbox, LayerRecord, NativeLayer, NativeRaster, NativeRecord, NativeStatus, NoData, OracleRecord,
    OracleStatus, RasterRecord,
};

/// One comparable fact. Adapters declare which checks they are ready to be held to; the baseline
/// sets a minimum agreement rate per check (and optionally per format).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Case-level: native status is compatible with the oracle verdict (corrupt cases must be
    /// `Error`/`Partial`).
    Status,
    /// Case-level: same set of layer names.
    Layers,
    /// Per vector layer: flattened geometry type + Z/M flags.
    GeometryType,
    /// Per vector/attributes layer: exact feature count.
    FeatureCount,
    /// Per spatial layer: native-CRS extent within the native tolerance.
    NativeExtent,
    /// Per raster layer: size and band count (+ data types, nodata, resolution when provided).
    Raster,
    /// Per spatial layer: identified EPSG code equal (both `None` agrees).
    Epsg,
    /// Per spatial layer: WGS84 extent within the degree tolerance (antimeridian-aware).
    Wgs84Extent,
}

impl Check {
    /// Every check, in report column order.
    pub const ALL: [Check; 8] = [
        Check::Status,
        Check::Layers,
        Check::GeometryType,
        Check::FeatureCount,
        Check::NativeExtent,
        Check::Raster,
        Check::Epsg,
        Check::Wgs84Extent,
    ];

    /// The snake_case name used in `baseline.toml` and the report.
    pub fn name(self) -> &'static str {
        match self {
            Check::Status => "status",
            Check::Layers => "layers",
            Check::GeometryType => "geometry_type",
            Check::FeatureCount => "feature_count",
            Check::NativeExtent => "native_extent",
            Check::Raster => "raster",
            Check::Epsg => "epsg",
            Check::Wgs84Extent => "wgs84_extent",
        }
    }

    /// Whether the check applies to an oracle layer of this kind.
    fn applies_to(self, layer: &LayerRecord) -> bool {
        let raster = layer.is_raster();
        let spatial = !layer.is_attributes();
        match self {
            Check::Status | Check::Layers => false,
            Check::GeometryType => !raster && spatial,
            Check::FeatureCount => !raster,
            Check::NativeExtent | Check::Epsg | Check::Wgs84Extent => spatial,
            Check::Raster => raster,
        }
    }
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Comparison tolerances (`[tolerance]` in `baseline.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tolerances {
    /// Absolute tolerance on native extents, in CRS units.
    pub native_abs: f64,
    /// Relative tolerance on native extents (× the larger magnitude); the looser of the two wins.
    pub native_rel: f64,
    /// Tolerance on WGS84 extents, in degrees (longitudes compared modulo 360).
    pub wgs84_deg: f64,
    /// Relative tolerance on raster resolution.
    pub resolution_rel: f64,
}

impl Default for Tolerances {
    /// The values proposed in session 1.2 (see `baseline.toml`).
    fn default() -> Self {
        Self {
            native_abs: 1e-6,
            native_rel: 1e-9,
            wgs84_deg: 0.01,
            resolution_rel: 1e-9,
        }
    }
}

/// Outcome of one check.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Native and oracle agree within tolerance.
    Agree,
    /// They don't; the string says how.
    Disagree(String),
}

impl Verdict {
    /// `true` for [`Verdict::Agree`].
    pub fn agrees(&self) -> bool {
        matches!(self, Verdict::Agree)
    }
}

/// One evaluated check.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    /// Which check.
    pub check: Check,
    /// Layer name for per-layer checks.
    pub layer: Option<String>,
    /// Outcome.
    pub verdict: Verdict,
}

/// Compare `native` against `oracle` for the `checks` the adapter declared.
///
/// Corrupt cases (tag `corrupt`) and cases GDAL refused are compared on [`Check::Status`] only:
/// GDAL is often more forgiving than we want a bounded header reader to be, so the oracle's
/// numbers for a broken file are not a target.
pub fn compare(
    case: &Case,
    oracle: &OracleRecord,
    native: &NativeRecord,
    checks: &[Check],
    tol: &Tolerances,
) -> Vec<CheckResult> {
    let wanted: BTreeSet<Check> = checks.iter().copied().collect();
    let mut out = Vec::new();
    let mut push = |check, layer: Option<&str>, verdict| {
        out.push(CheckResult {
            check,
            layer: layer.map(str::to_owned),
            verdict,
        });
    };

    if wanted.contains(&Check::Status) {
        push(
            Check::Status,
            None,
            status_verdict(case.is_corrupt(), oracle.status, native.status),
        );
    }
    if case.is_corrupt() || oracle.status == OracleStatus::Error {
        return out;
    }

    if wanted.contains(&Check::Layers) {
        let expected: BTreeSet<&str> = oracle.layers.iter().map(|l| l.name.as_str()).collect();
        let actual: BTreeSet<&str> = native.layers.iter().map(|l| l.name.as_str()).collect();
        let verdict = if expected == actual {
            Verdict::Agree
        } else {
            Verdict::Disagree(format!("expected layers {expected:?}, got {actual:?}"))
        };
        push(Check::Layers, None, verdict);
    }

    for layer in &oracle.layers {
        let found = native.layers.iter().find(|n| n.name == layer.name);
        for &check in &wanted {
            if !check.applies_to(layer) {
                continue;
            }
            let verdict = match found {
                None => Verdict::Disagree("layer missing from native output".into()),
                Some(n) => layer_verdict(check, layer, n, tol),
            };
            push(check, Some(&layer.name), verdict);
        }
    }
    out
}

fn status_verdict(corrupt: bool, oracle: OracleStatus, native: NativeStatus) -> Verdict {
    use NativeStatus as N;
    let ok = match (corrupt, oracle, native) {
        (_, _, N::Unsupported) => false,
        (true, _, s) => matches!(s, N::Error | N::Partial),
        (false, OracleStatus::Error, s) => matches!(s, N::Error | N::Partial),
        (false, OracleStatus::Ok, s) => matches!(s, N::Ok | N::Partial),
    };
    if ok {
        Verdict::Agree
    } else {
        let expected = if corrupt || oracle == OracleStatus::Error {
            "Error or Partial"
        } else {
            "Ok or Partial"
        };
        Verdict::Disagree(format!("expected {expected}, got {native:?}"))
    }
}

fn layer_verdict(check: Check, o: &LayerRecord, n: &NativeLayer, tol: &Tolerances) -> Verdict {
    match check {
        Check::Status | Check::Layers => Verdict::Agree,
        Check::GeometryType => {
            let expected = (o.geometry_type.as_deref(), o.has_z, o.has_m);
            let actual = (n.geometry_type.as_deref(), n.has_z, n.has_m);
            agree_if(expected == actual, || {
                format!("expected {expected:?}, got {actual:?}")
            })
        }
        Check::FeatureCount => agree_if(o.feature_count == n.feature_count, || {
            format!("expected {:?}, got {:?}", o.feature_count, n.feature_count)
        }),
        Check::NativeExtent => optional(o.extent, n.extent, |e, a| {
            bbox_close(&e, &a, |x, y| native_close(x, y, tol))
        }),
        Check::Raster => match (&o.raster, &n.raster) {
            (None, None) => Verdict::Agree,
            (Some(e), Some(a)) => raster_verdict(e, a, tol),
            (e, a) => Verdict::Disagree(format!(
                "raster facts present: oracle {}, native {}",
                e.is_some(),
                a.is_some()
            )),
        },
        Check::Epsg => agree_if(o.crs.epsg == n.epsg, || {
            format!("expected EPSG {:?}, got {:?}", o.crs.epsg, n.epsg)
        }),
        Check::Wgs84Extent => optional(o.wgs84_extent, n.wgs84_extent, |e, a| {
            wgs84_close(&e, &a, tol.wgs84_deg)
        }),
    }
}

fn agree_if(ok: bool, why: impl FnOnce() -> String) -> Verdict {
    if ok {
        Verdict::Agree
    } else {
        Verdict::Disagree(why())
    }
}

fn optional(
    expected: Option<Bbox>,
    actual: Option<Bbox>,
    close: impl FnOnce(Bbox, Bbox) -> bool,
) -> Verdict {
    match (expected, actual) {
        (None, None) => Verdict::Agree,
        (Some(e), Some(a)) => agree_if(close(e, a), || format!("expected {e:?}, got {a:?}")),
        (e, a) => Verdict::Disagree(format!("expected {e:?}, got {a:?}")),
    }
}

fn raster_verdict(e: &RasterRecord, a: &NativeRaster, tol: &Tolerances) -> Verdict {
    let mut problems = Vec::new();
    if (e.width, e.height, e.band_count) != (a.width, a.height, a.band_count) {
        problems.push(format!(
            "size/bands expected {}x{}x{}, got {}x{}x{}",
            e.width, e.height, e.band_count, a.width, a.height, a.band_count
        ));
    }
    if let Some(types) = &a.data_types
        && types != &e.data_types
    {
        problems.push(format!(
            "data types expected {:?}, got {types:?}",
            e.data_types
        ));
    }
    if let Some(nodata) = &a.nodata
        && !(nodata.len() == e.nodata.len()
            && nodata.iter().zip(&e.nodata).all(|(x, y)| nodata_eq(*x, *y)))
    {
        problems.push(format!("nodata expected {:?}, got {nodata:?}", e.nodata));
    }
    if let Some(res) = a.resolution {
        let ok = e.resolution.is_some_and(|r| {
            r.iter()
                .zip(res)
                .all(|(x, y)| (x - y).abs() <= tol.resolution_rel * x.abs().max(y.abs()))
        });
        if !ok {
            problems.push(format!(
                "resolution expected {:?}, got {res:?}",
                e.resolution
            ));
        }
    }
    agree_if(problems.is_empty(), || problems.join("; "))
}

fn nodata_eq(a: Option<NoData>, b: Option<NoData>) -> bool {
    // An adapter may build `NoData::Value(f64::NAN)` directly; fold every value through
    // `from_f64` so NaN/±inf always land in their own variants before comparing.
    let norm = |n: NoData| match n {
        NoData::Value(v) => NoData::from_f64(v),
        other => other,
    };
    match (a.map(norm), b.map(norm)) {
        (None, None) => true,
        (Some(NoData::NaN), Some(NoData::NaN)) => true,
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn native_close(a: f64, b: f64, tol: &Tolerances) -> bool {
    let diff = (a - b).abs();
    diff <= tol.native_abs || diff <= tol.native_rel * a.abs().max(b.abs())
}

fn bbox_close(e: &Bbox, a: &Bbox, close: impl Fn(f64, f64) -> bool) -> bool {
    e.iter().zip(a).all(|(x, y)| close(*x, *y))
}

/// Longitude span of `[west, _, east, _]` in degrees, 0–360: crossing boxes (`west > east`) wrap.
/// `[-180, 180]` is the full globe (360), `[180, 180]` and `[180, -180]` are empty (0).
fn lon_span(b: &Bbox) -> f64 {
    let (w, e) = (b[0], b[2]);
    if e >= w { e - w } else { e - w + 360.0 }
}

/// Longitude edges compared on the circle (so an east edge written as 181° matches -179°), plus
/// the longitude span, so that a box collapsed or inverted at ±180° can never match the full
/// globe (its edges are "equal" modulo 360, its span is not). Latitudes compared directly.
fn wgs84_close(e: &Bbox, a: &Bbox, deg: f64) -> bool {
    let lon = |x: f64, y: f64| {
        let d = (x - y).rem_euclid(360.0);
        d.min(360.0 - d) <= deg
    };
    let lat = |x: f64, y: f64| (x - y).abs() <= deg;
    // Each edge may be off by `deg`, so the span by twice that.
    let span_ok = (lon_span(e) - lon_span(a)).abs() <= 2.0 * deg;
    span_ok && lon(e[0], a[0]) && lat(e[1], a[1]) && lon(e[2], a[2]) && lat(e[3], a[3])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Location, Origin};
    use crate::record::{CrsRecord, CrsStatus, OracleVersion};

    fn case(tags: &[&str]) -> Case {
        Case {
            id: "c".into(),
            format: "shapefile".into(),
            origin: Origin::Synthetic,
            location: Location::Committed,
            anchor: "a.shp".into(),
            files: vec!["a.shp".into()],
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            description: "d".into(),
            provenance: "p".into(),
            licence: "MIT".into(),
            sha256: Default::default(),
        }
    }

    fn crs(epsg: Option<u32>) -> CrsRecord {
        CrsRecord {
            status: if epsg.is_some() {
                CrsStatus::Identified
            } else {
                CrsStatus::Missing
            },
            source: "prj".into(),
            raw_kind: None,
            raw: None,
            declared_authority: None,
            wkt2: None,
            epsg,
            epsg_method: None,
            epsg_confidence: None,
            candidates: vec![],
        }
    }

    fn vector_layer(name: &str) -> LayerRecord {
        LayerRecord {
            name: name.into(),
            kind: "vector".into(),
            geometry_type: Some("Point".into()),
            has_z: false,
            has_m: false,
            feature_count: Some(25),
            extent: Some([400000.0, 5000000.0, 600000.0, 5300000.0]),
            declared_extent: None,
            raster: None,
            fields: Some(vec![]),
            encoding: None,
            crs: crs(Some(32633)),
            wgs84_extent: Some([179.5, -18.0, -179.0, -16.0]),
            crosses_antimeridian: true,
            wgs84_error: None,
        }
    }

    fn oracle(status: OracleStatus, layers: Vec<LayerRecord>) -> OracleRecord {
        OracleRecord {
            schema: 1,
            case: "c".into(),
            format: "shapefile".into(),
            oracle: OracleVersion {
                gdal: "x".into(),
                proj: "y".into(),
            },
            status,
            error: None,
            messages: vec![],
            layers,
        }
    }

    fn native_from(o: &LayerRecord) -> NativeLayer {
        NativeLayer {
            name: o.name.clone(),
            geometry_type: o.geometry_type.clone(),
            has_z: o.has_z,
            has_m: o.has_m,
            feature_count: o.feature_count,
            extent: o.extent,
            raster: None,
            epsg: o.crs.epsg,
            wgs84_extent: o.wgs84_extent,
        }
    }

    fn verdicts(results: &[CheckResult]) -> Vec<(Check, bool)> {
        results
            .iter()
            .map(|r| (r.check, r.verdict.agrees()))
            .collect()
    }

    #[test]
    fn identical_records_agree_on_every_applicable_check() {
        let o = oracle(OracleStatus::Ok, vec![vector_layer("pts")]);
        let mut n = NativeRecord::new(NativeStatus::Ok);
        n.layers.push(native_from(&o.layers[0]));
        let r = compare(&case(&[]), &o, &n, &Check::ALL, &Tolerances::default());
        // Raster does not apply to a vector layer.
        assert_eq!(r.len(), 7, "{r:?}");
        assert!(r.iter().all(|c| c.verdict.agrees()), "{r:?}");
    }

    #[test]
    fn extent_tolerances() {
        let tol = Tolerances::default();
        let o = oracle(OracleStatus::Ok, vec![vector_layer("pts")]);
        let mut n = NativeRecord::new(NativeStatus::Ok);
        let mut l = native_from(&o.layers[0]);
        // Relative 1e-9 of 5.3e6 ≈ 5.3e-3: within.
        l.extent = Some([400000.0, 5000000.0, 600000.0, 5300000.004]);
        // 0.005° off on the antimeridian-crossing east edge, written as +181°: within 0.01°.
        l.wgs84_extent = Some([179.5, -18.0, 181.005, -16.0]);
        n.layers.push(l.clone());
        let checks = [Check::NativeExtent, Check::Wgs84Extent];
        let r = compare(&case(&[]), &o, &n, &checks, &tol);
        assert_eq!(
            verdicts(&r),
            vec![(Check::NativeExtent, true), (Check::Wgs84Extent, true)]
        );

        l.extent = Some([400000.0, 5000000.0, 600000.0, 5300001.0]);
        l.wgs84_extent = Some([179.5, -18.0, -178.9, -16.0]);
        n.layers[0] = l;
        let r = compare(&case(&[]), &o, &n, &checks, &tol);
        assert_eq!(
            verdicts(&r),
            vec![(Check::NativeExtent, false), (Check::Wgs84Extent, false)]
        );
    }

    #[test]
    fn missing_values_and_layers_disagree() {
        let o = oracle(OracleStatus::Ok, vec![vector_layer("pts")]);
        let mut n = NativeRecord::new(NativeStatus::Ok);
        let mut l = native_from(&o.layers[0]);
        l.extent = None;
        l.epsg = None;
        n.layers.push(l);
        let r = compare(
            &case(&[]),
            &o,
            &n,
            &[Check::NativeExtent, Check::Epsg],
            &Tolerances::default(),
        );
        assert_eq!(
            verdicts(&r),
            vec![(Check::NativeExtent, false), (Check::Epsg, false)]
        );

        n.layers[0].name = "other".into();
        let r = compare(
            &case(&[]),
            &o,
            &n,
            &[Check::Layers, Check::FeatureCount],
            &Tolerances::default(),
        );
        assert_eq!(
            verdicts(&r),
            vec![(Check::Layers, false), (Check::FeatureCount, false)]
        );
    }

    #[test]
    fn status_rules() {
        use NativeStatus::*;
        let cases = [
            // corrupt, oracle, native, agrees
            (true, OracleStatus::Ok, Error, true),
            (true, OracleStatus::Ok, Partial, true),
            (true, OracleStatus::Ok, Ok, false),
            (false, OracleStatus::Error, Error, true),
            (false, OracleStatus::Error, Ok, false),
            (false, OracleStatus::Ok, Ok, true),
            (false, OracleStatus::Ok, Partial, true),
            (false, OracleStatus::Ok, Error, false),
            (false, OracleStatus::Ok, Unsupported, false),
        ];
        for (corrupt, o, n, expected) in cases {
            assert_eq!(
                status_verdict(corrupt, o, n).agrees(),
                expected,
                "{corrupt} {o:?} {n:?}"
            );
        }
    }

    #[test]
    fn corrupt_cases_compare_status_only() {
        let o = oracle(OracleStatus::Ok, vec![vector_layer("pts")]);
        let n = NativeRecord::new(NativeStatus::Error);
        let r = compare(
            &case(&["corrupt"]),
            &o,
            &n,
            &Check::ALL,
            &Tolerances::default(),
        );
        assert_eq!(verdicts(&r), vec![(Check::Status, true)]);
    }

    #[test]
    fn raster_checks() {
        let mut layer = vector_layer("dem");
        layer.kind = "raster".into();
        layer.geometry_type = None;
        layer.feature_count = None;
        layer.raster = Some(RasterRecord {
            width: 64,
            height: 48,
            band_count: 1,
            data_types: vec!["Float32".into()],
            nodata: vec![Some(NoData::NaN)],
            geotransform: None,
            resolution: Some([30.0, 30.0]),
            georef_source: None,
            pixel_is_point: false,
            overview_count: 0,
            bigtiff: false,
            layout: None,
            compression: None,
        });
        let o = oracle(OracleStatus::Ok, vec![layer]);
        let mut n = NativeRecord::new(NativeStatus::Ok);
        let mut l = native_from(&o.layers[0]);
        l.raster = Some(NativeRaster {
            width: 64,
            height: 48,
            band_count: 1,
            data_types: Some(vec!["Float32".into()]),
            nodata: Some(vec![Some(NoData::NaN)]),
            resolution: Some([30.0, 30.0]),
        });
        n.layers.push(l);
        let r = compare(&case(&[]), &o, &n, &Check::ALL, &Tolerances::default());
        // Geometry type and feature count don't apply to rasters.
        assert!(
            r.iter()
                .all(|c| c.check != Check::GeometryType && c.check != Check::FeatureCount)
        );
        assert!(r.iter().all(|c| c.verdict.agrees()), "{r:?}");

        if let Some(raster) = n.layers[0].raster.as_mut() {
            raster.height = 47;
        }
        let r = compare(&case(&[]), &o, &n, &[Check::Raster], &Tolerances::default());
        assert_eq!(verdicts(&r), vec![(Check::Raster, false)]);
    }

    #[test]
    fn full_globe_does_not_match_collapsed_or_inverted_boxes() {
        let deg = Tolerances::default().wgs84_deg;
        let globe = [-180.0, -90.0, 180.0, 83.6];
        assert!(wgs84_close(&globe, &globe, deg));
        assert!(wgs84_close(&globe, &[-179.995, -90.0, 179.995, 83.6], deg));
        // Zero-width box at ±180: edges equal modulo 360, span 0 vs 360.
        assert!(!wgs84_close(&globe, &[180.0, -90.0, 180.0, 83.6], deg));
        assert!(!wgs84_close(&globe, &[-180.0, -90.0, -180.0, 83.6], deg));
        // Inverted (antimeridian-crossing, empty) box.
        assert!(!wgs84_close(&globe, &[180.0, -90.0, -180.0, 83.6], deg));
        assert!(!wgs84_close(&[180.0, -90.0, -180.0, 83.6], &globe, deg));
        // A genuine crossing box may write its east edge either way.
        let fiji = [176.9, -18.6, -178.3, -16.4];
        assert!(wgs84_close(&fiji, &[176.9, -18.6, 181.7, -16.4], deg));
        assert!(!wgs84_close(&fiji, &[-178.3, -18.6, 176.9, -16.4], deg));
    }

    #[test]
    fn nodata_nan_matches_whatever_its_representation() {
        let nan_value = Some(NoData::Value(f64::NAN));
        assert!(nodata_eq(nan_value, Some(NoData::NaN)));
        assert!(nodata_eq(Some(NoData::NaN), nan_value));
        assert!(nodata_eq(nan_value, nan_value));
        assert!(nodata_eq(
            Some(NoData::Value(f64::NEG_INFINITY)),
            Some(NoData::NegInf)
        ));
        assert!(!nodata_eq(nan_value, Some(NoData::Value(0.0))));
        assert!(!nodata_eq(nan_value, None));
        assert!(nodata_eq(
            Some(NoData::Value(-9999.0)),
            Some(NoData::Value(-9999.0))
        ));
    }
}
