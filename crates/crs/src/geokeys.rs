//! GeoTIFF GeoKeys → the shared CRS model (stage01/3.2).
//!
//! Raw `CrsDefinitionKind::GeoKeys` text is JSON with `directory` (the complete SHORT array),
//! `doubles` (GeoDoubleParamsTag), and `ascii` (GeoAsciiParamsTag, including its trailing NUL).
//! This preserves keys without making the reader interpret the CRS. Offsets are array indices
//! or ASCII byte offsets, never file offsets. See OGC GeoTIFF 1.1, 19-008r4.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    AxisOrder, BaseCrs, Crs, CrsError, CrsKind, Ellipsoid, ParamClass, ParamName, Parameter,
    Projection, ProjectionMethod, Unit, UnitKind, epsg_crs,
};

enum Key {
    Short(u16),
    Double(f64),
    Text(String),
}

/// Resolves raw GeoKey JSON to the catalog representation and the model used for transforms.
/// This is the GeoTIFF entry point alongside the text-format [`crate::resolve`].
pub fn resolve(text: &str) -> crate::Resolution {
    match parse(text) {
        Err(error) => crate::Resolution {
            resolved: whereabouts_core::ResolvedCrs::Unparsed,
            crs: None,
            identification: None,
            error: Some(error),
        },
        Ok(crs) => {
            let identification = crate::identify(&crs);
            let resolved = match &identification {
                Some(id) => whereabouts_core::ResolvedCrs::Identified {
                    epsg: id.epsg,
                    name: Some(id.name.clone()),
                },
                None => whereabouts_core::ResolvedCrs::Unidentified {
                    name: (!crs.name.is_empty()).then(|| crs.name.clone()),
                },
            };
            crate::Resolution {
                resolved,
                crs: Some(crs),
                identification,
                error: None,
            }
        }
    }
}

fn invalid(message: impl Into<String>) -> CrsError {
    CrsError::invalid("GeoKeys", message)
}

