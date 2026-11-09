//! PROJJSON (PROJ's JSON encoding of WKT2:2019, used by GeoParquet) → [`Crs`].
//!
//! Schema: <https://proj.org/schemas/v0.7/projjson.schema.json>. Only the members needed for the
//! model are read; unknown members are ignored. `serde_json` bounds nesting (128 levels), and
//! [`crate::MAX_INPUT_LEN`] bounds the size.

use serde_json::{Map, Value};

use crate::helmert::{self, Encoding, RawParam};
use crate::names::{ParamName, ProjectionMethod, harmonize_params, known_unit, refine_method};
use crate::{
    Authority, Axis, AxisDirection, AxisOrder, BaseCrs, Crs, CrsError, CrsKind, Datum, Dialect,
    Ellipsoid, Parameter, PrimeMeridian, Projection, Unit, UnitKind,
};

type Obj = Map<String, Value>;

/// Parses a PROJJSON document. The text must already be within [`crate::MAX_INPUT_LEN`].
pub(crate) fn parse(text: &str) -> Result<Crs, CrsError> {
    let value: Value = serde_json::from_str(text).map_err(|e| CrsError::Json(e.to_string()))?;
    crs(&value)
}

fn json_err(message: impl Into<String>) -> CrsError {
    CrsError::Json(message.into())
}

fn object<'a>(v: &'a Value, what: &str) -> Result<&'a Obj, CrsError> {
    v.as_object()
        .ok_or_else(|| json_err(format!("{what} is not an object")))
}

fn req<'a>(o: &'a Obj, key: &str) -> Result<&'a Value, CrsError> {
    o.get(key)
        .ok_or_else(|| json_err(format!("missing `{key}`")))
}

fn req_str<'a>(o: &'a Obj, key: &str) -> Result<&'a str, CrsError> {
    req(o, key)?
        .as_str()
        .ok_or_else(|| json_err(format!("`{key}` is not a string")))
}

fn name(o: &Obj) -> Result<String, CrsError> {
    req_str(o, "name").map(str::to_owned)
}

/// `"id": {…}` or the first of `"ids": […]`.
fn authority(o: &Obj) -> Option<Authority> {
    let id = o
        .get("id")
        .or_else(|| {
            o.get("ids")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
        })?
        .as_object()?;
    let name = id.get("authority")?.as_str()?.to_owned();
    let code = match id.get("code")? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    Some(Authority { name, code })
}

fn crs(v: &Value) -> Result<Crs, CrsError> {
    let o = object(v, "CRS")?;
    let ty = req_str(o, "type")?;
    let mut crs = match ty {
        "GeographicCRS" | "GeodeticCRS" => geodetic(o, ty)?,
        "ProjectedCRS" => projected(o)?,
        "CompoundCRS" => compound(o)?,
        "VerticalCRS" => simple(o, CrsKind::Vertical)?,
        "EngineeringCRS" => simple(o, CrsKind::Engineering)?,
        "BoundCRS" => return bound(o),
        t if t.ends_with("CRS") => Crs::empty(CrsKind::Unknown, Dialect::ProjJson, name(o)?),
        other => return Err(CrsError::UnknownRoot(other.to_owned())),
    };
    crs.authority = authority(o);
    Ok(crs)
}

/// A unit: `"metre"`/`"degree"`/`"unity"` (or any well-known name), or
/// `{"type": "LinearUnit", "name": …, "conversion_factor": …}`.
fn unit(v: &Value, context: UnitKind) -> Result<Unit, CrsError> {
    match v {
        Value::String(s) => known_unit(s).ok_or_else(|| json_err(format!("unknown unit `{s}`"))),
        Value::Object(o) => {
            let name = name(o)?;
            let kind = match o.get("type").and_then(Value::as_str) {
                Some("LinearUnit") => UnitKind::Linear,
                Some("AngularUnit") => UnitKind::Angular,
                Some("ScaleUnit") => UnitKind::Scale,
                Some("TimeUnit") => UnitKind::Time,
                Some("ParametricUnit") => UnitKind::Parametric,
                _ => context,
            };
            let factor = match o.get("conversion_factor").and_then(Value::as_f64) {
                Some(f) => f,
                None => known_unit(&name)
                    .map(|u| u.factor)
                    .ok_or_else(|| json_err(format!("unit `{name}` has no conversion_factor")))?,
            };
            if !(factor.is_finite() && factor > 0.0) {
                return Err(CrsError::invalid(
                    "unit",
                    format!("`{name}` has conversion factor {factor}"),
                ));
            }
            Ok(Unit { name, kind, factor })
        }
        _ => Err(json_err("unit is neither a string nor an object")),
    }
}

