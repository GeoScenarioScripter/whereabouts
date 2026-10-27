//! Native inverse projections for methods `proj4rs` does not have (or has only spherically):
//! Cassini-Soldner, Hotine Oblique Mercator, New Zealand Map Grid, Equal Earth, Robinson,
//! Sinusoidal and ellipsoidal Equidistant Cylindrical.
//!
//! Only the inverse (projected → geographic) is needed for bbox transformation. The formulas
//! follow PROJ's implementations (`cass.cpp`, `omerc.cpp`, `nzmg.cpp`, `eqearth.cpp`,
//! `robin.cpp`, `sinu.cpp`, `eqc.cpp`); they are checked against PROJ through the GDAL oracle points in
//! `tests/g2/`. Inputs are easting/northing relative to the false origin, divided by the
//! semi-major axis (as PROJ's `pj_inv` passes them). Outputs are `(λ − λ0, φ)` in radians, with
//! `λ0` the CRS's longitude of origin (`lon_0`/`lonc`). `None` means out of domain or no
//! convergence; nothing here panics or loops unboundedly.

use std::f64::consts::FRAC_PI_2;

use crate::spec::{Method, ProjSpec};

/// A native inverse projection, set up from a [`ProjSpec`].
#[derive(Debug, Clone)]
pub(crate) enum Native {
    Cass(Cass),
    Omerc(Box<Omerc>),
    Nzmg(Nzmg),
    EqEarth(EqEarth),
    Robin,
    Sinu(Sinu),
    Eqc(Eqc),
}

impl Native {
    /// The native implementation of `spec.method`, if this module has one.
    pub(crate) fn new(spec: &ProjSpec) -> Option<Result<Self, &'static str>> {
        let es = spec.es();
        Some(match spec.method {
            Method::Cass => Ok(Native::Cass(Cass::new(spec, es))),
            Method::Omerc => Omerc::new(spec, es).map(|o| Native::Omerc(Box::new(o))),
            Method::Nzmg => Ok(Native::Nzmg(Nzmg {
                phi0: spec.params.lat_0.to_radians(),
            })),
            Method::EqEarth => Ok(Native::EqEarth(EqEarth::new(es))),
            Method::Robin => Ok(Native::Robin),
            Method::Sinu => Ok(Native::Sinu(Sinu::new(es))),
            Method::Eqc => Ok(Native::Eqc(Eqc::new(spec, es))),
            _ => return None,
        })
    }

    /// Inverse projection of normalised `(x, y)` → `(λ − λ0, φ)` in radians.
    pub(crate) fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (lam, phi) = match self {
            Native::Cass(c) => c.inverse(x, y),
            Native::Omerc(o) => o.inverse(x, y),
            Native::Nzmg(n) => n.inverse(x, y),
            Native::EqEarth(e) => e.inverse(x, y),
            Native::Robin => robin_inverse(x, y),
            Native::Sinu(s) => s.inverse(x, y),
            Native::Eqc(q) => q.inverse(x, y),
        }?;
        (lam.is_finite() && phi.is_finite() && phi.abs() <= FRAC_PI_2 + 1e-12)
            .then_some((lam, phi.clamp(-FRAC_PI_2, FRAC_PI_2)))
    }
}

// ---- shared helpers (PROJ's pj_mlfn.cpp, pj_phi2.cpp, pj_tsfn.cpp, pj_qsfn.cpp, pj_auth.cpp) ---

/// Meridian distance coefficients (PROJ `pj_enfn`).
#[derive(Debug, Clone, Copy)]
struct Enfn([f64; 5]);

impl Enfn {
    fn new(es: f64) -> Self {
        const C00: f64 = 1.0;
        const C02: f64 = 0.25;
        const C04: f64 = 0.046875;
        const C06: f64 = 0.01953125;
        const C08: f64 = 0.01068115234375;
        const C22: f64 = 0.75;
        const C44: f64 = 0.46875;
        const C46: f64 = 0.013_020_833_333_333_334;
        const C48: f64 = 0.007_120_768_229_166_667;
        const C66: f64 = 0.364_583_333_333_333_3;
        const C68: f64 = 0.005_696_614_583_333_333;
        const C88: f64 = 0.3076171875;
        let mut en = [0.0; 5];
        en[0] = C00 - es * (C02 + es * (C04 + es * (C06 + es * C08)));
        en[1] = es * (C22 - es * (C04 + es * (C06 + es * C08)));
        let mut t = es * es;
        en[2] = t * (C44 - es * (C46 + es * C48));
        t *= es;
        en[3] = t * (C66 - es * C68);
        en[4] = t * es * C88;
        Enfn(en)
    }

