//! Fuzz-style robustness tests: truncated, mutated and pathological CRS text must produce an
//! error or a sane model — never a panic, a hang, a stack overflow or a non-finite number.
//!
//! Deterministic (seeded xorshift), so failures reproduce; each failure message names the
//! fixture and the mutation.

#![allow(clippy::unwrap_used)]

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use common::fixtures;
use whereabouts_crs::{Crs, CrsError, MAX_DEPTH, MAX_INPUT_LEN};

/// Parses `bytes`, turning a panic into a test failure that names the input, and checks the
/// invariants of any successful result.
fn parse_checked(bytes: &[u8], what: &dyn Fn() -> String) -> Result<Crs, CrsError> {
    let result = catch_unwind(AssertUnwindSafe(|| whereabouts_crs::parse_bytes(bytes)))
        .unwrap_or_else(|_| panic!("parser panicked on {}", what()));
    if let Ok(crs) = &result {
        check_invariants(crs, what);
    }
    result
}

fn check_invariants(crs: &Crs, what: &dyn Fn() -> String) {
    let units = [&crs.linear_unit, &crs.angular_unit];
    for u in units.into_iter().flatten() {
        assert!(
            u.factor.is_finite() && u.factor > 0.0,
            "bad unit {u:?} from {}",
            what()
        );
    }
    for a in &crs.axes {
        if let Some(u) = &a.unit {
            assert!(u.factor.is_finite() && u.factor > 0.0, "{}", what());
        }
    }
    if let Some(d) = &crs.datum {
        assert!(d.prime_meridian.longitude.is_finite(), "{}", what());
        if let Some(t) = &d.to_wgs84 {
            assert!(
                t.iter().all(|v| v.is_finite()),
                "to_wgs84 {t:?} from {}",
                what()
            );
        }
        if let Some(e) = &d.ellipsoid {
            assert!(
                e.semi_major_axis.is_finite() && e.semi_major_axis > 0.0,
                "{}",
                what()
            );
            assert!(
                e.inverse_flattening == 0.0
                    || (e.inverse_flattening.is_finite() && e.inverse_flattening > 1.0),
                "{}",
                what()
            );
        }
    }
    if let Some(p) = &crs.projection {
        for param in &p.parameters {
            assert!(param.value.is_finite(), "{}", what());
            assert!(param.normalized().is_finite(), "{}", what());
            if let Some(u) = &param.unit {
                assert!(u.factor.is_finite() && u.factor > 0.0, "{}", what());
            }
        }
    }
    for c in &crs.components {
        check_invariants(c, what);
    }
}

/// Small deterministic PRNG (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        // `n` is always > 0 at call sites.
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn every_truncation_is_handled() {
    let mut parsed = 0usize;
    let mut errors = 0usize;
    for f in fixtures() {
        let len = f.bytes.len();
        for cut in 0..len {
            let prefix = &f.bytes[..cut];
            let r = parse_checked(prefix, &|| format!("{} truncated to {cut} bytes", f.id));
            parsed += 1;
            if r.is_err() {
                errors += 1;
            }
        }
        // Cutting more than the trailing whitespace off a single-CRS definition must fail.
        let body = f.bytes.trim_ascii_end().len();
        let single_root = !f.id.ends_with("vertcs.prj") && !f.id.contains("7405/esri");
        if single_root && body > 1 {
            assert!(
                whereabouts_crs::parse_bytes(&f.bytes[..body - 1]).is_err(),
                "{} without its last byte still parses",
                f.id
            );
        }
    }
    println!("{parsed} truncated inputs, {errors} rejected");
    assert!(errors > parsed / 2);
}

