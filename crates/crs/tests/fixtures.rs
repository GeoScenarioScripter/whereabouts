//! Table-driven fixture tests: every definition in `tests/data` parses into the expected kind,
//! projection method, units and authority; the dialects of one CRS agree on parameters and
//! datum; and encoding noise (BOM, CRLF, NUL padding) doesn't change the result.
//!
//! Provenance of the fixtures: `tests/data/README.md`.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;

use common::{Fixture, fixtures};
use whereabouts_crs::{Crs, CrsKind, Dialect, ParamClass, ParamName, ProjectionMethod as M};

const DEG: f64 = 0.017_453_292_519_943_295;
const GRAD: f64 = 0.015_707_963_267_948_967;
const METRE: f64 = 1.0;
const FT_US: f64 = 0.304_800_609_601_219_2;

/// Expected parse result for every file of a fixture directory (`epsg-32633`) or for a single
/// file (`handwritten/wkt1-local-cs.txt`).
struct Case {
    fixture: &'static str,
    kind: CrsKind,
    /// Method of the horizontal CRS.
    method: Option<M>,
    /// Linear unit factor (of the horizontal CRS for compound CRSs).
    linear: Option<f64>,
    /// Angular unit factor of the horizontal CRS.
    angular: Option<f64>,
    /// Authority citation. For directory cases, `esri.prj` files are expected to have none (ESRI
    /// WKT carries no authority codes).
    authority: Option<(&'static str, &'static str)>,
}

const fn geog(fixture: &'static str, code: &'static str, angular: f64) -> Case {
    Case {
        fixture,
        kind: CrsKind::Geographic,
        method: None,
        linear: None,
        angular: Some(angular),
        authority: Some(("EPSG", code)),
    }
}

const fn proj(
    fixture: &'static str,
    authority: (&'static str, &'static str),
    method: M,
    linear: f64,
) -> Case {
    Case {
        fixture,
        kind: CrsKind::Projected,
        method: Some(method),
        linear: Some(linear),
        angular: Some(DEG),
        authority: Some(authority),
    }
}

const fn epsg(code: &'static str) -> (&'static str, &'static str) {
    ("EPSG", code)
}

