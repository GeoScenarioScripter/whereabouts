//! Helmert transformations to WGS 84 from WKT2 `BOUNDCRS` and PROJJSON `BoundCRS`, shared by
//! both parsers. The result uses WKT1 `TOWGS84` conventions: translations in metres, rotations
//! in arc-seconds with the position-vector sign convention, scale difference in ppm.
//!
//! A transformation that can't be read (unsupported method, missing or non-numeric parameter,
//! invalid unit, non-finite result) yields `None`: the bound CRS's source CRS is still valid, it
//! just has no `to_wgs84` — the same outcome as an unreadable WKT1 `TOWGS84`.

use crate::names::name_key;
use crate::{Crs, CrsKind, Unit, UnitKind};

/// One transformation parameter as written: its name, EPSG code (if cited), numeric value and
/// unit. `value`/`unit` are `Err(())` when the parameter was present but malformed.
#[derive(Debug)]
pub(crate) struct RawParam {
    pub name: String,
    pub epsg: Option<u32>,
    pub value: Option<f64>,
    pub unit: Result<Option<Unit>, ()>,
}

/// How the parameters were written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// WKT2 `ABRIDGEDTRANSFORMATION`: units are implied (ISO 19162 §18.3; PROJ's reader and
    /// writer): metres, arc-seconds, and the scale difference written as the multiplier
    /// `1 + dS` (e.g. `0.999979511` for −20.489 ppm). An explicit unit, if a writer adds one,
    /// still wins.
    Abridged,
    /// PROJJSON (non-abridged): a parameter without unit is taken in metres / arc-seconds / ppm.
    Full,
}

/// Whether a target CRS is WGS 84 (EPSG:4326 or named `WGS 84`).
pub(crate) fn is_wgs84(epsg: Option<u32>, name: &str) -> bool {
    epsg == Some(4326) || name_key(name) == "wgs84"
}

