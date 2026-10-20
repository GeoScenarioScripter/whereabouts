//! WKT2 (ISO 19162:2015 / OGC 12-063r5 and ISO 19162:2019 / OGC 18-010r7) → [`Crs`].
//!
//! Handles the CRS types that matter for georeferencing files: geodetic/geographic, projected,
//! vertical, engineering, compound and bound (`BOUNDCRS` — its Helmert transformation to WGS 84
//! becomes [`crate::Datum::to_wgs84`]). Other `*CRS` keywords parse to [`CrsKind::Unknown`].
//! Usage/scope/area/remarks are ignored.

use super::tree::Node;
use super::{authority, axes, ellipsoid, req_name, req_number, unit_child};
use crate::names::{
    ParamClass, ParamName, ProjectionMethod, harmonize_params, name_key, refine_method,
};
use crate::{
    AxisOrder, BaseCrs, Crs, CrsError, CrsKind, Datum, Dialect, Parameter, PrimeMeridian,
    Projection, Unit, UnitKind,
};

const GEODETIC: &[&str] = &["GEODCRS", "GEODETICCRS", "GEOGCRS", "GEOGRAPHICCRS"];
const PROJECTED: &[&str] = &["PROJCRS", "PROJECTEDCRS"];
const VERTICAL: &[&str] = &["VERTCRS", "VERTICALCRS"];
const ENGINEERING: &[&str] = &["ENGCRS", "ENGINEERINGCRS"];
const GEODETIC_DATUM: &[&str] = &["DATUM", "GEODETICDATUM", "TRF", "ENSEMBLE"];
const VERTICAL_DATUM: &[&str] = &["VDATUM", "VERTICALDATUM", "VRF", "ENSEMBLE"];
const ENGINEERING_DATUM: &[&str] = &["EDATUM", "ENGINEERINGDATUM"];

/// Whether `keyword` (upper-case) is a WKT2 CRS keyword.
pub(super) fn is_root(keyword: &str) -> bool {
    keyword.ends_with("CRS")
}

/// WKT2:2019 if any construct introduced in 2019 appears, else 2015.
pub(super) fn dialect(root: &Node) -> Dialect {
    const ONLY_2019: &[&str] = &[
        "GEOGCRS",
        "GEOGRAPHICCRS",
        "BASEGEOGCRS",
        "USAGE",
        "ENSEMBLE",
        "DYNAMIC",
        "TRF",
        "VRF",
        "GEOIDMODEL",
        "ANCHOREPOCH",
        "DERIVEDPROJCRS",
        "COORDINATEMETADATA",
        "POINTMOTIONOPERATION",
    ];
    let mut is_2019 = false;
    root.walk(&mut |n| is_2019 |= ONLY_2019.contains(&n.keyword.as_str()));
    if is_2019 {
        Dialect::Wkt2_2019
    } else {
        Dialect::Wkt2_2015
    }
}

/// Interprets one WKT2 CRS node.
pub(super) fn crs(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let kw = node.keyword.as_str();
    if GEODETIC.contains(&kw) {
        geodetic(node, dialect)
    } else if PROJECTED.contains(&kw) {
        projected(node, dialect)
    } else if VERTICAL.contains(&kw) {
        simple(node, dialect, CrsKind::Vertical, VERTICAL_DATUM)
    } else if ENGINEERING.contains(&kw) {
        simple(node, dialect, CrsKind::Engineering, ENGINEERING_DATUM)
    } else if kw == "COMPOUNDCRS" {
        compound(node, dialect)
    } else if kw == "BOUNDCRS" {
        bound(node, dialect)
    } else if is_root(kw) {
        let mut crs = Crs::empty(CrsKind::Unknown, dialect, req_name(node)?);
        crs.authority = authority(node);
        Ok(crs)
    } else {
        Err(CrsError::UnknownRoot(kw.to_owned()))
    }
}

/// CRS-level unit (after the axes) or, failing that, the first axis's unit.
fn cs_unit(node: &Node, crs: &Crs, kind: UnitKind) -> Result<Option<Unit>, CrsError> {
    Ok(unit_child(node, kind)?.or_else(|| crs.axes.iter().find_map(|a| a.unit.clone())))
}

fn finish_axes(crs: &mut Crs, node: &Node, kind: UnitKind) -> Result<(), CrsError> {
    crs.axes = axes(node, kind)?;
    crs.axis_order = AxisOrder::from_axes(&crs.axes);
    crs.authority = authority(node);
    Ok(())
}

