//! Grouping rules (`docs/04-formats-and-crs.md` §3): one directory's files → datasets.
//!
//! The unit of grouping is a directory. Its datasets depend only on the directory's own name and
//! its direct children, so a change anywhere only requires regrouping the directories it touches.
//!
//! - **Anchored files** (`.shp`, `.gpkg`, `.tif`, …): the anchor plus sidecars found by name.
//!   Stem sidecars replace the anchor's extension (`a.shp` → `a.dbf`); name sidecars extend the
//!   full name (`a.tif` → `a.tif.aux.xml`). Matching ignores case. When several files fold to the
//!   same name (case-sensitive directories), the one whose stem matches the anchor's exactly
//!   wins, else the first by name.
//! - **Folder datasets**: a directory named `*.gdb` holding `a00000001.gdbtable` is a File
//!   Geodatabase; a directory holding `hdr.adf` is an Esri Grid (name-only raster). Either way
//!   the folder is the anchor, its recognised files are `Internal` members, and nothing inside it
//!   is grouped on its own. A folder with the FGDB signature but without the `.gdb` name is not a
//!   dataset, and its contents are still never surfaced.
//! - **Lone `.dbf`**: a `.dbf` claimed by no shapefile or raster attribute table becomes a
//!   non-spatial table (`Format::Dbf`).
//!
//! Orphan sidecars (`.shx`, `.prj`, world files, `-wal`, …) never become datasets (D-04).

use whereabouts_core::{
    Dataset, DatasetId, DatasetKind, Format, Frn, Member, MemberRole, VolumeId,
};

use crate::fold::{extension, fold};
use crate::view::DirectoryView;

/// A dataset produced by grouping, with its anchor's location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Grouped {
    pub(crate) dataset: Dataset,
    /// The anchor's parent directory.
    pub(crate) parent: Frn,
    /// The anchor's name (WTF-8).
    pub(crate) name: Box<[u8]>,
}

/// Which sidecar rules apply to an anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sidecars {
    Shapefile,
    Raster,
    PointCloud,
    Plain,
}

#[derive(Debug, Clone, Copy)]
struct AnchorRule {
    format: Format,
    kind: DatasetKind,
    sidecars: Sidecars,
    /// Plain images (`.jpg`, `.png`, …) are datasets only when a world file georeferences them.
    needs_world_file: bool,
}

const fn rule(format: Format, sidecars: Sidecars) -> AnchorRule {
    AnchorRule {
        format,
        kind: format.default_kind(),
        sidecars,
        needs_world_file: false,
    }
}

/// The anchor rule for a folded extension.
fn anchor_rule(ext: &[u8]) -> Option<AnchorRule> {
    use Sidecars::{Plain, PointCloud, Raster, Shapefile};
    let r = match ext {
        b"SHP" => rule(Format::Shapefile, Shapefile),
        b"GPKG" => rule(Format::GeoPackage, Plain),
        b"TIF" | b"TIFF" => rule(Format::GeoTiff, Raster),
        b"JPG" | b"JPEG" | b"PNG" | b"GIF" | b"BMP" => AnchorRule {
            needs_world_file: true,
            ..rule(Format::WorldFileRaster, Raster)
        },
        b"FGB" => rule(Format::FlatGeobuf, Plain),
        b"PARQUET" | b"GEOPARQUET" => rule(Format::GeoParquet, Plain),
        b"LAS" => rule(Format::Las, PointCloud),
        b"LAZ" => rule(Format::Laz, PointCloud),
        b"GEOJSON" => rule(Format::GeoJson, Plain),
        b"KML" => rule(Format::Kml, Plain),
        b"KMZ" => rule(Format::Kmz, Plain),
        b"DXF" => rule(Format::Dxf, Plain),
        b"DWG" => rule(Format::Dwg, Plain),
        // Tier 3 rasters: name-only until the optional GDAL pack, but they carry raster sidecars.
        b"ECW" | b"SID" | b"JP2" => AnchorRule {
            kind: DatasetKind::Raster,
            ..rule(Format::NameOnly, Raster)
        },
        b"NC" | b"GRB" | b"GRB2" | b"GRIB" | b"GRIB2" => AnchorRule {
            kind: DatasetKind::Raster,
            ..rule(Format::NameOnly, Plain)
        },
        _ => return None,
    };
    Some(r)
}

