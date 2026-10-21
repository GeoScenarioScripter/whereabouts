//! EPSG identification: authority code → name match → parameter match (D-09, `04` §4).
//!
//! - **Authority**: the definition cites `EPSG:n` for itself (WKT1 `AUTHORITY`, WKT2 `ID`,
//!   PROJJSON `id`) and the table knows `n` → confidence 100. Geocentric, vertical and other
//!   non-horizontal CRSs are identified only this way (any cited code).
//! - **Name**: the CRS name (or an EPSG/Esri alias, e.g.
//!   `NAD_1983_StatePlane_California_III_FIPS_0403_Feet`) matches a table entry *and* the
//!   parameters are equivalent → confidence 100 (entries whose datum also matches win). A name
//!   match with different parameters is rejected. For methods without a transform (so no
//!   parameter comparison), a unique name match on an equally unsupported entry → 90.
//! - **Parameters**: method, parameters, ellipsoid, prime meridian, unit and axis directions are
//!   equivalent ([`ProjSpec::equivalent`]) and the datum name resolves to the entry's datum →
//!   confidence 70 (what PROJ's `identify()` gives a custom-named but otherwise equal CRS).
//!   Geographic CRSs need a datum match; projected CRSs whose datum name is unknown are
//!   accepted only if exactly one table entry matches.
//!
//! Ties between equally good candidates resolve to a non-deprecated, then 2D, entry; a remaining
//! tie means the CRS is ambiguous and is left unidentified rather than guessed.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::epsg::{Entry, key, table};
use crate::model::{Crs, CrsKind};
use crate::names::canonical_datum_name;
use crate::spec::{Method, ProjSpec};
use crate::transform::TransformError;

/// How an EPSG code was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchMethod {
    /// The definition cites the code itself.
    Authority,
    /// Name (or alias) match with equivalent parameters.
    Name,
    /// Parameter and datum match; the name differs.
    Parameters,
}

/// An EPSG identification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identification {
    /// The EPSG code.
    pub epsg: u32,
    /// The EPSG name of that CRS (the definition's own name for cited codes the embedded table
    /// does not have, such as vertical or geocentric CRSs).
    pub name: String,
    /// 100 for authority and name matches, 70 for parameter matches (PROJ's scale).
    pub confidence: u8,
    /// How it was found.
    pub method: MatchMethod,
}

/// Identifies the EPSG code of a parsed CRS — of its horizontal component for compound CRSs.
/// Geocentric, vertical and other non-horizontal CRSs are identified only by the code they cite.
/// `None` if nothing matches, or the match is ambiguous.
pub fn identify(crs: &Crs) -> Option<Identification> {
    let Some(h) = crs.horizontal() else {
        let epsg = crs.epsg().filter(|&c| c > 0)?;
        return Some(Identification {
            epsg,
            name: epsg_name_or(epsg, &crs.name),
            confidence: 100,
            method: MatchMethod::Authority,
        });
    };
    let t = table();
    let kind_ok = |e: Entry| match h.kind {
        CrsKind::Geographic => matches!(e, Entry::Geog(_)),
        _ => matches!(e, Entry::Proj(_)),
    };
    let found = |e: Entry, confidence, method| {
        Some(Identification {
            epsg: t.code(e)?,
            name: t.name(e)?.to_owned(),
            confidence,
            method,
        })
    };

    // 1. Authority.
    if let Some(code) = h.epsg()
        && let Some(e) = t.get(code)
        && kind_ok(e)
    {
        return found(e, 100, MatchMethod::Authority);
    }

    let datums = datum_codes(h);
    let specs = specs();
    let name_key = key(&h.name);
    let spec = match ProjSpec::from_crs(h) {
        Ok(spec) => spec,
        Err(TransformError::UnsupportedMethod(_)) => {
            // A method we cannot transform (polyconic, Colombia Urban, …): parameters can't be
            // compared, so only an unambiguous name match whose table entry is equally
            // unsupported counts, at a lower confidence.
            let named: Vec<Entry> = t
                .by_name(&name_key)
                .iter()
                .copied()
                .filter(|&e| kind_ok(e) && specs.get(e).is_none())
                .collect();
            return found(best(&named)?, 90, MatchMethod::Name);
        }
        Err(_) => return None,
    };
    let equivalent = |e: Entry| specs.get(e).is_some_and(|s| s.equivalent(&spec));
    let datum_ok = |e: Entry| match &datums {
        Some(codes) => t.datum_of(e).is_some_and(|d| codes.contains(&d)),
        None => true,
    };

    // 2. Name (+ equivalent parameters). Prefer entries whose datum matches too; national
    // realisations (ETRS89-NOR for ESRI's D_ETRS_1989) only match by name.
    let named: Vec<Entry> = t
        .by_name(&name_key)
        .iter()
        .copied()
        .filter(|&e| kind_ok(e) && equivalent(e))
        .collect();
    let with_datum: Vec<Entry> = named.iter().copied().filter(|&e| datum_ok(e)).collect();
    if let Some(e) = best(&with_datum).or_else(|| best(&named)) {
        return found(e, 100, MatchMethod::Name);
    }

    // 3. Parameters.
    if h.kind == CrsKind::Geographic && datums.is_none() {
        return None;
    }
    let matches: Vec<Entry> = specs
        .with_method(spec.method)
        .filter(|&e| kind_ok(e) && datum_ok(e) && equivalent(e))
        .collect();
    let e = if datums.is_some() {
        best(&matches)
    } else {
        // Unknown datum: only an unambiguous match counts.
        let current: Vec<Entry> = matches
            .iter()
            .copied()
            .filter(|&e| !t.deprecated(e))
            .collect();
        (current.len() == 1).then(|| current[0])
    }?;
    found(e, 70, MatchMethod::Parameters)
}