/// Reads a geocentric-translation, position-vector or coordinate-frame Helmert transformation.
pub(crate) fn helmert_from_params(
    method_name: &str,
    params: &[RawParam],
    encoding: Encoding,
) -> Option<[f64; 7]> {
    let method = name_key(method_name);
    let coordinate_frame = method.contains("coordinateframe");
    if !(method.contains("geocentrictranslation")
        || method.contains("positionvector")
        || coordinate_frame)
    {
        return None;
    }
    let arcsec = std::f64::consts::PI / 648_000.0;
    let mut out = [0.0; 7];
    for p in params {
        let Some(slot) = slot(p.epsg, &p.name) else {
            continue;
        };
        let raw = p.value?;
        let unit = p.unit.as_ref().ok()?.as_ref();
        out[slot] = match (slot, unit) {
            // Translations: metres unless a (linear) unit says otherwise.
            (0..=2, Some(u)) => raw * u.factor,
            (0..=2, None) => raw,
            // Rotations: arc-seconds unless a unit says otherwise.
            (3..=5, Some(u)) => raw * u.factor / arcsec,
            (3..=5, None) => raw,
            // Scale difference, decided by the unit:
            // - a ppm-sized unit (parts per million): the value is ppm;
            // - a unity-sized scale unit: a ratio — the multiplier `1 + dS` (≈ 1) or dS itself
            //   (|dS| < 1e-3 in practice); the two ranges can't be confused;
            // - no unit: abridged WKT2 writes the multiplier, PROJJSON ppm.
            (_, Some(u)) if u.kind == UnitKind::Scale || u.kind == UnitKind::Unknown => {
                let ratio = raw * u.factor;
                if u.factor >= 0.5e-3 && (ratio - 1.0).abs() < 0.5 {
                    (ratio - 1.0) * 1e6
                } else {
                    ratio * 1e6
                }
            }
            (_, Some(_)) => return None,
            (_, None) => match encoding {
                Encoding::Abridged => (raw - 1.0) * 1e6,
                Encoding::Full => raw,
            },
        };
    }
    if coordinate_frame {
        for r in &mut out[3..6] {
            *r = -*r;
        }
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// TOWGS84 slot of a Helmert parameter, by EPSG code or name.
fn slot(epsg: Option<u32>, name: &str) -> Option<usize> {
    let key = name_key(name);
    Some(match (epsg, key.as_str()) {
        (Some(8605), _) | (_, "xaxistranslation") => 0,
        (Some(8606), _) | (_, "yaxistranslation") => 1,
        (Some(8607), _) | (_, "zaxistranslation") => 2,
        (Some(8608), _) | (_, "xaxisrotation") => 3,
        (Some(8609), _) | (_, "yaxisrotation") => 4,
        (Some(8610), _) | (_, "zaxisrotation") => 5,
        (Some(8611), _) | (_, "scaledifference") => 6,
        _ => return None,
    })
}

/// Records `h` as `to_wgs84` on the CRS's horizontal datum (the first geographic/projected
/// component of a compound CRS).
pub(crate) fn apply_to_wgs84(crs: &mut Crs, h: [f64; 7]) {
    let datum = match crs.kind {
        CrsKind::Compound => crs
            .components
            .iter_mut()
            .find(|c| matches!(c.kind, CrsKind::Geographic | CrsKind::Projected))
            .and_then(|c| c.datum.as_mut()),
        _ => crs.datum.as_mut(),
    };
    if let Some(datum) = datum {
        datum.to_wgs84 = Some(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, value: f64, unit: Option<Unit>) -> RawParam {
        RawParam {
            name: name.into(),
            epsg: None,
            value: Some(value),
            unit: Ok(unit),
        }
    }

    fn ppm() -> Unit {
        Unit {
            name: "parts per million".into(),
            kind: UnitKind::Scale,
            factor: 1e-6,
        }
    }

    fn scale(params: &[RawParam], encoding: Encoding) -> Option<f64> {
        helmert_from_params("Position Vector transformation", params, encoding).map(|h| h[6])
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-9)
    }

    #[test]
    fn scale_difference_follows_the_unit_not_the_magnitude() {
        // 0.995 ppm is a realistic scale difference, not a multiplier.
        assert!(close(
            scale(&[p("Scale difference", 0.995, None)], Encoding::Full),
            0.995
        ));
        assert!(close(
            scale(&[p("Scale difference", 0.995, Some(ppm()))], Encoding::Full),
            0.995
        ));
        assert!(close(
            scale(
                &[p("Scale difference", 0.995, Some(ppm()))],
                Encoding::Abridged
            ),
            0.995
        ));
        // An explicit unity scale written as the multiplier 1 + dS.
        let unity = Some(Unit::unity());
        assert!(close(
            scale(
                &[p("Scale difference", 1.000_000_995, unity.clone())],
                Encoding::Full
            ),
            0.995
        ));
        // …or as dS itself.
        assert!(close(
            scale(&[p("Scale difference", 0.995e-6, unity)], Encoding::Full),
            0.995
        ));
        // Abridged WKT2 without unit: PROJ writes the multiplier.
        assert!(close(
            scale(
                &[p("Scale difference", 1.000_000_995, None)],
                Encoding::Abridged
            ),
            0.995
        ));
    }

    #[test]
    fn malformed_or_overflowing_parameters_drop_the_transformation() {
        let mut missing = p("X-axis translation", 1.0, None);
        missing.value = None;
        assert!(helmert_from_params("Position Vector", &[missing], Encoding::Full).is_none());
        let mut bad_unit = p("X-axis translation", 1.0, None);
        bad_unit.unit = Err(());
        assert!(helmert_from_params("Position Vector", &[bad_unit], Encoding::Full).is_none());
        let huge = Unit {
            name: "huge".into(),
            kind: UnitKind::Linear,
            factor: 1e300,
        };
        let overflow = p("X-axis translation", 1e300, Some(huge));
        assert!(helmert_from_params("Position Vector", &[overflow], Encoding::Full).is_none());
        assert!(
            helmert_from_params(
                "Molodensky",
                &[p("X-axis translation", 1.0, None)],
                Encoding::Full
            )
            .is_none()
        );
    }
}