/// A measure: a bare number (in `default` unit) or `{"value": …, "unit": …}`. Returns the value
/// and the unit it is in.
fn measure(v: &Value, default: &Unit) -> Result<(f64, Unit), CrsError> {
    match v {
        Value::Number(n) => n
            .as_f64()
            .map(|x| (x, default.clone()))
            .ok_or_else(|| json_err("number out of range")),
        Value::Object(o) => {
            let value = req(o, "value")?
                .as_f64()
                .ok_or_else(|| json_err("`value` is not a number"))?;
            let u = match o.get("unit") {
                Some(u) => unit(u, default.kind)?,
                None => default.clone(),
            };
            Ok((value, u))
        }
        _ => Err(json_err("measure is neither a number nor an object")),
    }
}

/// `"coordinate_system": {"subtype": …, "axis": [...]}` → (subtype, axes).
fn coordinate_system(o: &Obj, context: UnitKind) -> Result<(Option<String>, Vec<Axis>), CrsError> {
    let Some(cs) = o.get("coordinate_system") else {
        return Ok((None, Vec::new()));
    };
    let cs = object(cs, "coordinate_system")?;
    let subtype = cs
        .get("subtype")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let axes = match cs.get("axis").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .map(|a| {
                let a = object(a, "axis")?;
                Ok(Axis {
                    name: a
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    direction: AxisDirection::from_word(
                        a.get("direction")
                            .and_then(Value::as_str)
                            .unwrap_or("unspecified"),
                    ),
                    unit: a.get("unit").map(|u| unit(u, context)).transpose()?,
                })
            })
            .collect::<Result<Vec<_>, CrsError>>()?,
        None => Vec::new(),
    };
    Ok((subtype, axes))
}

fn set_axes(crs: &mut Crs, axes: Vec<Axis>) {
    crs.axis_order = AxisOrder::from_axes(&axes);
    crs.axes = axes;
}

fn first_axis_unit(crs: &Crs) -> Option<Unit> {
    crs.axes.iter().find_map(|a| a.unit.clone())
}

fn geodetic(o: &Obj, ty: &str) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(CrsKind::Geographic, Dialect::ProjJson, name(o)?);
    let (subtype, axes) = coordinate_system(o, UnitKind::Unknown)?;
    set_axes(&mut crs, axes);
    let unit = first_axis_unit(&crs);
    crs.kind = match (ty, subtype.as_deref()) {
        ("GeographicCRS", _) | (_, Some("ellipsoidal")) => CrsKind::Geographic,
        (_, Some("cartesian")) => CrsKind::Geocentric,
        (_, Some(_)) => CrsKind::Unknown,
        (_, None) => match unit.as_ref().map(|u| u.kind) {
            Some(UnitKind::Linear) => CrsKind::Geocentric,
            _ => CrsKind::Geographic,
        },
    };
    if crs.kind == CrsKind::Geographic {
        crs.angular_unit = Some(unit.unwrap_or_else(Unit::degree));
    } else {
        crs.linear_unit = unit;
    }
    crs.datum = Some(geodetic_datum(o)?);
    Ok(crs)
}

