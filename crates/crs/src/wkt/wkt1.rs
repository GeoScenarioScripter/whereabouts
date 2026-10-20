//! OGC WKT1 (01-009, as written by GDAL/PROJ) and ESRI WKT1 → [`Crs`].
//!
//! Conventions followed (they match GDAL's importer):
//! - `SPHEROID` semi-major axes are in metres.
//! - `PRIMEM` longitudes are in degrees, whatever the `GEOGCS` unit (GDAL and ESRI both write
//!   Paris as `2.33722917` next to `UNIT["grad",…]`).
//! - Angular projection parameters are in the `GEOGCS` angular unit; false easting/northing in
//!   the `PROJCS` linear unit.
//! - A `GEOGCS` without `UNIT` is in degrees.

use super::tree::Node;
use super::{authority, axes, ellipsoid, req_name, req_number, unit_child};
use crate::names::{ParamName, ProjectionMethod, harmonize_params, refine_method};
use crate::{
    AxisOrder, BaseCrs, Crs, CrsError, CrsKind, Datum, Dialect, Parameter, PrimeMeridian,
    Projection, Unit, UnitKind,
};

/// Interprets one WKT1 CRS node.
pub(super) fn crs(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    match node.keyword.as_str() {
        "GEOGCS" => geogcs(node, dialect),
        "PROJCS" => projcs(node, dialect),
        "GEOCCS" => geoccs(node, dialect),
        "VERT_CS" | "VERTCS" => vert_cs(node, dialect),
        "COMPD_CS" => compd_cs(node, dialect),
        "LOCAL_CS" => local_cs(node, dialect),
        "FITTED_CS" => Ok(Crs::empty(CrsKind::Unknown, dialect, req_name(node)?)),
        other => Err(CrsError::UnknownRoot(other.to_owned())),
    }
}

fn finish(mut crs: Crs, node: &Node, axis_unit: UnitKind) -> Result<Crs, CrsError> {
    crs.authority = authority(node);
    crs.axes = axes(node, axis_unit)?;
    crs.axis_order = AxisOrder::from_axes(&crs.axes);
    Ok(crs)
}

fn geogcs(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(CrsKind::Geographic, dialect, req_name(node)?);
    crs.datum = Some(geodetic_datum(node)?);
    crs.angular_unit = Some(unit_child(node, UnitKind::Angular)?.unwrap_or_else(Unit::degree));
    finish(crs, node, UnitKind::Angular)
}

fn geoccs(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(CrsKind::Geocentric, dialect, req_name(node)?);
    crs.datum = Some(geodetic_datum(node)?);
    crs.linear_unit = unit_child(node, UnitKind::Linear)?;
    finish(crs, node, UnitKind::Linear)
}

/// `DATUM[…]` + sibling `PRIMEM[…]` of a `GEOGCS`/`GEOCCS`.
fn geodetic_datum(cs: &Node) -> Result<Datum, CrsError> {
    let node = cs
        .child(&["DATUM"])
        .ok_or_else(|| CrsError::missing("DATUM", cs.keyword.clone()))?;
    let spheroid = node
        .child(&["SPHEROID", "ELLIPSOID"])
        .ok_or_else(|| CrsError::missing("SPHEROID", "DATUM"))?;
    let prime_meridian = match cs.child(&["PRIMEM"]) {
        Some(pm) => PrimeMeridian {
            name: req_name(pm)?,
            longitude: req_number(pm, 1, "prime meridian longitude")?,
            authority: authority(pm),
        },
        None => PrimeMeridian::default(),
    };
    Ok(Datum {
        name: req_name(node)?,
        ellipsoid: Some(ellipsoid(spheroid)?),
        prime_meridian,
        to_wgs84: node.child(&["TOWGS84"]).and_then(towgs84),
        authority: authority(node),
    })
}

/// `TOWGS84[dx,dy,dz]` or `TOWGS84[dx,dy,dz,rx,ry,rz,ds]`. Anything else — another arity or
/// any non-numeric argument — gives `None` (not an error: the CRS is still usable without the
/// shift). Values are never shifted into the wrong slots.
fn towgs84(node: &Node) -> Option<[f64; 7]> {
    if !matches!(node.args.len(), 3 | 7) {
        return None;
    }
    let mut out = [0.0; 7];
    for (i, slot) in out.iter_mut().take(node.args.len()).enumerate() {
        *slot = node.number(i)?;
    }
    Some(out)
}