#[test]
fn random_mutations_are_handled() {
    const PER_FIXTURE: usize = 300;
    const NASTY: &[&[u8]] = &[
        b"[",
        b"]",
        b"(",
        b")",
        b"\"",
        b",",
        b"1e400",
        b"-1e400",
        b"NaN",
        b"inf",
        b"-0",
        b"0",
        b"-1",
        b"\0",
        b"\xff\xfe",
        b"\xC3",
        b"{",
        b"}",
        b"null",
        b"[[[[[[[[[[[[[[[[",
        b"PARAMETER[\"x\",",
        b"UNIT[\"u\",0]",
        b"SPHEROID[\"s\",0,0]",
        b"\"\"",
    ];
    let mut rng = Rng(0x5EED_CAFE_F00D_0001);
    let mut total = 0usize;
    for f in fixtures() {
        for i in 0..PER_FIXTURE {
            let mut b = f.bytes.clone();
            let len = b.len().max(1);
            let op = rng.below(5);
            match op {
                // Overwrite one byte with a random byte.
                0 => {
                    let at = rng.below(len).min(b.len().saturating_sub(1));
                    if let Some(x) = b.get_mut(at) {
                        *x = (rng.next() & 0xFF) as u8;
                    }
                }
                // Delete a range.
                1 => {
                    let at = rng.below(len);
                    let n = rng.below(32) + 1;
                    let end = (at + n).min(b.len());
                    if at < end {
                        b.drain(at..end);
                    }
                }
                // Duplicate a range.
                2 => {
                    let at = rng.below(len).min(b.len());
                    let end = (at + rng.below(200) + 1).min(b.len());
                    let chunk = b[at..end].to_vec();
                    let to = rng.below(b.len() + 1);
                    b.splice(to..to, chunk);
                }
                // Insert a nasty token.
                3 => {
                    let tok = NASTY[rng.below(NASTY.len())];
                    let to = rng.below(b.len() + 1);
                    b.splice(to..to, tok.iter().copied());
                }
                // Replace a number with a nasty token.
                _ => {
                    let digits: Vec<usize> =
                        (0..b.len()).filter(|&j| b[j].is_ascii_digit()).collect();
                    if !digits.is_empty() {
                        let at = digits[rng.below(digits.len())];
                        let mut end = at;
                        while end < b.len() && (b[end].is_ascii_digit() || b[end] == b'.') {
                            end += 1;
                        }
                        let tok = NASTY[rng.below(NASTY.len())];
                        b.splice(at..end, tok.iter().copied());
                    }
                }
            }
            let _ = parse_checked(&b, &|| format!("{} mutation #{i} (op {op})", f.id));
            total += 1;
        }
    }
    println!("{total} mutated inputs handled");
}