    /// Meridian distance from the equator to `phi`, in units of the semi-major axis.
    fn mlfn(&self, phi: f64, sphi: f64, cphi: f64) -> f64 {
        let en = &self.0;
        let cphi = cphi * sphi;
        let sphi = sphi * sphi;
        en[0] * phi - cphi * (en[1] + sphi * (en[2] + sphi * (en[3] + sphi * en[4])))
    }

    /// Latitude whose meridian distance is `arg` (Newton iteration, at most 20 steps).
    fn inv_mlfn(&self, arg: f64, es: f64) -> Option<f64> {
        let k = 1.0 / (1.0 - es);
        let mut phi = arg;
        for _ in 0..20 {
            let s = phi.sin();
            let t = 1.0 - es * s * s;
            let d = (self.mlfn(phi, s, phi.cos()) - arg) * (t * t.sqrt()) * k;
            phi -= d;
            if !phi.is_finite() {
                return None;
            }
            if d.abs() < 1e-11 {
                return Some(phi);
            }
        }
        None
    }
}

/// PROJ `pj_tsfn`.
fn tsfn(phi: f64, sinphi: f64, e: f64) -> f64 {
    let denominator = ((1.0 - e * sinphi) / (1.0 + e * sinphi)).powf(0.5 * e);
    (0.5 * (FRAC_PI_2 - phi)).tan() / denominator
}

/// PROJ `pj_phi2`: latitude from the isometric-latitude function `ts`.
fn phi2(ts: f64, e: f64) -> Option<f64> {
    let half_e = 0.5 * e;
    let mut phi = FRAC_PI_2 - 2.0 * ts.atan();
    for _ in 0..15 {
        let con = e * phi.sin();
        let dphi = FRAC_PI_2 - 2.0 * (ts * ((1.0 - con) / (1.0 + con)).powf(half_e)).atan() - phi;
        phi += dphi;
        if !phi.is_finite() {
            return None;
        }
        if dphi.abs() <= 1e-10 {
            return Some(phi);
        }
    }
    None
}

/// PROJ `pj_qsfn`.
fn qsfn(sinphi: f64, e: f64, one_es: f64) -> f64 {
    if e >= 1e-7 {
        let con = e * sinphi;
        let div1 = 1.0 - con * con;
        let div2 = 1.0 + con;
        one_es * (sinphi / div1 - (0.5 / e) * ((1.0 - con) / div2).ln())
    } else {
        sinphi + sinphi
    }
}

/// PROJ `pj_authset` / `pj_authlat`: authalic → geodetic latitude.
#[derive(Debug, Clone, Copy)]
struct Authalic([f64; 3]);

impl Authalic {
    fn new(es: f64) -> Self {
        const P00: f64 = 0.333_333_333_333_333_3;
        const P01: f64 = 0.172_222_222_222_222_2;
        const P02: f64 = 0.102_579_365_079_365_08;
        const P10: f64 = 0.063_888_888_888_888_89;
        const P11: f64 = 0.066_402_116_402_116_4;
        const P20: f64 = 0.016_415_012_942_191_544;
        let mut apa = [0.0; 3];
        apa[0] = es * P00;
        let mut t = es * es;
        apa[0] += t * P01;
        apa[1] = t * P10;
        t *= es;
        apa[0] += t * P02;
        apa[1] += t * P11;
        apa[2] = t * P20;
        Authalic(apa)
    }

    fn lat(&self, beta: f64) -> f64 {
        let t = beta + beta;
        beta + self.0[0] * t.sin() + self.0[1] * (t + t).sin() + self.0[2] * (t + t + t).sin()
    }
}

// ---- Cassini-Soldner -------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct Cass {
    es: f64,
    en: Enfn,
    m0: f64,
    phi0: f64,
}

