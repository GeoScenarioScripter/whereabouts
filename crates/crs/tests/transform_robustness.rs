//! Identification and bbox transformation never panic, whatever the CRS parameters or bounds.
//!
//! CRS text is untrusted (AGENTS.md guardrail 5) and parses into arbitrary finite parameter
//! values; `proj4rs` and the native inverses must turn nonsense into errors or partial results.
//! Deterministic (seeded xorshift), so failures reproduce.

#![allow(clippy::unwrap_used)]

mod common;

use whereabouts_core::Bounds;
use whereabouts_crs::{AxisPolicy, Crs, TransformOptions, identify, transform_bounds_with};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.next() % items.len() as u64) as usize]
    }
}

const NASTY: &[f64] = &[
    0.0,
    -0.0,
    1.0,
    -1.0,
    90.0,
    -90.0,
    180.0,
    -180.0,
    360.0,
    1e-300,
    1e300,
    -1e300,
    1e15,
    89.999_999_999,
    45.0,
    0.5,
    2.0,
    1e-12,
];

const BOXES: &[[f64; 4]] = &[
    [0.0, 0.0, 0.0, 0.0],
    [-1e7, -1e7, 1e7, 1e7],
    [-1e300, -1e300, 1e300, 1e300],
    [-180.0, -90.0, 180.0, 90.0],
    [1e15, 1e15, 1e15 + 1.0, 1e15 + 1.0],
    [500_000.0, 5_000_000.0, 600_000.0, 5_100_000.0],
    [-0.0, -1e-300, 1e-300, 0.0],
];

fn exercise(crs: &Crs, rng: &mut Rng) {
    let _ = identify(crs);
    for policy in [AxisPolicy::TraditionalGis, AxisPolicy::Authority] {
        for b in BOXES {
            let bounds = Bounds::new(b[0], b[1], b[2], b[3]).unwrap();
            let densify = rng.pick(&[0, 1, 21, 5000]);
            let options = TransformOptions {
                densify,
                axis_policy: policy,
            };
            if let Ok(t) = transform_bounds_with(&bounds, crs, &options) {
                for p in t.extent.parts() {
                    assert!(p.min_x() >= -180.0 && p.max_x() <= 180.0);
                    assert!(p.min_y() >= -90.0 && p.max_y() <= 90.0);
                }
            }
        }
    }
}

#[test]
fn fixtures_with_nasty_bounds_and_parameters() {
    let mut rng = Rng(0x2400_c0ff_ee00_0001);
    let mut cases = 0usize;
    for f in common::fixtures() {
        let Ok(crs) = whereabouts_crs::parse_bytes(&f.bytes) else {
            continue;
        };
        exercise(&crs, &mut rng);
        cases += 1;
        // Mutate projection parameters, ellipsoid and units.
        for _ in 0..12 {
            let mut m = crs.clone();
            if let Some(p) = m.projection.as_mut() {
                for param in &mut p.parameters {
                    if rng.next().is_multiple_of(2) {
                        param.value = rng.pick(NASTY);
                    }
                }
            }
            if let Some(e) = m.datum.as_mut().and_then(|d| d.ellipsoid.as_mut())
                && rng.next().is_multiple_of(3)
            {
                e.inverse_flattening = rng.pick(&[0.0, 1.000_001, 2.0, 1e300, 298.257_223_563]);
                e.semi_major_axis = rng.pick(&[1e-300, 1.0, 6_378_137.0, 1e300]);
            }
            if let Some(u) = m.linear_unit.as_mut()
                && rng.next().is_multiple_of(4)
            {
                u.factor = rng.pick(&[1e-300, 1e300, 1.0]);
            }
            if let Some(d) = m.datum.as_mut()
                && rng.next().is_multiple_of(4)
            {
                d.prime_meridian.longitude = rng.pick(NASTY);
                d.to_wgs84 = Some([rng.pick(NASTY); 7]);
            }
            exercise(&m, &mut rng);
            cases += 1;
        }
    }
    assert!(cases > 2000, "{cases}");
}

#[test]
fn every_epsg_table_crs_transforms_without_panic() {
    let mut rng = Rng(7);
    let mut ok = 0;
    for code in 2000..=32_767u32 {
        let Some(crs) = whereabouts_crs::epsg_crs(code) else {
            continue;
        };
        let b = rng.pick(BOXES);
        let bounds = Bounds::new(b[0], b[1], b[2], b[3]).unwrap();
        let _ = transform_bounds_with(&bounds, &crs, &TransformOptions::default());
        ok += 1;
    }
    assert!(ok > 6000, "{ok}");
}