#[rustfmt::skip]
const CASES: &[Case] = &[
    // Geographic
    geog("epsg-4326", "4326", DEG),
    geog("epsg-4269", "4269", DEG),
    geog("epsg-4267", "4267", DEG),
    geog("epsg-4258", "4258", DEG),
    geog("epsg-4283", "4283", DEG),
    geog("epsg-7844", "7844", DEG),
    geog("epsg-4167", "4167", DEG),
    geog("epsg-4807", "4807", GRAD),
    geog("epsg-4277", "4277", DEG),
    geog("epsg-4230", "4230", DEG),
    // Geocentric
    Case { fixture: "epsg-4978", kind: CrsKind::Geocentric, method: None, linear: Some(METRE), angular: None, authority: Some(("EPSG", "4978")) },
    // Transverse Mercator family
    proj("epsg-32633", epsg("32633"), M::TransverseMercator, METRE),
    proj("epsg-32755", epsg("32755"), M::TransverseMercator, METRE),
    proj("epsg-26917", epsg("26917"), M::TransverseMercator, METRE),
    proj("epsg-25832", epsg("25832"), M::TransverseMercator, METRE),
    proj("epsg-27700", epsg("27700"), M::TransverseMercator, METRE),
    proj("epsg-2193", epsg("2193"), M::TransverseMercator, METRE),
    proj("epsg-28355", epsg("28355"), M::TransverseMercator, METRE),
    proj("epsg-31467", epsg("31467"), M::TransverseMercator, METRE),
    proj("epsg-3460", epsg("3460"), M::TransverseMercator, METRE),
    proj("epsg-2046", epsg("2046"), M::TransverseMercatorSouthOrientated, METRE),
    // Lambert Conformal Conic
    proj("epsg-2154", epsg("2154"), M::LambertConicConformal2Sp, METRE),
    proj("epsg-3347", epsg("3347"), M::LambertConicConformal2Sp, METRE),
    proj("epsg-2227", epsg("2227"), M::LambertConicConformal2Sp, FT_US),
    proj("epsg-2263", epsg("2263"), M::LambertConicConformal2Sp, FT_US),
    proj("epsg-3034", epsg("3034"), M::LambertConicConformal2Sp, METRE),
    proj("epsg-31370", epsg("31370"), M::LambertConicConformal2Sp, METRE),
    proj("epsg-31300", epsg("31300"), M::LambertConicConformal2SpBelgium, METRE),
    Case { fixture: "epsg-27572", kind: CrsKind::Projected, method: Some(M::LambertConicConformal1Sp), linear: Some(METRE), angular: Some(GRAD), authority: Some(("EPSG", "27572")) },
    // Albers
    proj("epsg-5070", epsg("5070"), M::AlbersEqualArea, METRE),
    proj("epsg-3577", epsg("3577"), M::AlbersEqualArea, METRE),
    proj("epsg-3005", epsg("3005"), M::AlbersEqualArea, METRE),
    proj("esri-102003", ("ESRI", "102003"), M::AlbersEqualArea, METRE),
    // Mercator
    proj("epsg-3857", epsg("3857"), M::PopularVisualisationPseudoMercator, METRE),
    proj("epsg-3395", epsg("3395"), M::MercatorVariantA, METRE),
    proj("epsg-3832", epsg("3832"), M::MercatorVariantA, METRE),
    // Polar stereographic
    proj("epsg-3031", epsg("3031"), M::PolarStereographicVariantB, METRE),
    proj("epsg-3413", epsg("3413"), M::PolarStereographicVariantB, METRE),
    proj("epsg-3995", epsg("3995"), M::PolarStereographicVariantB, METRE),
    proj("epsg-32661", epsg("32661"), M::PolarStereographicVariantA, METRE),
    proj("epsg-5041", epsg("5041"), M::PolarStereographicVariantA, METRE),
    // Other projection families
    proj("epsg-28992", epsg("28992"), M::ObliqueStereographic, METRE),
    proj("epsg-3035", epsg("3035"), M::LambertAzimuthalEqualArea, METRE),
    proj("epsg-6931", epsg("6931"), M::LambertAzimuthalEqualArea, METRE),
    proj("epsg-4087", epsg("4087"), M::EquidistantCylindrical, METRE),
    proj("epsg-3068", epsg("3068"), M::CassiniSoldner, METRE),
    proj("epsg-2056", epsg("2056"), M::HotineObliqueMercatorVariantB, METRE),
    proj("epsg-3375", epsg("3375"), M::HotineObliqueMercatorVariantA, METRE),
    proj("epsg-5514", epsg("5514"), M::KrovakNorthOrientated, METRE),
    proj("epsg-27200", epsg("27200"), M::NewZealandMapGrid, METRE),
    proj("epsg-8857", epsg("8857"), M::EqualEarth, METRE),
    proj("esri-54009", ("ESRI", "54009"), M::Mollweide, METRE),
    proj("esri-54030", ("ESRI", "54030"), M::Robinson, METRE),
    proj("esri-54034", ("ESRI", "54034"), M::LambertCylindricalEqualArea, METRE),
    // Compound and vertical
    Case { fixture: "epsg-7405", kind: CrsKind::Compound, method: Some(M::TransverseMercator), linear: Some(METRE), angular: Some(DEG), authority: Some(("EPSG", "7405")) },
    Case { fixture: "epsg-5703", kind: CrsKind::Vertical, method: None, linear: Some(METRE), angular: None, authority: Some(("EPSG", "5703")) },
    Case { fixture: "epsg-3855", kind: CrsKind::Vertical, method: None, linear: Some(METRE), angular: None, authority: Some(("EPSG", "3855")) },
    // Hand-written quirks
    Case { fixture: "handwritten/esri-ellipsoidal-height-vertcs.prj", kind: CrsKind::Vertical, method: None, linear: Some(METRE), angular: None, authority: None },
    Case { fixture: "handwritten/esri-geogcs-vertcs.prj", kind: CrsKind::Compound, method: None, linear: None, angular: Some(DEG), authority: None },
    Case { fixture: "handwritten/esri-sphere-mercator.prj", kind: CrsKind::Projected, method: Some(M::MercatorVariantB), linear: Some(METRE), angular: Some(DEG), authority: None },
    Case { fixture: "handwritten/projjson-boundcrs.json", kind: CrsKind::Geographic, method: None, linear: None, angular: Some(DEG), authority: Some(("EPSG", "4277")) },
    Case { fixture: "handwritten/wkt1-local-cs.txt", kind: CrsKind::Engineering, method: None, linear: Some(METRE), angular: None, authority: None },
    Case { fixture: "handwritten/wkt1-minimal-geogcs.txt", kind: CrsKind::Geographic, method: None, linear: None, angular: Some(DEG), authority: None },
    Case { fixture: "handwritten/wkt1-parentheses-lowercase.txt", kind: CrsKind::Projected, method: Some(M::TransverseMercator), linear: Some(METRE), angular: Some(DEG), authority: Some(("EPSG", "26717")) },
    Case { fixture: "handwritten/wkt1-towgs84.txt", kind: CrsKind::Projected, method: Some(M::TransverseMercator), linear: Some(METRE), angular: Some(DEG), authority: Some(("EPSG", "27700")) },
    Case { fixture: "handwritten/wkt2_2019-boundcrs.txt", kind: CrsKind::Projected, method: Some(M::TransverseMercator), linear: Some(METRE), angular: Some(DEG), authority: Some(("EPSG", "27700")) },
    Case { fixture: "handwritten/wkt2_2019-engcrs.txt", kind: CrsKind::Engineering, method: None, linear: Some(METRE), angular: None, authority: None },
    Case { fixture: "handwritten/wkt2_2019-escaped-quotes.txt", kind: CrsKind::Geographic, method: None, linear: None, angular: Some(DEG), authority: None },
    Case { fixture: "handwritten/wkt2_2019-usage-timeextent.txt", kind: CrsKind::Projected, method: Some(M::TransverseMercator), linear: Some(METRE), angular: Some(DEG), authority: Some(("EPSG", "32633")) },
    Case { fixture: "handwritten/wkt2_2019-rotated-pole.txt", kind: CrsKind::Unknown, method: None, linear: None, angular: None, authority: None },
    Case { fixture: "handwritten/projjson-rotated-pole.json", kind: CrsKind::Unknown, method: None, linear: None, angular: None, authority: None },
];