impl Cass {
    fn new(spec: &ProjSpec, es: f64) -> Self {
        let phi0 = spec.params.lat_0.to_radians();
        let en = Enfn::new(es);
        Cass {
            es,
            en,
            m0: en.mlfn(phi0, phi0.sin(), phi0.cos()),
            phi0,
        }
    }

    fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if self.es == 0.0 {
            let dd = y + self.phi0;
            let phi = (dd.sin() * x.cos()).clamp(-1.0, 1.0).asin();
            let lam = x.tan().atan2(dd.cos());
            return Some((lam, phi));
        }
        const C3: f64 = 1.0 / 24.0;
        const C4: f64 = 1.0 / 3.0;
        const C5: f64 = 1.0 / 15.0;
        let ph1 = self.en.inv_mlfn(self.m0 + y, self.es)?;
        let tn = ph1.tan();
        let t = tn * tn;
        let mut n = ph1.sin();
        let mut r = 1.0 / (1.0 - self.es * n * n);
        n = r.sqrt();
        r *= (1.0 - self.es) * n;
        let dd = x / n;
        let d2 = dd * dd;
        let phi = ph1 - (n * tn / r) * d2 * (0.5 - (1.0 + 3.0 * t) * d2 * C3);
        let lam = dd * (1.0 + t * d2 * (-C4 + (1.0 + 3.0 * t) * d2 * C5)) / ph1.cos();
        Some((lam, phi))
    }
}

// ---- Hotine Oblique Mercator -----------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct Omerc {
    e: f64,
    b: f64,
    e_big: f64,
    ar_b: f64,
    br_a: f64,
    r_b: f64,
    u_0: f64,
    singam: f64,
    cosgam: f64,
    sinrot: f64,
    cosrot: f64,
    /// `λ0 − lonc`: PROJ's computed central meridian relative to the longitude of centre.
    dlam0: f64,
}

impl Omerc {
    fn new(spec: &ProjSpec, es: f64) -> Result<Self, &'static str> {
        const TOL: f64 = 1e-7;
        let p = &spec.params;
        let e = es.sqrt();
        let one_es = 1.0 - es;
        let phi0 = p.lat_0.to_radians();
        let alpha_c = p.alpha.to_radians();
        let gamma = p.gamma.to_radians();
        if alpha_c.abs() <= TOL
            || (phi0.abs() - FRAC_PI_2).abs() <= TOL
            || (alpha_c.abs() - FRAC_PI_2).abs() <= TOL
        {
            return Err("oblique Mercator azimuth or latitude out of range");
        }
        let com = one_es.sqrt();
        let (b, a_big, d, f, e_big);
        if phi0.abs() > 1e-10 {
            let sinph0 = phi0.sin();
            let cosph0 = phi0.cos();
            let con = 1.0 - es * sinph0 * sinph0;
            let b2 = cosph0 * cosph0;
            b = (1.0 + es * b2 * b2 / one_es).sqrt();
            a_big = b * p.k_0 * com / con;
            d = b * com / (cosph0 * con.sqrt());
            let mut ff = d * d - 1.0;
            if ff <= 0.0 {
                if ff <= -TOL {
                    return Err("oblique Mercator parameters out of range");
                }
                ff = 0.0;
            } else {
                ff = ff.sqrt();
                if phi0 < 0.0 {
                    ff = -ff;
                }
            }
            // PROJ: `Q->E = F += D;` — F is D + sqrt(D² − 1) from here on.
            f = ff + d;
            e_big = f * tsfn(phi0, sinph0, e).powf(b);
        } else {
            b = 1.0 / com;
            a_big = p.k_0;
            d = 1.0;
            f = 1.0;
            e_big = 1.0;
        }
        let gamma0 = (alpha_c.sin() / d).asin();
        if !gamma0.is_finite() {
            return Err("oblique Mercator azimuth out of range");
        }
        let dlam0 = -(0.5 * (f - 1.0 / f) * gamma0.tan()).asin() / b;
        if !dlam0.is_finite() {
            return Err("oblique Mercator parameters out of range");
        }
        let r_b = 1.0 / b;
        let ar_b = a_big * r_b;
        let br_a = 1.0 / ar_b;
        let u_0 = if p.no_uoff {
            0.0
        } else {
            let u = (ar_b * ((d * d - 1.0).max(0.0).sqrt() / alpha_c.cos()).atan()).abs();
            if phi0 < 0.0 { -u } else { u }
        };
        let omerc = Omerc {
            e,
            b,
            e_big,
            ar_b,
            br_a,
            r_b,
            u_0,
            singam: gamma0.sin(),
            cosgam: gamma0.cos(),
            sinrot: gamma.sin(),
            cosrot: gamma.cos(),
            dlam0,
        };
        let all_finite = [
            omerc.b,
            omerc.e_big,
            omerc.ar_b,
            omerc.br_a,
            omerc.u_0,
            omerc.dlam0,
        ]
        .iter()
        .all(|v| v.is_finite());
        if all_finite {
            Ok(omerc)
        } else {
            Err("oblique Mercator parameters out of range")
        }
    }

    fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let v = x * self.cosrot - y * self.sinrot;
        let u = y * self.cosrot + x * self.sinrot + self.u_0;
        let qp = (-self.br_a * v).exp();
        if qp == 0.0 || !qp.is_finite() {
            return None;
        }
        let sp = 0.5 * (qp - 1.0 / qp);
        let tp = 0.5 * (qp + 1.0 / qp);
        let vp = (self.br_a * u).sin();
        let up = (vp * self.cosgam + sp * self.singam) / tp;
        let (lam, phi) = if (up.abs() - 1.0).abs() < 1e-10 {
            (0.0, FRAC_PI_2.copysign(up))
        } else {
            let t = self.e_big / ((1.0 + up) / (1.0 - up)).sqrt();
            let phi = phi2(t.powf(1.0 / self.b), self.e)?;
            let lam =
                -self.r_b * (sp * self.cosgam - vp * self.singam).atan2((self.br_a * u).cos());
            (lam, phi)
        };
        Some((lam + self.dlam0, phi))
    }
}

