//! Read operations and queries.

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use whereabouts_core::{
    AuthorityCode, Bounds, CrsDefinition, CrsDefinitionKind, CrsStatus, Dataset, DatasetId,
    DatasetKind, DatasetRecord, ExtractionStatus, FieldInfo, Format, Frn, GeometryType,
    LayerMetadata, LayerRecord, Member, MemberRole, RasterDataType, RasterInfo, ResolvedCrs,
    Timestamp, VolumeId, Wgs84Extent,
};
use whereabouts_source::{Checkpoint, SourceIdentity};

use crate::convert::{frn_from_sql, frn_to_sql, u32_from_sql, u64_from_sql, volume_from_sql};
use crate::error::{Result, corrupt};
use crate::schema::EXTENT_STRIDE;

/// A registered volume (source) as stored in the `volumes` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// Catalog-assigned id.
    pub id: VolumeId,
    /// The source's identity; matched on `kind` + `key`.
    pub identity: SourceIdentity,
    /// When the stored checkpoint was committed, if there is one.
    pub checkpoint_at: Option<Timestamp>,
}

/// Row counts, for progress output and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CatalogStats {
    /// Registered volumes.
    pub volumes: u64,
    /// Datasets.
    pub datasets: u64,
    /// Member files.
    pub files: u64,
    /// Layers.
    pub layers: u64,
    /// Distinct CRS definitions.
    pub crs: u64,
    /// R*Tree rows (WGS84 extent parts).
    pub extents: u64,
}

/// Dataset filter for [`crate::Catalog::find_datasets`]. Every criterion that is set must hold;
/// `epsg` and `bbox` must hold for the **same** layer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DatasetFilter {
    /// Only datasets on this volume.
    pub volume: Option<VolumeId>,
    /// Only datasets of one of these formats (empty: any format).
    pub formats: Vec<Format>,
    /// Only datasets with a layer whose CRS is identified as this EPSG code.
    pub epsg: Option<u32>,
    /// Only datasets with a layer whose WGS84 extent intersects this box. Build it with
    /// `Wgs84Extent::from_west_south_east_north`; a box that crosses the antimeridian has two
    /// parts and matches either.
    pub bbox: Option<Wgs84Extent>,
    /// At most this many results.
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------------------------------------
// Volumes

fn volume_row(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(i64, String, String, String, Option<i64>)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
}

fn volume_info(
    (id, kind, key, label, at): (i64, String, String, String, Option<i64>),
) -> Result<VolumeInfo> {
    Ok(VolumeInfo {
        id: volume_from_sql(id)?,
        identity: SourceIdentity { kind, key, label },
        checkpoint_at: at.map(Timestamp),
    })
}

