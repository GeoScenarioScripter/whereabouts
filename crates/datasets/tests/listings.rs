//! Table-driven grouping tests over messy directory listings (stage01/2.3 acceptance).
#![allow(clippy::unwrap_used)]

mod common;

use common::{Fx, ROOT, VOL};
use whereabouts_core::{DatasetKind, FileName, Format, Frn};
use whereabouts_datasets::{DirectoryView, Entry, Grouper, MemoryTree};

/// `(name, files, expected datasets)`; see `Fx::describe` for the expected format.
type Case = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

const CASES: &[Case] = &[
    (
        "complete shapefile",
        &["a.shp", "a.shx", "a.dbf", "a.prj", "a.cpg"],
        &["shp a.shp [cpg:a.cpg, dbf:a.dbf, prj:a.prj, shx:a.shx]"],
    ),
    (
        "mixed-case extensions",
        &["Roads.SHP", "roads.Dbf", "ROADS.shx", "rOaDs.PRJ"],
        &["shp Roads.SHP [dbf:roads.Dbf, prj:rOaDs.PRJ, shx:ROADS.shx]"],
    ),
    (
        "multi-dot stem",
        &["a.b.shp", "a.b.dbf", "a.b.shx", "a.shx", "a.prj"],
        &["shp a.b.shp [dbf:a.b.dbf, shx:a.b.shx]"],
    ),
    (
        "shp.xml metadata",
        &["a.shp", "a.dbf", "a.shp.xml", "a.SHP.XML.bak"],
        &["shp a.shp [dbf:a.dbf, xml:a.shp.xml]"],
    ),
    (
        "spatial indexes and qgis files",
        &[
            "a.shp", "a.sbn", "a.sbx", "a.qix", "a.qmd", "a.qpj", "a.fbn", "a.fbx", "a.ain",
            "a.aih",
        ],
        &[
            "shp a.shp [other:a.aih, other:a.ain, other:a.fbn, other:a.fbx, other:a.qpj, qix:a.qix, qmd:a.qmd, sbn:a.sbn, sbx:a.sbx]",
        ],
    ),
    ("bare .shp", &["lonely.shp"], &["shp lonely.shp []"]),
    (
        "orphan sidecars only",
        &[
            "a.shx",
            "a.prj",
            "a.cpg",
            "a.sbn",
            "b.tfw",
            "c.aux.xml",
            "d.ovr",
            "e.wld",
        ],
        &[],
    ),
    ("lone dbf is a table", &["t.dbf"], &["dbf t.dbf []"]),
    (
        "lone dbf with cpg and xml",
        &["T.DBF", "t.cpg", "T.dbf.xml", "t.prj"],
        &["dbf T.DBF [cpg:t.cpg, xml:T.dbf.xml]"],
    ),
    (
        "two shapefiles side by side",
        &["a.shp", "a.dbf", "b.shp", "b.dbf", "b.shx"],
        &["shp a.shp [dbf:a.dbf]", "shp b.shp [dbf:b.dbf, shx:b.shx]"],
    ),
    (
        "shared .prj between a shapefile and a raster",
        &["a.shp", "a.tif", "a.prj"],
        &["gtiff a.tif [prj:a.prj]", "shp a.shp [prj:a.prj]"],
    ),
    (
        "gpkg with wal, shm and journal",
        &["g.gpkg", "g.gpkg-wal", "g.gpkg-shm", "g.gpkg-journal"],
        &["gpkg g.gpkg []"],
    ),
    (
        "gpkg with metadata xml",
        &["g.GPKG", "g.gpkg.xml"],
        &["gpkg g.GPKG [xml:g.gpkg.xml]"],
    ),
    ("orphan wal and shm", &["g.gpkg-wal", "g.gpkg-shm"], &[]),
    (
        "geotiff with every sidecar",
        &[
            "r.tif",
            "r.tif.aux.xml",
            "r.tif.ovr",
            "r.tif.msk",
            "r.tfw",
            "r.prj",
            "r.tif.xml",
        ],
        &[
            "gtiff r.tif [aux_xml:r.tif.aux.xml, msk:r.tif.msk, ovr:r.tif.ovr, prj:r.prj, world_file:r.tfw, xml:r.tif.xml]",
        ],
    ),
    (
        "geotiff with internal georef and a world file",
        &["dem.tif", "dem.tfw"],
        &["gtiff dem.tif [world_file:dem.tfw]"],
    ),
    (
        ".tiff with tfw, tiffw and wld",
        &["x.tiff", "x.tfw", "x.tiffw", "x.wld"],
        &["gtiff x.tiff [world_file:x.tfw, world_file:x.tiffw, world_file:x.wld]"],
    ),
    (
        "tif with tifw",
        &["x.TIF", "x.TIFW"],
        &["gtiff x.TIF [world_file:x.TIFW]"],
    ),
    (
        "raster attribute table is not a lone dbf",
        &["c.tif", "c.tif.vat.dbf", "c.tif.vat.cpg"],
        &["gtiff c.tif [other:c.tif.vat.cpg, other:c.tif.vat.dbf]"],
    ),
    (
        "orphan raster attribute table is not a standalone table",
        &["c.tif.vat.dbf", "C.TIF.VAT.CPG", "d.vat.dbf"],
        &[],
    ),
    (
        "erdas aux and rrd",
        &["e.tif", "e.aux", "e.tif.aux", "e.rrd"],
        &["gtiff e.tif [other:e.aux, other:e.rrd, other:e.tif.aux]"],
    ),
    (
        "jpg with world file",
        &["photo.jpg", "photo.jgw", "photo.prj", "photo.jpg.aux.xml"],
        &[
            "world_raster photo.jpg [aux_xml:photo.jpg.aux.xml, prj:photo.prj, world_file:photo.jgw]",
        ],
    ),
    (
        "jpg without world file is not a dataset",
        &["holiday.jpg", "holiday.jpg.aux.xml"],
        &[],
    ),
    (
        "png with pgw and another with wld",
        &["a.png", "a.pgw", "b.PNG", "b.wld", "c.png"],
        &[
            "world_raster a.png [world_file:a.pgw]",
            "world_raster b.PNG [world_file:b.wld]",
        ],
    ),
    (
        "jpeg uses jgw and jpegw",
        &["m.jpeg", "m.jgw", "m.jpegw"],
        &["world_raster m.jpeg [world_file:m.jgw, world_file:m.jpegw]"],
    ),
    (
        "file geodatabase",
        &[
            "data.gdb/a00000001.gdbtable",
            "data.gdb/a00000001.gdbtablx",
            "data.gdb/a00000004.spx",
            "data.gdb/a00000004.gdbindexes",
            "data.gdb/gdb",
            "data.gdb/timestamps",
            "data.gdb/_gdb.host.123.4.sr.lock",
            "data.gdb/a00000009.CatItemsByType.atx",
        ],
        &[
            "fgdb data.gdb [internal:data.gdb/a00000001.gdbtable, internal:data.gdb/a00000001.gdbtablx, internal:data.gdb/a00000004.gdbindexes, internal:data.gdb/a00000004.spx, internal:data.gdb/a00000009.CatItemsByType.atx, internal:data.gdb/gdb, internal:data.gdb/timestamps]",
        ],
    ),
    (
        "gdb folder without signature",
        &["empty.gdb/", "notes.gdb/readme.txt"],
        &[],
    ),
    (
        "signature without .gdb name: contents never surface",
        &[
            "copy/a00000001.gdbtable",
            "copy/stray.shp",
            "copy/stray.dbf",
        ],
        &[],
    ),
    (
        "nested gdb deep in the tree",
        &[
            "projects/2024/city.gdb/a00000001.gdbtable",
            "projects/2024/city.gdb/gdb",
            "projects/2024/readme.txt",
        ],
        &[
            "fgdb projects/2024/city.gdb [internal:projects/2024/city.gdb/a00000001.gdbtable, internal:projects/2024/city.gdb/gdb]",
        ],
    ),
    (
        "gdb inside a gdb",
        &[
            "outer.gdb/a00000001.gdbtable",
            "outer.gdb/inner.gdb/a00000001.gdbtable",
        ],
        &[
            "fgdb outer.gdb [internal:outer.gdb/a00000001.gdbtable]",
            "fgdb outer.gdb/inner.gdb [internal:outer.gdb/inner.gdb/a00000001.gdbtable]",
        ],
    ),
    (
        "upper-case gdb",
        &["DATA.GDB/A00000001.GDBTABLE", "DATA.GDB/A00000001.GDBTABLX"],
        &[
            "fgdb DATA.GDB [internal:DATA.GDB/A00000001.GDBTABLE, internal:DATA.GDB/A00000001.GDBTABLX]",
        ],
    ),
    (
        "esri grid folder",
        &[
            "elev/hdr.adf",
            "elev/w001001.adf",
            "elev/prj.adf",
            "elev/log",
            "info/arc.dir",
        ],
        &[
            "name_only elev [internal:elev/hdr.adf, internal:elev/prj.adf, internal:elev/w001001.adf]",
        ],
    ),
    (
        "single-file tier 1 formats",
        &[
            "a.fgb",
            "b.geojson",
            "c.kml",
            "d.kmz",
            "e.las",
            "f.laz",
            "f.lax",
            "g.parquet",
            "h.geoparquet",
            "i.dxf",
        ],
        &[
            "dxf i.dxf []",
            "fgb a.fgb []",
            "geojson b.geojson []",
            "gpq g.parquet []",
            "gpq h.geoparquet []",
            "kml c.kml []",
            "kmz d.kmz []",
            "las e.las []",
            "laz f.laz [other:f.lax]",
        ],
    ),
    (
        "dwg is name-only",
        &["plan.dwg", "plan.dwg.xml"],
        &["dwg plan.dwg [xml:plan.dwg.xml]"],
    ),
    (
        "tier 3 rasters with sidecars",
        &[
            "o.ecw",
            "o.eww",
            "s.sid",
            "s.sdw",
            "j.jp2",
            "j.j2w",
            "j.jp2.aux.xml",
        ],
        &[
            "name_only j.jp2 [aux_xml:j.jp2.aux.xml, world_file:j.j2w]",
            "name_only o.ecw [world_file:o.eww]",
            "name_only s.sid [world_file:s.sdw]",
        ],
    ),
    (
        "netcdf and grib",
        &["t.nc", "w.grib2", "w.grb"],
        &[
            "name_only t.nc []",
            "name_only w.grb []",
            "name_only w.grib2 []",
        ],
    ),
    (
        "directories named like datasets are ignored",
        &["x.shp/", "y.tif/inner.shp", "y.tif/inner.dbf"],
        &["shp y.tif/inner.shp [dbf:y.tif/inner.dbf]"],
    ),
    (
        "sidecars in another directory do not join",
        &["a.shp", "sub/a.dbf", "sub/a.shx"],
        &["dbf sub/a.dbf []", "shp a.shp []"],
    ),
    (
        "unicode stems fold case",
        &["Ärger.shp", "ärger.DBF", "ΣΧΈΔΙΟ.shp", "σχέδιο.shx"],
        &[
            "shp Ärger.shp [dbf:ärger.DBF]",
            "shp ΣΧΈΔΙΟ.shp [shx:σχέδιο.shx]",
        ],
    ),
    (
        "sharp s does not fold to ss",
        &["straße.shp", "STRASSE.dbf"],
        &["dbf STRASSE.dbf []", "shp straße.shp []"],
    ),
    (
        "irrelevant files only",
        &[
            "readme.txt",
            "photo.jpg",
            "data.json",
            "notes.docx",
            "a.gpkg-wal",
            "b.lock",
        ],
        &[],
    ),
    (
        "tif and tiff with the same stem share sidecars",
        &["s.tif", "s.tiff", "s.tfw", "s.prj"],
        &[
            "gtiff s.tif [prj:s.prj, world_file:s.tfw]",
            "gtiff s.tiff [prj:s.prj, world_file:s.tfw]",
        ],
    ),
    ("empty stem", &[".shp", ".dbf"], &["shp .shp [dbf:.dbf]"]),
    (
        "shapefile .dbf with different case stem",
        &["ROADS.shp", "roads.dbf"],
        &["shp ROADS.shp [dbf:roads.dbf]"],
    ),
    (
        "sidecar that is a directory",
        &["a.shp", "a.dbf/", "a.prj"],
        &["shp a.shp [prj:a.prj]"],
    ),
    (
        "json is not classified without sniffing",
        &["x.json", "y.topojson"],
        &[],
    ),
    (
        "dbf next to an unrelated shapefile",
        &["roads.shp", "roads.dbf", "lookup.dbf"],
        &["dbf lookup.dbf []", "shp roads.shp [dbf:roads.dbf]"],
    ),
    (
        "world file for an image that is not there",
        &["a.jgw", "a.tfw", "a.pgw"],
        &[],
    ),
];

