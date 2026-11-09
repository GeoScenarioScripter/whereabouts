//! TIFF metadata adapter; CRS resolution is also checked at its actual GeoKeys hand-off.

use crate::adapters::{ResolvedCase, native};
use crate::compare::Check;
use crate::record::NativeRecord;
use whereabouts_core::{Format, MemberRole};

pub(crate) const CHECKS: &[Check] = &[
    Check::Status,
    Check::Layers,
    Check::NativeExtent,
    Check::Raster,
    Check::Epsg,
    Check::Wgs84Extent,
];

pub(crate) fn read(case: &ResolvedCase<'_>) -> Option<NativeRecord> {
    let ds = native::resolve(case, Format::GeoTiff, |path| {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        Some(if name.ends_with(".aux.xml") {
            MemberRole::AuxXml
        } else if name.ends_with(".ovr") {
            MemberRole::Ovr
        } else if name.ends_with(".msk") {
            MemberRole::Msk
        } else if name.ends_with(".prj") {
            MemberRole::Prj
        } else if [".tfw", ".tifw", ".tiffw", ".wld"]
            .iter()
            .any(|e| name.ends_with(e))
        {
            MemberRole::WorldFile
        } else {
            MemberRole::Other
        })
    });
    Some(native::read(&ds))
}