fn projcs(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let name = req_name(node)?;
    let base_node = node
        .child(&["GEOGCS"])
        .ok_or_else(|| CrsError::missing("GEOGCS", format!("PROJCS `{name}`")))?;
    let base = geogcs(base_node, dialect)?;
    let projection = node
        .child(&["PROJECTION"])
        .ok_or_else(|| CrsError::missing("PROJECTION", format!("PROJCS `{name}`")))?;
    let linear_unit = unit_child(node, UnitKind::Linear)?;
    let angular_unit = base.angular_unit.clone().unwrap_or_else(Unit::degree);

    let mut parameters = node
        .children_named(&["PARAMETER"])
        .map(|p| {
            let original_name = req_name(p)?;
            let name = ParamName::from_name(&original_name);
            let unit = name
                .class()
                .default_unit(&angular_unit, linear_unit.as_ref());
            Parameter {
                name,
                value: req_number(p, 1, "parameter value")?,
                original_name,
                unit,
                authority: authority(p),
            }
            .validated()
        })
        .collect::<Result<Vec<_>, CrsError>>()?;

    let proj4_extension = node
        .children_named(&["EXTENSION"])
        .find(|e| e.name().is_some_and(|n| n.eq_ignore_ascii_case("PROJ4")))
        .and_then(|e| match e.args.get(1) {
            Some(super::Value::Text(s)) => Some(s.clone()),
            _ => None,
        });

    let method_name = req_name(projection)?;
    let method = refine_method(
        &method_name,
        ProjectionMethod::from_name(&method_name),
        &parameters,
        proj4_extension.as_deref(),
    );
    harmonize_params(method, &mut parameters);

    let mut crs = Crs::empty(CrsKind::Projected, dialect, name);
    crs.base = Some(BaseCrs {
        name: base.name,
        authority: base.authority,
    });
    crs.datum = base.datum;
    crs.angular_unit = base.angular_unit;
    crs.linear_unit = linear_unit;
    crs.projection = Some(Projection {
        method,
        method_name,
        method_authority: authority(projection),
        parameters,
    });
    crs.proj4_extension = proj4_extension;
    finish(crs, node, UnitKind::Linear)
}

fn vert_cs(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(CrsKind::Vertical, dialect, req_name(node)?);
    crs.datum = if node.child(&["DATUM"]).is_some() {
        // ESRI ellipsoidal heights: VERTCS["WGS_1984",DATUM["D_WGS_1984",SPHEROID[…]],…]
        Some(geodetic_datum(node)?)
    } else {
        node.child(&["VERT_DATUM", "VDATUM"])
            .map(simple_datum)
            .transpose()?
    };
    crs.linear_unit = unit_child(node, UnitKind::Linear)?;
    finish(crs, node, UnitKind::Linear)
}

fn local_cs(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(CrsKind::Engineering, dialect, req_name(node)?);
    crs.datum = node.child(&["LOCAL_DATUM"]).map(simple_datum).transpose()?;
    crs.linear_unit = unit_child(node, UnitKind::Linear)?;
    finish(crs, node, UnitKind::Linear)
}

/// A datum without an ellipsoid (vertical, local).
fn simple_datum(node: &Node) -> Result<Datum, CrsError> {
    Ok(Datum {
        name: req_name(node)?,
        ellipsoid: None,
        prime_meridian: PrimeMeridian::default(),
        to_wgs84: None,
        authority: authority(node),
    })
}

