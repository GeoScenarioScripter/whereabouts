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
//! # What is here (session 2.4)
//!
//! - An embedded EPSG table (`data/epsg.txt`, generated from PROJ's `proj.db` by
//!   `scripts/gen_epsg.py`): datums, geographic and projected CRSs with Esri aliases and a
//!   preferred Helmert shift to WGS 84. [`epsg_crs`] builds a [`Crs`] for a bare code.
//! - [`identify`]: authority code → name/alias match → parameter match ([`Identification`]).
//! - [`transform_bounds`] / [`Transformer`]: native bounds → [`whereabouts_core::Wgs84Extent`]
//!   with 21 points per edge, traditional GIS axis order by default ([`AxisPolicy`]),
//!   antimeridian split, poles, out-of-domain → partial. Projections come from `proj4rs`
//!   (TM, LCC, Albers, Mercator, Web Mercator, stereographic, LAEA, equal-area cylindrical,
//!   Krovak, Swiss oblique Mercator, Mollweide, Miller, geostationary) or from native inverses
//!   in this crate (Cassini, Hotine oblique Mercator, NZMG, Equal Earth, Robinson, Sinusoidal,
//!   ellipsoidal equidistant cylindrical). Other methods (azimuthal equidistant, polyconic,
//!   equidistant conic, orthographic, …) are [`TransformError::UnsupportedMethod`].
//! - [`resolve`]: a reader's [`whereabouts_core::CrsDefinition`] →
//!   [`whereabouts_core::ResolvedCrs`] + model.
//!
//! Not here yet: GeoKeys (3.2).
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

mod epsg;
mod error;
mod helmert;
mod identify;
mod model;
pub mod names;
mod native;
mod projjson;
mod resolve;
mod spec;
mod transform;
mod wkt;

pub use error::CrsError;
pub use identify::{Identification, MatchMethod, identify};
pub use model::{
    Authority, Axis, AxisDirection, AxisOrder, BaseCrs, Crs, CrsKind, Datum, Dialect, Ellipsoid,
    Parameter, PrimeMeridian, Projection, Unit, UnitKind,
};
pub use names::{ParamClass, ParamName, ProjectionMethod};
pub use resolve::{Resolution, resolve};
pub use transform::{
    AxisPolicy, DEFAULT_DENSIFY, MAX_DENSIFY, MIN_DENSIFY, TransformError, TransformOptions,
    TransformedBounds, Transformer, transform_bounds, transform_bounds_with,
};

/// Largest CRS definition accepted, in bytes (256 KiB). Real definitions are a few KiB; the
/// longest EPSG WKT2 strings (compound CRSs with usage text) stay well under 16 KiB.
pub const MAX_INPUT_LEN: usize = 256 * 1024;

/// Deepest WKT bracket nesting accepted. Real WKT2 nests about 8 levels (`BOUNDCRS` → `SOURCECRS`
/// → `PROJCRS` → `BASEGEOGCRS` → `DATUM` → `ELLIPSOID` → `LENGTHUNIT` → `ID`).
pub const MAX_DEPTH: usize = 32;

/// Most WKT keyword nodes accepted in one definition.
pub const MAX_NODES: usize = 16_384;

/// The geographic or projected CRS with this EPSG code, built from the embedded EPSG table
/// (`None` for codes the table does not have: vertical, geocentric, compound, unknown).
pub fn epsg_crs(code: u32) -> Option<Crs> {
    let t = epsg::table();
    t.crs(t.get(code)?)
}

/// The EPSG name of a geographic or projected CRS in the embedded table.
pub fn epsg_name(code: u32) -> Option<&'static str> {
    let t = epsg::table();
    t.name(t.get(code)?)
}

/// Provenance of the embedded EPSG table (dataset version and date, PROJ version).
pub fn epsg_table_source() -> &'static str {
    epsg::table().source
}

/// Parses a CRS definition in any supported text format: PROJJSON if it starts with `{`,
/// otherwise WKT (WKT1, ESRI WKT1 or WKT2, detected from the content).
pub fn parse(text: &str) -> Result<Crs, CrsError> {
    check_len(text.len())?;
    parse_unbounded(text)
}