// ---- New Zealand Map Grid --------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct Nzmg {
    phi0: f64,
}

impl Nzmg {
    /// Complex coefficients B1..B6 of z = Σ Bn ζⁿ (LINZ / PROJ `nzmg.cpp`).
    const BF: [(f64, f64); 6] = [
        (0.7557853228, 0.0),
        (0.249204646, 0.003371507),
        (-0.001541739, 0.04105856),
        (-0.10162907, 0.01727609),
        (-0.26623489, -0.36249218),
        (-0.6870983, -1.1651967),
    ];
    /// Δφ series D1..D9 (units of 10⁵ arc-seconds).
    const TPHI: [f64; 9] = [
        1.5627014243,
        0.5185406398,
        -0.03333098,
        -0.1052906,
        -0.0368594,
        0.007317,
        0.01220,
        0.00394,
        -0.0013,
    ];
    const SEC5_TO_RAD: f64 = 0.484_813_681_109_536;

    fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        // Complex Newton iteration on P(ζ) − z = 0 with P(ζ) = Σ Bk ζᵏ (k = 1..6), z = y + i·x.
        let mul =
            |(ar, ai): (f64, f64), (br, bi): (f64, f64)| (ar * br - ai * bi, ar * bi + ai * br);
        let z = (y, x);
        let mut p = z;
        let mut converged = false;
        for _ in 0..20 {
            let (mut val, mut der) = ((0.0, 0.0), (0.0, 0.0));
            let mut pow = (1.0, 0.0); // ζ^(k-1)
            for (k, &b) in Self::BF.iter().enumerate() {
                let term = mul(b, pow);
                let kf = (k + 1) as f64;
                der = (der.0 + kf * term.0, der.1 + kf * term.1);
                pow = mul(pow, p);
                let t = mul(b, pow);
                val = (val.0 + t.0, val.1 + t.1);
            }
            let f = (val.0 - z.0, val.1 - z.1);
            let den = der.0 * der.0 + der.1 * der.1;
            if den == 0.0 || !den.is_finite() {
                return None;
            }
            let dp = (
                -(f.0 * der.0 + f.1 * der.1) / den,
                -(f.1 * der.0 - f.0 * der.1) / den,
            );
            p = (p.0 + dp.0, p.1 + dp.1);
            if !(p.0.is_finite() && p.1.is_finite()) {
                return None;
            }
            if dp.0.abs() + dp.1.abs() <= 1e-10 {
                converged = true;
                break;
            }
        }
        if !converged {
            return None;
        }
        let (pr, pi) = p;
        // Σ Dn Δψⁿ = Δψ · (D1 + Δψ (D2 + …)).
        let mut s = 0.0;
        for &c in Self::TPHI.iter().rev() {
            s = s * pr + c;
        }
        let phi = self.phi0 + pr * s * Self::SEC5_TO_RAD;
        Some((pi, phi))
    }
}

