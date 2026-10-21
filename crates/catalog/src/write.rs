//! Write operations. Every function runs on the caller's transaction; `CatalogTx` exposes them.

use rusqlite::{Connection, OptionalExtension, params};
use whereabouts_core::{
    Bounds, CrsDefinition, DatasetId, DatasetRecord, ExtractionStatus, LayerRecord, ResolvedCrs,
    Timestamp, VolumeId,
};
use whereabouts_source::{Checkpoint, SourceIdentity};

use crate::convert::{crs_hash, finite, frn_to_sql, u64_to_sql, volume_from_sql};
use crate::error::{CatalogError, Result};
use crate::schema::EXTENT_STRIDE;

// ---------------------------------------------------------------------------------------------
// Volumes

pub(crate) fn register_volume(conn: &Connection, identity: &SourceIdentity) -> Result<VolumeId> {
    let id: i64 = conn
        .prepare_cached(
            "INSERT INTO volumes (kind, key, label) VALUES (?1, ?2, ?3)
             ON CONFLICT (kind, key) DO UPDATE SET label = excluded.label
             RETURNING id",
        )?
        .query_row(params![identity.kind, identity.key, identity.label], |r| {
            r.get(0)
        })?;
    match volume_from_sql(id) {
        Ok(v) => Ok(v),
        Err(_) => Err(CatalogError::OutOfRange(format!("volume id {id}"))),
    }
}

pub(crate) fn ensure_volume(conn: &Connection, volume: VolumeId) -> Result<()> {
    let found = conn
        .prepare_cached("SELECT 1 FROM volumes WHERE id = ?1")?
        .query_row([volume.0], |_| Ok(()))
        .optional()?;
    found.ok_or(CatalogError::UnknownVolume(volume))
}

pub(crate) fn set_checkpoint(
    conn: &Connection,
    volume: VolumeId,
    checkpoint: Option<&Checkpoint>,
    at: Option<Timestamp>,
) -> Result<()> {
    let n = conn
        .prepare_cached("UPDATE volumes SET checkpoint = ?2, checkpoint_at = ?3 WHERE id = ?1")?
        .execute(params![
            volume.0,
            checkpoint.map(Checkpoint::to_bytes),
            at.map(|t| t.0)
        ])?;
    if n == 0 {
        return Err(CatalogError::UnknownVolume(volume));
    }
    Ok(())
}

pub(crate) fn delete_volume(conn: &Connection, volume: VolumeId) -> Result<bool> {
    let n = conn
        .prepare_cached("DELETE FROM volumes WHERE id = ?1")?
        .execute([volume.0])?;
    if n > 0 {
        conn.prepare_cached(
            "DELETE FROM crs WHERE NOT EXISTS (SELECT 1 FROM layers WHERE layers.crs_id = crs.id)",
        )?
        .execute([])?;
    }
    Ok(n > 0)
}

// ---------------------------------------------------------------------------------------------
// Datasets

fn dataset_rowid(conn: &Connection, id: DatasetId) -> Result<Option<i64>> {
    Ok(conn
        .prepare_cached("SELECT id FROM datasets WHERE volume_id = ?1 AND anchor_frn = ?2")?
        .query_row(params![id.volume.0, frn_to_sql(id.anchor)], |r| r.get(0))
        .optional()?)
}

/// Inserts or replaces a dataset with its members and layers. The surrogate row id is kept.
pub(crate) fn upsert_dataset(conn: &Connection, rec: &DatasetRecord) -> Result<()> {
    rec.validate()?;
    let ds = &rec.dataset;
    ensure_volume(conn, ds.volume())?;
    let total_size = rec
        .total_size
        .map(|s| u64_to_sql(s, "total_size"))
        .transpose()?;
    let row: i64 = conn
        .prepare_cached(
            "INSERT INTO datasets (volume_id, anchor_frn, kind, format, primary_path, name,
                                   total_size, mtime, status, status_detail, extracted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (volume_id, anchor_frn) DO UPDATE SET
               kind = excluded.kind, format = excluded.format,
               primary_path = excluded.primary_path, name = excluded.name,
               total_size = excluded.total_size, mtime = excluded.mtime,
               status = excluded.status, status_detail = excluded.status_detail,
               extracted_at = excluded.extracted_at
             RETURNING id",
        )?
        .query_row(
            params![
                ds.volume().0,
                frn_to_sql(ds.anchor()),
                ds.kind().code(),
                ds.format().code(),
                rec.primary_path,
                rec.name,
                total_size,
                rec.mtime.map(|t| t.0),
                rec.status.code(),
                rec.status.detail().filter(|d| !d.is_empty()),
                rec.extracted_at.map(|t| t.0),
            ],
            |r| r.get(0),
        )?;

    conn.prepare_cached("DELETE FROM dataset_files WHERE dataset_id = ?1")?
        .execute([row])?;
    let mut insert_member = conn
        .prepare_cached("INSERT INTO dataset_files (dataset_id, frn, role) VALUES (?1, ?2, ?3)")?;
    for m in ds.members() {
        insert_member.execute(params![row, frn_to_sql(m.frn), m.role.code()])?;
    }

    let old_crs = drop_layers(conn, row)?;
    for (ordinal, layer) in rec.layers.iter().enumerate() {
        insert_layer(conn, row, ordinal, layer)?;
    }
    prune_crs(conn, &old_crs)
}