/// Per-file method exceptions, where a dialect legitimately names a different (equivalent or
/// less specific) method than the EPSG definition.
const METHOD_OVERRIDES: &[(&str, M)] = &[
    // ESRI writes World Mercator / PDC Mercator as `Mercator` with `Standard_Parallel_1 = 0`,
    // i.e. variant B with a zero standard parallel (identical to variant A with k0 = 1).
    ("epsg-3395/esri.prj", M::MercatorVariantB),
    ("epsg-3832/esri.prj", M::MercatorVariantB),
    // OGC WKT1 has no north-orientated Krovak; GDAL encodes the orientation in the axes.
    ("epsg-5514/wkt1.txt", M::Krovak),
];

/// Expected dialect from the file name: `esri*`, `wkt1*`, `wkt2_2015*`, `wkt2_2019*`, `projjson*`.
fn expected_dialect(file: &str) -> Dialect {
    let token = file.split(['.', '-']).next().unwrap_or("");
    match token {
        "esri" => Dialect::Wkt1Esri,
        "wkt1" => Dialect::Wkt1Ogc,
        "wkt2_2015" => Dialect::Wkt2_2015,
        "wkt2_2019" => Dialect::Wkt2_2019,
        "projjson" => Dialect::ProjJson,
        other => panic!("fixture file `{file}` has no dialect prefix (`{other}`)"),
    }
}

fn case_for(f: &Fixture) -> Option<&'static Case> {
    CASES
        .iter()
        .find(|c| c.fixture == f.id || c.fixture == f.dir)
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0)
}

fn horizontal(crs: &Crs) -> &Crs {
    crs.horizontal().unwrap_or(crs)
}

