//! Schema and migrations (`PRAGMA user_version`).
//!
//! Migration `n` (1-based) moves a database from `user_version = n - 1` to `n`. Released
//! migrations are append-only: never edit one, add a new one.

use rusqlite::{Connection, TransactionBehavior};

use crate::error::{CatalogError, Result};

/// Maximum number of WGS84 extent parts stored per layer. R*Tree row ids are
/// `layer_id * EXTENT_STRIDE + part`, so a layer's rows can be found (and deleted) by rowid
/// lookups without scanning. A layer with more parts is stored as their union (see `write.rs`).
pub(crate) const EXTENT_STRIDE: i64 = 4;

/// Schema version 1 — stage01 (`docs/03-architecture.md` §4 plus the stage01 delta).
const V1: &str = r#"
CREATE TABLE volumes (
  id            INTEGER PRIMARY KEY,          -- VolumeId (u32), catalog-assigned
  kind          TEXT NOT NULL,                -- SourceIdentity.kind, e.g. 'ntfs'
  key           TEXT NOT NULL,                -- SourceIdentity.key (NTFS: volume GUID path)
  label         TEXT NOT NULL,                -- SourceIdentity.label, e.g. 'D:'
  checkpoint    BLOB,                         -- Checkpoint::to_bytes() of the last commit
  checkpoint_at INTEGER,                      -- ns since Unix epoch
  UNIQUE (kind, key)
);

CREATE TABLE crs (
  id         INTEGER PRIMARY KEY,
  def_hash   INTEGER NOT NULL,                -- FNV-1a 64 of (def_kind, definition, authority, auth_code)
  def_kind   TEXT NOT NULL,                   -- CrsDefinitionKind code (incl. 'epsg')
  definition TEXT NOT NULL,                   -- raw WKT / PROJJSON / GeoKey summary / EPSG code
  authority  TEXT,                            -- CrsDefinition.authority (e.g. GPKG organization)
  auth_code  TEXT,
  status     TEXT NOT NULL,                   -- CrsStatus code: 'identified' | 'unidentified' | 'unparsed'
  epsg       INTEGER,                         -- NULL unless identified
  name       TEXT
);
CREATE INDEX crs_def_hash ON crs (def_hash);
CREATE INDEX crs_epsg ON crs (epsg);

CREATE TABLE datasets (
  id            INTEGER PRIMARY KEY,
  volume_id     INTEGER NOT NULL REFERENCES volumes (id) ON DELETE CASCADE,
  anchor_frn    INTEGER NOT NULL,             -- DatasetId.anchor, u64 bit-cast to i64
  kind          TEXT NOT NULL,                -- DatasetKind code
  format        TEXT NOT NULL,                -- Format code
  primary_path  TEXT NOT NULL,
  name          TEXT NOT NULL,
  total_size    INTEGER,
  mtime         INTEGER,                      -- ns since Unix epoch
  status        TEXT NOT NULL,                -- ExtractionStatus code
  status_detail TEXT,
  extracted_at  INTEGER,                      -- ns since Unix epoch
  UNIQUE (volume_id, anchor_frn)
);
CREATE INDEX datasets_format ON datasets (format);
CREATE INDEX datasets_status_mtime ON datasets (status, mtime);

CREATE TABLE dataset_files (
  dataset_id INTEGER NOT NULL REFERENCES datasets (id) ON DELETE CASCADE,
  frn        INTEGER NOT NULL,                -- Member.frn, u64 bit-cast to i64
  role       TEXT NOT NULL,                   -- MemberRole code
  PRIMARY KEY (dataset_id, frn)
);
CREATE INDEX dataset_files_frn ON dataset_files (frn);

CREATE TABLE layers (
  id            INTEGER PRIMARY KEY,
  dataset_id    INTEGER NOT NULL REFERENCES datasets (id) ON DELETE CASCADE,
  ordinal       INTEGER NOT NULL,             -- LayerId.ordinal = index in the reader's layer list
  name          TEXT NOT NULL,
  geometry_type TEXT,                         -- GeometryType display form, e.g. 'Polygon Z'
  feature_count INTEGER,
  crs_id        INTEGER REFERENCES crs (id),
  minx REAL, miny REAL, maxx REAL, maxy REAL, -- native CRS bounds (all NULL or all set)
  footprint     BLOB,                         -- simplified WGS84 footprint: NULL until M1 (D-16)
  fields_json   TEXT NOT NULL,                -- JSON array of FieldInfo
  wgs84_parts   INTEGER NOT NULL DEFAULT 0,   -- rows in layer_extents for this layer
  UNIQUE (dataset_id, ordinal)
);
CREATE INDEX layers_crs ON layers (crs_id);

CREATE TABLE rasters (
  layer_id       INTEGER PRIMARY KEY REFERENCES layers (id) ON DELETE CASCADE,
  width          INTEGER NOT NULL,
  height         INTEGER NOT NULL,
  bands          INTEGER NOT NULL,
  dtype          TEXT NOT NULL,               -- RasterDataType code
  res_x          REAL,
  res_y          REAL,
  nodata         TEXT,
  overview_count INTEGER NOT NULL,
  geotransform   TEXT                         -- JSON array of 6 numbers (GDAL order)
);

-- WGS84 extent parts (one row per antimeridian part) for the coarse spatial filter.
-- R*Tree coordinates are 32-bit floats rounded outwards; the auxiliary columns keep the exact
-- f64 values for the exact test and for reading the extent back.
CREATE VIRTUAL TABLE layer_extents USING rtree (
  id, minx, maxx, miny, maxy,
  +layer_id INTEGER, +west REAL, +south REAL, +east REAL, +north REAL
);

-- Virtual tables can't take part in foreign keys: drop a layer's extent rows with it (this also
-- fires for ON DELETE CASCADE from datasets and volumes). Rows are found by rowid, no scan.
CREATE TRIGGER layers_delete_extents AFTER DELETE ON layers WHEN OLD.wgs84_parts > 0 BEGIN
  DELETE FROM layer_extents
  WHERE id IN (OLD.id * 4, OLD.id * 4 + 1, OLD.id * 4 + 2, OLD.id * 4 + 3);
END;
"#;

/// All migrations, in order. `MIGRATIONS[i]` upgrades `user_version` `i` → `i + 1`.
const MIGRATIONS: &[&str] = &[V1];

/// The schema version this build reads and writes.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

pub(crate) fn user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Applies every pending migration, each in its own immediate transaction.
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    loop {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = user_version(&tx)?;
        if !(0..=SCHEMA_VERSION).contains(&current) {
            return Err(CatalogError::UnsupportedSchema {
                found: current,
                supported: SCHEMA_VERSION,
            });
        }
        let Some(sql) = usize::try_from(current)
            .ok()
            .and_then(|i| MIGRATIONS.get(i))
        else {
            return Ok(());
        };
        tracing::debug!(from = current, to = current + 1, "migrating catalog schema");
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", current + 1))?;
        tx.commit()?;
    }
}

/// Checks that a database opened without migrating is at exactly this build's version.
pub(crate) fn check_version(conn: &Connection) -> Result<()> {
    let found = user_version(conn)?;
    if found == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(CatalogError::UnsupportedSchema {
            found,
            supported: SCHEMA_VERSION,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_matches_trigger() {
        // The trigger hard-codes the stride; keep them in sync.
        assert_eq!(EXTENT_STRIDE, 4);
        assert!(V1.contains("OLD.id * 4 + 3"));
    }
}