/// [`parse`] without the size check, for text whose size the caller has bounded.
fn parse_unbounded(text: &str) -> Result<Crs, CrsError> {
    let text = trim(text)?;
    if text.starts_with('{') {
        projjson::parse(text)
    } else {
        wkt::parse(text)
    }
}

/// [`parse`] for raw file bytes (`.prj` contents, SQLite text).
///
/// A UTF-8 BOM is stripped. Valid UTF-8 is used as is; anything else is decoded as
/// Windows-1252 (a superset of Latin-1), which is what old `.prj` files with accented names
/// (`Réseau`) are written in. [`MAX_INPUT_LEN`] applies to the raw bytes; a decoded CP1252 byte
/// takes at most 3 bytes of UTF-8, so the parsed text is bounded by `3 × MAX_INPUT_LEN`.
pub fn parse_bytes(bytes: &[u8]) -> Result<Crs, CrsError> {
    check_len(bytes.len())?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => parse_unbounded(text),
        Err(_) => parse_unbounded(&decode_cp1252(bytes)),
    }
}

/// Windows-1252 → UTF-8. 0x00–0x7F are ASCII, 0xA0–0xFF map to U+00A0–U+00FF (as in
/// Latin-1), 0x80–0x9F use the CP1252 table; its five unassigned bytes map to the C1 control of
/// the same value (as the WHATWG encoding standard does).
fn decode_cp1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}',
        '\u{017D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}',
        '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}',
        '\u{9D}', '\u{017E}', '\u{0178}',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}

/// Parses WKT1 (OGC or ESRI) or WKT2 (2015 or 2019).
pub fn parse_wkt(text: &str) -> Result<Crs, CrsError> {
    check_len(text.len())?;
    wkt::parse(trim(text)?)
}

/// Parses a PROJJSON document.
pub fn parse_projjson(text: &str) -> Result<Crs, CrsError> {
    check_len(text.len())?;
    projjson::parse(trim(text)?)
}

fn check_len(len: usize) -> Result<(), CrsError> {
    if len > MAX_INPUT_LEN {
        Err(CrsError::TooLarge {
            len,
            limit: MAX_INPUT_LEN,
        })
    } else {
        Ok(())
    }
}

/// BOM and whitespace/NUL trimming (fixed-width fields and some `.prj` writers pad with NULs).
fn trim(text: &str) -> Result<&str, CrsError> {
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
        assert_eq!(crs.name, "Réseau");
    }

    #[test]
    fn cp1252_names_survive_and_utf8_is_untouched() {
        // Latin-1 / CP1252 bytes: é = 0xE9, € = 0x80 in CP1252, ’ = 0x92.
        let latin1 = b"GEOGCS[\"Nouvelle Triangulation Fran\xE7aise \x80 l\x92\xEEle\",DATUM[\"D_x\",SPHEROID[\"s\",6378137,298.257]]]";
        let crs = parse_bytes(latin1).expect("parses");
        assert_eq!(crs.name, "Nouvelle Triangulation Française € l’île");
        // Valid UTF-8 is not re-decoded.
        let utf8 = "GEOGCS[\"Réseau géodésique\",DATUM[\"D_x\",SPHEROID[\"s\",6378137,298.257]]]";
        assert_eq!(
            parse_bytes(utf8.as_bytes()).expect("parses").name,
            "Réseau géodésique"
        );
        // A full-size CP1252 input decodes to more than MAX_INPUT_LEN bytes of UTF-8 but is
        // still accepted (the limit applies to the raw input).
        let mut big = b"GEOGCS[\"".to_vec();
        let tail = b"\",DATUM[\"d\",SPHEROID[\"s\",6378137,298.257]]]";
        big.resize(MAX_INPUT_LEN - tail.len(), 0xE9);
        big.extend_from_slice(tail);
        let crs = parse_bytes(&big).expect("parses");
        assert!(crs.name.len() > MAX_INPUT_LEN);
    }
}
