//! GeoPackage metadata using bounded SQLite table-page reads. The format is specified at
//! <https://www.geopackage.org/spec/>. User databases are never opened by a SQL engine.

mod sqlite;

use std::path::Path;

use whereabouts_core::{
    AuthorityCode, Bounds, CrsDefinition, CrsDefinitionKind, DatasetMetadata, Format, FormatReader,
    GeometryKind, GeometryType, LayerMetadata, ReadError, ReadLimits, ResolvedDataset,
};

use crate::io::{Budget, SourceFile};
use sqlite::{Database, Table, Value, bad};

/// Read-only, bounded GeoPackage metadata reader (stage01/3.1).
///
/// Reads committed database pages without SQLite locks, `-wal`/`-shm` creation, or SQL execution.
/// An active WAL or rollback journal returns a transient I/O error: it cannot be read consistently
/// without locking. A checkpointed database works even in a directory without write permission.
#[derive(Debug, Clone, Copy, Default)]
pub struct GeoPackageReader;

impl FormatReader for GeoPackageReader {
    fn name(&self) -> &'static str {
        "geopackage"
    }
    fn formats(&self) -> &'static [Format] {
        &[Format::GeoPackage]
    }

    fn read(
        &self,
        dataset: &ResolvedDataset,
        limits: &ReadLimits,
    ) -> Result<DatasetMetadata, ReadError> {
        let budget = Budget::new(limits.max_bytes);
        let file = SourceFile::open(&dataset.anchor, &budget)?;
        if let Some(reason) = pending_transaction(&dataset.anchor)? {
            return Err(retry(&dataset.anchor, reason));
        }
        let mut db = Database::open(file, limits.time_budget)?;
        let (contents, rows) = db.rows("gpkg_contents")?;
        let mut md = DatasetMetadata::default();
        for row in rows {
            if md.layers.len() >= 1024 {
                md.partial
                    .push("GeoPackage layer limit (1024) exhausted".into());
                break;
            }
            let name = contents
                .get(&row, "table_name")
                .text()
                .ok_or_else(|| bad("missing layer table_name"))?;
            let kind = contents
                .get(&row, "data_type")
                .text()
                .ok_or_else(|| bad("missing layer data_type"))?;
            let mut layer = LayerMetadata {
                name: name.to_owned(),
                native_bounds: bounds(&contents, &row),
                ..LayerMetadata::default()
            };
            let result = read_layer(&mut db, &contents, &row, kind, &mut layer, &mut md.partial);
            if let Err(e) = result {
                md.partial.push(format!("{name}: {e}"));
            }
            md.layers.push(layer);
        }
        md.layers.sort_by(|a, b| a.name.cmp(&b.name));
        if !db.unchanged()? || pending_transaction(&dataset.anchor)?.is_some() {
            return Err(retry(
                &dataset.anchor,
                "SQLite changed during read; retry extraction",
            ));
        }
        Ok(md)
    }
}

fn retry(path: &Path, reason: impl Into<String>) -> ReadError {
    ReadError::Io {
        path: path.into(),
        source: std::io::Error::new(std::io::ErrorKind::WouldBlock, reason.into()),
    }
}

fn pending_transaction(path: &Path) -> Result<Option<String>, ReadError> {
    for suffix in ["-wal", "-journal"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        match std::fs::metadata(Path::new(&name)) {
            Ok(m) if m.len() > 0 => {
                return Ok(Some(format!(
                    "active SQLite {suffix}; defer until checkpointed"
                )));
            }
            Err(source) if source.kind() != std::io::ErrorKind::NotFound => {
                return Err(ReadError::Io {
                    path: name.into(),
                    source,
                });
            }
            _ => {}
        }
    }
    Ok(None)
}

fn bounds(table: &Table, row: &[Value]) -> Option<Bounds> {
    Bounds::new(
        table.get(row, "min_x").number()?,
        table.get(row, "min_y").number()?,
        table.get(row, "max_x").number()?,
        table.get(row, "max_y").number()?,
    )
    .ok()
}

