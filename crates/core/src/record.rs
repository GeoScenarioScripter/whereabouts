//! The extracted state of a dataset as the catalog persists it.

use serde::{Deserialize, Serialize};

use crate::{
    CoreError, Dataset, ExtractionStatus, LayerId, LayerMetadata, ResolvedCrs, Timestamp,
    Wgs84Extent,
};

/// A layer after extraction: the reader's facts plus the resolved CRS and WGS84 extent.
///
/// Invariants (checked by [`LayerRecord::validate`]):
/// - `crs.is_some() == metadata.crs.is_some()`;
/// - `wgs84` is `Some` only if `metadata.native_bounds` is `Some` and `crs` is `Identified` or
///   `Unidentified` (an `Unparsed` or missing CRS cannot be transformed);
/// - a raster layer (`metadata.raster` is `Some`) has no `geometry_type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerRecord {
    /// What the reader returned for this layer.
    pub metadata: LayerMetadata,
    /// Resolution of `metadata.crs` (`None` iff the layer has no CRS definition).
    pub crs: Option<ResolvedCrs>,
    /// Extent in WGS84; `None` when there are no native bounds or the CRS could not be used.
    /// Layers without it are excluded from the spatial index but stay searchable by name.
    pub wgs84: Option<Wgs84Extent>,
}

impl LayerRecord {
    /// Checks the documented invariants.
    pub fn validate(&self) -> Result<(), CoreError> {
        let invalid = |msg: &str| Err(CoreError::InvalidRecord(msg.to_owned()));
        if self.crs.is_some() != self.metadata.crs.is_some() {
            return invalid("crs resolution present iff the layer has a CRS definition");
        }
        if self.wgs84.is_some() {
            if self.metadata.native_bounds.is_none() {
                return invalid("WGS84 extent without native bounds");
            }
            match &self.crs {
                Some(ResolvedCrs::Identified { .. } | ResolvedCrs::Unidentified { .. }) => {}
                Some(ResolvedCrs::Unparsed) | None => {
                    return invalid("WGS84 extent without a usable (parsed) CRS");
                }
            }
        }
        if self.metadata.raster.is_some() && self.metadata.geometry_type.is_some() {
            return invalid("raster layer with a geometry type");
        }
        Ok(())
    }
}

/// Everything the catalog stores about one dataset (one `datasets` row plus its `dataset_files`,
/// `layers`, `rasters`, `crs` and `layer_extents` rows). Produced by the indexer.
///
/// Fields are public for construction; the catalog calls [`DatasetRecord::validate`] before
/// persisting. Invariants:
/// - `layers` is empty unless `status` is `Ok` or `Partial`;
/// - `extracted_at` is `Some` unless `status` is `Pending` (a re-queued dataset may keep the time
///   of its previous extraction);
/// - at most `u32::MAX` layers (`LayerId::ordinal` is a `u32`);
/// - every layer passes [`LayerRecord::validate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetRecord {
    /// Identity, kind, format and member files.
    pub dataset: Dataset,
    /// Display name (anchor file name without the format's extension, or folder name).
    pub name: String,
    /// Full path of the anchor, for display and as a cache. Lossy for names that are not valid
    /// Unicode; the authoritative location is the anchor FRN (via the name index).
    pub primary_path: String,
    /// Sum of member file sizes in bytes, when known.
    pub total_size: Option<u64>,
    /// Latest modification time across members, when known. With `total_size`, the content stamp
    /// used to skip re-extraction.
    pub mtime: Option<Timestamp>,
    /// Extraction status.
    pub status: ExtractionStatus,
    /// When extraction last ran (`None` while `Pending` and never extracted).
    pub extracted_at: Option<Timestamp>,
    /// Extracted layers (empty unless `status` is `Ok` or `Partial`).
    pub layers: Vec<LayerRecord>,
}

impl DatasetRecord {
    /// Checks the documented invariants (including each layer's).
    pub fn validate(&self) -> Result<(), CoreError> {
        let has_layers_allowed = matches!(
            self.status,
            ExtractionStatus::Ok | ExtractionStatus::Partial(_)
        );
        if !self.layers.is_empty() && !has_layers_allowed {
            return Err(CoreError::InvalidRecord(format!(
                "status {} must not carry layers",
                self.status.code()
            )));
        }
        if self.extracted_at.is_none() && self.status != ExtractionStatus::Pending {
            return Err(CoreError::InvalidRecord(format!(
                "status {} requires extracted_at",
                self.status.code()
            )));
        }
        if u32::try_from(self.layers.len()).is_err() {
            return Err(CoreError::InvalidRecord("more than u32::MAX layers".into()));
        }
        for (i, layer) in self.layers.iter().enumerate() {
            layer.validate().map_err(|e| match e {
                CoreError::InvalidRecord(msg) => {
                    CoreError::InvalidRecord(format!("layer {i}: {msg}"))
                }
                other => other,
            })?;
        }
        Ok(())
    }

    /// IDs of the layers, in order (`ordinal` = index in `layers`).
    pub fn layer_ids(&self) -> impl Iterator<Item = LayerId> + '_ {
        let id = self.dataset.id();
        (0..self.layers.len()).map(move |i| LayerId::new(id, u32::try_from(i).unwrap_or(u32::MAX)))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::{
        Bounds, CrsDefinition, DatasetId, DatasetKind, Format, Frn, GeometryKind, GeometryType,
        Member, MemberRole, VolumeId,
    };

