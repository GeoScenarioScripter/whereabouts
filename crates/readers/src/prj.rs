//! `.prj` sidecars → raw [`CrsDefinition`]s.
//!
//! The text is passed on verbatim; the only interpretation is telling its syntax apart (ESRI
//! WKT1, OGC WKT1, WKT2, PROJJSON) so `whereabouts-crs` picks the right dialect. The rule mirrors
//! the oracle's `raw_kind` (`tests/corpus/scripts/golden.py`): a WKT1 definition is ESRI-flavoured
//! when it has no `AUTHORITY` node and uses Esri's `"GCS_…"` / `"D_…"` names.

use std::path::Path;

use whereabouts_core::{CrsDefinition, CrsDefinitionKind};

use crate::io::{Budget, SourceFile};

/// Largest `.prj` read. Real ones are well under 4 KiB (a compound CRS with a vertical part is
/// about 2 KiB); anything larger is not a projection file.
pub(crate) const MAX_PRJ_BYTES: usize = 16 * 1024;

/// Reads a `.prj` file. `Err` holds a reason for [`DatasetMetadata::partial`]: the file could not
/// be read, is empty, too large, or does not hold a recognised CRS syntax.
///
/// [`DatasetMetadata::partial`]: whereabouts_core::DatasetMetadata::partial
pub(crate) fn read_prj(path: &Path, budget: &Budget) -> Result<CrsDefinition, String> {
    let mut file = SourceFile::open(path, budget).map_err(|e| e.to_string())?;
    if file.len() > MAX_PRJ_BYTES as u64 {
        return Err(format!(
            ".prj is {} bytes, over the {MAX_PRJ_BYTES}-byte limit; CRS ignored",
            file.len()
        ));
    }
    let bytes = file.read_prefix(MAX_PRJ_BYTES).map_err(|e| e.to_string())?;
    parse_prj(&bytes)
}

/// Decodes and classifies `.prj` bytes.
pub(crate) fn parse_prj(bytes: &[u8]) -> Result<CrsDefinition, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    // Non-UTF-8 text is decoded as Windows-1252, like `whereabouts-crs` does (D-32).
    let (text, _) = crate::encoding::decode(bytes, None);
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if text.is_empty() {
        return Err("empty .prj; CRS unknown".to_owned());
    }
    match classify(text) {
        Some(kind) => Ok(CrsDefinition::new(kind, text)),
        None => Err("unrecognised .prj content (not WKT or PROJJSON); CRS ignored".to_owned()),
    }
}

/// The syntax of a CRS definition text, or `None` if it is not one we hand on.
pub(crate) fn classify(text: &str) -> Option<CrsDefinitionKind> {
    let text = text.trim_start();
    if text.starts_with('{') {
        return Some(CrsDefinitionKind::ProjJson);
    }
    let head_end = text.find(['[', '('])?;
    let head = text.get(..head_end)?.trim().to_ascii_uppercase();
    match head.as_str() {
        "PROJCRS" | "PROJECTEDCRS" | "GEOGCRS" | "GEOGRAPHICCRS" | "GEODCRS" | "GEODETICCRS"
        | "BOUNDCRS" | "COMPOUNDCRS" | "VERTCRS" | "VERTICALCRS" | "ENGCRS" | "ENGINEERINGCRS"
        | "DERIVEDPROJCRS" => Some(CrsDefinitionKind::Wkt2),
        "PROJCS" | "GEOGCS" | "GEOCCS" | "COMPD_CS" | "LOCAL_CS" | "VERT_CS" | "VERTCS" => {
            let upper = text.to_ascii_uppercase();
            let has_authority = upper.contains("AUTHORITY[") || upper.contains("AUTHORITY(");
            let esri_names = text.contains("\"GCS_") || text.contains("\"D_");
            Some(if !has_authority && esri_names {
                CrsDefinitionKind::Wkt1Esri
            } else {
                CrsDefinitionKind::Wkt1
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const ESRI: &str = r#"PROJCS["WGS_1984_UTM_Zone_33N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",15.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
    const OGC: &str = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433],AUTHORITY["EPSG","4326"]]"#;

    #[test]
    fn classifies_dialects() {
        assert_eq!(classify(ESRI), Some(CrsDefinitionKind::Wkt1Esri));
        assert_eq!(classify(OGC), Some(CrsDefinitionKind::Wkt1));
        // ESRI-looking names but with an AUTHORITY node: GDAL-written OGC WKT1.
        let mixed = r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137,298.257223563]],AUTHORITY["EPSG","4326"]]"#;
        assert_eq!(classify(mixed), Some(CrsDefinitionKind::Wkt1));
        // OGC WKT1 without authority or Esri names (custom CRS).
        let custom = r#"LOCAL_CS["Site grid",UNIT["metre",1]]"#;
        assert_eq!(classify(custom), Some(CrsDefinitionKind::Wkt1));
        assert_eq!(
            classify(r#"geogcs("GCS_WGS_1984",DATUM("D_WGS_1984"))"#),
            Some(CrsDefinitionKind::Wkt1Esri)
        );
        assert_eq!(
            classify(r#"PROJCRS["WGS 84 / UTM zone 33N",BASEGEOGCRS["WGS 84"]]"#),
            Some(CrsDefinitionKind::Wkt2)
        );
        assert_eq!(
            classify(r#"  GEOGCRS ["WGS 84"]"#),
            Some(CrsDefinitionKind::Wkt2)
        );
        assert_eq!(
            classify(r#"{"type":"GeographicCRS"}"#),
            Some(CrsDefinitionKind::ProjJson)
        );
        for other in [
            "EPSG:4326",
            "4326",
            "Projection UTM\nZone 33",
            "SPHEROID[\"x\"]",
            "[",
        ] {
            assert_eq!(classify(other), None, "{other:?}");
        }
    }

    #[test]
    fn parse_trims_bom_whitespace_and_nuls() {
        let mut bytes = b"\xEF\xBB\xBF \r\n".to_vec();
        bytes.extend_from_slice(ESRI.as_bytes());
        bytes.extend_from_slice(b"\r\n\0\0");
        let def = parse_prj(&bytes).unwrap();
        assert_eq!(def.kind, CrsDefinitionKind::Wkt1Esri);
        assert_eq!(def.text, ESRI);
        assert_eq!(def.authority, None);
    }

    #[test]
    fn parse_rejects_empty_and_garbage_and_decodes_latin() {
        assert!(parse_prj(b"").unwrap_err().contains("empty"));
        assert!(parse_prj(b" \r\n\0").unwrap_err().contains("empty"));
        assert!(parse_prj(b"\xFF\xFE\x00garbage").is_err());
        // Windows-1252 "é" in a name is decoded, not rejected.
        let def = parse_prj(b"GEOGCS[\"GCS_Caf\xE9\",DATUM[\"D_X\"]]").unwrap();
        assert!(def.text.contains("Café"));
    }

    #[test]
    fn oversized_prj_is_ignored_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.prj");
        let mut text = ESRI.to_owned();
        text.push_str(&" ".repeat(MAX_PRJ_BYTES));
        std::fs::write(&path, text).unwrap();
        let budget = Budget::new(1 << 20);
        let (result, stats) = crate::io::measure(|| read_prj(&path, &budget));
        assert!(result.unwrap_err().contains("limit"));
        assert_eq!(stats.bytes_read, 0);
    }
}