/// Parses the bounded, lossless JSON GeoKey representation emitted by the TIFF reader.
/// Unknown/user-defined projection methods return an error; no CRS is guessed.
pub fn parse(text: &str) -> Result<Crs, CrsError> {
    crate::check_len(text.len())?;
    let value: Value = serde_json::from_str(text).map_err(|e| invalid(e.to_string()))?;
    let directory = value["directory"]
        .as_array()
        .ok_or_else(|| invalid("missing directory"))?;
    let directory: Vec<u16> = directory
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| n.try_into().ok())
                .ok_or_else(|| invalid("directory is not SHORT"))
        })
        .collect::<Result<_, _>>()?;
    if directory.len() < 4 || directory[0] != 1 || directory[1] != 1 || directory[2] > 1 {
        return Err(invalid("unsupported/truncated directory header"));
    }
    let end = 4 + usize::from(directory[3]) * 4;
    if end > directory.len() || directory[3] > 1024 {
        return Err(invalid("directory key count"));
    }
    let doubles = value["doubles"]
        .as_array()
        .ok_or_else(|| invalid("missing doubles"))?;
    let ascii = value["ascii"]
        .as_str()
        .ok_or_else(|| invalid("missing ascii"))?;
    let mut keys = BTreeMap::new();
    for entry in directory[4..end].as_chunks::<4>().0 {
        let (id, loc, count, offset) = (
            entry[0],
            entry[1],
            usize::from(entry[2]),
            usize::from(entry[3]),
        );
        if count == 0 {
            return Err(invalid("zero key count"));
        }
        let end = offset
            .checked_add(count)
            .ok_or_else(|| invalid("key range overflow"))?;
        let key = match loc {
            0 if count == 1 => Key::Short(entry[3]),
            34735 if count == 1 && offset >= 4 + usize::from(directory[3]) * 4 => Key::Short(
                *directory
                    .get(offset)
                    .ok_or_else(|| invalid("SHORT offset"))?,
            ),
            34736 if count == 1 => Key::Double(
                doubles
                    .get(offset)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| invalid("double offset/value"))?,
            ),
            34737 => Key::Text(
                ascii
                    .get(offset..end)
                    .ok_or_else(|| invalid("ASCII range"))?
                    .trim_end_matches(['|', '\0'])
                    .to_owned(),
            ),
            // Unknown keys are still range-checked; supported numeric keys must be scalar.
            34735 if end <= directory.len() => return Err(invalid("non-scalar SHORT key")),
            34736 if end <= doubles.len() => return Err(invalid("non-scalar double key")),
            _ => return Err(invalid("key location/count/range")),
        };
        if (matches!(id,1024 | 1025 | 2048 | 2050..=2054 | 2056 | 2060 | 3072 | 3074..=3076)
            && !matches!(&key, Key::Short(_)))
            || (matches!(id,2055 | 2057..=2059 | 2061 | 3077..=3096)
                && !matches!(&key, Key::Double(_)))
            || (matches!(id, 1026 | 2049 | 3073) && !matches!(&key, Key::Text(_)))
        {
            return Err(invalid(format!("GeoKey {id} has the wrong value type")));
        }
        if keys.insert(id, key).is_some() {
            return Err(invalid("duplicate GeoKey"));
        }
    }
    let short = |id| match keys.get(&id) {
        Some(Key::Short(v)) => Some(*v),
        _ => None,
    };
    let number = |id| match keys.get(&id) {
        Some(Key::Double(v)) => Some(*v),
        _ => None,
    };
    let name = |id| match keys.get(&id) {
        Some(Key::Text(s)) => Some(s.clone()),
        _ => None,
    };
    let model = short(1024).ok_or_else(|| invalid("missing model type"))?;
    let code = match model {
        1 => short(3072),
        2 => short(2048),
        _ => return Err(invalid("unsupported model type")),
    };
    if let Some(code) = code.filter(|c| *c != 0 && *c != 32767) {
        return epsg_crs(u32::from(code))
            .filter(|c| (model == 1) == (c.kind == CrsKind::Projected))
            .ok_or_else(|| invalid("unknown/mismatched EPSG CRS"));
    }
    let geog = short(2048)
        .filter(|c| *c != 0 && *c != 32767)
        .map(u32::from)
        .or_else(|| match short(2050) {
            Some(6326) => Some(4326),
            Some(6269) => Some(4269),
            Some(6267) => Some(4267),
            Some(6258) => Some(4258),
            _ => None,
        });
    let mut crs = if let Some(code) = geog {
        epsg_crs(code)
            .filter(|c| c.kind == CrsKind::Geographic)
            .ok_or_else(|| invalid("unknown geographic base"))?
    } else {
        let (a, rf) = match short(2056) {
            Some(7030) => (6378137.0, 298.257223563),
            Some(7019) => (6378137.0, 298.257222101),
            _ => {
                let a = number(2057).ok_or_else(|| invalid("missing semi-major axis"))?;
                let rf = number(2059)
                    .or_else(|| number(2058).map(|b| if a == b { 0.0 } else { a / (a - b) }))
                    .ok_or_else(|| invalid("missing flattening"))?;
                (a, rf)
            }
        };
        let mut c = epsg_crs(4326).ok_or_else(|| invalid("missing WGS84 table entry"))?;
        let d = c.datum.as_mut().ok_or_else(|| invalid("missing datum"))?;
        d.name = name(2049).unwrap_or_else(|| "User-defined datum".into());
        d.authority = None;
        d.to_wgs84 = None;
        d.ellipsoid = Some(Ellipsoid::checked(
            "User-defined ellipsoid".into(),
            a,
            rf,
            None,
        )?);
        c
    };
    crs.authority = None;
    crs.axes.clear();
    crs.axis_order = AxisOrder::Unspecified;
    crs.name = name(if model == 1 { 3073 } else { 2049 })
        .unwrap_or_else(|| "User-defined GeoTIFF CRS".into());
    let angular = match short(2054).unwrap_or(9102) {
        9101 => Unit {
            name: "radian".into(),
            kind: UnitKind::Angular,
            factor: 1.0,
        },
        9102 => Unit::degree(),
        9105 | 9106 => Unit {
            name: "grad".into(),
            kind: UnitKind::Angular,
            factor: std::f64::consts::PI / 200.0,
        },
        32767 => checked_unit(UnitKind::Angular, number(2055))?,
        _ => return Err(invalid("unsupported angular unit")),
    };
    crs.angular_unit = Some(angular.clone());
    if let Some(pm) = number(2061) {
        let datum = crs.datum.as_mut().ok_or_else(|| invalid("missing datum"))?;
        datum.prime_meridian.longitude = angular
            .to_degrees(pm)
            .ok_or_else(|| invalid("prime meridian"))?;
        datum.prime_meridian.authority = None;
    } else if short(2051).is_some_and(|c| c != 8901) {
        return Err(invalid("unsupported prime meridian"));
    }
    if model == 2 {
        return Ok(crs);
    }
    let linear = match short(3076).unwrap_or(9001) {
        9001 => Unit::metre(),
        9002 => Unit {
            name: "foot".into(),
            kind: UnitKind::Linear,
            factor: 0.3048,
        },
        9003 => Unit {
            name: "US survey foot".into(),
            kind: UnitKind::Linear,
            factor: 1200.0 / 3937.0,
        },
        32767 => checked_unit(UnitKind::Linear, number(3077))?,
        _ => return Err(invalid("unsupported linear unit")),
    };
    use ProjectionMethod as M;
    let method = match short(3075) {
        Some(1) => M::TransverseMercator,
        Some(3) => M::HotineObliqueMercatorVariantA,
        Some(7) => {
            if number(3078).is_some() {
                M::MercatorVariantB
            } else {
                M::MercatorVariantA
            }
        }
        Some(8) => M::LambertConicConformal2Sp,
        Some(9) => M::LambertConicConformal1Sp,
        Some(10) => M::LambertAzimuthalEqualArea,
        Some(11) => M::AlbersEqualArea,
        Some(14) => M::ObliqueStereographic,
        Some(15) => M::PolarStereographicVariantA,
        Some(16) => M::ObliqueStereographic,
        Some(17) => M::EquidistantCylindrical,
        Some(18) => M::CassiniSoldner,
        Some(20) => M::MillerCylindrical,
        Some(24) => M::Sinusoidal,
        Some(27) => M::TransverseMercatorSouthOrientated,
        Some(28) => M::LambertCylindricalEqualArea,
        _ => return Err(invalid("unsupported/missing projection method")),
    };
    use ParamName as P;
    let mut parameters = Vec::new();
    for (ids, param) in [
        (&[3078][..], P::StandardParallel1),
        (&[3079][..], P::StandardParallel2),
        (&[3080, 3084, 3088, 3095][..], P::LongitudeOfOrigin),
        (&[3081, 3085, 3089][..], P::LatitudeOfOrigin),
        (&[3082, 3086, 3090][..], P::FalseEasting),
        (&[3083, 3087, 3091][..], P::FalseNorthing),
        (&[3092, 3093][..], P::ScaleFactor),
        (&[3094][..], P::Azimuth),
        (&[3096][..], P::RectifiedGridAngle),
    ] {
        if let Some(value) = ids.iter().find_map(|id| number(*id)) {
            let unit = match param.class() {
                ParamClass::Angular => angular.clone(),
                ParamClass::Linear => linear.clone(),
                _ => Unit::unity(),
            };
            parameters.push(
                Parameter {
                    name: param,
                    original_name: format!("{param:?}"),
                    value,
                    unit: Some(unit),
                    authority: None,
                }
                .validated()?,
            );
        }
    }
    crs.kind = CrsKind::Projected;
    crs.base = Some(BaseCrs {
        name: "GeoTIFF geographic base".into(),
        authority: geog.map(|c| crate::Authority {
            name: "EPSG".into(),
            code: c.to_string(),
        }),
    });
    crs.linear_unit = Some(linear);
    crs.projection = Some(Projection {
        method,
        method_name: format!("{method:?}"),
        method_authority: None,
        parameters,
    });
    Ok(crs)
}

