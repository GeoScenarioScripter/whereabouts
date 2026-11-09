//! TIFF metadata adapter; CRS resolution is also checked at its actual GeoKeys hand-off.

use crate::adapters::{ResolvedCase, native};
use crate::compare::Check;
use crate::record::{NativeRecord, NoData};
use whereabouts_core::{Format, MemberRole, ReadLimits};

pub(crate) const CHECKS: &[Check] = &[
    Check::Status,
    Check::Layers,
    Check::NativeExtent,
    Check::Raster,
    Check::Epsg,
];

pub(crate) fn read(case: &ResolvedCase<'_>) -> Option<NativeRecord> {
    let ds = native::resolve(case, Format::GeoTiff, |path| {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        Some(if name.ends_with(".aux.xml") {
            MemberRole::AuxXml
        } else if name.ends_with(".ovr") {
            MemberRole::Ovr
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
    let result = whereabouts_readers::read(&ds, &ReadLimits::default());
    let Ok(md) = result else {
        return Some(native::from_result(result));
    };
    let mut rec = native::from_result(Ok(md.clone()));
    {
        for (layer, raw) in rec.layers.iter_mut().zip(&md.layers) {
            layer.epsg = raw
                .crs
                .as_ref()
                .and_then(|d| whereabouts_crs::resolve(d).resolved.epsg());
            if let (Some(r), Some(n)) = (&raw.raster, &mut layer.raster) {
                n.data_types = Some(vec![r.dtype.code().into(); r.bands as usize]);
                let value = r
                    .nodata
                    .as_ref()
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(NoData::from_f64);
                n.nodata = Some(vec![value; r.bands as usize]);
            }
        }
    }
    Some(rec)
}