fn read_layer(
    db: &mut Database<'_>,
    contents: &Table,
    row: &[Value],
    kind: &str,
    layer: &mut LayerMetadata,
    partial: &mut Vec<String>,
) -> Result<(), ReadError> {
    if let Some(id) = contents.get(row, "srs_id").int() {
        match crs(db, id) {
            Ok(crs) => layer.crs = crs,
            Err(e) => partial.push(format!("{}: CRS: {e}", layer.name)),
        }
    }
    match kind {
        "attributes" => {
            layer.native_bounds = None;
            layer.crs = None;
            layer.feature_count = Some(count(db, &layer.name, None)?);
        }
        "features" => {
            let (table, rows) = db.rows("gpkg_geometry_columns")?;
            let row = rows
                .iter()
                .find(|r| table.get(r, "table_name").text() == Some(&layer.name))
                .ok_or_else(|| bad("missing geometry_columns row"))?;
            let geometry = table
                .get(row, "column_name")
                .text()
                .ok_or_else(|| bad("missing geometry column"))?;
            let geometry_kind = table
                .get(row, "geometry_type_name")
                .text()
                .and_then(GeometryKind::parse_lenient)
                .ok_or_else(|| bad("unknown geometry type"))?;
            let dimension = |name| match table.get(row, name).int() {
                Some(0) => Ok(false),
                Some(1 | 2) => Ok(true),
                _ => Err(bad("invalid Z/M dimension flag")),
            };
            layer.geometry_type = Some(GeometryType {
                kind: geometry_kind,
                has_z: dimension("z")?,
                has_m: dimension("m")?,
            });
            let rtree = format!("rtree_{}_{}", layer.name, geometry);
            layer.feature_count = Some(count(db, &layer.name, Some(&rtree))?);
            if layer.feature_count == Some(0) {
                layer.native_bounds = None;
                return Ok(());
            }
            if db.tables.contains_key(&format!("{rtree}_node")) {
                let indexed = rtree_bounds(db, &rtree)?;
                // R-tree bounds are float32, rounded outwards. Ignore only that rounding when
                // deciding whether gpkg_contents is stale; otherwise trust the spatial index.
                if indexed.is_some_and(|b| layer.native_bounds.is_none_or(|d| !same_bounds(d, b))) {
                    layer.native_bounds = indexed;
                    partial.push(format!(
                        "{}: missing/stale contents bounds; R-tree fallback",
                        layer.name
                    ));
                    // Optional exact refinement reads only geometry envelopes (or Point's two
                    // doubles), never general coordinate arrays. The same byte/time caps apply.
                    match envelope_bounds(db, &layer.name, geometry) {
                        Ok(Some(exact)) => layer.native_bounds = Some(exact),
                        Ok(None) => {}
                        Err(e) => {
                            partial.push(format!("{}: exact envelope refinement: {e}", layer.name))
                        }
                    }
                }
            }
            if layer.native_bounds.is_none() {
                partial.push(format!("{}: no usable spatial bounds", layer.name));
            }
        }
        "tiles" | "2d-gridded-coverage" => {
            // Tiles/coverage remain layers even when pixel facts would require decoding imagery.
            if layer.native_bounds.is_none() {
                let (t, rows) = db.rows("gpkg_tile_matrix_set")?;
                layer.native_bounds = rows
                    .iter()
                    .find(|r| t.get(r, "table_name").text() == Some(&layer.name))
                    .and_then(|r| bounds(&t, r));
            }
            if layer.native_bounds.is_none() {
                partial.push(format!("{}: no tile matrix bounds", layer.name));
            }
        }
        _ => partial.push(format!("{}: unsupported contents type {kind}", layer.name)),
    }
    Ok(())
}

fn crs(db: &mut Database<'_>, id: i64) -> Result<Option<CrsDefinition>, ReadError> {
    if id <= 0 {
        return Ok(None);
    }
    let (t, rows) = db.rows("gpkg_spatial_ref_sys")?;
    let r = rows
        .iter()
        .find(|r| t.get(r, "srs_id").int() == Some(id))
        .ok_or_else(|| bad("missing spatial_ref_sys row"))?;
    let defined = |s: &str| !s.trim().is_empty() && !s.trim().eq_ignore_ascii_case("undefined");
    let wkt2 = t.get(r, "definition_12_063").text().filter(|s| defined(s));
    let wkt1 = t.get(r, "definition").text().filter(|s| defined(s));
    let mut definition = match wkt2.or(wkt1) {
        Some(text) => CrsDefinition::new(
            if wkt2.is_some() {
                CrsDefinitionKind::Wkt2
            } else {
                CrsDefinitionKind::Wkt1
            },
            text.trim(),
        ),
        None => return Ok(None),
    };
    if let (Some(authority), Some(code)) = (
        t.get(r, "organization").text(),
        t.get(r, "organization_coordsys_id").int(),
    ) && !authority.eq_ignore_ascii_case("NONE")
        && !authority.is_empty()
    {
        definition.authority = Some(AuthorityCode {
            authority: authority.into(),
            code: code.to_string(),
        });
    }
    Ok(Some(definition))
}

fn count(db: &mut Database<'_>, name: &str, rtree: Option<&str>) -> Result<u64, ReadError> {
    if db.tables.contains_key("gpkg_ogr_contents") {
        let (t, rows) = db.rows("gpkg_ogr_contents")?;
        if let Some(n) = rows
            .iter()
            .find(|r| t.get(r, "table_name").text() == Some(name))
            .and_then(|r| t.get(r, "feature_count").int())
            .and_then(|n| u64::try_from(n).ok())
        {
            return Ok(n);
        }
    }
    if let Some(rtree) = rtree {
        let rowid = format!("{rtree}_rowid");
        if db.tables.contains_key(&rowid) {
            return db.count(&rowid);
        }
    }
    db.count(name)
}

fn same_bounds(a: Bounds, b: Bounds) -> bool {
    [a.min_x(), a.min_y(), a.max_x(), a.max_y()]
        .into_iter()
        .zip([b.min_x(), b.min_y(), b.max_x(), b.max_y()])
        .all(|(a, b)| (a - b).abs() <= 2e-6 * a.abs().max(b.abs()).max(1.0))
}

