//! Shared corpus adapter for the same reader/CRS pipeline used by indexing and inspect.
use crate::adapters::ResolvedCase;
use crate::record::{NativeLayer, NativeRaster, NativeRecord, NativeStatus, NoData};
use std::path::{Path, PathBuf};
use whereabouts_core::{
    ExtractionStatus, Format, LayerRecord, MemberRole, ReadLimits, ResolvedDataset, ResolvedMember,
    Wgs84Extent,
};

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

pub(crate) fn read(dataset: &ResolvedDataset) -> NativeRecord {
    let extracted = whereabouts_indexer::extract(dataset, &ReadLimits::default());
    let (status, reason) = match extracted.status {
        ExtractionStatus::Ok => (NativeStatus::Ok, None),
        ExtractionStatus::Partial(reason) => (NativeStatus::Partial, Some(reason)),
        ExtractionStatus::Error(reason) => (NativeStatus::Error, Some(reason)),
        ExtractionStatus::Unsupported => (NativeStatus::Unsupported, None),
        ExtractionStatus::Pending => (
            NativeStatus::Error,
            Some("unexpected pending extraction".into()),
        ),
    };
    NativeRecord {
        status,
        reason,
        retry: extracted.retry,
        layers: extracted.layers.iter().map(layer).collect(),
    }
}

// The oracle uses a wrapping west > east box; core uses split boxes at +/-180.
fn bbox(extent: &Wgs84Extent) -> [f64; 4] {
    let parts = extent.parts();
    let south = parts
        .iter()
        .map(|p| p.min_y())
        .fold(f64::INFINITY, f64::min);
    let north = parts
        .iter()
        .map(|p| p.max_y())
        .fold(f64::NEG_INFINITY, f64::max);
    let (west, east) = if extent.is_split() {
        (
            parts
                .iter()
                .map(|p| p.min_x())
                .fold(f64::NEG_INFINITY, f64::max),
            parts
                .iter()
                .map(|p| p.max_x())
                .fold(f64::INFINITY, f64::min),
        )
    } else {
        (parts[0].min_x(), parts[0].max_x())
    };
    [west, south, east, north]
}

fn layer(record: &LayerRecord) -> NativeLayer {
    let l = &record.metadata;
    NativeLayer {
        name: l.name.clone(),
        // Core calls unconstrained/mixed geometry `Geometry`; GDAL calls it `Unknown`.
        geometry_type: l.geometry_type.map(|g| {
            if g.kind == whereabouts_core::GeometryKind::Geometry {
                "Unknown".to_owned()
            } else {
                g.kind.code().to_owned()
            }
        }),
        has_z: l.geometry_type.is_some_and(|g| g.has_z),
        has_m: l.geometry_type.is_some_and(|g| g.has_m),
        feature_count: l.feature_count,
        extent: l
            .native_bounds
            .map(|b| [b.min_x(), b.min_y(), b.max_x(), b.max_y()]),
        raster: l.raster.as_ref().map(|r| NativeRaster {
            width: r.width,
            height: r.height,
            band_count: r.bands,
            resolution: r.res_x.zip(r.res_y).map(|(x, y)| [x, y]),
            data_types: Some(vec![r.dtype.code().into(); r.bands as usize]),
            nodata: Some(vec![
                r.nodata
                    .as_ref()
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(NoData::from_f64);
                r.bands as usize
            ]),
        }),
        epsg: record.crs.as_ref().and_then(|c| c.epsg()),
        wgs84_extent: record.wgs84.as_ref().map(bbox),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unconstrained_geometry_alias_does_not_erase_specific_types() {
        use whereabouts_core::{GeometryKind, GeometryType, LayerMetadata};
        for (kind, expected) in [
            (GeometryKind::Geometry, "Unknown"),
            (GeometryKind::Point, "Point"),
            (GeometryKind::PolyhedralSurface, "PolyhedralSurface"),
        ] {
            let record = LayerRecord {
                metadata: LayerMetadata {
                    geometry_type: Some(GeometryType::xy(kind)),
                    ..LayerMetadata::default()
                },
                crs: None,
                wgs84: None,
            };
            assert_eq!(layer(&record).geometry_type.as_deref(), Some(expected));
        }
    }
    #[test]
    fn wrapping_extent_preserves_crossing_direction() {
        let extent =
            Wgs84Extent::from_west_south_east_north(170., -20., -175., 10.).expect("extent");
        assert_eq!(bbox(&extent), [170., -20., -175., 10.]);
        let world =
            Wgs84Extent::from_west_south_east_north(-180., -90., 180., 90.).expect("extent");
        assert_eq!(bbox(&world), [-180., -90., 180., 90.]);
    }
}