pub(crate) fn volumes(conn: &Connection) -> Result<Vec<VolumeInfo>> {
    let mut stmt =
        conn.prepare_cached("SELECT id, kind, key, label, checkpoint_at FROM volumes ORDER BY id")?;
    let rows = stmt
        .query_map([], volume_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(volume_info).collect()
}

pub(crate) fn volume(conn: &Connection, id: VolumeId) -> Result<Option<VolumeInfo>> {
    conn.prepare_cached("SELECT id, kind, key, label, checkpoint_at FROM volumes WHERE id = ?1")?
        .query_row([id.0], volume_row)
        .optional()?
        .map(volume_info)
        .transpose()
}

pub(crate) fn find_volume(conn: &Connection, kind: &str, key: &str) -> Result<Option<VolumeInfo>> {
    conn.prepare_cached(
        "SELECT id, kind, key, label, checkpoint_at FROM volumes WHERE kind = ?1 AND key = ?2",
    )?
    .query_row([kind, key], volume_row)
    .optional()?
    .map(volume_info)
    .transpose()
}

/// `Ok(None)`: no checkpoint committed (or the volume is unknown). Undecodable bytes are
/// `Err(CatalogError::Checkpoint)`, so the caller can fall back to a full rescan.
pub(crate) fn checkpoint(conn: &Connection, id: VolumeId) -> Result<Option<Checkpoint>> {
    let bytes: Option<Vec<u8>> = conn
        .prepare_cached("SELECT checkpoint FROM volumes WHERE id = ?1")?
        .query_row([id.0], |r| r.get(0))
        .optional()?
        .flatten();
    Ok(bytes.map(|b| Checkpoint::from_bytes(&b)).transpose()?)
}

// ---------------------------------------------------------------------------------------------
// Datasets

pub(crate) fn get_dataset(conn: &Connection, id: DatasetId) -> Result<Option<DatasetRecord>> {
    type Row = (
        i64,
        String,
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        String,
        Option<String>,
        Option<i64>,
    );
    let row: Option<Row> = conn
        .prepare_cached(
            "SELECT id, kind, format, primary_path, name, total_size, mtime, status,
                    status_detail, extracted_at
             FROM datasets WHERE volume_id = ?1 AND anchor_frn = ?2",
        )?
        .query_row(params![id.volume.0, frn_to_sql(id.anchor)], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
            ))
        })
        .optional()?;
    let Some((row, kind, format, primary_path, name, size, mtime, status, detail, extracted)) = row
    else {
        return Ok(None);
    };

    let kind = DatasetKind::from_code(&kind).ok_or_else(|| corrupt(format!("kind {kind:?}")))?;
    let format = Format::from_code(&format).ok_or_else(|| corrupt(format!("format {format:?}")))?;
    let status = ExtractionStatus::from_parts(&status, detail.as_deref())
        .map_err(|e| corrupt(format!("dataset {id}: {e}")))?;

    let mut stmt = conn.prepare_cached(
        "SELECT frn, role FROM dataset_files WHERE dataset_id = ?1 ORDER BY rowid",
    )?;
    let raw = stmt
        .query_map([row], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let members = raw
        .into_iter()
        .map(|(frn, role)| {
            Ok(Member {
                frn: frn_from_sql(frn),
                role: MemberRole::from_code(&role)
                    .ok_or_else(|| corrupt(format!("member role {role:?}")))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let dataset = Dataset::new(id, kind, format, members)
        .map_err(|e| corrupt(format!("dataset {id}: {e}")))?;

    Ok(Some(DatasetRecord {
        dataset,
        name,
        primary_path,
        total_size: size.map(|s| u64_from_sql(s, "total_size")).transpose()?,
        mtime: mtime.map(Timestamp),
        status,
        extracted_at: extracted.map(Timestamp),
        layers: read_layers(conn, row)?,
    }))
}

/// One `layers` row joined with its CRS and raster rows, before decoding.
struct RawLayer {
    id: i64,
    ordinal: i64,
    name: String,
    geometry_type: Option<String>,
    feature_count: Option<i64>,
    bounds: [Option<f64>; 4],
    fields_json: String,
    wgs84_parts: i64,
    crs: Option<RawCrs>,
    raster: Option<RawRaster>,
}

struct RawCrs {
    def_kind: String,
    definition: String,
    authority: Option<String>,
    auth_code: Option<String>,
    status: String,
    epsg: Option<i64>,
    name: Option<String>,
}

struct RawRaster {
    width: i64,
    height: i64,
    bands: i64,
    dtype: String,
    res_x: Option<f64>,
    res_y: Option<f64>,
    nodata: Option<String>,
    overview_count: i64,
    geotransform: Option<String>,
}

fn read_layers(conn: &Connection, dataset_row: i64) -> Result<Vec<LayerRecord>> {
    let mut stmt = conn.prepare_cached(
        "SELECT l.id, l.ordinal, l.name, l.geometry_type, l.feature_count,
                l.minx, l.miny, l.maxx, l.maxy, l.fields_json, l.wgs84_parts,
                c.id, c.def_kind, c.definition, c.authority, c.auth_code, c.status, c.epsg, c.name,
                r.layer_id, r.width, r.height, r.bands, r.dtype, r.res_x, r.res_y, r.nodata,
                r.overview_count, r.geotransform
         FROM layers l
         LEFT JOIN crs c ON c.id = l.crs_id
         LEFT JOIN rasters r ON r.layer_id = l.id
         WHERE l.dataset_id = ?1
         ORDER BY l.ordinal",
    )?;
    let raws = stmt
        .query_map([dataset_row], |r| {
            let crs = match r.get::<_, Option<i64>>(11)? {
                None => None,
                Some(_) => Some(RawCrs {
                    def_kind: r.get(12)?,
                    definition: r.get(13)?,
                    authority: r.get(14)?,
                    auth_code: r.get(15)?,
                    status: r.get(16)?,
                    epsg: r.get(17)?,
                    name: r.get(18)?,
                }),
            };
            let raster = match r.get::<_, Option<i64>>(19)? {
                None => None,
                Some(_) => Some(RawRaster {
                    width: r.get(20)?,
                    height: r.get(21)?,
                    bands: r.get(22)?,
                    dtype: r.get(23)?,
                    res_x: r.get(24)?,
                    res_y: r.get(25)?,
                    nodata: r.get(26)?,
                    overview_count: r.get(27)?,
                    geotransform: r.get(28)?,
                }),
            };
            Ok(RawLayer {
                id: r.get(0)?,
                ordinal: r.get(1)?,
                name: r.get(2)?,
                geometry_type: r.get(3)?,
                feature_count: r.get(4)?,
                bounds: [r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?],
                fields_json: r.get(9)?,
                wgs84_parts: r.get(10)?,
                crs,
                raster,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut layers = Vec::with_capacity(raws.len());
    for (expected, raw) in (0_i64..).zip(raws) {
        if raw.ordinal != expected {
            return Err(corrupt(format!(
                "layer ordinals not contiguous: expected {expected}, found {}",
                raw.ordinal
            )));
        }
        layers.push(decode_layer(conn, raw)?);
    }
    Ok(layers)
}

fn decode_layer(conn: &Connection, raw: RawLayer) -> Result<LayerRecord> {
    let geometry_type = raw
        .geometry_type
        .map(|g| {
            g.parse::<GeometryType>()
                .map_err(|e| corrupt(format!("geometry type: {e}")))
        })
        .transpose()?;
    let native_bounds = match raw.bounds {
        [None, None, None, None] => None,
        [Some(a), Some(b), Some(c), Some(d)] => {
            Some(Bounds::new(a, b, c, d).map_err(|e| corrupt(format!("native bounds: {e}")))?)
        }
        _ => return Err(corrupt("partially NULL native bounds")),
    };
    let fields: Vec<FieldInfo> =
        serde_json::from_str(&raw.fields_json).map_err(|e| corrupt(format!("fields_json: {e}")))?;

    let (crs_def, resolved) = match raw.crs {
        None => (None, None),
        Some(c) => {
            let (def, res) = decode_crs(c)?;
            (Some(def), Some(res))
        }
    };
    let raster = raw.raster.map(decode_raster).transpose()?;
    let wgs84 = read_extent(conn, raw.id, raw.wgs84_parts)?;

    Ok(LayerRecord {
        metadata: LayerMetadata {
            name: raw.name,
            geometry_type,
            feature_count: raw
                .feature_count
                .map(|c| u64_from_sql(c, "feature_count"))
                .transpose()?,
            crs: crs_def,
            native_bounds,
            fields,
            raster,
        },
        crs: resolved,
        wgs84,
    })
}

fn decode_crs(c: RawCrs) -> Result<(CrsDefinition, ResolvedCrs)> {
    let kind = CrsDefinitionKind::from_code(&c.def_kind)
        .ok_or_else(|| corrupt(format!("crs def_kind {:?}", c.def_kind)))?;
    let authority = match (c.authority, c.auth_code) {
        (Some(authority), Some(code)) => Some(AuthorityCode { authority, code }),
        (None, None) => None,
        _ => return Err(corrupt("crs authority without code (or vice versa)")),
    };
    let def = CrsDefinition {
        kind,
        text: c.definition,
        authority,
    };
    let status = CrsStatus::from_code(&c.status)
        .ok_or_else(|| corrupt(format!("crs status {:?}", c.status)))?;
    let resolved = match (status, c.epsg) {
        (CrsStatus::Identified, Some(epsg)) => ResolvedCrs::Identified {
            epsg: u32_from_sql(epsg, "epsg")?,
            name: c.name,
        },
        (CrsStatus::Unidentified, None) => ResolvedCrs::Unidentified { name: c.name },
        (CrsStatus::Unparsed, None) => ResolvedCrs::Unparsed,
        (s, e) => return Err(corrupt(format!("crs status {s} with epsg {e:?}"))),
    };
    Ok((def, resolved))
}

fn decode_raster(r: RawRaster) -> Result<RasterInfo> {
    Ok(RasterInfo {
        width: u64_from_sql(r.width, "raster width")?,
        height: u64_from_sql(r.height, "raster height")?,
        bands: u32_from_sql(r.bands, "raster bands")?,
        dtype: RasterDataType::from_code(&r.dtype)
            .ok_or_else(|| corrupt(format!("raster dtype {:?}", r.dtype)))?,
        res_x: r.res_x,
        res_y: r.res_y,
        nodata: r.nodata,
        overview_count: u32_from_sql(r.overview_count, "overview_count")?,
        geotransform: r
            .geotransform
            .map(|g| {
                serde_json::from_str::<[f64; 6]>(&g)
                    .map_err(|e| corrupt(format!("geotransform: {e}")))
            })
            .transpose()?,
    })
}

fn read_extent(conn: &Connection, layer_row: i64, n_parts: i64) -> Result<Option<Wgs84Extent>> {
    if n_parts == 0 {
        return Ok(None);
    }
    if !(1..=EXTENT_STRIDE).contains(&n_parts) {
        return Err(corrupt(format!("wgs84_parts = {n_parts}")));
    }
    let base = layer_row
        .checked_mul(EXTENT_STRIDE)
        .ok_or_else(|| corrupt(format!("layer id {layer_row}")))?;
    let mut stmt = conn.prepare_cached(
        "SELECT west, south, east, north FROM layer_extents WHERE id = ?1 AND layer_id = ?2",
    )?;
    let mut parts = Vec::new();
    for i in 0..n_parts {
        let (w, s, e, n): (f64, f64, f64, f64) = stmt
            .query_row(params![base + i, layer_row], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()?
            .ok_or_else(|| corrupt(format!("layer {layer_row}: extent part {i} missing")))?;
        parts.push(Bounds::new(w, s, e, n).map_err(|e| corrupt(format!("extent: {e}")))?);
    }
    Wgs84Extent::new(parts)
        .map(Some)
        .map_err(|e| corrupt(format!("extent: {e}")))
}

/// Datasets that have `frn` as a member on `volume`, with the member's role.
pub(crate) fn datasets_with_member(
    conn: &Connection,
    volume: VolumeId,
    frn: Frn,
) -> Result<Vec<(DatasetId, MemberRole)>> {
    let mut stmt = conn.prepare_cached(
        // CROSS JOIN fixes the join order: drive from the FRN index. Without statistics the
        // planner otherwise scans the volume's datasets (26 ms at 100k datasets vs µs).
        "SELECT d.anchor_frn, f.role FROM dataset_files f
         CROSS JOIN datasets d ON d.id = f.dataset_id
         WHERE f.frn = ?1 AND d.volume_id = ?2
         ORDER BY d.id",
    )?;
    let raw = stmt
        .query_map(params![frn_to_sql(frn), volume.0], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(anchor, role)| {
            Ok((
                DatasetId::new(volume, frn_from_sql(anchor)),
                MemberRole::from_code(&role)
                    .ok_or_else(|| corrupt(format!("member role {role:?}")))?,
            ))
        })
        .collect()
}

/// Every dataset id on a volume (for reconciliation after a rescan).
pub(crate) fn dataset_ids(conn: &Connection, volume: VolumeId) -> Result<Vec<DatasetId>> {
    let mut stmt =
        conn.prepare_cached("SELECT anchor_frn FROM datasets WHERE volume_id = ?1 ORDER BY id")?;
    let ids = stmt
        .query_map([volume.0], |r| r.get::<_, i64>(0))?
        .map(|a| a.map(|a| DatasetId::new(volume, frn_from_sql(a))))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// Pending datasets, most recently modified first (the extraction priority).
pub(crate) fn pending_datasets(
    conn: &Connection,
    volume: Option<VolumeId>,
    limit: u32,
) -> Result<Vec<DatasetId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT volume_id, anchor_frn FROM datasets
         WHERE status = 'pending' AND (?1 IS NULL OR volume_id = ?1)
         ORDER BY mtime DESC NULLS LAST, id
         LIMIT ?2",
    )?;
    let raw = stmt
        .query_map(params![volume.map(|v| v.0), limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(v, a)| Ok(DatasetId::new(volume_from_sql(v)?, frn_from_sql(a))))
        .collect()
}

pub(crate) fn find_datasets(conn: &Connection, filter: &DatasetFilter) -> Result<Vec<DatasetId>> {
    let mut sql = String::from("SELECT d.volume_id, d.anchor_frn FROM datasets d WHERE 1 = 1");
    let mut args: Vec<Value> = Vec::new();

    if let Some(v) = filter.volume {
        sql.push_str(" AND d.volume_id = ?");
        args.push(Value::Integer(i64::from(v.0)));
    }
    if !filter.formats.is_empty() {
        sql.push_str(" AND d.format IN (");
        for (i, f) in filter.formats.iter().enumerate() {
            sql.push_str(if i == 0 { "?" } else { ", ?" });
            args.push(Value::Text(f.code().to_owned()));
        }
        sql.push(')');
    }
    if filter.epsg.is_some() || filter.bbox.is_some() {
        // Layer-level criteria: the same layer must satisfy all of them.
        sql.push_str(" AND d.id IN (SELECT l.dataset_id FROM layers l");
        if filter.epsg.is_some() {
            sql.push_str(" JOIN crs c ON c.id = l.crs_id");
        }
        sql.push_str(" WHERE 1 = 1");
        if let Some(epsg) = filter.epsg {
            sql.push_str(" AND c.epsg = ?");
            args.push(Value::Integer(i64::from(epsg)));
        }
        if let Some(bbox) = &filter.bbox {
            // One R*Tree probe per query part (UNION, not OR: an OR would defeat the R*Tree
            // index). R*Tree coordinates are rounded outwards; the auxiliary columns hold the
            // exact values, so the second half of each test is exact.
            sql.push_str(" AND l.id IN (");
            for (i, q) in bbox.parts().iter().enumerate() {
                if i > 0 {
                    sql.push_str(" UNION ");
                }
                sql.push_str(
                    "SELECT layer_id FROM layer_extents
                     WHERE minx <= ? AND maxx >= ? AND miny <= ? AND maxy >= ?
                       AND west <= ? AND east >= ? AND south <= ? AND north >= ?",
                );
                for _ in 0..2 {
                    args.extend([q.max_x(), q.min_x(), q.max_y(), q.min_y()].map(Value::Real));
                }
            }
            sql.push(')');
        }
        sql.push(')');
    }
    sql.push_str(" ORDER BY d.id");
    if let Some(limit) = filter.limit {
        sql.push_str(" LIMIT ?");
        args.push(Value::Integer(i64::from(limit)));
    }

    let mut stmt = conn.prepare_cached(&sql)?;
    let raw = stmt
        .query_map(params_from_iter(args), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(v, a)| Ok(DatasetId::new(volume_from_sql(v)?, frn_from_sql(a))))
        .collect()
}

pub(crate) fn stats(conn: &Connection) -> Result<CatalogStats> {
    let count = |table: &str| -> Result<u64> {
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        u64_from_sql(n, table)
    };
    Ok(CatalogStats {
        volumes: count("volumes")?,
        datasets: count("datasets")?,
        files: count("dataset_files")?,
        layers: count("layers")?,
        crs: count("crs")?,
        extents: count("layer_extents")?,
    })
}