#[test]
fn messy_listings() {
    assert!(
        CASES.len() >= 40,
        "acceptance asks for at least 40 listings"
    );
    let mut failures = Vec::new();
    for (name, files, expected) in CASES {
        let fx = Fx::with(files);
        let got = fx.describe_all(&fx.scan());
        let mut want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        want.sort();
        if got != want {
            failures.push(format!("{name}:\n  got  {got:#?}\n  want {want:#?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn kinds_and_ids() {
    let fx = Fx::with(&[
        "a.shp",
        "t.dbf",
        "d.gdb/a00000001.gdbtable",
        "g/hdr.adf",
        "x.ecw",
        "c.dwg",
    ]);
    let g = fx.scan();
    let kind = |path: &str| {
        let id = whereabouts_core::DatasetId::new(VOL, fx.frn(path));
        let ds = g.dataset(id).unwrap();
        assert_eq!(ds.id(), id);
        (ds.format(), ds.kind())
    };
    assert_eq!(kind("a.shp"), (Format::Shapefile, DatasetKind::Vector));
    assert_eq!(kind("t.dbf"), (Format::Dbf, DatasetKind::Table));
    assert_eq!(
        kind("d.gdb"),
        (Format::FileGeodatabase, DatasetKind::Container)
    );
    assert_eq!(kind("g"), (Format::NameOnly, DatasetKind::Raster));
    assert_eq!(kind("x.ecw"), (Format::NameOnly, DatasetKind::Raster));
    assert_eq!(kind("c.dwg"), (Format::Dwg, DatasetKind::Cad));
    // Another volume's ID is not found.
    let other = whereabouts_core::DatasetId::new(whereabouts_core::VolumeId(9), fx.frn("a.shp"));
    assert!(g.dataset(other).is_none());
    // Members point back to their datasets.
    let shp = fx.frn("a.shp");
    assert_eq!(g.datasets_containing(shp).count(), 1);
    assert_eq!(g.datasets_in(ROOT).count(), 4, "a.shp, t.dbf, x.ecw, c.dwg");
}

#[test]
fn no_member_is_surfaced_standalone() {
    // D-04: every anchor's extension is an anchor type; FGDB internals never anchor.
    let fx = Fx::with(&[
        "a.shp",
        "a.shx",
        "a.dbf",
        "a.prj",
        "r.tif",
        "r.tfw",
        "r.tif.ovr",
        "r.tif.aux.xml",
        "g.gpkg",
        "g.gpkg-wal",
        "d.gdb/a00000001.gdbtable",
        "d.gdb/a00000001.gdbtablx",
        "d.gdb/stray.shp",
    ]);
    let g = fx.scan();
    let anchors: Vec<String> = g.datasets().map(|d| fx.path_of(d.anchor())).collect();
    for a in &anchors {
        assert!(
            !a.ends_with(".shx") && !a.ends_with(".dbf") && !a.contains("d.gdb/"),
            "{a} surfaced"
        );
    }
    assert_eq!(anchors.len(), 4);
}

#[test]
fn case_sensitive_directory_prefers_exact_stem() {
    // Two names that fold together can coexist in a case-sensitive directory.
    let fx = Fx::with(&[
        "a.shp", "A.shp", "a.dbf", "A.DBF", "x.shp", "X.SHX", "Y.shx",
    ]);
    let got = fx.describe_all(&fx.scan());
    assert_eq!(
        got,
        [
            "shp A.shp [dbf:A.DBF]",
            "shp a.shp [dbf:a.dbf]",
            "shp x.shp [shx:X.SHX]",
        ]
    );
}

#[test]
fn lone_surrogate_names_group_losslessly() {
    let mut fx = Fx::new();
    // "a\u{D800}" + extension, as UTF-16 with an unpaired surrogate.
    let name = |ext: &str| {
        let mut units = vec![u16::from(b'a'), 0xD800];
        units.extend(ext.encode_utf16());
        FileName::from_utf16(&units)
    };
    let shp = fx.add_named(ROOT, name(".shp"), false);
    let dbf = fx.add_named(ROOT, name(".DBF"), false);
    // A different lone surrogate must not match.
    let mut other = vec![u16::from(b'a'), 0xD801];
    other.extend(".shx".encode_utf16());
    fx.add_named(ROOT, FileName::from_utf16(&other), false);
    let g = fx.scan();
    assert_eq!(g.len(), 1);
    let ds = g.datasets().next().unwrap();
    assert_eq!(ds.anchor(), shp);
    assert_eq!(ds.members().len(), 2);
    assert!(ds.contains(dbf));
}

/// A view that lists one FRN under two names (a hard link inside one directory).
struct HardLinks {
    tree: MemoryTree,
    extra: Vec<(Frn, Frn, &'static [u8])>,
}

impl DirectoryView for HardLinks {
    fn for_each_child(&self, dir: Frn, visit: &mut dyn FnMut(Entry<'_>)) {
        self.tree.for_each_child(dir, visit);
        for &(frn, parent, name) in &self.extra {
            if parent == dir {
                visit(Entry {
                    frn,
                    parent,
                    name,
                    is_dir: false,
                });
            }
        }
    }

    fn entry(&self, frn: Frn) -> Option<Entry<'_>> {
        self.tree.entry(frn)
    }
}

#[test]
fn hard_links_do_not_break_invariants() {
    let fx = Fx::with(&["r.tif", "r.prj", "a.shp"]);
    let prj = fx.frn("r.prj");
    let tif = fx.frn("r.tif");
    let shp = fx.frn("a.shp");
    let view = HardLinks {
        tree: fx.tree.clone(),
        // r.prj is also r.tfw; r.tif is also r.tif.ovr; a.shp also listed as b.shp.
        extra: vec![
            (prj, ROOT, b"r.tfw"),
            (tif, ROOT, b"r.tif.ovr"),
            (shp, ROOT, b"b.shp"),
        ],
    };
    let mut g = Grouper::new(VOL);
    let delta = g.full_scan(&view, ROOT);
    assert_eq!(delta.added.len(), 2, "a.shp/b.shp collapse to one dataset");
    let tif_ds = g
        .dataset(whereabouts_core::DatasetId::new(VOL, tif))
        .unwrap();
    assert_eq!(tif_ds.members().len(), 2, "anchor + r.prj once: {tif_ds:?}");
}

#[test]
fn large_directory() {
    let mut fx = Fx::new();
    let dir = fx.add("big/");
    for i in 0..2_000 {
        for (k, ext) in ["shp", "shx", "dbf", "prj"].into_iter().enumerate() {
            fx.tree.upsert(&common::record(
                Frn(1_000_000 + i * 10 + k as u64),
                dir,
                FileName::from(format!("layer_{i}.{ext}")),
                false,
            ));
        }
        fx.tree.upsert(&common::record(
            Frn(5_000_000 + i),
            dir,
            FileName::from(format!("doc_{i}.txt")),
            false,
        ));
    }
    let g = fx.scan();
    assert_eq!(g.len(), 2_000);
    assert!(g.datasets().all(|d| d.members().len() == 4));
}