/// The `CS[type, dim]` type word, lower-cased.
fn cs_type(node: &Node) -> Option<String> {
    node.child(&["CS"])
        .and_then(Node::name)
        .map(str::to_ascii_lowercase)
}

fn geodetic(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(CrsKind::Geographic, dialect, req_name(node)?);
    finish_axes(&mut crs, node, UnitKind::Angular)?;
    let unit = cs_unit(node, &crs, UnitKind::Unknown)?;
    crs.kind = match (node.keyword.as_str(), cs_type(node).as_deref()) {
        ("GEOGCRS" | "GEOGRAPHICCRS", _) | (_, Some("ellipsoidal")) => CrsKind::Geographic,
        (_, Some("cartesian")) => CrsKind::Geocentric,
        (_, Some(_)) => CrsKind::Unknown,
        // No CS (sloppy 2015 writers): decide by the unit.
        (_, None) => match unit.as_ref().map(|u| u.kind) {
            Some(UnitKind::Linear) => CrsKind::Geocentric,
            _ => CrsKind::Geographic,
        },
    };
    let angular_fallback = match crs.kind {
        CrsKind::Geographic => {
            let u = with_kind(unit, UnitKind::Angular).unwrap_or_else(Unit::degree);
            crs.angular_unit = Some(u.clone());
            u
        }
        _ => {
            crs.linear_unit = with_kind(unit, UnitKind::Linear);
            Unit::degree()
        }
    };
    crs.datum = Some(geodetic_datum(node, &angular_fallback)?);
    Ok(crs)
}

/// A generic `UNIT` takes the kind its context implies.
fn with_kind(unit: Option<Unit>, kind: UnitKind) -> Option<Unit> {
    unit.map(|mut u| {
        if u.kind == UnitKind::Unknown {
            u.kind = kind;
        }
        u
    })
}

/// Datum (or ensemble) + `PRIMEM` children of a geodetic CRS / base CRS. The prime meridian
/// longitude is converted to degrees using its own unit, else `angular_fallback`.
fn geodetic_datum(crs: &Node, angular_fallback: &Unit) -> Result<Datum, CrsError> {
    let node = crs
        .child(GEODETIC_DATUM)
        .ok_or_else(|| CrsError::missing("DATUM", crs.keyword.clone()))?;
    let ellipsoid_node = node
        .child(&["ELLIPSOID", "SPHEROID"])
        .ok_or_else(|| CrsError::missing("ELLIPSOID", node.keyword.clone()))?;
    // PRIMEM is a sibling of DATUM in WKT2 (inside ENSEMBLE in some writers).
    let prime_meridian = match crs
        .child(&["PRIMEM", "PRIMEMERIDIAN"])
        .or_else(|| node.child(&["PRIMEM", "PRIMEMERIDIAN"]))
    {
        Some(pm) => {
            let value = req_number(pm, 1, "prime meridian longitude")?;
            let unit = with_kind(unit_child(pm, UnitKind::Angular)?, UnitKind::Angular)
                .unwrap_or_else(|| angular_fallback.clone());
            PrimeMeridian {
                name: req_name(pm)?,
                longitude: unit.to_degrees(value).unwrap_or(value),
                authority: authority(pm),
            }
        }
        None => PrimeMeridian::default(),
    };
    Ok(Datum {
        name: req_name(node)?,
        ellipsoid: Some(ellipsoid(ellipsoid_node)?),
        prime_meridian,
        to_wgs84: None,
        authority: authority(node),
    })
}

fn projected(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let name = req_name(node)?;
    let base_node = node
        .child(&["BASEGEOGCRS", "BASEGEODCRS"])
        .ok_or_else(|| CrsError::missing("BASEGEOGCRS", format!("PROJCRS `{name}`")))?;
    // The base CRS's angular unit: its own unit if written, else its prime meridian's unit
    // (PROJ omits the base CS but keeps `PRIMEM[…, ANGLEUNIT["grad", …]]`), else degrees.
    let primem_unit = match base_node.child(&["PRIMEM", "PRIMEMERIDIAN"]) {
        Some(pm) => unit_child(pm, UnitKind::Angular)?,
        None => None,
    };
    let base_angular = with_kind(
        unit_child(base_node, UnitKind::Angular)?.or(primem_unit),
        UnitKind::Angular,
    )
    .unwrap_or_else(Unit::degree);
    let datum = geodetic_datum(base_node, &base_angular)?;

    let mut crs = Crs::empty(CrsKind::Projected, dialect, name);
    finish_axes(&mut crs, node, UnitKind::Linear)?;
    crs.linear_unit = with_kind(cs_unit(node, &crs, UnitKind::Linear)?, UnitKind::Linear);

    let conversion = node
        .child(&["CONVERSION"])
        .ok_or_else(|| CrsError::missing("CONVERSION", format!("PROJCRS `{}`", crs.name)))?;
    crs.projection = Some(projection(
        conversion,
        &base_angular,
        crs.linear_unit.as_ref(),
    )?);
    crs.base = Some(BaseCrs {
        name: req_name(base_node)?,
        authority: authority(base_node),
    });
    crs.datum = Some(datum);
    crs.angular_unit = Some(base_angular);
    Ok(crs)
}