#[test]
fn pathological_inputs_are_rejected() {
    let cases: Vec<(String, Vec<u8>)> = vec![
        ("deep WKT nesting".into(), b"A[".repeat(1_000_000)),
        (
            "nesting just past the limit".into(),
            [
                b"GEOGCS[\"x\",".repeat(MAX_DEPTH + 1),
                b"]".repeat(MAX_DEPTH + 1),
            ]
            .concat(),
        ),
        ("deep JSON nesting".into(), b"[".repeat(100_000)),
        ("deep JSON objects".into(), b"{\"type\":".repeat(10_000)),
        ("oversized".into(), vec![b' '; MAX_INPUT_LEN + 1]),
        (
            "oversized WKT".into(),
            format!("GEOGCS[\"{}\"]", "x".repeat(MAX_INPUT_LEN)).into_bytes(),
        ),
        (
            "long number".into(),
            format!("A[{}]", "9".repeat(5000)).into_bytes(),
        ),
        (
            "many siblings".into(),
            format!("GEOGCS[\"x\"{}]", ",AXIS[\"a\",NORTH]".repeat(20_000)).into_bytes(),
        ),
        (
            "unterminated string".into(),
            format!("GEOGCS[\"{}", "x".repeat(100_000)).into_bytes(),
        ),
        (
            "binary garbage".into(),
            (0..=255u8).cycle().take(10_000).collect(),
        ),
        ("only brackets".into(), b"[[[]]]".to_vec()),
        ("lone keyword".into(), b"PROJCS".to_vec()),
        ("nul bytes".into(), vec![0u8; 1000]),
        (
            "utf-16 text".into(),
            "GEOGCS[\"x\"]"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
    ];
    for (name, bytes) in cases {
        let r = parse_checked(&bytes, &|| name.clone());
        assert!(r.is_err(), "{name} should be rejected, got {r:?}");
    }
}

/// Finite values times finite unit factors can still overflow (1e300 × 1e300). Every such
/// product must be rejected (or, for a bound CRS's transformation, dropped) — never stored as
/// ±inf.
#[test]
fn overflowing_unit_products_never_reach_the_model() {
    // Every fixture with its unit factors blown up to 1e307: values above ~18 overflow, and
    // degree conversions overflow for any value above ~0.3.
    let mut variants = 0usize;
    for f in fixtures() {
        let text = String::from_utf8_lossy(&f.bytes).into_owned();
        for (from, to) in [
            ("0.0174532925199433", "1e307"),
            ("0.017453292519943295", "1e307"),
            ("0.0157079632679489", "1e307"),
            ("\"metre\",1", "\"metre\",1e307"),
            ("0.304800609601219", "1e307"),
        ] {
            if text.contains(from) {
                let blown = text.replace(from, to);
                let _ = parse_checked(blown.as_bytes(), &|| {
                    format!("{} with {from} -> {to}", f.id)
                });
                variants += 1;
            }
        }
    }
    assert!(variants > 100, "only {variants} variants");

    let geog = |inner: &str| {
        format!(
            r#"GEOGCRS["x",DATUM["d",ELLIPSOID["e",{inner}]],CS[ellipsoidal,2],AXIS["lat",north],AXIS["lon",east],ANGLEUNIT["degree",0.0174532925199433]]"#
        )
    };
    let bound = |param: &str| {
        format!(
            r#"BOUNDCRS[SOURCECRS[{}],TARGETCRS[GEOGCRS["WGS 84",DATUM["WGS 84",ELLIPSOID["WGS 84",6378137,298.257223563]],CS[ellipsoidal,2],AXIS["lat",north],AXIS["lon",east],ANGLEUNIT["degree",0.0174532925199433],ID["EPSG",4326]]],ABRIDGEDTRANSFORMATION["t",METHOD["Position Vector transformation"],{param}]]"#,
            geog("6378137,298.257")
        )
    };
    let bound_json = |param: &str| {
        format!(
            r#"{{"type":"BoundCRS","source_crs":{{"type":"GeographicCRS","name":"x","datum":{{"name":"d","ellipsoid":{{"name":"e","semi_major_axis":6378137,"inverse_flattening":298.257}}}}}},"target_crs":{{"type":"GeographicCRS","name":"WGS 84","datum":{{"name":"d","ellipsoid":{{"name":"e","semi_major_axis":6378137,"inverse_flattening":298.257}}}}}},"transformation":{{"name":"t","method":{{"name":"Position Vector transformation"}},"parameters":[{param}]}}}}"#
        )
    };
    let huge_len = r#"{"type":"LinearUnit","name":"u","conversion_factor":1e300}"#;
    let huge_ang = r#"{"type":"AngularUnit","name":"u","conversion_factor":1e300}"#;
    let huge_scale = r#"{"type":"ScaleUnit","name":"u","conversion_factor":1e300}"#;
    let cases = [
        geog(r#"1e300,298.257,LENGTHUNIT["u",1e300]"#),
        r#"GEOGCRS["x",DATUM["d",ELLIPSOID["e",6378137,298.257]],PRIMEM["p",1e300,ANGLEUNIT["u",1e300]],CS[ellipsoidal,2],AXIS["lat",north],AXIS["lon",east]]"#.to_owned(),
        r#"PROJCRS["x",BASEGEOGCRS["b",DATUM["d",ELLIPSOID["e",6378137,298.257]]],CONVERSION["c",METHOD["Mercator"],PARAMETER["Latitude of 1st standard parallel",1e300,ANGLEUNIT["u",1e300]]],CS[Cartesian,2],AXIS["e",east],AXIS["n",north],LENGTHUNIT["metre",1]]"#.to_owned(),
        r#"PROJCS["x",GEOGCS["g",DATUM["d",SPHEROID["s",6378137,298.257]],UNIT["u",1e300]],PROJECTION["Transverse_Mercator"],PARAMETER["central_meridian",1e300],UNIT["metre",1]]"#.to_owned(),
        r#"PROJCS["x",GEOGCS["g",DATUM["d",SPHEROID["s",6378137,298.257]],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["false_easting",1e300],UNIT["u",1e300]]"#.to_owned(),
        bound(r#"PARAMETER["X-axis translation",1e300,LENGTHUNIT["u",1e300]]"#),
        bound(r#"PARAMETER["X-axis rotation",1e300,ANGLEUNIT["u",1e300]]"#),
        // 1e306 radians is ~2e311 arc-seconds: overflows only after the conversion.
        bound(r#"PARAMETER["X-axis rotation",1e306,ANGLEUNIT["radian",1]]"#),
        bound(r#"PARAMETER["Scale difference",1e300,SCALEUNIT["u",1e300]]"#),
        bound(r#"PARAMETER["Scale difference",1e305]"#),
        bound_json(&format!(
            r#"{{"name":"X-axis translation","value":1e300,"unit":{huge_len}}}"#
        )),
        bound_json(&format!(
            r#"{{"name":"Z-axis rotation","value":1e300,"unit":{huge_ang}}}"#
        )),
        bound_json(&format!(
            r#"{{"name":"Scale difference","value":1e300,"unit":{huge_scale}}}"#
        )),
        bound_json(r#"{"name":"Scale difference","value":1e305,"unit":"unity"}"#),
        format!(
            r#"{{"type":"GeographicCRS","name":"x","datum":{{"name":"d","ellipsoid":{{"name":"e","semi_major_axis":6378137,"inverse_flattening":298.257}},"prime_meridian":{{"name":"p","longitude":{{"value":1e300,"unit":{huge_ang}}}}}}}}}"#
        ),
        format!(
            r#"{{"type":"GeographicCRS","name":"x","datum":{{"name":"d","ellipsoid":{{"name":"e","semi_major_axis":6378137,"semi_minor_axis":{{"value":1e300,"unit":{huge_len}}}}}}}}}"#
        ),
    ];
    for (i, case) in cases.iter().enumerate() {
        let r = parse_checked(case.as_bytes(), &|| format!("overflow case #{i}: {case}"));
        if let Ok(crs) = &r {
            // Only bound CRSs may survive, and then without the transformation.
            let to_wgs84 = crs.datum.as_ref().and_then(|d| d.to_wgs84);
            assert_eq!(to_wgs84, None, "overflow case #{i} kept {to_wgs84:?}");
        }
    }
}