    #[test]
    fn dataset_record_round_trip() {
        let id = DatasetId::new(VolumeId(1), Frn(42));
        let dataset = Dataset::new(
            id,
            DatasetKind::Vector,
            Format::Shapefile,
            vec![
                Member {
                    frn: Frn(42),
                    role: MemberRole::Main,
                },
                Member {
                    frn: Frn(43),
                    role: MemberRole::Prj,
                },
            ],
        )
        .unwrap();
        let layer = LayerRecord {
            metadata: LayerMetadata {
                name: "parcels".into(),
                geometry_type: Some(GeometryType::xy(GeometryKind::Polygon)),
                feature_count: Some(10),
                crs: Some(CrsDefinition::epsg(2193)),
                native_bounds: Some(Bounds::new(1.7e6, 5.4e6, 1.8e6, 5.5e6).unwrap()),
                ..LayerMetadata::default()
            },
            crs: Some(ResolvedCrs::Identified {
                epsg: 2193,
                name: None,
            }),
            wgs84: Some(
                Wgs84Extent::from_west_south_east_north(174.0, -41.5, 175.0, -41.0).unwrap(),
            ),
        };
        let rec = DatasetRecord {
            dataset,
            name: "parcels".into(),
            primary_path: r"D:\gis\parcels.shp".into(),
            total_size: Some(123_456),
            mtime: Some(Timestamp(1_700_000_000_000_000_000)),
            status: ExtractionStatus::Ok,
            extracted_at: Some(Timestamp(1_700_000_001_000_000_000)),
            layers: vec![layer.clone(), layer],
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert_eq!(serde_json::from_str::<DatasetRecord>(&json).unwrap(), rec);
        let ids: Vec<_> = rec.layer_ids().collect();
        assert_eq!(ids, vec![LayerId::new(id, 0), LayerId::new(id, 1)]);
        rec.validate().unwrap();
    }

    fn valid_record() -> DatasetRecord {
        let id = DatasetId::new(VolumeId(1), Frn(42));
        let dataset = Dataset::new(
            id,
            DatasetKind::Vector,
            Format::Shapefile,
            vec![Member {
                frn: Frn(42),
                role: MemberRole::Main,
            }],
        )
        .unwrap();
        DatasetRecord {
            dataset,
            name: "parcels".into(),
            primary_path: r"D:\gis\parcels.shp".into(),
            total_size: Some(1),
            mtime: Some(Timestamp(1)),
            status: ExtractionStatus::Ok,
            extracted_at: Some(Timestamp(2)),
            layers: vec![LayerRecord {
                metadata: LayerMetadata {
                    name: "parcels".into(),
                    geometry_type: Some(GeometryType::xy(GeometryKind::Polygon)),
                    crs: Some(CrsDefinition::epsg(2193)),
                    native_bounds: Some(Bounds::new(1.7e6, 5.4e6, 1.8e6, 5.5e6).unwrap()),
                    ..LayerMetadata::default()
                },
                crs: Some(ResolvedCrs::Unidentified { name: None }),
                wgs84: Some(
                    Wgs84Extent::from_west_south_east_north(174.0, -41.5, 175.0, -41.0).unwrap(),
                ),
            }],
        }
    }

    fn assert_invalid(rec: &DatasetRecord, needle: &str) {
        match rec.validate() {
            Err(CoreError::InvalidRecord(msg)) => assert!(msg.contains(needle), "{msg}"),
            other => panic!("expected InvalidRecord containing {needle:?}, got {other:?}"),
        }
    }

    #[test]
    fn validate_dataset_level_invariants() {
        valid_record().validate().unwrap();

        for status in [
            ExtractionStatus::Pending,
            ExtractionStatus::Unsupported,
            ExtractionStatus::Error("x".into()),
        ] {
            let mut r = valid_record();
            r.status = status;
            assert_invalid(&r, "must not carry layers");
        }
        let mut r = valid_record();
        r.status = ExtractionStatus::Partial("stale bounds".into());
        r.validate().unwrap();

        let mut r = valid_record();
        r.extracted_at = None;
        assert_invalid(&r, "requires extracted_at");
        r.layers.clear();
        r.status = ExtractionStatus::Error("bad header".into());
        assert_invalid(&r, "requires extracted_at");
        r.status = ExtractionStatus::Pending;
        r.validate().unwrap();
        r.extracted_at = Some(Timestamp(5));
        r.validate().unwrap(); // re-queued after a previous extraction
    }

    #[test]
    fn validate_layer_level_invariants() {
        let mut r = valid_record();
        r.layers[0].crs = None;
        assert_invalid(&r, "layer 0: crs resolution");

        let mut r = valid_record();
        r.layers[0].metadata.crs = None;
        r.layers[0].crs = None;
        assert_invalid(&r, "usable (parsed) CRS");
        r.layers[0].wgs84 = None;
        r.validate().unwrap(); // no CRS: searchable by name, not spatially

        let mut r = valid_record();
        r.layers[0].crs = Some(ResolvedCrs::Unparsed);
        assert_invalid(&r, "usable (parsed) CRS");

        let mut r = valid_record();
        r.layers[0].metadata.native_bounds = None;
        assert_invalid(&r, "without native bounds");

        let mut r = valid_record();
        r.layers[0].metadata.raster = Some(crate::RasterInfo {
            width: 1,
            height: 1,
            bands: 1,
            dtype: crate::RasterDataType::Byte,
            res_x: None,
            res_y: None,
            nodata: None,
            overview_count: 0,
            geotransform: None,
        });
        assert_invalid(&r, "raster layer with a geometry type");
        r.layers[0].metadata.geometry_type = None;
        r.validate().unwrap();
    }
}
