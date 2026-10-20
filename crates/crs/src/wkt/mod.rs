//! WKT parsing: OGC WKT1 (01-009), the ESRI WKT1 dialect, and WKT2 (ISO 19162:2015 / 2019).
//!
//! One tokenizer/tree parser ([`tree`]) feeds two interpreters: [`wkt1`] (OGC and ESRI) and
//! [`wkt2`]. The dialect is decided from the root keyword and, for WKT1, ESRI tell-tales.

mod tree;
mod wkt1;
mod wkt2;

use crate::names::known_unit;
use crate::{Authority, Axis, AxisDirection, Crs, CrsError, Dialect, Unit, UnitKind};
use tree::{Node, Value};

/// Root keywords of OGC/ESRI WKT1.
const WKT1_ROOTS: &[&str] = &[
    "GEOGCS",
    "PROJCS",
    "GEOCCS",
    "VERT_CS",
    "VERTCS",
    "COMPD_CS",
    "LOCAL_CS",
    "FITTED_CS",
];

/// Parses a WKT1 (OGC or ESRI) or WKT2 string.
///
/// The text must already be within [`crate::MAX_INPUT_LEN`]; [`crate::parse_wkt`] checks that.
pub(crate) fn parse(text: &str) -> Result<Crs, CrsError> {
    let roots = tree::parse_document(text)?;
    let Some(first) = roots.first() else {
        return Err(CrsError::Empty);
    };
    if WKT1_ROOTS.contains(&first.keyword.as_str()) {
        let dialect = if roots.iter().any(is_esri) {
            Dialect::Wkt1Esri
        } else {
            Dialect::Wkt1Ogc
        };
        if roots.len() == 1 {
            wkt1::crs(first, dialect)
        } else {
            // ESRI writes compound CRSs as `PROJCS[…],VERTCS[…]` with no wrapper.
            let components = roots
                .iter()
                .map(|n| wkt1::crs(n, dialect))
                .collect::<Result<Vec<_>, _>>()?;
            let name = components
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(" + ");
            let mut crs = Crs::empty(crate::CrsKind::Compound, dialect, name);
            crs.components = components;
            Ok(crs)
        }
    } else if wkt2::is_root(&first.keyword) {
        if roots.len() > 1 {
            return Err(CrsError::syntax(0, "more than one top-level WKT2 CRS"));
        }
        wkt2::crs(first, wkt2::dialect(first))
    } else {
        Err(CrsError::UnknownRoot(first.keyword.clone()))
    }
}

/// ESRI tell-tales: `VERTCS`, `GCS_*` / `D_*` names, Title_Case parameter names, ESRI unit
/// spellings (`Degree`, `Meter`, `Foot_US`), or ESRI-only projection names. GDAL's OGC WKT1 uses
/// lower-case parameter and unit names and EPSG-style datum names.
fn is_esri(root: &Node) -> bool {
    const ESRI_METHODS: &[&str] = &[
        "albers",
        "lambertconformalconic",
        "mercatorauxiliarysphere",
        "gausskruger",
        "doublestereographic",
        "platecarree",
        "equidistantcylindrical",
        "stereographicnorthpole",
        "stereographicsouthpole",
        "rectifiedskeworthomorphicnaturalorigin",
        "rectifiedskeworthomorphiccenter",
        "polarstereographicvarianta",
        "polarstereographicvariantb",
        "polarstereographicvariantc",
    ];
    let mut esri = root.keyword == "VERTCS";
    // ESRI unit spellings only count when no AUTHORITY appears anywhere: GDAL's WKT1 export of
    // ESRI-catalog CRSs keeps `UNIT["Degree",…]` but adds authority codes.
    let mut esri_unit = false;
    let mut has_authority = false;
    root.walk(&mut |n| {
        let name = n.name().unwrap_or("");
        match n.keyword.as_str() {
            "GEOGCS" => esri |= name.starts_with("GCS_"),
            "DATUM" => esri |= name.starts_with("D_"),
            "PARAMETER" => esri |= name.bytes().any(|b| b.is_ascii_uppercase()),
            "PROJECTION" => {
                esri |= ESRI_METHODS.contains(&crate::names::name_key(name).as_str());
            }
            "UNIT" => {
                esri_unit |= matches!(
                    name,
                    "Degree" | "Meter" | "Foot_US" | "Foot" | "Grad" | "Radian"
                );
            }
            "AUTHORITY" => has_authority = true,
            _ => {}
        }
    });
    esri || (esri_unit && !has_authority)
}

/// The node's name (first argument), required.
fn req_name(node: &Node) -> Result<String, CrsError> {
    node.name()
        .map(str::to_owned)
        .ok_or_else(|| CrsError::missing("name", node.keyword.clone()))
}

/// A required numeric argument.
fn req_number(node: &Node, index: usize, element: &'static str) -> Result<f64, CrsError> {
    node.number(index)
        .ok_or_else(|| CrsError::missing(element, node.keyword.clone()))
}

/// The node's own `AUTHORITY[…]` (WKT1) or first `ID[…]` (WKT2) child.
fn authority(node: &Node) -> Option<Authority> {
    let a = node.child(&["AUTHORITY", "ID"])?;
    let name = a.name()?.to_owned();
    let code = match a.args.get(1)? {
        Value::Text(s) | Value::Word(s) => s.clone(),
        Value::Number { raw, .. } => raw.clone(),
        Value::Node(_) | Value::Literal(_) => return None,
    };
    Some(Authority { name, code })
}

