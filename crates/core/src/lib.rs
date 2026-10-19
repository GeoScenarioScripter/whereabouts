//! Shared domain types for Whereabouts.
//!
//! Datasets (not files) are the unit of search results (D-04). This crate holds the vocabulary every
//! other crate speaks: dataset/layer identities, bounds, CRS references, file identities (FRNs) and
//! extraction status. It has no dependency on any other workspace crate and does no I/O.
//!
//! Spec: `docs/03-architecture.md` §2, §4; contracts frozen by stage01 session 1.1
//! (`docs/stages/stage01/sessions/1.1-core-contracts.md`).
//!
//! # Contracts summary (for downstream sessions)
//!
//! **Identities** ([`ids`])
//! - [`VolumeId`]`(u32)` — catalog-assigned per registered source; not the NTFS serial.
//! - [`Frn`]`(u64)` — file reference number (48-bit record + 16-bit sequence on NTFS).
//! - [`DatasetId`]` { volume, anchor: Frn }` — stable across renames/moves of the anchor and
//!   sidecar changes; the catalog keeps an integer surrogate plus a unique `(volume_id, anchor_frn)`.
//! - [`LayerId`]` { dataset, ordinal }` — `ordinal` = index in [`DatasetMetadata::layers`].
//!
//! **Files** ([`mod@file`]) — [`FileRecord`]` { frn, parent, name: FileName, is_dir, attrs }`.
//! [`FileName`] is lossless WTF-8 (UTF-16 names with lone surrogates survive); `as_str()` borrows
//! for valid Unicode. [`FileAttributes`] are raw `FILE_ATTRIBUTE_*` bits.
//!
//! **Datasets** ([`dataset`], [`mod@format`]) — [`Dataset`] (validated: exactly one
//! [`MemberRole::Main`] member == anchor, no duplicate FRNs) with [`Format`], [`DatasetKind`],
//! [`Member`]` { frn, role }`. Enum codes (`Format::code()` etc.) are the catalog's TEXT values and
//! the serde form; they never change once released.
//!
//! **Geometry** ([`bounds`]) — [`Bounds`] (finite, `min <= max`) in native units;
//! [`Wgs84Extent`] = 1+ lon/lat parts, split at the antimeridian.
//!
//! **CRS** ([`crs`]) — readers return a raw [`CrsDefinition`]` { kind, text, authority }`;
//! `indexer::extract` resolves it via `whereabouts-crs` into a [`ResolvedCrs`].
//!
//! **Reader output** ([`metadata`]) — [`DatasetMetadata`]` { layers, partial }` of
//! [`LayerMetadata`]` { name, geometry_type, feature_count, crs, native_bounds, fields, raster }`.
//!
//! **Reader contract** ([`reader`]) — [`FormatReader::read`]`(&ResolvedDataset, &ReadLimits) ->
//! Result<DatasetMetadata, ReadError>`: bounded, read-only, never panics on file contents.
//!
//! **Catalog unit** ([`record`]) — [`DatasetRecord`] (dataset + name, path, content stamp,
//! [`ExtractionStatus`], [`Timestamp`]s, [`LayerRecord`]s with resolved CRS + WGS84 extent).
//!
//! **Errors** — [`CoreError`] for invariant violations/unknown codes; [`ReadError`] for readers.
//! The `Source` trait and its events live in `whereabouts-source`.

#![warn(missing_docs)]

mod code;

pub mod bounds;
pub mod crs;
pub mod dataset;
pub mod error;
pub mod file;
pub mod format;
pub mod ids;
pub mod metadata;
pub mod reader;
pub mod record;
pub mod status;

pub use bounds::{Bounds, Wgs84Extent};
pub use crs::{AuthorityCode, CrsDefinition, CrsDefinitionKind, CrsStatus, ResolvedCrs};
pub use dataset::{Dataset, Member, MemberRole};
pub use error::CoreError;
pub use file::{FileAttributes, FileName, FileRecord};
pub use format::{DatasetKind, Format};
pub use ids::{DatasetId, Frn, LayerId, VolumeId};
pub use metadata::{
    DatasetMetadata, FieldInfo, FieldKind, GeometryKind, GeometryType, LayerMetadata,
    RasterDataType, RasterInfo,
};
pub use reader::{FormatReader, ReadError, ReadLimits, ResolvedDataset, ResolvedMember};
pub use record::{DatasetRecord, LayerRecord};
pub use status::{ExtractionStatus, Timestamp};
