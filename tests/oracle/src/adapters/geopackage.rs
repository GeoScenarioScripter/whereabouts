//! GeoPackage metadata and CRS through the shared extraction pipeline.

use whereabouts_core::Format;

use crate::adapters::{ResolvedCase, native};
use crate::compare::Check;
use crate::record::NativeRecord;

pub(crate) const CHECKS: &[Check] = &[
    Check::Status,
    Check::Layers,
    Check::GeometryType,
    Check::FeatureCount,
    Check::NativeExtent,
    Check::Epsg,
    Check::Wgs84Extent,
];

pub(crate) fn read(case: &ResolvedCase<'_>) -> Option<NativeRecord> {
    Some(native::read(&native::resolve(
        case,
        Format::GeoPackage,
        |_| None,
    )))
}