/// Deletes a dataset (members, layers, rasters and extents cascade). Returns whether it existed.
pub(crate) fn delete_dataset(conn: &Connection, id: DatasetId) -> Result<bool> {
    let Some(row) = dataset_rowid(conn, id)? else {
        return Ok(false);
    };
    let old_crs = layer_crs_ids(conn, row)?;
    conn.prepare_cached("DELETE FROM datasets WHERE id = ?1")?
        .execute([row])?;
    prune_crs(conn, &old_crs)?;
    Ok(true)
}

/// Sets the extraction status. Statuses that carry no layers (`pending`, `unsupported`,
/// `error`) drop the dataset's layers, so the stored dataset always satisfies
/// `DatasetRecord::validate`.
pub(crate) fn set_status(
    conn: &Connection,
    id: DatasetId,
    status: &ExtractionStatus,
    extracted_at: Option<Timestamp>,
) -> Result<bool> {
    if extracted_at.is_none() && *status != ExtractionStatus::Pending {
        return Err(CatalogError::InvalidRecord(
            whereabouts_core::CoreError::InvalidRecord(format!(
                "status {} requires extracted_at",
                status.code()
            )),
        ));
    }
    let Some(row) = dataset_rowid(conn, id)? else {
        return Ok(false);
    };
    conn.prepare_cached(
        "UPDATE datasets SET status = ?2, status_detail = ?3, extracted_at = ?4 WHERE id = ?1",
    )?
    .execute(params![
        row,
        status.code(),
        status.detail().filter(|d| !d.is_empty()),
        extracted_at.map(|t| t.0)
    ])?;
    if !matches!(status, ExtractionStatus::Ok | ExtractionStatus::Partial(_)) {
        let old_crs = drop_layers(conn, row)?;
        prune_crs(conn, &old_crs)?;
    }
    Ok(true)
}

/// Updates name and path after an anchor rename/move that needs no re-extraction.
pub(crate) fn rename_dataset(
    conn: &Connection,
    id: DatasetId,
    name: &str,
    primary_path: &str,
) -> Result<bool> {
    let n = conn
        .prepare_cached(
            "UPDATE datasets SET name = ?3, primary_path = ?4
             WHERE volume_id = ?1 AND anchor_frn = ?2",
        )?
        .execute(params![
            id.volume.0,
            frn_to_sql(id.anchor),
            name,
            primary_path
        ])?;
    Ok(n > 0)
}

// ---------------------------------------------------------------------------------------------
// Layers

fn layer_crs_ids(conn: &Connection, dataset_row: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT crs_id FROM layers WHERE dataset_id = ?1 AND crs_id IS NOT NULL",
    )?;
    let ids = stmt
        .query_map([dataset_row], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(ids)
}

/// Deletes a dataset's layers (rasters cascade, extents via trigger); returns their CRS ids.
fn drop_layers(conn: &Connection, dataset_row: i64) -> Result<Vec<i64>> {
    let old = layer_crs_ids(conn, dataset_row)?;
    conn.prepare_cached("DELETE FROM layers WHERE dataset_id = ?1")?
        .execute([dataset_row])?;
    Ok(old)
}

/// Removes CRS rows that no layer references any more.
fn prune_crs(conn: &Connection, ids: &[i64]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "DELETE FROM crs WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM layers WHERE crs_id = ?1)",
    )?;
    for id in ids {
        stmt.execute([id])?;
    }
    Ok(())
}