/// `"datum"` or `"datum_ensemble"` of a geodetic CRS.
fn geodetic_datum(o: &Obj) -> Result<Datum, CrsError> {
    let d = o
        .get("datum")
        .or_else(|| o.get("datum_ensemble"))
        .ok_or_else(|| json_err("missing `datum` / `datum_ensemble`"))?;
    let d = object(d, "datum")?;
    let e = object(req(d, "ellipsoid")?, "ellipsoid")?;
    let metre = Unit::metre();
    let ellipsoid_name = name(e)?;
    let (a, inverse_flattening) = if let Some(r) = e.get("radius") {
        let (v, u) = measure(r, &metre)?;
        (v * u.factor, 0.0)
    } else {
        let (v, u) = measure(req(e, "semi_major_axis")?, &metre)?;
        let a = v * u.factor;
        let invf = if let Some(invf) = e.get("inverse_flattening") {
            invf.as_f64()
                .ok_or_else(|| json_err("`inverse_flattening` is not a number"))?
        } else {
            let (v, u) = measure(req(e, "semi_minor_axis")?, &metre)?;
            let b = v * u.factor;
            if !(b.is_finite() && b > 0.0 && b <= a) {
                return Err(CrsError::invalid(
                    "ellipsoid",
                    format!("`{ellipsoid_name}` semi-minor axis {b}"),
                ));
            }
            if b == a { 0.0 } else { a / (a - b) }
        };
        (a, invf)
    };
    let ellipsoid = Ellipsoid::checked(ellipsoid_name, a, inverse_flattening, authority(e))?;

    let prime_meridian = match d.get("prime_meridian").or_else(|| o.get("prime_meridian")) {
        Some(pm) => {
            let pm = object(pm, "prime_meridian")?;
            let longitude = match pm.get("longitude") {
                Some(l) => {
                    let (v, u) = measure(l, &Unit::degree())?;
                    crate::model::finite("prime meridian longitude", u.to_degrees(v).unwrap_or(v))?
                }
                None => 0.0,
            };
            PrimeMeridian {
                name: name(pm)?,
                longitude,
                authority: authority(pm),
            }
        }
        None => PrimeMeridian::default(),
    };
    Ok(Datum {
        name: name(d)?,
        ellipsoid: Some(ellipsoid),
        prime_meridian,
        to_wgs84: None,
        authority: authority(d),
    })
}

fn projected(o: &Obj) -> Result<Crs, CrsError> {
    let base_obj = object(req(o, "base_crs")?, "base_crs")?;
    let base_type = base_obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("GeographicCRS");
    let base = geodetic(base_obj, base_type)?;
    let base_angular = base.angular_unit.clone().unwrap_or_else(Unit::degree);

    let mut crs = Crs::empty(CrsKind::Projected, Dialect::ProjJson, name(o)?);
    let (_, axes) = coordinate_system(o, UnitKind::Linear)?;
    set_axes(&mut crs, axes);
    crs.linear_unit = first_axis_unit(&crs);

    let conversion = object(req(o, "conversion")?, "conversion")?;
    let method = object(req(conversion, "method")?, "method")?;
    let method_name = name(method)?;
    let method_authority = authority(method);
    let mut parameters = match conversion.get("parameters").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .map(|p| parameter(p, &base_angular, crs.linear_unit.as_ref()))
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };
    let method_enum = method_authority
        .as_ref()
        .and_then(Authority::epsg)
        .and_then(ProjectionMethod::from_epsg)
        .unwrap_or_else(|| {
            refine_method(
                &method_name,
                ProjectionMethod::from_name(&method_name),
                &parameters,
                None,
            )
        });
    harmonize_params(method_enum, &mut parameters);
    crs.projection = Some(Projection {
        method: method_enum,
        method_name,
        method_authority,
        parameters,
    });
    crs.base = Some(BaseCrs {
        name: base.name,
        authority: authority(base_obj),
    });
    crs.datum = base.datum;
    crs.angular_unit = Some(base_angular);
    Ok(crs)
}

fn parameter(p: &Value, angular: &Unit, linear: Option<&Unit>) -> Result<Parameter, CrsError> {
    let p = object(p, "parameter")?;
    let original_name = name(p)?;
    let param_authority = authority(p);
    let name = param_authority
        .as_ref()
        .and_then(Authority::epsg)
        .and_then(ParamName::from_epsg)
        .unwrap_or_else(|| ParamName::from_name(&original_name));
    let value = req(p, "value")?
        .as_f64()
        .ok_or_else(|| json_err(format!("parameter `{original_name}` value is not a number")))?;
    let unit = match p.get("unit") {
        Some(u) => Some(unit(u, name.class().unit_kind())?),
        None => name.class().default_unit(angular, linear),
    };
    Parameter {
        name,
        original_name,
        value,
        unit,
        authority: param_authority,
    }
    .validated()
}