/// Compares one parsed fixture with its case; returns the mismatches.
fn check(f: &Fixture, case: &Case, crs: &Crs) -> Vec<String> {
    let mut errors = Vec::new();
    let mut expect = |ok: bool, what: String| {
        if !ok {
            errors.push(format!("{}: {what}", f.id));
        }
    };
    let dialect = expected_dialect(&f.file);
    expect(
        crs.dialect == dialect,
        format!("dialect {:?}, expected {dialect:?}", crs.dialect),
    );
    expect(
        crs.kind == case.kind,
        format!("kind {:?}, expected {:?}", crs.kind, case.kind),
    );

    let h = horizontal(crs);
    let method = h.projection.as_ref().map(|p| p.method);
    let expected_method = METHOD_OVERRIDES
        .iter()
        .find(|(id, _)| *id == f.id)
        .map(|(_, m)| *m)
        .or(case.method);
    expect(
        method == expected_method,
        format!("method {method:?}, expected {expected_method:?}"),
    );

    let linear = h.linear_unit.as_ref().map(|u| u.factor);
    expect(
        match (linear, case.linear) {
            (Some(a), Some(b)) => approx(a, b),
            (a, b) => a.is_none() && b.is_none(),
        },
        format!("linear unit {linear:?}, expected {:?}", case.linear),
    );
    let angular = h.angular_unit.as_ref().map(|u| u.factor);
    expect(
        match (angular, case.angular) {
            (Some(a), Some(b)) => approx(a, b),
            (a, b) => a.is_none() && b.is_none(),
        },
        format!("angular unit {angular:?}, expected {:?}", case.angular),
    );

    let authority = crs
        .authority
        .as_ref()
        .map(|a| (a.name.as_str(), a.code.as_str()));
    let expected_authority = if dialect == Dialect::Wkt1Esri {
        None
    } else {
        case.authority
    };
    expect(
        authority == expected_authority,
        format!("authority {authority:?}, expected {expected_authority:?}"),
    );

    // Every geodetic CRS has a datum with an ellipsoid; every projection has parameters.
    if matches!(h.kind, CrsKind::Geographic | CrsKind::Projected) {
        let ellipsoid = h.datum.as_ref().and_then(|d| d.ellipsoid.as_ref());
        expect(ellipsoid.is_some(), "no datum ellipsoid".into());
    }
    if let Some(p) = &h.projection {
        expect(!p.parameters.is_empty(), "no projection parameters".into());
    }
    errors
}

#[test]
fn every_fixture_parses_to_the_expected_model() {
    let all = fixtures();
    let mut errors = Vec::new();
    for f in &all {
        let Some(case) = case_for(f) else {
            errors.push(format!("{}: no entry in CASES", f.id));
            continue;
        };
        match whereabouts_crs::parse_bytes(&f.bytes) {
            Ok(crs) => errors.extend(check(f, case, &crs)),
            Err(e) => errors.push(format!("{}: failed to parse: {e}", f.id)),
        }
    }
    assert!(
        errors.is_empty(),
        "{} problem(s):\n{}",
        errors.len(),
        errors.join("\n")
    );
    println!("{} fixtures parsed as expected", all.len());
}

#[test]
fn fixture_table_matches_the_data_directory() {
    let all = fixtures();
    // The session spec asks for at least 60 definitions.
    assert!(all.len() >= 60, "only {} fixtures", all.len());
    for case in CASES {
        assert!(
            all.iter()
                .any(|f| f.id == case.fixture || f.dir == case.fixture),
            "CASES entry `{}` has no fixture files",
            case.fixture
        );
    }
    let dialects: BTreeSet<String> = all
        .iter()
        .map(|f| format!("{:?}", expected_dialect(&f.file)))
        .collect();
    assert_eq!(dialects.len(), 5, "every dialect is covered: {dialects:?}");
}

/// Parameter values (angles in degrees, lengths in metres) shared by two dialects of the same
/// CRS must agree, as must ellipsoids and prime meridians. This exercises unit handling (grads,
/// US survey feet, explicit vs inherited units) across WKT1, ESRI, WKT2 and PROJJSON.
#[test]
fn dialects_of_one_crs_agree() {
    let all = fixtures();
    let mut errors = Vec::new();
    let dirs: BTreeSet<&str> = all
        .iter()
        .map(|f| f.dir.as_str())
        .filter(|d| *d != "handwritten")
        .collect();
    let mut compared = 0;
    for dir in dirs {
        let parsed: Vec<(&Fixture, Crs)> = all
            .iter()
            .filter(|f| f.dir == dir)
            .map(|f| (f, whereabouts_crs::parse_bytes(&f.bytes).unwrap()))
            .collect();
        for (i, (fa, a)) in parsed.iter().enumerate() {
            for (fb, b) in &parsed[i + 1..] {
                compared += 1;
                compare(fa, horizontal(a), fb, horizontal(b), &mut errors);
            }
        }
    }
    assert!(
        errors.is_empty(),
        "{} disagreement(s):\n{}",
        errors.len(),
        errors.join("\n")
    );
    assert!(compared > 100, "only {compared} pairs compared");
    println!("{compared} dialect pairs agree");
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * a.abs().max(b.abs()).max(1.0)
}