type SidecarRule = (&'static [u8], MemberRole);

/// Shapefile sidecars: `<stem><suffix>`.
const SHAPEFILE_STEM: &[SidecarRule] = &[
    (b".SHX", MemberRole::Shx),
    (b".DBF", MemberRole::Dbf),
    (b".PRJ", MemberRole::Prj),
    (b".CPG", MemberRole::Cpg),
    (b".SBN", MemberRole::Sbn),
    (b".SBX", MemberRole::Sbx),
    (b".QIX", MemberRole::Qix),
    (b".QMD", MemberRole::Qmd),
    (b".QPJ", MemberRole::Other),
    (b".AIH", MemberRole::Other),
    (b".AIN", MemberRole::Other),
    (b".FBN", MemberRole::Other),
    (b".FBX", MemberRole::Other),
    (b".IXS", MemberRole::Other),
    (b".MXS", MemberRole::Other),
];

/// Raster sidecars: `<stem><suffix>` (world files are added per extension).
const RASTER_STEM: &[SidecarRule] = &[
    (b".PRJ", MemberRole::Prj),
    (b".AUX", MemberRole::Other),
    (b".RRD", MemberRole::Other),
];

/// Raster sidecars: `<name><suffix>` (GDAL PAM, overviews, mask, ERDAS aux, attribute table).
const RASTER_NAME: &[SidecarRule] = &[
    (b".AUX.XML", MemberRole::AuxXml),
    (b".OVR", MemberRole::Ovr),
    (b".MSK", MemberRole::Msk),
    (b".AUX", MemberRole::Other),
    (b".VAT.DBF", MemberRole::Other),
    (b".VAT.CPG", MemberRole::Other),
];

/// Point-cloud sidecars: `<stem><suffix>` (LAStools spatial index).
const POINT_CLOUD_STEM: &[SidecarRule] = &[(b".LAX", MemberRole::Other)];

/// Lone `.dbf` sidecars: `<stem><suffix>`.
const DBF_STEM: &[SidecarRule] = &[(b".CPG", MemberRole::Cpg)];

/// Sidecars of every anchor: `<name><suffix>` (ArcGIS metadata, e.g. `a.shp.xml`).
const ANY_NAME: &[SidecarRule] = &[(b".XML", MemberRole::XmlMetadata)];

/// Folded name of the FGDB signature file.
const FGDB_SIGNATURE: &[u8] = b"A00000001.GDBTABLE";
/// Folded name of the Esri Grid signature file.
const GRID_SIGNATURE: &[u8] = b"HDR.ADF";

/// Image extensions that take world files.
const WORLD_IMAGE_EXTS: &[&[u8]] = &[
    b"TIF", b"TIFF", b"JPG", b"JPEG", b"PNG", b"GIF", b"BMP", b"ECW", b"SID", b"JP2",
];

/// Extensions of sidecars and folder-dataset internals (anchors and world files are matched
/// separately).
const OTHER_EXTS: &[&[u8]] = &[
    b"SHX",
    b"DBF",
    b"PRJ",
    b"CPG",
    b"SBN",
    b"SBX",
    b"QIX",
    b"QMD",
    b"QPJ",
    b"AIH",
    b"AIN",
    b"FBN",
    b"FBX",
    b"IXS",
    b"MXS",
    b"XML",
    b"OVR",
    b"MSK",
    b"AUX",
    b"RRD",
    b"LAX",
    b"WLD",
    // File Geodatabase internals.
    b"GDBTABLE",
    b"GDBTABLX",
    b"GDBINDEXES",
    b"ATX",
    b"SPX",
    b"FREELIST",
    b"HORIZON",
    b"CDF",
    // Esri Grid internals.
    b"ADF",
];

/// Extension-less names inside a File Geodatabase.
const FOLDER_NAMES: &[&[u8]] = &[b"GDB", b"TIMESTAMPS"];

/// The world-file extensions GDAL tries for an image extension: first + last letter + `W`
/// (`TFW`), the extension + `W` (`TIFW`), and `WLD`.
fn world_file_suffixes(ext: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(3);
    if let (Some(&first), Some(&last)) = (ext.first(), ext.last()) {
        out.push(vec![b'.', first, last, b'W']);
    }
    let mut long = Vec::with_capacity(ext.len() + 2);
    long.push(b'.');
    long.extend_from_slice(ext);
    long.push(b'W');
    out.push(long);
    out.push(b".WLD".to_vec());
    out
}

fn is_world_file_ext(ext: &[u8]) -> bool {
    WORLD_IMAGE_EXTS.iter().any(|img| {
        let short = matches!(
            (ext, img.first(), img.last()),
            ([a, b, b'W'], Some(f), Some(l)) if a == f && b == l
        );
        let long = ext.len() == img.len() + 1 && ext.starts_with(img) && ext.ends_with(b"W");
        short || long
    })
}

/// `true` if a file with this name (WTF-8) can affect grouping: an anchor, a sidecar, a world
/// file or a folder-dataset internal. Files for which this is `false` never change any dataset,
/// so change events about them can be skipped and a [`DirectoryView`](crate::DirectoryView) may
/// leave them out. Allocation-free.
pub fn is_relevant_name(name: &[u8]) -> bool {
    const MAX_EXT: usize = 12;
    let mut buf = [0u8; MAX_EXT];
    let ext = match name.iter().rposition(|&b| b == b'.') {
        Some(dot) => name.get(dot + 1..).unwrap_or_default(),
        None => {
            return FOLDER_NAMES.iter().any(|n| n.eq_ignore_ascii_case(name));
        }
    };
    if ext.is_empty() || ext.len() > MAX_EXT || !ext.is_ascii() {
        return false;
    }
    let Some(key) = buf.get_mut(..ext.len()) else {
        return false;
    };
    key.copy_from_slice(ext);
    key.make_ascii_uppercase();
    let key: &[u8] = key;
    anchor_rule(key).is_some() || OTHER_EXTS.contains(&key) || is_world_file_ext(key)
}

/// One relevant file of the directory being grouped.
#[derive(Debug)]
struct File {
    frn: Frn,
    /// Original WTF-8 name.
    name: Box<[u8]>,
    /// Folded name ([`fold`]).
    key: Box<[u8]>,
}

/// Groups the direct children of `dir` into datasets (deterministic order: by anchor name).
pub(crate) fn group_directory<V: DirectoryView + ?Sized>(
    volume: VolumeId,
    view: &V,
    dir: Frn,
) -> Vec<Grouped> {
    let mut files = Vec::new();
    view.for_each_child(dir, &mut |e| {
        if !e.is_dir && e.frn != dir && is_relevant_name(e.name) {
            files.push(File {
                frn: e.frn,
                name: e.name.into(),
                key: fold(e.name).into_boxed_slice(),
            });
        }
    });
    // Sorted by key so equal keys are adjacent (binary-searchable); ties by name, then FRN.
    files.sort_unstable_by(|a, b| (&a.key, &a.name, a.frn).cmp(&(&b.key, &b.name, b.frn)));

    let fgdb = files.iter().any(|f| *f.key == *FGDB_SIGNATURE);
    let grid = files.iter().any(|f| *f.key == *GRID_SIGNATURE);
    if fgdb || grid {
        return folder_dataset(volume, view, dir, fgdb, grid, &files)
            .into_iter()
            .collect();
    }
    Listing { files: &files }.group(volume, dir)
}

/// The dataset a folder with a signature file forms (its contents are never grouped on their
/// own, whatever the outcome).
fn folder_dataset<V: DirectoryView + ?Sized>(
    volume: VolumeId,
    view: &V,
    dir: Frn,
    fgdb: bool,
    grid: bool,
    files: &[File],
) -> Option<Grouped> {
    let entry = view.entry(dir)?;
    let (format, kind) = if fgdb && fold(entry.name).ends_with(b".GDB") {
        (
            Format::FileGeodatabase,
            Format::FileGeodatabase.default_kind(),
        )
    } else if grid {
        (Format::NameOnly, DatasetKind::Raster)
    } else {
        return None;
    };
    let sidecars = files
        .iter()
        .map(|f| Member {
            frn: f.frn,
            role: MemberRole::Internal,
        })
        .collect();
    build(
        volume,
        dir,
        entry.parent,
        entry.name.into(),
        format,
        kind,
        sidecars,
    )
}

/// The relevant files of one ordinary directory, sorted by key.
struct Listing<'a> {
    files: &'a [File],
}

