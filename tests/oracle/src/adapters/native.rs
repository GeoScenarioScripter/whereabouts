//! Shared adapter plumbing: build a [`ResolvedDataset`] from a corpus case and convert a native
//! reader's result into a [`NativeRecord`]. Used by every reader adapter.

use std::path::{Path, PathBuf};

use whereabouts_core::{
    DatasetMetadata, ExtractionStatus, Format, LayerMetadata, MemberRole, ReadError, ReadLimits,
    ResolvedDataset, ResolvedMember,
};

use crate::adapters::ResolvedCase;
use crate::record::{NativeLayer, NativeRaster, NativeRecord, NativeStatus};

/// Resolves `case` as a dataset of `format`: the manifest anchor is the anchor, every other file
/// is a sidecar whose role `role_of` picks from its path (`None` = not a member).
///
/// The oracle does its own (trivial) grouping from the manifest instead of calling the dataset
/// grouper, so reader agreement is measured independently of grouping.
pub(crate) fn resolve(
    case: &ResolvedCase<'_>,
    format: Format,
    role_of: impl Fn(&Path) -> Option<MemberRole>,
) -> ResolvedDataset {
    let anchor = case.anchor_path();
    let sidecars = case
        .file_paths()
        .into_iter()
        .filter(|p| *p != anchor)
        .filter_map(|path: PathBuf| role_of(&path).map(|role| ResolvedMember { role, path }))
        .collect();
    ResolvedDataset {
        format,
        anchor,
        sidecars,
    }
}

/// Reads `dataset` through the reader registry with default limits and converts the outcome.
pub(crate) fn read(dataset: &ResolvedDataset) -> NativeRecord {
    from_result(whereabouts_readers::read(dataset, &ReadLimits::default()))
}

/// Converts a reader's result. Errors carry no layers; EPSG and WGS84 stay `None` until the
/// adapters switch to `indexer::extract` (stage01/4.3).
pub(crate) fn from_result(result: Result<DatasetMetadata, ReadError>) -> NativeRecord {
    match result {
        Err(e) => NativeRecord::new(match e.status() {
            ExtractionStatus::Unsupported => NativeStatus::Unsupported,
            _ => NativeStatus::Error,
        }),
        Ok(md) => NativeRecord {
            status: if md.partial.is_empty() {
                NativeStatus::Ok
            } else {
                NativeStatus::Partial
            },
            layers: md.layers.iter().map(layer).collect(),
        },
    }
}

fn layer(l: &LayerMetadata) -> NativeLayer {
    NativeLayer {
        name: l.name.clone(),
        geometry_type: l.geometry_type.map(|g| g.kind.code().to_owned()),
        has_z: l.geometry_type.is_some_and(|g| g.has_z),
        has_m: l.geometry_type.is_some_and(|g| g.has_m),
        feature_count: l.feature_count,
        extent: l
            .native_bounds
            .map(|b| [b.min_x(), b.min_y(), b.max_x(), b.max_y()]),
        // Size and band count only; per-band types and nodata are left for the raster adapters
        // to fill in if their reader reports them per band.
        raster: l.raster.as_ref().map(|r| NativeRaster {
            width: r.width,
            height: r.height,
            band_count: r.bands,
            resolution: r.res_x.zip(r.res_y).map(|(x, y)| [x, y]),
            ..NativeRaster::default()
        }),
        epsg: None,
        wgs84_extent: None,
    }
}

#[cfg(test)]
mod tests {
    use whereabouts_core::{Bounds, GeometryKind, GeometryType};

    use super::*;

    #[test]
    fn converts_status_and_layers() {
        let err = from_result(Err(ReadError::MissingMember(MemberRole::Shx)));
        assert_eq!(err.status, NativeStatus::Error);
        assert!(err.layers.is_empty());
        let unsupported = from_result(Err(ReadError::Unsupported("x".into())));
        assert_eq!(unsupported.status, NativeStatus::Unsupported);

        let md = DatasetMetadata {
            layers: vec![LayerMetadata {
                name: "roads".into(),
                geometry_type: Some(GeometryType {
                    kind: GeometryKind::LineString,
                    has_z: true,
                    has_m: false,
                }),
                feature_count: Some(6),
                native_bounds: Bounds::new(1.0, 2.0, 3.0, 4.0).ok(),
                ..LayerMetadata::default()
            }],
            partial: vec!["something".into()],
        };
        let rec = from_result(Ok(md));
        assert_eq!(rec.status, NativeStatus::Partial);
        let l = &rec.layers[0];
        assert_eq!(l.name, "roads");
        assert_eq!(l.geometry_type.as_deref(), Some("LineString"));
        assert!(l.has_z && !l.has_m);
        assert_eq!(l.feature_count, Some(6));
        assert_eq!(l.extent, Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(l.raster, None);
    }
}
