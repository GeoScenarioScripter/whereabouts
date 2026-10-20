//! Pure-Rust CRS layer (D-09).
//!
//! Parses ESRI WKT1 / OGC WKT1 / WKT2 / PROJJSON / GeoTIFF GeoKeys into an internal model, identifies
//! EPSG codes (authority → name match → parameter match), and transforms densified bounds to WGS84
//! with `proj4rs`. Metres of error from ignoring datum grids are acceptable: this is a search index.
//!
//! Spec: `docs/04-formats-and-crs.md` §4. Filled in by stage01 sessions 1.4 and 2.4.
//!
//! # What is here (session 1.4)
//!
//! - [`parse`] / [`parse_bytes`]: any supported text, format auto-detected.
//! - [`parse_wkt`]: OGC WKT1 (01-009), the ESRI WKT1 dialect of `.prj` files, WKT2:2015 and
//!   WKT2:2019 (ISO 19162) — `[]` or `()` brackets, case-insensitive keywords, `""` escapes.
//! - [`parse_projjson`]: PROJJSON (GeoParquet).
//! - The model: [`Crs`] with [`Datum`] / [`Ellipsoid`] / [`PrimeMeridian`], [`Projection`] with
//!   canonical [`ProjectionMethod`] and [`ParamName`]s, [`Unit`]s, [`Axis`] / [`AxisOrder`],
//!   [`Authority`] — plus name normalisation in [`names`] (ESRI `GCS_*`/`D_*`, method and
//!   parameter aliases) for the identification step.
//!
//! Not here yet: EPSG identification and bbox transformation (session 2.4), GeoKeys (3.2).
//!
//! # Untrusted input
//!
//! CRS text comes from user files. Parsing never panics: input size ([`MAX_INPUT_LEN`]), nesting
//! depth ([`MAX_DEPTH`]) and node count ([`MAX_NODES`]) are bounded, every number is checked for
//! finiteness, and units/ellipsoids are validated. Failures are [`CrsError`]s.
//!
//! # Example
//!
//! ```
//! use whereabouts_crs::{CrsKind, ParamName, ProjectionMethod};
//!
//! let prj = r#"PROJCS["WGS_1984_UTM_Zone_33N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",
//!     SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],
//!     UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],
//!     PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],
//!     PARAMETER["Central_Meridian",15.0],PARAMETER["Scale_Factor",0.9996],
//!     PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
//! let crs = whereabouts_crs::parse(prj)?;
//! assert_eq!(crs.kind, CrsKind::Projected);
//! let projection = crs.projection.as_ref().expect("projected");
//! assert_eq!(projection.method, ProjectionMethod::TransverseMercator);
//! assert_eq!(projection.param(ParamName::LongitudeOfOrigin).and_then(|p| p.degrees()), Some(15.0));
//! # Ok::<(), whereabouts_crs::CrsError>(())
//! ```

mod error;
mod model;
pub mod names;
mod projjson;
mod wkt;

pub use error::CrsError;
pub use model::{
    Authority, Axis, AxisDirection, AxisOrder, BaseCrs, Crs, CrsKind, Datum, Dialect, Ellipsoid,
    Parameter, PrimeMeridian, Projection, Unit, UnitKind,
};
pub use names::{ParamClass, ParamName, ProjectionMethod};

/// Largest CRS definition accepted, in bytes (256 KiB). Real definitions are a few KiB; the
/// longest EPSG WKT2 strings (compound CRSs with usage text) stay well under 16 KiB.
pub const MAX_INPUT_LEN: usize = 256 * 1024;

/// Deepest WKT bracket nesting accepted. Real WKT2 nests about 8 levels (`BOUNDCRS` → `SOURCECRS`
/// → `PROJCRS` → `BASEGEOGCRS` → `DATUM` → `ELLIPSOID` → `LENGTHUNIT` → `ID`).
pub const MAX_DEPTH: usize = 32;

/// Most WKT keyword nodes accepted in one definition.
pub const MAX_NODES: usize = 16_384;

/// Parses a CRS definition in any supported text format: PROJJSON if it starts with `{`,
/// otherwise WKT (WKT1, ESRI WKT1 or WKT2, detected from the content).
pub fn parse(text: &str) -> Result<Crs, CrsError> {
    let text = prepare(text)?;
    if text.starts_with('{') {
        projjson::parse(text)
    } else {
        wkt::parse(text)
    }
}

/// [`parse`] for raw file bytes (`.prj` contents, SQLite text): strips a UTF-8 BOM and decodes
/// lossily — non-UTF-8 bytes (Latin-1 names in old `.prj` files) only ever appear inside names.
pub fn parse_bytes(bytes: &[u8]) -> Result<Crs, CrsError> {
    if bytes.len() > MAX_INPUT_LEN {
        return Err(CrsError::TooLarge {
            len: bytes.len(),
            limit: MAX_INPUT_LEN,
        });
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    parse(&String::from_utf8_lossy(bytes))
}

/// Parses WKT1 (OGC or ESRI) or WKT2 (2015 or 2019).
pub fn parse_wkt(text: &str) -> Result<Crs, CrsError> {
    wkt::parse(prepare(text)?)
}

/// Parses a PROJJSON document.
pub fn parse_projjson(text: &str) -> Result<Crs, CrsError> {
    projjson::parse(prepare(text)?)
}

/// Size check, BOM and whitespace/NUL trimming (fixed-width fields and some `.prj` writers pad
/// with NULs).
fn prepare(text: &str) -> Result<&str, CrsError> {
    if text.len() > MAX_INPUT_LEN {
        return Err(CrsError::TooLarge {
            len: text.len(),
            limit: MAX_INPUT_LEN,
        });
    }
    let text = text
        .trim_start_matches('\u{feff}')
        .trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if text.is_empty() {
        Err(CrsError::Empty)
    } else {
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_oversized_inputs_are_errors() {
        assert_eq!(parse(""), Err(CrsError::Empty));
        assert_eq!(parse(" \n\0\0"), Err(CrsError::Empty));
        assert_eq!(parse_bytes(b"\xEF\xBB\xBF  "), Err(CrsError::Empty));
        let big = "A".repeat(MAX_INPUT_LEN + 1);
        assert!(matches!(parse(&big), Err(CrsError::TooLarge { .. })));
        assert!(matches!(
            parse_bytes(big.as_bytes()),
            Err(CrsError::TooLarge { .. })
        ));
    }

    #[test]
    fn bytes_with_bom_and_latin1() {
        let mut bytes = b"\xEF\xBB\xBFGEOGCS[\"R\xE9seau\",DATUM[\"D_x\",SPHEROID[\"s\",6378137,298.257]],UNIT[\"Degree\",0.0174532925199433]]\0\0".to_vec();
        bytes.extend_from_slice(b"\n");
        let crs = parse_bytes(&bytes).expect("parses");
        assert_eq!(crs.kind, CrsKind::Geographic);
        assert_eq!(crs.dialect, Dialect::Wkt1Esri);
        assert!(crs.name.starts_with('R'));
    }
}