impl Listing<'_> {
    /// Index of the file whose key is `key`, preferring one whose name minus the last
    /// `suffix_len` bytes is exactly `prefix`.
    fn find(&self, key: &[u8], prefix: &[u8], suffix_len: usize) -> Option<usize> {
        let start = self.files.partition_point(|f| *f.key < *key);
        let same = self
            .files
            .get(start..)?
            .iter()
            .take_while(|f| *f.key == *key)
            .count();
        let candidates = start..start + same;
        let exact = candidates.clone().find(|&i| {
            self.files.get(i).is_some_and(|f| {
                f.name
                    .len()
                    .checked_sub(suffix_len)
                    .and_then(|n| f.name.get(..n))
                    == Some(prefix)
            })
        });
        exact.or_else(|| (same > 0).then_some(start))
    }

    /// Looks up `<prefix><suffix>` for each rule and pushes the hits.
    fn sidecars(
        &self,
        key_prefix: &[u8],
        name_prefix: &[u8],
        rules: &[SidecarRule],
        out: &mut Vec<(usize, MemberRole)>,
    ) {
        let mut key = Vec::with_capacity(key_prefix.len() + 8);
        for &(suffix, role) in rules {
            key.clear();
            key.extend_from_slice(key_prefix);
            key.extend_from_slice(suffix);
            if let Some(i) = self.find(&key, name_prefix, suffix.len()) {
                out.push((i, role));
            }
        }
    }

    fn group(&self, volume: VolumeId, dir: Frn) -> Vec<Grouped> {
        let mut out = Vec::new();
        // `.dbf` files claimed by a shapefile or a raster attribute table.
        let mut claimed = vec![false; self.files.len()];
        let mut hits = Vec::new();
        let mut anchors: Vec<(usize, &File)> = self.files.iter().enumerate().collect();
        // Emit in name order, independent of the key sort.
        anchors.sort_by(|a, b| (&a.1.name, a.1.frn).cmp(&(&b.1.name, b.1.frn)));

        for &(_, file) in &anchors {
            let Some(ext) = extension(&file.key) else {
                continue;
            };
            let Some(rule) = anchor_rule(ext) else {
                continue;
            };
            let ext_len = ext.len() + 1;
            // The extension is ASCII, so it has the same length in the key and the name.
            let (Some(stem_key), Some(stem)) =
                (strip(&file.key, ext_len), strip(&file.name, ext_len))
            else {
                continue;
            };
            hits.clear();
            match rule.sidecars {
                Sidecars::Shapefile => self.sidecars(stem_key, stem, SHAPEFILE_STEM, &mut hits),
                Sidecars::Raster => {
                    self.sidecars(stem_key, stem, RASTER_STEM, &mut hits);
                    self.sidecars(&file.key, &file.name, RASTER_NAME, &mut hits);
                }
                Sidecars::PointCloud => {
                    self.sidecars(stem_key, stem, POINT_CLOUD_STEM, &mut hits);
                }
                Sidecars::Plain => {}
            }
            let mut has_world_file = false;
            if rule.sidecars == Sidecars::Raster {
                for suffix in world_file_suffixes(ext) {
                    let mut key = stem_key.to_vec();
                    key.extend_from_slice(&suffix);
                    if let Some(i) = self.find(&key, stem, suffix.len()) {
                        hits.push((i, MemberRole::WorldFile));
                        has_world_file = true;
                    }
                }
            }
            self.sidecars(&file.key, &file.name, ANY_NAME, &mut hits);
            if rule.needs_world_file && !has_world_file {
                continue;
            }

            let mut sidecars = Vec::with_capacity(hits.len());
            for &(i, role) in &hits {
                let Some(f) = self.files.get(i) else {
                    continue;
                };
                if extension(&f.key) == Some(b"DBF")
                    && let Some(c) = claimed.get_mut(i)
                {
                    *c = true;
                }
                sidecars.push(Member { frn: f.frn, role });
            }
            out.extend(build(
                volume,
                file.frn,
                dir,
                file.name.clone(),
                rule.format,
                rule.kind,
                sidecars,
            ));
        }

        // Lone `.dbf` files: non-spatial tables.
        for &(i, file) in &anchors {
            if extension(&file.key) != Some(b"DBF") || claimed.get(i).copied().unwrap_or(true) {
                continue;
            }
            // `x.tif.vat.dbf` is a raster value-attribute table: never a standalone table.
            if file.key.ends_with(b".VAT.DBF") {
                continue;
            }
            let (Some(stem_key), Some(stem)) = (strip(&file.key, 4), strip(&file.name, 4)) else {
                continue;
            };
            hits.clear();
            self.sidecars(stem_key, stem, DBF_STEM, &mut hits);
            self.sidecars(&file.key, &file.name, ANY_NAME, &mut hits);
            let sidecars = hits
                .iter()
                .filter_map(|&(i, role)| self.files.get(i).map(|f| Member { frn: f.frn, role }))
                .collect();
            out.extend(build(
                volume,
                file.frn,
                dir,
                file.name.clone(),
                Format::Dbf,
                Format::Dbf.default_kind(),
                sidecars,
            ));
        }
        out
    }
}