fn compare(fa: &Fixture, a: &Crs, fb: &Fixture, b: &Crs, errors: &mut Vec<String>) {
    let pair = format!("{} vs {}", fa.id, fb.id);
    if let (Some(da), Some(db)) = (&a.datum, &b.datum) {
        if let (Some(ea), Some(eb)) = (&da.ellipsoid, &db.ellipsoid)
            && (!close(ea.semi_major_axis, eb.semi_major_axis, 1e-12)
                || !close(ea.inverse_flattening, eb.inverse_flattening, 1e-9))
        {
            errors.push(format!("{pair}: ellipsoid {ea:?} vs {eb:?}"));
        }
        if (da.prime_meridian.longitude - db.prime_meridian.longitude).abs() > 1e-7 {
            errors.push(format!(
                "{pair}: prime meridian {} vs {}",
                da.prime_meridian.longitude, db.prime_meridian.longitude
            ));
        }
    }
    let (Some(pa), Some(pb)) = (&a.projection, &b.projection) else {
        return;
    };
    for p in &pa.parameters {
        if p.name == ParamName::Other || p.class() == ParamClass::Unknown {
            continue;
        }
        let Some(q) = pb.param(p.name) else { continue };
        if p.unit.is_none() || q.unit.is_none() {
            errors.push(format!("{pair}: {:?} has no unit", p.name));
            continue;
        }
        // 1e-9 relative: ESRI and EPSG round repeating decimals differently. Angles compare
        // modulo 360° (ESRI writes the 3375 azimuth as -36.97°, EPSG as 323.03°).
        let (x, y) = (p.normalized(), q.normalized());
        let agree = if p.class() == ParamClass::Angular {
            let d = (x - y).rem_euclid(360.0);
            d.min(360.0 - d) <= 1e-9 * x.abs().max(y.abs()).max(1.0)
        } else {
            close(x, y, 1e-9)
        };
        if !agree {
            errors.push(format!(
                "{pair}: {:?} = {} vs {}",
                p.name,
                p.normalized(),
                q.normalized()
            ));
        }
    }
}

