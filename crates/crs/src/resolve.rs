//! From a reader's raw [`CrsDefinition`] to the catalog's [`ResolvedCrs`]: parse, identify,
//! and keep the parsed model for the bbox transform.

use whereabouts_core::{CrsDefinition, CrsDefinitionKind, ResolvedCrs};

use crate::epsg::{Entry, table};
use crate::identify::{Identification, MatchMethod, identify};
use crate::model::{Crs, CrsKind};
use crate::{CrsError, parse};

/// The outcome of [`resolve`].
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// What the catalog stores (`crs.status`, `crs.epsg`, `crs.name`).
    pub resolved: ResolvedCrs,
    /// The parsed model (or the EPSG table's, for bare codes) — the input to
    /// [`crate::transform_bounds`]. `None` when nothing usable could be built.
    pub crs: Option<Crs>,
    /// How the EPSG code was found, if it was.
    pub identification: Option<Identification>,
    /// Why the definition could not be parsed, for `Unparsed`.
    pub error: Option<CrsError>,
}

/// Resolves a raw definition:
///
/// - `epsg`: the code is looked up in the embedded EPSG table (identified at confidence 100;
///   `crs` is `None` for a code the table does not have, such as a vertical CRS).
/// - WKT / PROJJSON: parsed, then [`identify`]d. A GeoPackage-style out-of-band
///   [`CrsDefinition::authority`] (`EPSG`) is used when the text cites no authority, the text
///   itself does not identify, and the code is a table CRS of the same kind (geographic vs
///   projected).
/// - `geokeys`: not handled yet (stage01 session 3.2 adds it) → `Unparsed`.
pub fn resolve(definition: &CrsDefinition) -> Resolution {
    let unparsed = |error| Resolution {
        resolved: ResolvedCrs::Unparsed,
        crs: None,
        identification: None,
        error,
    };
    match definition.kind {
        CrsDefinitionKind::Epsg => {
            let Some(code) = definition.epsg_code() else {
                return unparsed(Some(CrsError::invalid(
                    "EPSG code",
                    format!("`{}` is not a code", definition.text),
                )));
            };
            let t = table();
            let entry = t.get(code);
            let crs = entry.and_then(|e| t.crs(e));
            let name = entry.and_then(|e| t.name(e));
            Resolution {
                resolved: ResolvedCrs::Identified {
                    epsg: code,
                    name: name.map(str::to_owned),
                },
                crs,
                identification: name.map(|name| Identification {
                    epsg: code,
                    name: name.to_owned(),
                    confidence: 100,
                    method: MatchMethod::Authority,
                }),
                error: None,
            }
        }
        CrsDefinitionKind::GeoKeys => unparsed(None),
        CrsDefinitionKind::Wkt1Esri
        | CrsDefinitionKind::Wkt1
        | CrsDefinitionKind::Wkt2
        | CrsDefinitionKind::ProjJson => match parse(&definition.text) {
            Err(e) => unparsed(Some(e)),
            Ok(crs) => {
                let identification = identify(&crs).or_else(|| out_of_band(definition, &crs));
                let resolved = match &identification {
                    Some(id) => ResolvedCrs::Identified {
                        epsg: id.epsg,
                        name: Some(id.name.clone()),
                    },
                    None => ResolvedCrs::Unidentified {
                        name: (!crs.name.is_empty()).then(|| crs.name.clone()),
                    },
                };
                Resolution {
                    resolved,
                    crs: Some(crs),
                    identification,
                    error: None,
                }
            }
        },
    }
}

/// The definition's out-of-band EPSG authority, when the text has none of its own.
fn out_of_band(definition: &CrsDefinition, crs: &Crs) -> Option<Identification> {
    let auth = definition.authority.as_ref()?;
    let h = crs.horizontal()?;
    if !auth.authority.eq_ignore_ascii_case("EPSG") || h.authority.is_some() {
        return None;
    }
    let code: u32 = auth.code.trim().parse().ok()?;
    let t = table();
    let e = t.get(code)?;
    let same_kind = match e {
        Entry::Geog(_) => h.kind == CrsKind::Geographic,
        Entry::Proj(_) => h.kind == CrsKind::Projected,
    };
    same_kind.then(|| Identification {
        epsg: code,
        name: t.name(e).unwrap_or_default().to_owned(),
        confidence: 100,
        method: MatchMethod::Authority,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use whereabouts_core::{AuthorityCode, CrsStatus};

    #[test]
    fn bare_codes_and_text() {
        let r = resolve(&CrsDefinition::epsg(2193));
        assert_eq!(r.resolved.epsg(), Some(2193));
        assert!(r.crs.is_some());
        let r = resolve(&CrsDefinition::epsg(5703)); // vertical: identified, no model
        assert_eq!(r.resolved.status(), CrsStatus::Identified);
        assert!(r.crs.is_none());
        let r = resolve(&CrsDefinition::new(CrsDefinitionKind::Wkt1Esri, "garbage"));
        assert_eq!(r.resolved, ResolvedCrs::Unparsed);
        assert!(r.error.is_some());
    }

    #[test]
    fn gpkg_out_of_band_authority() {
        let wkt = r#"PROJCS["Custom",GEOGCS["x",DATUM["unknown datum",SPHEROID["s",6378137,298.257223563]],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["central_meridian",15.123],UNIT["metre",1]]"#;
        let mut def = CrsDefinition::new(CrsDefinitionKind::Wkt1, wkt);
        assert_eq!(resolve(&def).resolved.status(), CrsStatus::Unidentified);
        def.authority = Some(AuthorityCode {
            authority: "EPSG".into(),
            code: "32633".into(),
        });
        assert_eq!(resolve(&def).resolved.epsg(), Some(32633));
        def.authority = Some(AuthorityCode {
            authority: "EPSG".into(),
            code: "4326".into(), // geographic code on a projected CRS: ignored
        });
        assert_eq!(resolve(&def).resolved.status(), CrsStatus::Unidentified);
    }
}