fn simple(o: &Obj, kind: CrsKind) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(kind, Dialect::ProjJson, name(o)?);
    let (_, axes) = coordinate_system(o, UnitKind::Linear)?;
    set_axes(&mut crs, axes);
    crs.linear_unit = first_axis_unit(&crs);
    crs.datum = match o.get("datum").or_else(|| o.get("datum_ensemble")) {
        Some(d) => {
            let d = object(d, "datum")?;
            Some(Datum {
                name: name(d)?,
                ellipsoid: None,
                prime_meridian: PrimeMeridian::default(),
                to_wgs84: None,
                authority: authority(d),
            })
        }
        None => None,
    };
    Ok(crs)
}

fn compound(o: &Obj) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(CrsKind::Compound, Dialect::ProjJson, name(o)?);
    let list = req(o, "components")?
        .as_array()
        .ok_or_else(|| json_err("`components` is not an array"))?;
    crs.components = list.iter().map(self::crs).collect::<Result<Vec<_>, _>>()?;
    if crs.components.is_empty() {
        return Err(CrsError::missing("component CRS", "CompoundCRS"));
    }
    Ok(crs)
}

/// `BoundCRS` → the source CRS with a Helmert transformation to WGS 84 as `to_wgs84`.
fn bound(o: &Obj) -> Result<Crs, CrsError> {
    let mut crs = self::crs(req(o, "source_crs")?)?;
    let target = object(req(o, "target_crs")?, "target_crs")?;
    let target_is_wgs84 = helmert::is_wgs84(
        authority(target).and_then(|a| a.epsg()),
        target.get("name").and_then(Value::as_str).unwrap_or(""),
    );
    let transformation = o.get("transformation").and_then(Value::as_object);
    if let (true, Some(t)) = (target_is_wgs84, transformation) {
        let method = t
            .get("method")
            .and_then(Value::as_object)
            .and_then(|m| m.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        // A malformed parameter list or parameter drops the transformation (see `helmert`).
        let params: Option<Vec<RawParam>> =
            t.get("parameters")
                .and_then(Value::as_array)
                .and_then(|list| {
                    list.iter()
                        .map(|p| {
                            let p = p.as_object()?;
                            Some(RawParam {
                                name: p
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_owned(),
                                epsg: authority(p).and_then(|a| a.epsg()),
                                value: p.get("value").and_then(Value::as_f64),
                                unit: p
                                    .get("unit")
                                    .map(|u| unit(u, UnitKind::Unknown))
                                    .transpose()
                                    .map_err(|_| ()),
                            })
                        })
                        .collect()
                });
        if let Some(h) =
            params.and_then(|ps| helmert::helmert_from_params(method, &ps, Encoding::Full))
        {
            helmert::apply_to_wgs84(&mut crs, h);
        }
    }
    Ok(crs)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn minimal_geographic() {
        let crs = parse(
            r#"{"type":"GeographicCRS","name":"WGS 84","datum":{"type":"GeodeticReferenceFrame","name":"World Geodetic System 1984","ellipsoid":{"name":"WGS 84","semi_major_axis":6378137,"inverse_flattening":298.257223563}},"coordinate_system":{"subtype":"ellipsoidal","axis":[{"name":"Latitude","abbreviation":"lat","direction":"north","unit":"degree"},{"name":"Longitude","abbreviation":"lon","direction":"east","unit":"degree"}]},"id":{"authority":"EPSG","code":4326}}"#,
        )
        .unwrap();
        assert_eq!(crs.kind, CrsKind::Geographic);
        assert_eq!(crs.dialect, Dialect::ProjJson);
        assert_eq!(crs.epsg(), Some(4326));
        assert_eq!(crs.axis_order, AxisOrder::NorthEast);
        assert!(crs.angular_unit.as_ref().unwrap().is_degree());
    }

    const OSGB: &str = r#"{"type":"GeographicCRS","name":"OSGB 1936","datum":{"name":"OSGB 1936","ellipsoid":{"name":"Airy 1830","semi_major_axis":6377563.396,"inverse_flattening":299.3249646}}}"#;
    const WGS84: &str = r#"{"type":"GeographicCRS","name":"WGS 84","datum":{"name":"WGS 84","ellipsoid":{"name":"WGS 84","semi_major_axis":6378137,"inverse_flattening":298.257223563}},"id":{"authority":"EPSG","code":4326}}"#;

    fn bound_with(params: &str) -> Crs {
        parse(&format!(
            r#"{{"type":"BoundCRS","source_crs":{OSGB},"target_crs":{WGS84},"transformation":{{"name":"t","method":{{"name":"Position Vector transformation (geog2D domain)"}},"parameters":[{{"name":"X-axis translation","value":446.448,"unit":"metre"}},{params}]}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn bound_scale_difference_and_malformed_parameters() {
        // No unit on a (non-abridged) PROJJSON scale difference: ppm, whatever its magnitude.
        let crs = bound_with(r#"{"name":"Scale difference","value":0.995}"#);
        let t = crs.datum.unwrap().to_wgs84.unwrap();
        assert!((t[6] - 0.995).abs() < 1e-9, "{t:?}");
        // Explicit unity multiplier.
        let crs = bound_with(r#"{"name":"Scale difference","value":1.000000995,"unit":"unity"}"#);
        let t = crs.datum.unwrap().to_wgs84.unwrap();
        assert!((t[6] - 0.995).abs() < 1e-9, "{t:?}");
        // Malformed parameters drop the transformation but keep the source CRS.
        for bad in [
            r#"{"name":"Y-axis translation","value":"abc"}"#,
            r#"{"name":"Y-axis translation"}"#,
            r#"{"name":"Y-axis translation","value":1,"unit":"furlong"}"#,
            r#"{"name":"Y-axis translation","value":1e300,"unit":{"type":"LinearUnit","name":"huge","conversion_factor":1e300}}"#,
            r#"42"#,
        ] {
            let crs = bound_with(bad);
            assert_eq!(crs.kind, CrsKind::Geographic, "{bad}");
            assert_eq!(crs.name, "OSGB 1936");
            assert_eq!(crs.datum.unwrap().to_wgs84, None, "{bad}");
        }
    }

    #[test]
    fn overflowing_unit_products_are_rejected() {
        for bad in [
            r#"{"type":"GeographicCRS","name":"x","datum":{"name":"d","ellipsoid":{"name":"e","semi_major_axis":6378137,"inverse_flattening":298.257},"prime_meridian":{"name":"p","longitude":{"value":1e300,"unit":{"type":"AngularUnit","name":"u","conversion_factor":1e300}}}}}"#,
            r#"{"type":"GeographicCRS","name":"x","datum":{"name":"d","ellipsoid":{"name":"e","semi_major_axis":{"value":1e300,"unit":{"type":"LinearUnit","name":"u","conversion_factor":1e300}},"inverse_flattening":298.257}}}"#,
            r#"{"type":"ProjectedCRS","name":"x","base_crs":{"name":"b","datum":{"name":"d","ellipsoid":{"name":"e","semi_major_axis":6378137,"inverse_flattening":298.257}}},"conversion":{"name":"c","method":{"name":"Transverse Mercator"},"parameters":[{"name":"False easting","value":1e300,"unit":{"type":"LinearUnit","name":"u","conversion_factor":1e300}}]}}"#,
        ] {
            assert!(
                matches!(parse(bad), Err(CrsError::Invalid { .. })),
                "{bad}: {:?}",
                parse(bad)
            );
        }
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "[]",
            "{}",
            r#"{"type":"Banana"}"#,
            r#"{"type":"GeographicCRS","name":"x"}"#,
            r#"{"type":"GeographicCRS","name":"x","datum":{"name":"d","ellipsoid":{"name":"e","semi_major_axis":-1,"inverse_flattening":298}}}"#,
            r#"{"type":"GeographicCRS","name":"x","datum":{"name":"d","ellipsoid":{"name":"e","semi_major_axis":6378137,"semi_minor_axis":7000000}}}"#,
            r#"{"type":"ProjectedCRS","name":"x"}"#,
            r#"{"type":"CompoundCRS","name":"x","components":[]}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad} should fail");
        }
    }
}