/// Parses a unit node (`UNIT`, `LENGTHUNIT`, `ANGLEUNIT`, `SCALEUNIT`, …). `context` is the kind
/// a generic `UNIT` has where it appears. A missing conversion factor falls back to the
/// well-known unit table; a non-positive factor is an error.
fn unit(node: &Node, context: UnitKind) -> Result<Unit, CrsError> {
    let kind = match node.keyword.as_str() {
        "LENGTHUNIT" => UnitKind::Linear,
        "ANGLEUNIT" => UnitKind::Angular,
        "SCALEUNIT" => UnitKind::Scale,
        "TIMEUNIT" | "TEMPORALQUANTITY" => UnitKind::Time,
        "PARAMETRICUNIT" => UnitKind::Parametric,
        _ => context,
    };
    let name = req_name(node)?;
    let factor = match node.number(1) {
        Some(f) => f,
        None => known_unit(&name)
            .map(|u| u.factor)
            .ok_or_else(|| CrsError::missing("unit conversion factor", name.clone()))?,
    };
    if !(factor.is_finite() && factor > 0.0) {
        return Err(CrsError::invalid(
            "unit",
            format!("`{name}` has conversion factor {factor}"),
        ));
    }
    Ok(Unit { name, kind, factor })
}

const UNIT_KEYWORDS: &[&str] = &[
    "UNIT",
    "LENGTHUNIT",
    "ANGLEUNIT",
    "SCALEUNIT",
    "TIMEUNIT",
    "TEMPORALQUANTITY",
    "PARAMETRICUNIT",
];

/// The first unit child of `node`, parsed.
fn unit_child(node: &Node, context: UnitKind) -> Result<Option<Unit>, CrsError> {
    node.child(UNIT_KEYWORDS)
        .map(|u| unit(u, context))
        .transpose()
}

/// `AXIS["name", DIRECTION, …]` children. WKT2 axes may carry their own unit.
fn axes(node: &Node, context: UnitKind) -> Result<Vec<Axis>, CrsError> {
    node.children_named(&["AXIS"])
        .map(|a| {
            let name = a.name().unwrap_or("").to_owned();
            let direction = match a.args.get(1) {
                Some(Value::Word(w) | Value::Text(w)) => AxisDirection::from_word(w),
                _ => AxisDirection::Other("unspecified".into()),
            };
            let unit = unit_child(a, context)?;
            Ok(Axis {
                name,
                direction,
                unit,
            })
        })
        .collect()
}

/// Validated ellipsoid from `SPHEROID`/`ELLIPSOID["name", a, invf, LENGTHUNIT?]`.
fn ellipsoid(node: &Node) -> Result<crate::Ellipsoid, CrsError> {
    let name = req_name(node)?;
    let a = req_number(node, 1, "semi-major axis")?;
    let inverse_flattening = req_number(node, 2, "inverse flattening")?;
    let to_metres = unit_child(node, UnitKind::Linear)?.map_or(1.0, |u| u.factor);
    crate::Ellipsoid::checked(name, a * to_metres, inverse_flattening, authority(node))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{AxisOrder, CrsKind, ParamName, ProjectionMethod};

    #[test]
    fn esri_utm() {
        let crs = parse(
            r#"PROJCS["WGS_1984_UTM_Zone_33N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",15.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#,
        )
        .unwrap();
        assert_eq!(crs.kind, CrsKind::Projected);
        assert_eq!(crs.dialect, Dialect::Wkt1Esri);
        assert_eq!(crs.axis_order, AxisOrder::Unspecified);
        let proj = crs.projection.as_ref().unwrap();
        assert_eq!(proj.method, ProjectionMethod::TransverseMercator);
        let cm = proj.param(ParamName::LongitudeOfOrigin).unwrap();
        assert_eq!(cm.degrees(), Some(15.0));
        let fe = proj.param(ParamName::FalseEasting).unwrap();
        assert_eq!(fe.metres(), Some(500_000.0));
        assert!(crs.linear_unit.as_ref().unwrap().is_metre());
        assert!(crs.angular_unit.as_ref().unwrap().is_degree());
        let datum = crs.datum.as_ref().unwrap();
        assert_eq!(datum.canonical_name(), "World Geodetic System 1984");
        assert_eq!(
            datum.ellipsoid.as_ref().unwrap().semi_major_axis,
            6_378_137.0
        );
        assert_eq!(crs.epsg(), None);
    }

    #[test]
    fn esri_compound_without_wrapper() {
        let crs = parse(
            r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],VERTCS["EGM96_Geoid",VDATUM["EGM96_Geoid"],PARAMETER["Vertical_Shift",0.0],PARAMETER["Direction",1.0],UNIT["Meter",1.0]]"#,
        )
        .unwrap();
        assert_eq!(crs.kind, CrsKind::Compound);
        assert_eq!(crs.components.len(), 2);
        assert_eq!(crs.components[1].kind, CrsKind::Vertical);
        assert_eq!(crs.horizontal().unwrap().kind, CrsKind::Geographic);
    }

    #[test]
    fn rejects_bad_ellipsoid_and_units() {
        for bad in [
            r#"GEOGCS["x",DATUM["d",SPHEROID["s",0,298]],UNIT["degree",0.0174]]"#,
            r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,0.5]],UNIT["degree",0.0174]]"#,
            r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298]],UNIT["degree",-1]]"#,
            r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298]],UNIT["furlong"]]"#,
            r#"GEOGCS["x",DATUM["d"]]"#,
            r#"GEOGCS["x"]"#,
            r#"PROJCS["x",PROJECTION["Transverse_Mercator"]]"#,
            r#"FOO["x"]"#,
        ] {
            assert!(parse(bad).is_err(), "{bad} should fail");
        }
    }
}