fn checked_unit(kind: UnitKind, factor: Option<f64>) -> Result<Unit, CrsError> {
    Ok(Unit {
        name: "user-defined".into(),
        kind,
        factor: factor
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or_else(|| invalid("invalid/missing unit scale"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(keys: &[[u16; 4]], doubles: &[f64], ascii: &str) -> String {
        let mut directory = vec![1, 1, 0, keys.len() as u16];
        directory.extend(keys.iter().flatten());
        serde_json::json!({"directory":directory,"doubles":doubles,"ascii":ascii}).to_string()
    }
    #[test]
    fn direct_codes_and_invalid_ranges() {
        for (model, key, code) in [(1, 3072, 32633), (2, 2048, 4326), (1, 3072, 2193)] {
            assert_eq!(
                parse(&text(&[[1024, 0, 1, model], [key, 0, 1, code]], &[], ""))
                    .expect("code")
                    .epsg(),
                Some(u32::from(code))
            );
        }
        for keys in [
            vec![[1024, 0, 1, 1], [3073, 34737, 4, 65535]],
            vec![[1024, 0, 1, 1], [3080, 34736, 1, 2]],
            vec![[1024, 0, 1, 1], [1024, 0, 1, 2]],
        ] {
            assert!(parse(&text(&keys, &[], "xx")).is_err());
        }
    }
    #[test]
    fn user_defined_utm_identifies_by_parameters() {
        let keys = [
            [1024, 0, 1, 1],
            [3072, 0, 1, 32767],
            [2048, 0, 1, 4326],
            [3075, 0, 1, 1],
            [3076, 0, 1, 9001],
            [3080, 34736, 1, 0],
            [3081, 34736, 1, 1],
            [3092, 34736, 1, 2],
            [3082, 34736, 1, 3],
            [3083, 34736, 1, 4],
        ];
        let c = parse(&text(&keys, &[15.0, 0.0, 0.9996, 500000.0, 0.0], "")).expect("custom TM");
        assert_eq!(crate::identify(&c).map(|i| i.epsg), Some(32633));
    }
}