/// Builds a dataset: drops sidecars repeating the anchor or an earlier FRN (hard links), orders
/// sidecars by role then FRN.
fn build(
    volume: VolumeId,
    anchor: Frn,
    parent: Frn,
    name: Box<[u8]>,
    format: Format,
    kind: DatasetKind,
    mut sidecars: Vec<Member>,
) -> Option<Grouped> {
    // Stable sort keeps the first role found for a repeated FRN.
    sidecars.sort_by_key(|m| m.frn);
    sidecars.dedup_by_key(|m| m.frn);
    sidecars.retain(|m| m.frn != anchor);
    sidecars.sort_unstable_by_key(|m| (role_rank(m.role), m.frn));
    let mut members = Vec::with_capacity(sidecars.len() + 1);
    members.push(Member {
        frn: anchor,
        role: MemberRole::Main,
    });
    members.extend(sidecars);
    match Dataset::new(DatasetId::new(volume, anchor), kind, format, members) {
        Ok(dataset) => Some(Grouped {
            dataset,
            parent,
            name,
        }),
        Err(e) => {
            // Unreachable by construction (one Main, deduplicated FRNs); never panic on it.
            tracing::warn!(%anchor, error = %e, "grouper built an invalid dataset; skipped");
            None
        }
    }
}

/// `bytes` without its last `n` bytes.
fn strip(bytes: &[u8], n: usize) -> Option<&[u8]> {
    bytes.get(..bytes.len().checked_sub(n)?)
}