/// `CONVERSION["name", METHOD[…], PARAMETER[…]…]`. Parameters without an explicit unit inherit
/// the base CRS's angular unit / the projected CRS's linear unit, as in WKT1.
fn projection(
    conversion: &Node,
    angular: &Unit,
    linear: Option<&Unit>,
) -> Result<Projection, CrsError> {
    let method_node = conversion
        .child(&["METHOD", "PROJECTION"])
        .ok_or_else(|| CrsError::missing("METHOD", "CONVERSION"))?;
    let method_name = req_name(method_node)?;
    let method_authority = authority(method_node);

    let mut parameters = conversion
        .children_named(&["PARAMETER"])
        .map(|p| {
            let original_name = req_name(p)?;
            let param_authority = authority(p);
            let name = param_authority
                .as_ref()
                .and_then(crate::Authority::epsg)
                .and_then(ParamName::from_epsg)
                .unwrap_or_else(|| ParamName::from_name(&original_name));
            let context = match name.class() {
                ParamClass::Angular => UnitKind::Angular,
                ParamClass::Linear => UnitKind::Linear,
                ParamClass::Scale => UnitKind::Scale,
                ParamClass::Unknown => UnitKind::Unknown,
            };
            let unit = match unit_child(p, context)? {
                Some(u) => Some(u),
                None => match name.class() {
                    ParamClass::Angular => Some(angular.clone()),
                    ParamClass::Linear => linear.cloned(),
                    ParamClass::Scale => Some(Unit::unity()),
                    ParamClass::Unknown => None,
                },
            };
            Ok(Parameter {
                name,
                value: req_number(p, 1, "parameter value")?,
                original_name,
                unit,
                authority: param_authority,
            })
        })
        .collect::<Result<Vec<_>, CrsError>>()?;

    let by_code = method_authority
        .as_ref()
        .and_then(crate::Authority::epsg)
        .and_then(ProjectionMethod::from_epsg);
    let method = by_code.unwrap_or_else(|| {
        refine_method(
            &method_name,
            ProjectionMethod::from_name(&method_name),
            &parameters,
            None,
        )
    });
    harmonize_params(method, &mut parameters);
    Ok(Projection {
        method,
        method_name,
        method_authority,
        parameters,
    })
}

/// Vertical and engineering CRSs: a datum without ellipsoid, a CS, a linear unit.
fn simple(
    node: &Node,
    dialect: Dialect,
    kind: CrsKind,
    datum_keywords: &[&str],
) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(kind, dialect, req_name(node)?);
    finish_axes(&mut crs, node, UnitKind::Linear)?;
    crs.linear_unit = with_kind(cs_unit(node, &crs, UnitKind::Linear)?, UnitKind::Linear);
    crs.datum = node
        .child(datum_keywords)
        .map(|d| {
            Ok::<_, CrsError>(Datum {
                name: req_name(d)?,
                ellipsoid: None,
                prime_meridian: PrimeMeridian::default(),
                to_wgs84: None,
                authority: authority(d),
            })
        })
        .transpose()?;
    Ok(crs)
}

fn is_crs_node(n: &Node) -> bool {
    is_root(&n.keyword) && !n.keyword.starts_with("BASE")
}

fn compound(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(CrsKind::Compound, dialect, req_name(node)?);
    crs.components = node
        .children()
        .filter(|n| is_crs_node(n))
        .map(|c| self::crs(c, dialect))
        .collect::<Result<Vec<_>, _>>()?;
    if crs.components.is_empty() {
        return Err(CrsError::missing("component CRS", "COMPOUNDCRS"));
    }
    crs.authority = authority(node);
    Ok(crs)
}