#[test]
fn spot_checks() {
    let parse = |id: &str| {
        let f = fixtures().into_iter().find(|f| f.id == id).unwrap();
        whereabouts_crs::parse_bytes(&f.bytes).unwrap()
    };
    let param = |crs: &Crs, name: ParamName| {
        horizontal(crs)
            .projection
            .as_ref()
            .unwrap()
            .param(name)
            .unwrap()
            .normalized()
    };

    // US survey feet → metres (6561666.667 ftUS ≈ 2,000,000 m).
    for id in [
        "epsg-2227/esri.prj",
        "epsg-2227/wkt1.txt",
        "epsg-2227/wkt2_2015.txt",
        "epsg-2227/wkt2_2019.txt",
        "epsg-2227/projjson.json",
    ] {
        let fe = param(&parse(id), ParamName::FalseEasting);
        assert!((fe - 2_000_000.0).abs() < 0.001, "{id}: {fe}");
    }
    // Grads → degrees (52 grad = 46.8°); Paris meridian 2.5969213 grad = 2.33722917°.
    for id in [
        "epsg-27572/esri.prj",
        "epsg-27572/wkt1.txt",
        "epsg-27572/wkt2_2019.txt",
        "epsg-27572/projjson.json",
    ] {
        let crs = parse(id);
        let lat0 = param(&crs, ParamName::LatitudeOfOrigin);
        assert!((lat0 - 46.8).abs() < 1e-9, "{id}: {lat0}");
        let pm = crs.datum.as_ref().unwrap().prime_meridian.longitude;
        assert!((pm - 2.337_229_17).abs() < 1e-7, "{id}: {pm}");
    }
    // Polar stereographic variant B: the standard parallel lands in the same canonical
    // parameter whatever the dialect (OGC WKT1 writes it as `latitude_of_origin`).
    for id in [
        "epsg-3031/esri.prj",
        "epsg-3031/wkt1.txt",
        "epsg-3031/wkt2_2015.txt",
        "epsg-3031/wkt2_2019.txt",
        "epsg-3031/projjson.json",
    ] {
        let sp = param(&parse(id), ParamName::StandardParallel1);
        assert!((sp + 71.0).abs() < 1e-12, "{id}: {sp}");
    }
    // Spheres: inverse flattening 0.
    let sphere = parse("handwritten/esri-sphere-mercator.prj");
    let e = sphere.datum.as_ref().unwrap().ellipsoid.as_ref().unwrap();
    assert!(e.is_sphere());
    assert_eq!(e.semi_minor_axis(), 6_371_000.0);
    // TOWGS84 from WKT1, and the same shift from a WKT2 BOUNDCRS (position vector) and a
    // PROJJSON BoundCRS (coordinate frame, rotations in arc-seconds, scale in ppm).
    let expected = [446.448, -125.157, 542.06, 0.15, 0.247, 0.842, -20.489];
    for id in [
        "handwritten/wkt1-towgs84.txt",
        "handwritten/wkt2_2019-boundcrs.txt",
        "handwritten/projjson-boundcrs.json",
    ] {
        let crs = parse(id);
        let t = crs.datum.as_ref().unwrap().to_wgs84.unwrap();
        for (got, want) in t.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "{id}: {t:?}");
        }
    }
    // Derived geographic CRSs (rotated pole) are Unknown in both encodings.
    let wkt = parse("handwritten/wkt2_2019-rotated-pole.txt");
    let json = parse("handwritten/projjson-rotated-pole.json");
    assert_eq!(wkt.kind, CrsKind::Unknown);
    assert_eq!((wkt.kind, &wkt.name), (json.kind, &json.name));
    // USAGE with TIMEEXTENT date literals is skipped; the CRS is the same as without it.
    let with_usage = parse("handwritten/wkt2_2019-usage-timeextent.txt");
    let plain = parse("epsg-32633/wkt2_2019.txt");
    assert_eq!(
        with_usage.projection.as_ref().map(|p| p.method),
        plain.projection.as_ref().map(|p| p.method)
    );
    assert_eq!(
        param(&with_usage, ParamName::LongitudeOfOrigin),
        param(&plain, ParamName::LongitudeOfOrigin)
    );
    // Escaped quotes and axis order.
    let quoted = parse("handwritten/wkt2_2019-escaped-quotes.txt");
    assert_eq!(quoted.name, r#"Somebody's "custom" WGS 84"#);
    assert_eq!(quoted.axis_order, whereabouts_crs::AxisOrder::EastNorth);
    assert_eq!(
        parse("epsg-4326/wkt2_2019.txt").axis_order,
        whereabouts_crs::AxisOrder::NorthEast
    );
    // ESRI names normalise to EPSG datum names.
    assert_eq!(
        parse("epsg-32633/esri.prj").datum.unwrap().canonical_name(),
        parse("epsg-32633/wkt2_2019.txt")
            .datum
            .unwrap()
            .canonical_name()
    );
}

/// BOM, CRLF + indentation, trailing NUL padding and surrounding whitespace never change the
/// parse result.
#[test]
fn encoding_noise_is_ignored() {
    for f in fixtures() {
        let clean = whereabouts_crs::parse_bytes(&f.bytes).unwrap();
        let text = String::from_utf8(f.bytes.clone()).unwrap();
        let mut variants: Vec<Vec<u8>> = Vec::new();
        variants.push([b"\xEF\xBB\xBF".as_slice(), &f.bytes].concat());
        variants.push([&f.bytes[..], &[0u8; 64][..]].concat());
        variants.push(format!("\r\n  {}\r\n\r\n", text.trim()).into_bytes());
        variants.push(text.replace('\n', "\r\n").into_bytes());
        if !text.trim_start().starts_with('{') {
            // Pretty-printing WKT: a newline + indent after every comma outside quotes.
            variants.push(pretty(&text).into_bytes());
        }
        for (i, v) in variants.iter().enumerate() {
            let parsed = whereabouts_crs::parse_bytes(v)
                .unwrap_or_else(|e| panic!("{} variant {i}: {e}", f.id));
            assert_eq!(parsed, clean, "{} variant {i}", f.id);
        }
    }
}

fn pretty(wkt: &str) -> String {
    let mut out = String::new();
    let mut in_quotes = false;
    for c in wkt.chars() {
        out.push(c);
        if c == '"' {
            in_quotes = !in_quotes;
        } else if c == ',' && !in_quotes {
            out.push_str("\r\n\t ");
        }
    }
    out
}