// ---- Equal Earth -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct EqEarth {
    rqda: f64,
    apa: Option<Authalic>,
}

impl EqEarth {
    const A1: f64 = 1.340264;
    const A2: f64 = -0.081106;
    const A3: f64 = 0.000893;
    const A4: f64 = 0.003796;
    const M: f64 = 0.866_025_403_784_438_6; // sqrt(3) / 2
    const MAX_Y: f64 = 1.317_362_759_157_4;

    fn new(es: f64) -> Self {
        if es == 0.0 {
            EqEarth {
                rqda: 1.0,
                apa: None,
            }
        } else {
            let e = es.sqrt();
            let qp = qsfn(1.0, e, 1.0 - es);
            EqEarth {
                rqda: (0.5 * qp).sqrt(),
                apa: Some(Authalic::new(es)),
            }
        }
    }

    fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let x = x / self.rqda;
        let y = y / self.rqda;
        if y.abs() > Self::MAX_Y * (1.0 + 1e-9) {
            return None;
        }
        let mut yc = y.clamp(-Self::MAX_Y, Self::MAX_Y);
        let mut converged = false;
        for _ in 0..12 {
            let y2 = yc * yc;
            let y6 = y2 * y2 * y2;
            let f = yc * (Self::A1 + Self::A2 * y2 + y6 * (Self::A3 + Self::A4 * y2)) - y;
            let fder = Self::A1 + 3.0 * Self::A2 * y2 + y6 * (7.0 * Self::A3 + 9.0 * Self::A4 * y2);
            let tol = f / fder;
            yc -= tol;
            if tol.abs() < 1e-12 {
                converged = true;
                break;
            }
        }
        if !converged {
            return None;
        }
        let y2 = yc * yc;
        let y6 = y2 * y2 * y2;
        let lam = Self::M
            * x
            * (Self::A1 + 3.0 * Self::A2 * y2 + y6 * (7.0 * Self::A3 + 9.0 * Self::A4 * y2))
            / yc.cos();
        let beta = (yc.sin() / Self::M).clamp(-1.0, 1.0).asin();
        let phi = match &self.apa {
            Some(apa) => apa.lat(beta),
            None => beta,
        };
        Some((lam, phi))
    }
}

// ---- Robinson --------------------------------------------------------------------------------

/// Robinson's table at 5° steps: X = parallel length, Y = distance from the equator.
const ROBIN_X: [f64; 19] = [
    1.0000, 0.9986, 0.9954, 0.9900, 0.9822, 0.9730, 0.9600, 0.9427, 0.9216, 0.8962, 0.8679, 0.8350,
    0.7986, 0.7597, 0.7186, 0.6732, 0.6213, 0.5722, 0.5322,
];
const ROBIN_Y: [f64; 19] = [
    0.0000, 0.0620, 0.1240, 0.1860, 0.2480, 0.3100, 0.3720, 0.4340, 0.4958, 0.5571, 0.6176, 0.6769,
    0.7346, 0.7903, 0.8435, 0.8936, 0.9394, 0.9761, 1.0000,
];
const ROBIN_FXC: f64 = 0.8487;
const ROBIN_FYC: f64 = 1.3523;

/// Cubic (Catmull-Rom) interpolation of a Robinson table at fractional index `t` ∈ [0, 18].
fn robin_interp(table: &[f64; 19], t: f64) -> f64 {
    let i = (t.floor() as usize).min(17);
    let f = t - i as f64;
    let at = |k: isize| -> f64 {
        let k = k.clamp(0, 18) as usize;
        // Mirror beyond the ends (Y is odd about the equator, X even); clamping is enough at the
        // pole end, where the table is only used up to 90°.
        table[k]
    };
    let i = i as isize;
    let (p0, p1, p2, p3) = if i == 0 {
        // Reflect across the equator: X(-5°) = X(5°), Y(-5°) = -Y(5°).
        let before = if table[0] == 0.0 { -table[1] } else { table[1] };
        (before, at(0), at(1), at(2))
    } else {
        (at(i - 1), at(i), at(i + 1), at(i + 2))
    };
    let f2 = f * f;
    let f3 = f2 * f;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * f
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * f2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * f3)
}