/// `BOUNDCRS[SOURCECRS[…], TARGETCRS[…], ABRIDGEDTRANSFORMATION[…]]` → the source CRS, with a
/// Helmert transformation to WGS 84 recorded as `to_wgs84` on its (horizontal) datum.
fn bound(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let source = node
        .child(&["SOURCECRS"])
        .and_then(|s| s.children().find(|n| is_crs_node(n)))
        .ok_or_else(|| CrsError::missing("SOURCECRS", "BOUNDCRS"))?;
    let mut crs = self::crs(source, dialect)?;
    let target_is_wgs84 = node
        .child(&["TARGETCRS"])
        .and_then(|t| t.children().find(|n| is_crs_node(n)))
        .is_some_and(|t| {
            authority(t).and_then(|a| a.epsg()) == Some(4326)
                || name_key(t.name().unwrap_or("")) == "wgs84"
        });
    let helmert = node
        .child(&["ABRIDGEDTRANSFORMATION"])
        .and_then(|t| helmert_to_wgs84(t).transpose())
        .transpose()?;
    if let (true, Some(h)) = (target_is_wgs84, helmert) {
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
    Ok(crs)
}

/// Reads a 3- or 7-parameter Helmert transformation into TOWGS84 conventions (metres,
/// arc-seconds, ppm, position-vector rotation signs). Unknown methods give `None`.
fn helmert_to_wgs84(t: &Node) -> Result<Option<[f64; 7]>, CrsError> {
    let method = t
        .child(&["METHOD"])
        .and_then(Node::name)
        .map(name_key)
        .unwrap_or_default();
    let coordinate_frame = method.contains("coordinateframe");
    if !(method.contains("geocentrictranslation")
        || method.contains("positionvector")
        || coordinate_frame)
    {
        return Ok(None);
    }
    let arcsec = std::f64::consts::PI / 648_000.0;
    let mut out = [0.0; 7];
    for p in t.children_named(&["PARAMETER"]) {
        let code = authority(p).and_then(|a| a.epsg());
        let key = name_key(p.name().unwrap_or(""));
        let slot = match (code, key.as_str()) {
            (Some(8605), _) | (_, "xaxistranslation") => 0,
            (Some(8606), _) | (_, "yaxistranslation") => 1,
            (Some(8607), _) | (_, "zaxistranslation") => 2,
            (Some(8608), _) | (_, "xaxisrotation") => 3,
            (Some(8609), _) | (_, "yaxisrotation") => 4,
            (Some(8610), _) | (_, "zaxisrotation") => 5,
            (Some(8611), _) | (_, "scaledifference") => 6,
            _ => continue,
        };
        let raw = req_number(p, 1, "transformation parameter")?;
        let unit = unit_child(p, UnitKind::Unknown)?;
        let factor = unit.as_ref().map_or(1.0, |u| u.factor);
        out[slot] = match slot {
            // Translations: metres (the default when no unit is given).
            0..=2 => raw * factor,
            // Rotations: arc-seconds (the default when no unit is given).
            3..=5 if unit.is_some() => raw * factor / arcsec,
            // Scale: PROJ writes the multiplier `1 + ds` (e.g. 0.999979511); EPSG uses ppm.
            6 if factor == 1.0 && (raw - 1.0).abs() < 0.01 => (raw - 1.0) * 1e6,
            6 if unit.is_some() => raw * factor / 1e-6,
            _ => raw,
        };
    }
    if coordinate_frame {
        for r in &mut out[3..6] {
            *r = -*r;
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use crate::{AxisOrder, CrsKind, Dialect, ParamName, ProjectionMethod, parse_wkt};

    #[test]
    fn wkt2_2015_geodetic_with_ellipsoidal_cs() {
        let crs = parse_wkt(
            r#"GEODCRS["WGS 84",DATUM["World Geodetic System 1984",ELLIPSOID["WGS 84",6378137,298.257223563,LENGTHUNIT["metre",1.0]]],CS[ellipsoidal,2],AXIS["latitude",north,ORDER[1]],AXIS["longitude",east,ORDER[2]],ANGLEUNIT["degree",0.0174532925199433],ID["EPSG",4326]]"#,
        )
        .unwrap();
        assert_eq!(crs.dialect, Dialect::Wkt2_2015);
        assert_eq!(crs.kind, CrsKind::Geographic);
        assert_eq!(crs.axis_order, AxisOrder::NorthEast);
        assert!(crs.angular_unit.as_ref().unwrap().is_degree());
        assert_eq!(crs.epsg(), Some(4326));
    }

    #[test]
    fn wkt2_geocentric_and_bound() {
        let crs = parse_wkt(
            r#"GEODCRS["WGS 84",DATUM["World Geodetic System 1984",ELLIPSOID["WGS 84",6378137,298.257223563]],CS[Cartesian,3],AXIS["(X)",geocentricX],AXIS["(Y)",geocentricY],AXIS["(Z)",geocentricZ],LENGTHUNIT["metre",1]]"#,
        )
        .unwrap();
        assert_eq!(crs.kind, CrsKind::Geocentric);
        assert!(crs.linear_unit.as_ref().unwrap().is_metre());

        let crs = parse_wkt(
            r#"BOUNDCRS[SOURCECRS[PROJCRS["OSGB 1936 / British National Grid",BASEGEOGCRS["OSGB 1936",DATUM["OSGB 1936",ELLIPSOID["Airy 1830",6377563.396,299.3249646,LENGTHUNIT["metre",1]]],PRIMEM["Greenwich",0,ANGLEUNIT["degree",0.0174532925199433]]],CONVERSION["unnamed",METHOD["Transverse Mercator",ID["EPSG",9807]],PARAMETER["Latitude of natural origin",49,ANGLEUNIT["degree",0.0174532925199433],ID["EPSG",8801]],PARAMETER["Longitude of natural origin",-2,ANGLEUNIT["degree",0.0174532925199433],ID["EPSG",8802]],PARAMETER["Scale factor at natural origin",0.9996012717,SCALEUNIT["unity",1],ID["EPSG",8805]],PARAMETER["False easting",400000,LENGTHUNIT["metre",1],ID["EPSG",8806]],PARAMETER["False northing",-100000,LENGTHUNIT["metre",1],ID["EPSG",8807]]],CS[Cartesian,2],AXIS["(E)",east,ORDER[1],LENGTHUNIT["metre",1]],AXIS["(N)",north,ORDER[2],LENGTHUNIT["metre",1]]]],TARGETCRS[GEOGCRS["WGS 84",DATUM["World Geodetic System 1984",ELLIPSOID["WGS 84",6378137,298.257223563,LENGTHUNIT["metre",1]]],PRIMEM["Greenwich",0,ANGLEUNIT["degree",0.0174532925199433]],CS[ellipsoidal,2],AXIS["latitude",north,ORDER[1],ANGLEUNIT["degree",0.0174532925199433]],AXIS["longitude",east,ORDER[2],ANGLEUNIT["degree",0.0174532925199433]],ID["EPSG",4326]]],ABRIDGEDTRANSFORMATION["Transformation from OSGB 1936 to WGS84",METHOD["Position Vector transformation (geog2D domain)",ID["EPSG",9606]],PARAMETER["X-axis translation",446.448,ID["EPSG",8605]],PARAMETER["Y-axis translation",-125.157,ID["EPSG",8606]],PARAMETER["Z-axis translation",542.06,ID["EPSG",8607]],PARAMETER["X-axis rotation",0.15,ID["EPSG",8608]],PARAMETER["Y-axis rotation",0.247,ID["EPSG",8609]],PARAMETER["Z-axis rotation",0.842,ID["EPSG",8610]],PARAMETER["Scale difference",0.999979511,ID["EPSG",8611]]]]"#,
        )
        .unwrap();
        assert_eq!(crs.kind, CrsKind::Projected);
        assert_eq!(crs.dialect, Dialect::Wkt2_2019);
        let proj = crs.projection.as_ref().unwrap();
        assert_eq!(proj.method, ProjectionMethod::TransverseMercator);
        assert_eq!(
            proj.param(ParamName::FalseNorthing).unwrap().metres(),
            Some(-100_000.0)
        );
        let t = crs.datum.unwrap().to_wgs84.unwrap();
        assert_eq!(t[0], 446.448);
        assert_eq!(t[5], 0.842);
        assert!((t[6] - -20.489).abs() < 1e-6, "{}", t[6]);
    }

    #[test]
    fn unknown_wkt2_kinds_and_garbage() {
        let crs = parse_wkt(
            r#"TIMECRS["GPS time",TDATUM["Time origin",TIMEORIGIN[1980-01-06T00:00:00.0Z]],CS[temporal,1],AXIS["time",future],TIMEUNIT["day",86400.0]]"#,
        );
        // TIMEORIGIN holds an unquoted ISO date, which isn't valid WKT token syntax for us:
        // either outcome is acceptable as long as nothing panics.
        if let Ok(crs) = crs {
            assert_eq!(crs.kind, CrsKind::Unknown);
        }
        let crs = parse_wkt(r#"PARAMETRICCRS["WMO standard atmosphere",PDATUM["Mean Sea Level",ANCHOR["1013.25 hPa at 15°C"]],CS[parametric,1],AXIS["pressure (hPa)",up],PARAMETRICUNIT["hPa",100.0]]"#).unwrap();
        assert_eq!(crs.kind, CrsKind::Unknown);
        assert!(parse_wkt("PROJCRS[\"x\"]").is_err());
    }
}
