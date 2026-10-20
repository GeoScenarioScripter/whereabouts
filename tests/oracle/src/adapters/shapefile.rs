//! Shapefile adapter (stage01/2.6): `whereabouts_readers::ShapefileReader` via the registry.

use std::path::Path;

use whereabouts_core::{Format, MemberRole};

use crate::adapters::{ResolvedCase, native};
use crate::compare::Check;
use crate::record::NativeRecord;

/// Native-only checks: EPSG and WGS84 arrive with `indexer::extract` (stage01/4.3).
pub(crate) const CHECKS: &[Check] = &[
    Check::Status,
    Check::Layers,
    Check::GeometryType,
    Check::FeatureCount,
    Check::NativeExtent,
];

/// Reads one shapefile case.
pub(crate) fn read(case: &ResolvedCase<'_>) -> Option<NativeRecord> {
    let dataset = native::resolve(case, Format::Shapefile, role);
    Some(native::read(&dataset))
}

/// Member role by extension (`.shp.xml` is ArcGIS metadata).
fn role(path: &Path) -> Option<MemberRole> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".shp.xml") {
        return Some(MemberRole::XmlMetadata);
    }
    let ext = name.rsplit_once('.')?.1;
    Some(match ext {
        "shx" => MemberRole::Shx,
        "dbf" => MemberRole::Dbf,
        "prj" => MemberRole::Prj,
        "cpg" => MemberRole::Cpg,
        "sbn" => MemberRole::Sbn,
        "sbx" => MemberRole::Sbx,
        "qix" => MemberRole::Qix,
        _ => MemberRole::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_by_extension() {
        assert_eq!(role(Path::new("a/b.SHX")), Some(MemberRole::Shx));
        assert_eq!(role(Path::new("b.dbf")), Some(MemberRole::Dbf));
        assert_eq!(role(Path::new("b.prj")), Some(MemberRole::Prj));
        assert_eq!(role(Path::new("b.cpg")), Some(MemberRole::Cpg));
        assert_eq!(role(Path::new("b.shp.xml")), Some(MemberRole::XmlMetadata));
        assert_eq!(role(Path::new("b.txt")), Some(MemberRole::Other));
        assert_eq!(role(Path::new("noext")), None);
    }
}