fn robin_inverse(x: f64, y: f64) -> Option<(f64, f64)> {
    let target = (y / ROBIN_FYC).abs();
    // PROJ's spline puts the pole a hair above 1.3523·R (≈ 0.5 m); accept that.
    if target > 1.0 + 1e-6 {
        return None;
    }
    let target = target.min(1.0);
    // Y is monotonic in the table index: bisection to 1e-13.
    let (mut lo, mut hi) = (0.0_f64, 18.0_f64);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if robin_interp(&ROBIN_Y, mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t = 0.5 * (lo + hi);
    let phi = (t * 5.0).to_radians().copysign(y);
    let px = robin_interp(&ROBIN_X, t);
    if px <= 0.0 {
        return None;
    }
    let lam = x / (ROBIN_FXC * px);
    Some((lam, phi))
}

// ---- Equidistant Cylindrical -----------------------------------------------------------------

/// Equidistant Cylindrical, ellipsoidal (EPSG 1028; spherical 1029 when `es = 0`):
/// x = ν₁ cos φ₁ (λ − λ0), y = M(φ) − M(φ0). `proj4rs`' `eqc` is spherical only, which is
/// 0.15° off at the poles on WGS 84.
#[derive(Debug, Clone)]
pub(crate) struct Eqc {
    es: f64,
    en: Enfn,
    /// Parallel scale ν₁ cos φ₁ / a.
    k: f64,
    m0: f64,
}

impl Eqc {
    fn new(spec: &ProjSpec, es: f64) -> Self {
        let phi1 = spec.params.lat_ts.to_radians();
        let phi0 = spec.params.lat_0.to_radians();
        let en = Enfn::new(es);
        let s1 = phi1.sin();
        Eqc {
            es,
            en,
            k: phi1.cos() / (1.0 - es * s1 * s1).sqrt(),
            m0: if es == 0.0 {
                phi0
            } else {
                en.mlfn(phi0, phi0.sin(), phi0.cos())
            },
        }
    }

    fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if self.k <= 1e-12 {
            return None;
        }
        let phi = if self.es == 0.0 {
            y + self.m0
        } else {
            self.en.inv_mlfn(y + self.m0, self.es)?
        };
        Some((x / self.k, phi))
    }
}

// ---- Sinusoidal ------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct Sinu {
    es: f64,
    en: Enfn,
}

impl Sinu {
    fn new(es: f64) -> Self {
        Sinu {
            es,
            en: Enfn::new(es),
        }
    }

    fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if self.es == 0.0 {
            let phi = y;
            if phi.abs() > FRAC_PI_2 + 1e-12 {
                return None;
            }
            let c = phi.cos();
            return Some((if c.abs() < 1e-12 { 0.0 } else { x / c }, phi));
        }
        let phi = self.en.inv_mlfn(y, self.es)?;
        let s = phi.abs();
        if s < FRAC_PI_2 {
            let sp = phi.sin();
            Some((x * (1.0 - self.es * sp * sp).sqrt() / phi.cos(), phi))
        } else if s - 1e-10 < FRAC_PI_2 {
            Some((0.0, phi))
        } else {
            None
        }
    }
}

/// Forward projections, only for round-trip tests.
#[cfg(test)]
pub(crate) mod forward {
    use super::*;

    /// Equal Earth forward (spherical/ellipsoidal), for round-trip tests.
    pub fn eqearth(e: &EqEarth, es: f64, lam: f64, phi: f64) -> (f64, f64) {
        let sbeta = if es == 0.0 {
            phi.sin()
        } else {
            let ee = es.sqrt();
            let qp = qsfn(1.0, ee, 1.0 - es);
            (qsfn(phi.sin(), ee, 1.0 - es) / qp).clamp(-1.0, 1.0)
        };
        let psi = (EqEarth::M * sbeta).asin();
        let psi2 = psi * psi;
        let psi6 = psi2 * psi2 * psi2;
        let x = lam * psi.cos()
            / (EqEarth::M
                * (EqEarth::A1
                    + 3.0 * EqEarth::A2 * psi2
                    + psi6 * (7.0 * EqEarth::A3 + 9.0 * EqEarth::A4 * psi2)));
        let y =
            psi * (EqEarth::A1 + EqEarth::A2 * psi2 + psi6 * (EqEarth::A3 + EqEarth::A4 * psi2));
        (x * e.rqda, y * e.rqda)
    }

