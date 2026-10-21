//! The catalog: SQLite (WAL) tables for datasets, files, layers, rasters and CRS, plus an R*Tree of
//! WGS84 layer extents for the coarse spatial filter (D-10, D-16).
//!
//! Spec: `docs/03-architecture.md` §4, adapted to the frozen core types by stage01 session 2.5
//! (the schema delta is recorded in `docs/stages/stage01/sessions/2.5-catalog.md`).
//!
//! # Model
//! - The unit of storage is a [`DatasetRecord`]: a dataset with its member files and extracted
//!   layers. [`CatalogTx::upsert_dataset`] validates it (`DatasetRecord::validate`) and replaces
//!   whatever was stored for the same [`DatasetId`] (volume + anchor FRN, D-27), members and
//!   layers included, in the caller's transaction.
//! - Layers keep their reader order (`LayerId.ordinal`). Each layer's WGS84 extent is stored as
//!   one R*Tree row per part, so an extent split at the antimeridian has two rows.
//! - CRS definitions are de-duplicated by a stable hash of kind + text + authority. The stored
//!   resolution (status, EPSG, name) belongs to the definition; the latest write wins.
//! - Volumes are registered by `SourceIdentity` (kind + key) and get a catalog-assigned
//!   [`VolumeId`]. The last committed `Checkpoint` is stored per volume, so a batch of dataset
//!   changes and the checkpoint they bring the catalog to can commit atomically.
//!
//! # Usage
//! ```no_run
//! # fn main() -> Result<(), whereabouts_catalog::CatalogError> {
//! use whereabouts_catalog::{Catalog, DatasetFilter};
//! use whereabouts_core::Wgs84Extent;
//!
//! let mut catalog = Catalog::open("catalog.db")?;
//! // Writes go through a transaction; `write` commits on Ok and rolls back on Err.
//! catalog.write(|tx| {
//!     // tx.upsert_dataset(&record)?; tx.set_checkpoint(volume, &checkpoint)?;
//!     Ok(())
//! })?;
//! let hits = catalog.find_datasets(&DatasetFilter {
//!     // Crosses the antimeridian: matched as two boxes.
//!     bbox: Some(Wgs84Extent::from_west_south_east_north(170.0, -50.0, -170.0, -30.0).unwrap()),
//!     ..DatasetFilter::default()
//! })?;
//! # let _ = hits; Ok(()) }
//! ```
//!
//! # Durability
//! WAL mode with `synchronous = NORMAL`: a commit is atomic and the database stays consistent,
//! but the last commits can be lost on power failure. Callers that also persist state elsewhere
//! (the name-index snapshot) commit the catalog first and reconcile on mismatch.

use std::path::Path;
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use whereabouts_core::{
    DatasetId, DatasetRecord, ExtractionStatus, Frn, MemberRole, Timestamp, VolumeId,
};
use whereabouts_source::{Checkpoint, SourceIdentity};

mod convert;
mod error;
mod read;
mod schema;
mod write;

pub use error::{CatalogError, Result};
pub use read::{CatalogStats, DatasetFilter, VolumeInfo};
pub use schema::SCHEMA_VERSION;

/// How long a connection waits for a lock held by another connection before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A connection to a catalog database.
///
/// One writer at a time (SQLite); any number of readers can open the same file concurrently
/// thanks to WAL. Not `Sync`: open one `Catalog` per thread.
#[derive(Debug)]
pub struct Catalog {
    conn: Connection,
}

/// A write transaction. Dropped without [`commit`](CatalogTx::commit), it rolls back.
#[derive(Debug)]
pub struct CatalogTx<'a> {
    tx: rusqlite::Transaction<'a>,
}

/// Read methods shared by [`Catalog`] and [`CatalogTx`] (a transaction sees its own writes).
macro_rules! read_methods {
    () => {
        /// All registered volumes, by id.
        pub fn volumes(&self) -> Result<Vec<VolumeInfo>> {
            read::volumes(self.conn())
        }

        /// The volume with this id, if registered.
        pub fn volume(&self, id: VolumeId) -> Result<Option<VolumeInfo>> {
            read::volume(self.conn(), id)
        }

        /// The volume registered for this source identity (`kind` + `key`), if any.
        pub fn find_volume(&self, kind: &str, key: &str) -> Result<Option<VolumeInfo>> {
            read::find_volume(self.conn(), kind, key)
        }

        /// The last committed checkpoint of a volume. `Ok(None)` if there is none (or the volume
        /// is unknown); [`CatalogError::Checkpoint`] if the stored bytes don't decode, in which
        /// case the caller should rescan.
        pub fn checkpoint(&self, volume: VolumeId) -> Result<Option<Checkpoint>> {
            read::checkpoint(self.conn(), volume)
        }

        /// The stored record for a dataset. Members come back in the order they were written,
        /// layers by ordinal.
        pub fn get_dataset(&self, id: DatasetId) -> Result<Option<DatasetRecord>> {
            read::get_dataset(self.conn(), id)
        }

        /// The datasets that have `frn` as a member on `volume` (normally at most one), with the
        /// member's role. This is how a name-index hit becomes a dataset.
        pub fn datasets_with_member(
            &self,
            volume: VolumeId,
            frn: Frn,
        ) -> Result<Vec<(DatasetId, MemberRole)>> {
            read::datasets_with_member(self.conn(), volume, frn)
        }

        /// Every dataset on a volume, in insertion order.
        pub fn dataset_ids(&self, volume: VolumeId) -> Result<Vec<DatasetId>> {
            read::dataset_ids(self.conn(), volume)
        }

        /// Up to `limit` datasets with status `pending`, most recently modified first.
        pub fn pending_datasets(
            &self,
            volume: Option<VolumeId>,
            limit: u32,
        ) -> Result<Vec<DatasetId>> {
            read::pending_datasets(self.conn(), volume, limit)
        }

        /// Datasets matching every criterion of `filter`, in insertion order.
        pub fn find_datasets(&self, filter: &DatasetFilter) -> Result<Vec<DatasetId>> {
            read::find_datasets(self.conn(), filter)
        }

        /// Row counts.
        pub fn stats(&self) -> Result<CatalogStats> {
            read::stats(self.conn())
        }
    };
}