fn insert_layer(
    conn: &Connection,
    dataset_row: i64,
    ordinal: usize,
    layer: &LayerRecord,
) -> Result<()> {
    let md = &layer.metadata;
    let ordinal = i64::try_from(ordinal)
        .map_err(|_| CatalogError::OutOfRange(format!("layer ordinal {ordinal}")))?;
    let crs_id = match (&md.crs, &layer.crs) {
        (Some(def), Some(resolved)) => Some(crs_row(conn, def, resolved)?),
        _ => None,
    };
    let feature_count = md
        .feature_count
        .map(|c| u64_to_sql(c, "feature_count"))
        .transpose()?;
    let nb = md.native_bounds;
    let fields = serde_json::to_string(&md.fields)
        .map_err(|e| CatalogError::OutOfRange(format!("fields: {e}")))?;
    let parts = layer.wgs84.as_ref().map(|e| extent_rows(e.parts()));
    let n_parts = parts.as_ref().map_or(0, Vec::len);

    let layer_row: i64 = conn
        .prepare_cached(
            "INSERT INTO layers (dataset_id, ordinal, name, geometry_type, feature_count, crs_id,
                                 minx, miny, maxx, maxy, fields_json, wgs84_parts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             RETURNING id",
        )?
        .query_row(
            params![
                dataset_row,
                ordinal,
                md.name,
                md.geometry_type.map(|g| g.to_string()),
                feature_count,
                crs_id,
                nb.map(|b| b.min_x()),
                nb.map(|b| b.min_y()),
                nb.map(|b| b.max_x()),
                nb.map(|b| b.max_y()),
                fields,
                n_parts as i64,
            ],
            |r| r.get(0),
        )?;

    if let Some(r) = &md.raster {
        let geotransform = r
            .geotransform
            .filter(|gt| gt.iter().all(|v| v.is_finite()))
            .map(|gt| serde_json::to_string(&gt))
            .transpose()
            .map_err(|e| CatalogError::OutOfRange(format!("geotransform: {e}")))?;
        conn.prepare_cached(
            "INSERT INTO rasters (layer_id, width, height, bands, dtype, res_x, res_y, nodata,
                                  overview_count, geotransform)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?
        .execute(params![
            layer_row,
            u64_to_sql(r.width, "raster width")?,
            u64_to_sql(r.height, "raster height")?,
            r.bands,
            r.dtype.code(),
            finite(r.res_x),
            finite(r.res_y),
            r.nodata,
            r.overview_count,
            geotransform,
        ])?;
    }

    if let Some(parts) = parts {
        let base = layer_row
            .checked_mul(EXTENT_STRIDE)
            .ok_or_else(|| CatalogError::OutOfRange(format!("layer id {layer_row}")))?;
        let mut stmt = conn.prepare_cached(
            "INSERT INTO layer_extents (id, minx, maxx, miny, maxy,
                                        layer_id, west, south, east, north)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?2, ?4, ?3, ?5)",
        )?;
        for (i, p) in (0_i64..).zip(&parts) {
            stmt.execute(params![
                base + i,
                p.min_x(),
                p.max_x(),
                p.min_y(),
                p.max_y(),
                layer_row
            ])?;
        }
    }
    Ok(())
}

/// The parts to store: as given, or — beyond [`EXTENT_STRIDE`] parts — their union, which is a
/// superset and so still correct for a coarse filter.
fn extent_rows(parts: &[Bounds]) -> Vec<Bounds> {
    let max = usize::try_from(EXTENT_STRIDE).unwrap_or(1);
    if parts.len() <= max {
        return parts.to_vec();
    }
    tracing::warn!(
        parts = parts.len(),
        "WGS84 extent has more parts than the catalog stores; storing their union"
    );
    parts
        .iter()
        .copied()
        .reduce(|acc, p| acc.union(&p))
        .into_iter()
        .collect()
}

/// Finds or inserts the `crs` row for a definition. The resolution (status, EPSG, name) is a
/// property of the definition: the latest one written wins, so improved identification in a
/// newer build upgrades every layer sharing the definition.
fn crs_row(conn: &Connection, def: &CrsDefinition, resolved: &ResolvedCrs) -> Result<i64> {
    let hash = crs_hash(def);
    let authority = def.authority.as_ref().map(|a| a.authority.as_str());
    let auth_code = def.authority.as_ref().map(|a| a.code.as_str());
    let status = resolved.status().code();
    let epsg = resolved.epsg();
    let name = resolved.name();

    let existing: Option<(i64, String, Option<u32>, Option<String>)> = conn
        .prepare_cached(
            "SELECT id, status, epsg, name FROM crs
             WHERE def_hash = ?1 AND def_kind = ?2 AND definition = ?3
               AND authority IS ?4 AND auth_code IS ?5",
        )?
        .query_row(
            params![hash, def.kind.code(), def.text, authority, auth_code],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;

    match existing {
        Some((id, s, e, n)) => {
            if s != status || e != epsg || n.as_deref() != name {
                conn.prepare_cached(
                    "UPDATE crs SET status = ?2, epsg = ?3, name = ?4 WHERE id = ?1",
                )?
                .execute(params![id, status, epsg, name])?;
            }
            Ok(id)
        }
        None => Ok(conn
            .prepare_cached(
                "INSERT INTO crs (def_hash, def_kind, definition, authority, auth_code,
                                  status, epsg, name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 RETURNING id",
            )?
            .query_row(
                params![
                    hash,
                    def.kind.code(),
                    def.text,
                    authority,
                    auth_code,
                    status,
                    epsg,
                    name
                ],
                |r| r.get(0),
            )?),
    }
}