    /// Sinusoidal forward.
    pub fn sinu(s: &Sinu, lam: f64, phi: f64) -> (f64, f64) {
        if s.es == 0.0 {
            return (lam * phi.cos(), phi);
        }
        let sp = phi.sin();
        let cp = phi.cos();
        (
            lam * cp / (1.0 - s.es * sp * sp).sqrt(),
            s.en.mlfn(phi, sp, cp),
        )
    }

    /// Robinson forward.
    pub fn robin(lam: f64, phi: f64) -> (f64, f64) {
        let t = phi.abs().to_degrees() / 5.0;
        (
            ROBIN_FXC * robin_interp(&ROBIN_X, t) * lam,
            (ROBIN_FYC * robin_interp(&ROBIN_Y, t)).copysign(phi),
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::forward;
    use super::*;
    use crate::spec::{Params, ProjSpec};

    fn spec(method: Method, params: Params, rf: f64) -> ProjSpec {
        ProjSpec {
            method,
            params,
            a: 6_378_137.0,
            rf,
            pm: 0.0,
            unit: 1.0,
            axes: *b"en",
            gis_swap: false,
            spherical: false,
            to_wgs84: None,
        }
    }

    #[test]
    fn meridian_arc_round_trips() {
        let es = 0.006_694_379_990_14;
        let en = Enfn::new(es);
        for deg in [-89.0, -45.0, 0.0, 12.5, 60.0, 89.9] {
            let phi = f64::to_radians(deg);
            let m = en.mlfn(phi, phi.sin(), phi.cos());
            assert!((en.inv_mlfn(m, es).unwrap() - phi).abs() < 1e-11);
        }
    }

    #[test]
    fn forward_inverse_round_trips() {
        let es = 0.006_694_379_990_14;
        let ee = EqEarth::new(es);
        let si = Sinu::new(es);
        for (lon, lat) in [(0.0, 0.0), (10.0, 20.0), (-170.0, -80.0), (179.0, 89.0)] {
            let (lam, phi) = (f64::to_radians(lon), f64::to_radians(lat));
            let (x, y) = forward::eqearth(&ee, es, lam, phi);
            let (l2, p2) = ee.inverse(x, y).unwrap();
            assert!(
                (l2 - lam).abs() < 1e-9 && (p2 - phi).abs() < 1e-6,
                "eqearth {lon} {lat}"
            );
            let (x, y) = forward::sinu(&si, lam, phi);
            let (l2, p2) = si.inverse(x, y).unwrap();
            assert!(
                (l2 - lam).abs() < 1e-9 && (p2 - phi).abs() < 1e-9,
                "sinu {lon} {lat}"
            );
            let (x, y) = forward::robin(lam, phi);
            let (l2, p2) = robin_inverse(x, y).unwrap();
            assert!(
                (l2 - lam).abs() < 1e-9 && (p2 - phi).abs() < 1e-9,
                "robin {lon} {lat}"
            );
        }
    }

    #[test]
    fn out_of_domain_is_none_not_panic() {
        let es = 0.006_694_379_990_14;
        let s = spec(Method::Cass, Params::default(), 298.257_223_563);
        let natives = [
            Native::new(&s).unwrap().unwrap(),
            Native::EqEarth(EqEarth::new(es)),
            Native::Robin,
            Native::Sinu(Sinu::new(es)),
            Native::Nzmg(Nzmg { phi0: -0.7 }),
        ];
        for n in &natives {
            for (x, y) in [
                (1e9, 1e9),
                (f64::NAN, 0.0),
                (f64::INFINITY, 1.0),
                (-1e300, 1e300),
                (0.0, 5.0),
            ] {
                let _ = n.inverse(x, y); // must not panic
            }
        }
        assert!(natives[1].inverse(0.0, 5.0).is_none());
        assert!(natives[2].inverse(0.0, 5.0).is_none());
    }
}