fn role_rank(role: MemberRole) -> usize {
    MemberRole::ALL
        .iter()
        .position(|r| *r == role)
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevance() {
        for name in [
            "a.shp",
            "A.SHX",
            "a.b.dbf",
            "x.tif.aux.xml",
            "x.TFW",
            "x.tifw",
            "x.jgw",
            "x.j2w",
            "x.wld",
            "gdb",
            "TIMESTAMPS",
            "a00000001.gdbtable",
            "hdr.adf",
            "a.gpkg",
            "c.las",
            "d.lax",
            "e.geoparquet",
            "f.dwg",
            "g.ecw",
            "h.eww",
            "i.grib2",
        ] {
            assert!(is_relevant_name(name.as_bytes()), "{name}");
        }
        for name in [
            "readme.txt",
            "a.gpkg-wal",
            "a.gpkg-shm",
            "a.gpkg-journal",
            "x.json",
            "a.lock",
            "_gdb.host.1.2.sr.lock",
            "noext",
            "a.",
            "x.verylongextension",
            "Ärger.säp",
            "",
        ] {
            assert!(!is_relevant_name(name.as_bytes()), "{name}");
        }
    }

    #[test]
    fn world_file_suffix_rules() {
        let s = |e: &str| {
            world_file_suffixes(e.as_bytes())
                .into_iter()
                .map(|v| String::from_utf8(v).unwrap_or_default())
                .collect::<Vec<_>>()
        };
        assert_eq!(s("TIF"), [".TFW", ".TIFW", ".WLD"]);
        assert_eq!(s("TIFF"), [".TFW", ".TIFFW", ".WLD"]);
        assert_eq!(s("JPEG"), [".JGW", ".JPEGW", ".WLD"]);
        assert!(is_world_file_ext(b"PGW") && is_world_file_ext(b"PNGW"));
        assert!(!is_world_file_ext(b"PNG") && !is_world_file_ext(b"XYW"));
    }
}