fn epsg_name_or(code: u32, fallback: &str) -> String {
    crate::epsg_name(code).unwrap_or(fallback).to_owned()
}

/// EPSG datum codes the CRS's datum name resolves to, `None` if it resolves to none.
fn datum_codes(h: &Crs) -> Option<Vec<u32>> {
    let datum = h.datum.as_ref()?;
    let t = table();
    let mut codes: Vec<u32> = t.datums_named(&key(&datum.name)).to_vec();
    for code in t.datums_named(&key(&canonical_datum_name(&datum.name))) {
        if !codes.contains(code) {
            codes.push(*code);
        }
    }
    (!codes.is_empty()).then_some(codes)
}

/// The single best candidate: non-deprecated first, then 2D geographic over 3D; `None` if
/// several remain.
fn best(candidates: &[Entry]) -> Option<Entry> {
    let t = table();
    let rank = |e: Entry| {
        let is_3d = match e {
            Entry::Geog(i) => t.geog.get(i).is_some_and(|g| g.is_3d),
            Entry::Proj(_) => false,
        };
        (t.deprecated(e), is_3d)
    };
    let top = candidates.iter().map(|&e| rank(e)).min()?;
    let mut best = candidates.iter().copied().filter(|&e| rank(e) == top);
    let first = best.next()?;
    let mut codes = vec![t.code(first)];
    codes.extend(best.map(|e| t.code(e)));
    codes.dedup();
    (codes.len() == 1).then_some(first)
}

/// Specs of every table entry, built on first use, indexed by method.
struct Specs {
    geog: Vec<Option<ProjSpec>>,
    proj: Vec<Option<ProjSpec>>,
    by_method: HashMap<Method, Vec<Entry>>,
}

impl Specs {
    fn get(&self, e: Entry) -> Option<&ProjSpec> {
        match e {
            Entry::Geog(i) => self.geog.get(i)?.as_ref(),
            Entry::Proj(i) => self.proj.get(i)?.as_ref(),
        }
    }

    fn with_method(&self, method: Method) -> impl Iterator<Item = Entry> + '_ {
        self.by_method.get(&method).into_iter().flatten().copied()
    }
}

fn specs() -> &'static Specs {
    static SPECS: OnceLock<Specs> = OnceLock::new();
    SPECS.get_or_init(|| {
        let t = table();
        let spec_of = |e: Entry| t.crs(e).and_then(|c| ProjSpec::from_crs(&c).ok());
        let geog: Vec<Option<ProjSpec>> =
            (0..t.geog.len()).map(|i| spec_of(Entry::Geog(i))).collect();
        let proj: Vec<Option<ProjSpec>> =
            (0..t.proj.len()).map(|i| spec_of(Entry::Proj(i))).collect();
        let mut by_method: HashMap<Method, Vec<Entry>> = HashMap::new();
        for (i, s) in geog.iter().enumerate() {
            if let Some(s) = s {
                by_method.entry(s.method).or_default().push(Entry::Geog(i));
            }
        }
        for (i, s) in proj.iter().enumerate() {
            if let Some(s) = s {
                by_method.entry(s.method).or_default().push(Entry::Proj(i));
            }
        }
        Specs {
            geog,
            proj,
            by_method,
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn esri_utm_with_custom_name_matches_by_parameters() {
        let prj = r#"PROJCS["Site_Grid_Custom",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",15.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
        let id = identify(&crate::parse(prj).unwrap()).unwrap();
        assert_eq!(
            (id.epsg, id.confidence, id.method),
            (32633, 70, MatchMethod::Parameters)
        );
    }

    #[test]
    fn esri_name_and_authority() {
        let prj = r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;
        let id = identify(&crate::parse(prj).unwrap()).unwrap();
        assert_eq!((id.epsg, id.method), (4326, MatchMethod::Name));
        let crs = crate::epsg_crs(2193).unwrap();
        assert_eq!(identify(&crs).unwrap().method, MatchMethod::Authority);
    }

    #[test]
    fn unknown_datum_geographic_is_not_identified() {
        let prj = r#"GEOGCS["Mystery",DATUM["D_Mystery",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;
        assert!(identify(&crate::parse(prj).unwrap()).is_none());
    }
}