impl Catalog {
    /// Opens (creating if needed) a catalog file read-write, enables WAL and foreign keys, and
    /// migrates the schema to [`SCHEMA_VERSION`].
    pub fn open(path: impl AsRef<Path>) -> Result<Catalog> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            tracing::warn!(mode, "catalog is not in WAL mode");
        }
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
        schema::migrate(&mut conn)?;
        Ok(Catalog { conn })
    }

    /// Opens an existing catalog read-only (e.g. for a query process while the indexer writes).
    /// The schema must be exactly [`SCHEMA_VERSION`]; nothing is migrated.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Catalog> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        schema::check_version(&conn)?;
        Ok(Catalog { conn })
    }

    /// Starts a write transaction (`BEGIN IMMEDIATE`, so it never fails half-way on a lock
    /// upgrade).
    pub fn transaction(&mut self) -> Result<CatalogTx<'_>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Ok(CatalogTx { tx })
    }

    /// Runs `f` in a write transaction: commits if it returns `Ok`, rolls back otherwise.
    pub fn write<T>(&mut self, f: impl FnOnce(&CatalogTx<'_>) -> Result<T>) -> Result<T> {
        let tx = self.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    fn conn(&self) -> &Connection {
        &self.conn
    }

    read_methods!();
}

impl CatalogTx<'_> {
    /// Commits the transaction.
    pub fn commit(self) -> Result<()> {
        Ok(self.tx.commit()?)
    }

    fn conn(&self) -> &Connection {
        &self.tx
    }

    read_methods!();

    /// Registers a source, or returns the id it already has (matched on `kind` + `key`; the
    /// label is updated).
    pub fn register_volume(&self, identity: &SourceIdentity) -> Result<VolumeId> {
        write::register_volume(self.conn(), identity)
    }

    /// Stores the checkpoint the catalog is consistent with, stamped with the current time.
    pub fn set_checkpoint(&self, volume: VolumeId, checkpoint: &Checkpoint) -> Result<()> {
        let now = Timestamp::from_system_time(SystemTime::now());
        write::set_checkpoint(self.conn(), volume, Some(checkpoint), now)
    }

    /// Forgets a volume's checkpoint (e.g. when its source requires a full rescan).
    pub fn clear_checkpoint(&self, volume: VolumeId) -> Result<()> {
        write::set_checkpoint(self.conn(), volume, None, None)
    }

    /// Deletes a volume and everything stored for it. Returns whether it existed.
    pub fn delete_volume(&self, volume: VolumeId) -> Result<bool> {
        write::delete_volume(self.conn(), volume)
    }

    /// Inserts or replaces a dataset with its members and layers.
    ///
    /// Validates the record first (`DatasetRecord::validate`); the volume must be registered.
    /// Upserting the same record twice leaves the catalog unchanged. A WGS84 extent with more
    /// than four parts is stored as the union of its parts.
    pub fn upsert_dataset(&self, record: &DatasetRecord) -> Result<()> {
        write::upsert_dataset(self.conn(), record)
    }

    /// Deletes a dataset with its members, layers, rasters and extents. Returns whether it
    /// existed.
    pub fn delete_dataset(&self, id: DatasetId) -> Result<bool> {
        write::delete_dataset(self.conn(), id)
    }

    /// Sets a dataset's extraction status. `extracted_at` is required except for `pending`.
    /// Statuses that carry no layers (`pending`, `unsupported`, `error`) drop the stored layers.
    /// Returns whether the dataset exists.
    pub fn set_status(
        &self,
        id: DatasetId,
        status: &ExtractionStatus,
        extracted_at: Option<Timestamp>,
    ) -> Result<bool> {
        write::set_status(self.conn(), id, status, extracted_at)
    }

    /// Updates a dataset's display name and path after its anchor was renamed or moved (no
    /// re-extraction needed). Returns whether the dataset exists.
    pub fn rename_dataset(&self, id: DatasetId, name: &str, primary_path: &str) -> Result<bool> {
        write::rename_dataset(self.conn(), id, name, primary_path)
    }
}