fn union(a: Option<Bounds>, b: Bounds) -> Option<Bounds> {
    match a {
        None => Some(b),
        Some(a) => Bounds::new(
            a.min_x().min(b.min_x()),
            a.min_y().min(b.min_y()),
            a.max_x().max(b.max_x()),
            a.max_y().max(b.max_y()),
        )
        .ok(),
    }
}

fn rtree_bounds(db: &mut Database<'_>, name: &str) -> Result<Option<Bounds>, ReadError> {
    let (t, rows) = db.rows(&format!("{name}_node"))?;
    let mut out = None;
    for r in rows {
        let blob = t
            .get(&r, "data")
            .blob()
            .ok_or_else(|| bad("invalid R-tree node"))?;
        let header = blob.get(..4).ok_or_else(|| bad("short R-tree header"))?;
        let count = usize::from(u16::from_be_bytes([header[2], header[3]]));
        for i in 0..count {
            let cell = blob
                .get(4 + i * 24..4 + (i + 1) * 24)
                .ok_or_else(|| bad("short R-tree cell"))?;
            let mut xy = [0.0; 4];
            for (out, v) in xy.iter_mut().zip(cell[8..].as_chunks::<4>().0.iter()) {
                *out = f64::from(f32::from_be_bytes([v[0], v[1], v[2], v[3]]));
            }
            out = union(
                out,
                Bounds::new(xy[0], xy[2], xy[1], xy[3])
                    .map_err(|_| bad("invalid R-tree bounds"))?,
            );
        }
    }
    Ok(out)
}

fn envelope_bounds(
    db: &mut Database<'_>,
    name: &str,
    geometry: &str,
) -> Result<Option<Bounds>, ReadError> {
    let mut out = None;
    for blob in db.blob_prefixes(name, geometry)? {
        if let Some(b) = envelope(&blob)? {
            out = union(out, b);
        }
    }
    Ok(out)
}

fn envelope(blob: &[u8]) -> Result<Option<Bounds>, ReadError> {
    let header = blob
        .get(..8)
        .ok_or_else(|| bad("short GeoPackage geometry header"))?;
    if &header[..2] != b"GP" || header[2] != 0 {
        return Err(bad("invalid GeoPackage geometry header"));
    }
    let flags = header[3];
    if flags & 16 != 0 {
        return Ok(None);
    }
    let le = flags & 1 != 0;
    let double = |at: usize, le: bool| -> Result<f64, ReadError> {
        let b: [u8; 8] = blob
            .get(at..at + 8)
            .ok_or_else(|| bad("short geometry envelope"))?
            .try_into()
            .map_err(|_| bad("bad double"))?;
        Ok(if le {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        })
    };
    let mode = (flags >> 1) & 7;
    let b = match mode {
        1..=4 => Bounds::new(
            double(8, le)?,
            double(24, le)?,
            double(16, le)?,
            double(32, le)?,
        ),
        0 => {
            let wkb = blob.get(8..13).ok_or_else(|| bad("short Point WKB"))?;
            let le = match wkb[0] {
                0 => false,
                1 => true,
                _ => return Err(bad("invalid WKB byte order")),
            };
            let ty: [u8; 4] = wkb[1..5].try_into().map_err(|_| bad("short WKB type"))?;
            let ty = if le {
                u32::from_le_bytes(ty)
            } else {
                u32::from_be_bytes(ty)
            };
            if ty % 1000 != 1 {
                return Err(bad("geometry has no envelope; coordinate scan deferred"));
            }
            let x = double(13, le)?;
            let y = double(21, le)?;
            Bounds::new(x, y, x, y)
        }
        _ => return Err(bad("invalid geometry envelope indicator")),
    }
    .map_err(|_| bad("invalid geometry envelope bounds"))?;
    Ok(Some(b))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn geometry_envelopes_both_endian_and_point_without_envelope() {
        for le in [false, true] {
            let mut b = vec![b'G', b'P', 0, 2 | u8::from(le), 0, 0, 0, 0];
            for v in [1.0f64, 3.0, 2.0, 4.0] {
                b.extend(if le { v.to_le_bytes() } else { v.to_be_bytes() });
            }
            assert_eq!(envelope(&b).unwrap(), Bounds::new(1.0, 2.0, 3.0, 4.0).ok());
            for n in 0..b.len() {
                assert!(envelope(&b[..n]).is_err());
            }
            b[3] |= 16;
            assert_eq!(envelope(&b).unwrap(), None);
        }
        let mut point = b"GP\0\x01\0\0\0\0\x01\x01\0\0\0".to_vec();
        point.extend(1.25f64.to_le_bytes());
        point.extend((-3.5f64).to_le_bytes());
        assert_eq!(
            envelope(&point).unwrap(),
            Bounds::new(1.25, -3.5, 1.25, -3.5).ok()
        );
        point[3] = 14;
        assert!(envelope(&point).is_err());
    }
}
