//! CRS references as found in files, and the result of resolving them.
//!
//! Parsing, EPSG identification and transforms live in `whereabouts-crs`; this module only carries
//! the raw definition from readers to that crate and the outcome on to the catalog.

use serde::{Deserialize, Serialize};

use crate::code::code_enum;

code_enum! {
    /// The syntax of a raw CRS definition (stored in `crs.def_kind`).
    pub enum CrsDefinitionKind {
        /// ESRI-flavoured WKT1 (typical `.prj`; often no `AUTHORITY`).
        Wkt1Esri => "wkt1_esri",
        /// OGC WKT1 (OGC 01-009 / GDAL style).
        Wkt1 => "wkt1",
        /// OGC WKT2 (2015 or 2019).
        Wkt2 => "wkt2",
        /// PROJJSON (GeoParquet).
        ProjJson => "projjson",
        /// A textual summary of GeoTIFF GeoKeys (format defined by `whereabouts-crs`).
        GeoKeys => "geokeys",
        /// A bare EPSG code; `text` is the decimal code, e.g. `"2193"`.
        Epsg => "epsg",
    }
}

/// An authority code that came with a definition but outside its text, e.g. a GeoPackage
/// `gpkg_spatial_ref_sys.organization` / `organization_coordsys_id` pair next to WKT that lacks
/// an `AUTHORITY` node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuthorityCode {
    /// Authority name as written in the source, e.g. `"EPSG"`, `"ESRI"`.
    pub authority: String,
    /// The code within the authority, e.g. `"2193"`.
    pub code: String,
}

/// A CRS definition exactly as a reader found it. Readers never interpret it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CrsDefinition {
    /// The syntax of `text`.
    pub kind: CrsDefinitionKind,
    /// The definition text, verbatim (trimmed of surrounding whitespace and NULs at most).
    pub text: String,
    /// An out-of-band authority code, if the format provides one.
    pub authority: Option<AuthorityCode>,
}

impl CrsDefinition {
    /// A definition with no out-of-band authority code.
    pub fn new(kind: CrsDefinitionKind, text: impl Into<String>) -> Self {
        CrsDefinition {
            kind,
            text: text.into(),
            authority: None,
        }
    }

    /// A bare EPSG code definition.
    pub fn epsg(code: u32) -> Self {
        CrsDefinition::new(CrsDefinitionKind::Epsg, code.to_string())
    }

    /// The EPSG code if this is a valid [`CrsDefinitionKind::Epsg`] definition.
    pub fn epsg_code(&self) -> Option<u32> {
        match self.kind {
            CrsDefinitionKind::Epsg => self.text.trim().parse().ok(),
            _ => None,
        }
    }
}

code_enum! {
    /// How far a CRS definition could be resolved.
    pub enum CrsStatus {
        /// Parsed and identified as an EPSG CRS.
        Identified => "identified",
        /// Parsed, but no EPSG match; bounds may still transform from the parsed parameters.
        Unidentified => "unidentified",
        /// The definition could not be parsed; no WGS84 extent (`crs_status = unknown`).
        Unparsed => "unparsed",
    }
}

/// The outcome of resolving a layer's [`CrsDefinition`] (produced by `indexer::extract` using
/// `whereabouts-crs`; a pure function of the definition, so the catalog de-duplicates it with it).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResolvedCrs {
    /// Parsed and identified as an EPSG CRS.
    Identified {
        /// The EPSG code.
        epsg: u32,
        /// CRS name (from the EPSG table or the definition).
        name: Option<String>,
    },
    /// Parsed, but no EPSG match.
    Unidentified {
        /// CRS name from the definition, if any.
        name: Option<String>,
    },
    /// The definition could not be parsed.
    Unparsed,
}

impl ResolvedCrs {
    /// The status code (stored in the catalog).
    pub fn status(&self) -> CrsStatus {
        match self {
            ResolvedCrs::Identified { .. } => CrsStatus::Identified,
            ResolvedCrs::Unidentified { .. } => CrsStatus::Unidentified,
            ResolvedCrs::Unparsed => CrsStatus::Unparsed,
        }
    }

    /// The identified EPSG code, if any (`crs.epsg`).
    pub fn epsg(&self) -> Option<u32> {
        match self {
            ResolvedCrs::Identified { epsg, .. } => Some(*epsg),
            _ => None,
        }
    }

    /// The CRS name, if known (`crs.name`).
    pub fn name(&self) -> Option<&str> {
        match self {
            ResolvedCrs::Identified { name, .. } | ResolvedCrs::Unidentified { name } => {
                name.as_deref()
            }
            ResolvedCrs::Unparsed => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn epsg_helpers() {
        let d = CrsDefinition::epsg(2193);
        assert_eq!(d.text, "2193");
        assert_eq!(d.epsg_code(), Some(2193));
        assert_eq!(
            CrsDefinition::new(CrsDefinitionKind::Wkt1, "2193").epsg_code(),
            None
        );
        assert_eq!(
            CrsDefinition::new(CrsDefinitionKind::Epsg, "x").epsg_code(),
            None
        );
    }

    #[test]
    fn serde_round_trip() {
        let mut d = CrsDefinition::new(
            CrsDefinitionKind::Wkt1Esri,
            r#"PROJCS["NZGD_2000_New_Zealand_Transverse_Mercator",GEOGCS["GCS_NZGD_2000"]]"#,
        );
        d.authority = Some(AuthorityCode {
            authority: "EPSG".into(),
            code: "2193".into(),
        });
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"wkt1_esri\""));
        assert_eq!(serde_json::from_str::<CrsDefinition>(&json).unwrap(), d);
        let r = ResolvedCrs::Identified {
            epsg: 2193,
            name: Some("NZGD2000 / New Zealand Transverse Mercator 2000".into()),
        };
        assert_eq!((r.status(), r.epsg()), (CrsStatus::Identified, Some(2193)));
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""status":"identified""#), "{json}");
        assert_eq!(serde_json::from_str::<ResolvedCrs>(&json).unwrap(), r);
        for r in [
            ResolvedCrs::Unparsed,
            ResolvedCrs::Unidentified { name: None },
        ] {
            assert_eq!(r.epsg(), None);
            assert_eq!(r.name(), None);
            let json = serde_json::to_string(&r).unwrap();
            assert!(json.contains(r.status().code()), "{json}");
            assert_eq!(serde_json::from_str::<ResolvedCrs>(&json).unwrap(), r);
        }
        for k in CrsDefinitionKind::ALL {
            assert_eq!(k.code().parse::<CrsDefinitionKind>().unwrap(), *k);
        }
    }
}
