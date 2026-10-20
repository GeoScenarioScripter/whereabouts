//! Dataset formats and kinds.

use crate::code::code_enum;

code_enum! {
    /// The on-disk format of a dataset, as classified by the grouper from names alone.
    ///
    /// Codes are stored in `datasets.format`. Tier 1 formats get native readers (stage01 implements
    /// Shapefile, GeoPackage and GeoTIFF; the grouper classifies the rest and extraction reports
    /// them `Unsupported` until their readers land). See `docs/04-formats-and-crs.md` §1.
    pub enum Format {
        /// ESRI Shapefile (`.shp` + sidecars).
        Shapefile => "shp",
        /// OGC GeoPackage (`.gpkg`), one dataset with one layer per `gpkg_contents` row.
        GeoPackage => "gpkg",
        /// FlatGeobuf (`.fgb`).
        FlatGeobuf => "fgb",
        /// GeoParquet (`.parquet` / `.geoparquet` with `geo` metadata).
        GeoParquet => "gpq",
        /// ASPRS LAS point cloud (`.las`).
        Las => "las",
        /// LASzip-compressed LAS (`.laz`).
        Laz => "laz",
        /// GeoTIFF / Cloud-Optimized GeoTIFF (`.tif`, `.tiff`), incl. georeferencing from sidecars.
        GeoTiff => "gtiff",
        /// A non-TIFF image georeferenced by a world file (`.jpg` + `.jgw`, `.png` + `.pgw`, …).
        WorldFileRaster => "world_raster",
        /// GeoJSON (`.geojson`, `.json` when recognised).
        GeoJson => "geojson",
        /// KML (`.kml`).
        Kml => "kml",
        /// Zipped KML (`.kmz`).
        Kmz => "kmz",
        /// AutoCAD DXF (`.dxf`); extent from header, no CRS.
        Dxf => "dxf",
        /// Esri File Geodatabase (a `*.gdb` folder; Tier 2).
        FileGeodatabase => "fgdb",
        /// A dBase table (`.dbf`) with no `.shp`: a non-spatial table, never a spatial result.
        Dbf => "dbf",
        /// AutoCAD DWG: indexed by name only (no open reader exists).
        Dwg => "dwg",
        /// Any other recognised GIS file indexed by name only (Tier 3 formats without the
        /// optional GDAL pack: ECW, MrSID, JPEG 2000, NetCDF, …).
        NameOnly => "name_only",
    }
}

impl Format {
    /// The kind a dataset of this format has unless the grouper knows better.
    pub const fn default_kind(self) -> DatasetKind {
        match self {
            Format::Shapefile
            | Format::FlatGeobuf
            | Format::GeoParquet
            | Format::GeoJson
            | Format::Kml
            | Format::Kmz => DatasetKind::Vector,
            Format::GeoPackage | Format::FileGeodatabase => DatasetKind::Container,
            Format::Las | Format::Laz => DatasetKind::PointCloud,
            Format::GeoTiff | Format::WorldFileRaster => DatasetKind::Raster,
            Format::Dxf | Format::Dwg => DatasetKind::Cad,
            Format::Dbf => DatasetKind::Table,
            Format::NameOnly => DatasetKind::Unknown,
        }
    }

    /// `true` for formats that are never handed to a metadata reader.
    pub const fn is_name_only(self) -> bool {
        matches!(self, Format::Dwg | Format::NameOnly)
    }
}

code_enum! {
    /// Broad category of a dataset (stored in `datasets.kind`).
    pub enum DatasetKind {
        /// Vector features.
        Vector => "vector",
        /// Gridded raster data.
        Raster => "raster",
        /// Point cloud (LAS/LAZ).
        PointCloud => "pointcloud",
        /// A container of several layers that may mix vectors, rasters and tables (GPKG, FGDB).
        Container => "container",
        /// CAD drawing (DXF, DWG).
        Cad => "cad",
        /// Non-spatial attribute table (lone `.dbf`).
        Table => "table",
        /// Recognised but not categorised (name-only formats).
        Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::CoreError;

    #[test]
    fn codes_round_trip_and_are_unique() {
        for f in Format::ALL {
            assert_eq!(Format::from_code(f.code()), Some(*f));
            assert_eq!(f.code().parse::<Format>().unwrap(), *f);
            let json = serde_json::to_string(f).unwrap();
            assert_eq!(json, format!("\"{}\"", f.code()));
            assert_eq!(serde_json::from_str::<Format>(&json).unwrap(), *f);
            let _ = f.default_kind();
        }
        let mut codes: Vec<_> = Format::ALL.iter().map(|f| f.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), Format::ALL.len());
        for k in DatasetKind::ALL {
            assert_eq!(k.code().parse::<DatasetKind>().unwrap(), *k);
        }
    }

    #[test]
    fn unknown_codes_are_errors() {
        assert_eq!(
            "SHP".parse::<Format>(),
            Err(CoreError::UnknownCode {
                kind: "Format",
                code: "SHP".into()
            })
        );
        assert!(serde_json::from_str::<DatasetKind>("\"mesh\"").is_err());
    }

    #[test]
    fn name_only_formats() {
        assert!(Format::Dwg.is_name_only());
        assert!(Format::NameOnly.is_name_only());
        assert!(!Format::Shapefile.is_name_only());
        assert_eq!(Format::GeoPackage.default_kind(), DatasetKind::Container);
        assert_eq!(Format::Dbf.default_kind(), DatasetKind::Table);
    }
}