fn compd_cs(node: &Node, dialect: Dialect) -> Result<Crs, CrsError> {
    let mut crs = Crs::empty(CrsKind::Compound, dialect, req_name(node)?);
    crs.components = node
        .children_named(super::WKT1_ROOTS)
        .map(|c| self::crs(c, dialect))
        .collect::<Result<Vec<_>, _>>()?;
    if crs.components.is_empty() {
        return Err(CrsError::missing("component CRS", "COMPD_CS"));
    }
    crs.authority = authority(node);
    Ok(crs)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use crate::{CrsKind, Dialect, ParamName, ProjectionMethod, parse_wkt};

    #[test]
    fn ogc_web_mercator_from_extension() {
        let crs = parse_wkt(
            r#"PROJCS["WGS 84 / Pseudo-Mercator",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],PRIMEM["Greenwich",0,AUTHORITY["EPSG","8901"]],UNIT["degree",0.0174532925199433,AUTHORITY["EPSG","9122"]],AUTHORITY["EPSG","4326"]],PROJECTION["Mercator_1SP"],PARAMETER["central_meridian",0],PARAMETER["scale_factor",1],PARAMETER["false_easting",0],PARAMETER["false_northing",0],UNIT["metre",1,AUTHORITY["EPSG","9001"]],AXIS["Easting",EAST],AXIS["Northing",NORTH],EXTENSION["PROJ4","+proj=merc +a=6378137 +b=6378137 +lat_ts=0 +lon_0=0 +x_0=0 +y_0=0 +k=1 +units=m +nadgrids=@null +wktext +no_defs"],AUTHORITY["EPSG","3857"]]"#,
        )
        .unwrap();
        assert_eq!(crs.dialect, Dialect::Wkt1Ogc);
        assert_eq!(crs.epsg(), Some(3857));
        assert_eq!(
            crs.projection.unwrap().method,
            ProjectionMethod::PopularVisualisationPseudoMercator
        );
        assert_eq!(crs.base.unwrap().authority.unwrap().epsg(), Some(4326));
    }

    #[test]
    fn grads_and_paris_meridian() {
        let crs = parse_wkt(
            r#"PROJCS["NTF_Paris_Lambert_Zone_II",GEOGCS["GCS_NTF_Paris",DATUM["D_NTF",SPHEROID["Clarke_1880_IGN",6378249.2,293.4660212936265]],PRIMEM["Paris",2.337229166666667],UNIT["Grad",0.01570796326794897]],PROJECTION["Lambert_Conformal_Conic"],PARAMETER["False_Easting",600000.0],PARAMETER["False_Northing",2200000.0],PARAMETER["Central_Meridian",0.0],PARAMETER["Standard_Parallel_1",52.0],PARAMETER["Scale_Factor",0.99987742],PARAMETER["Latitude_Of_Origin",52.0],UNIT["Meter",1.0]]"#,
        )
        .unwrap();
        assert_eq!(crs.dialect, Dialect::Wkt1Esri);
        let proj = crs.projection.as_ref().unwrap();
        assert_eq!(proj.method, ProjectionMethod::LambertConicConformal1Sp);
        let lat0 = proj.param(ParamName::LatitudeOfOrigin).unwrap();
        assert!((lat0.degrees().unwrap() - 46.8).abs() < 1e-9);
        let pm = &crs.datum.as_ref().unwrap().prime_meridian;
        assert!((pm.longitude - 2.337_229_166_666_667).abs() < 1e-12);
    }

    #[test]
    fn malformed_towgs84_is_dropped_whole() {
        let with = |towgs84: &str| {
            parse_wkt(&format!(
                r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298.257],{towgs84}],UNIT["degree",0.0174532925199433]]"#
            ))
            .unwrap()
            .datum
            .unwrap()
            .to_wgs84
        };
        assert_eq!(
            with("TOWGS84[1,2,3]"),
            Some([1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0])
        );
        // A non-numeric argument must not shift later values into earlier slots.
        assert_eq!(with(r#"TOWGS84[1,"x",3,4,5,6,7]"#), None);
        assert_eq!(with("TOWGS84[1,2,3,NORTH]"), None);
        // Wrong arity.
        assert_eq!(with("TOWGS84[1,2,3,4,5,6]"), None);
        assert_eq!(with("TOWGS84[]"), None);
    }

    #[test]
    fn local_cs_and_towgs84() {
        let crs = parse_wkt(
            r#"LOCAL_CS["Site grid",LOCAL_DATUM["Site datum",0],UNIT["metre",1],AXIS["X",EAST],AXIS["Y",NORTH]]"#,
        )
        .unwrap();
        assert_eq!(crs.kind, CrsKind::Engineering);
        assert_eq!(crs.axis_order, crate::AxisOrder::EastNorth);

        let crs = parse_wkt(
            r#"GEOGCS["OSGB 1936",DATUM["OSGB_1936",SPHEROID["Airy 1830",6377563.396,299.3249646],TOWGS84[446.448,-125.157,542.06,0.15,0.247,0.842,-20.489]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#,
        )
        .unwrap();
        let t = crs.datum.unwrap().to_wgs84.unwrap();
        assert_eq!(t[0], 446.448);
        assert_eq!(t[6], -20.489);
    }
}
