# Wave 4 additions: portable static cases. Live WAL and filesystem permissions remain runtime tests.
@builder("shp_undeclared_qix")
def _(d):
    shp(d, "points", srs_epsg(4326), ogr.wkbPoint, named(["POINT (1 2)", "POINT (3 4)"]))
    p = d / "points.dbf"
    data = bytearray(p.read_bytes()); data[29] = 0; p.write_bytes(data)
    ds = ogr.Open(str(d / "points.shp"), 1)
    ds.ExecuteSQL("CREATE SPATIAL INDEX ON points")
    ds = None

@builder("shp_cp437_names")
def _(d):
    shp(d, "places", srs_epsg(4326), ogr.wkbPoint, [("POINT (1 2)", {"id": 1, "café": "café"})],
        fields=[("id", ogr.OFTInteger, 10), ("café", ogr.OFTString, 32)], extra_opts=["ENCODING=CP437"])

@builder("shp_pointz_no_m")
def _(d):
    shp(d, "heights", srs_epsg(4326), ogr.wkbPoint25D, named(["POINT Z (1 2 3)", "POINT Z (3 4 5)"]))

@builder("shp_corrupt_count_mismatch")
def _(d):
    shp(d, "points", srs_epsg(4326), ogr.wkbPoint, named(["POINT (1 2)", "POINT (3 4)"]))
    p = d / "points.dbf"
    data = bytearray(p.read_bytes()); struct.pack_into("<I", data, 4, 1); p.write_bytes(data)

@builder("shp_cassini_3068")
def _(d):
    shp(d, "survey", srs_epsg(3068), ogr.wkbPoint, named([wkt_point(p) for p in project(3068, [(13.3, 52.4), (13.4, 52.5)])]))

@builder("shp_krovak_5514")
def _(d):
    shp(d, "survey", srs_epsg(5514), ogr.wkbPoint, named([wkt_point(p) for p in project(5514, [(14.3, 50.0), (14.4, 50.1)])]))

@builder("shp_nzmg_27200")
def _(d):
    shp(d, "survey", srs_epsg(27200), ogr.wkbPoint, named([wkt_point(p) for p in project(27200, [(174.7, -41.3), (174.8, -41.2)])]))

@builder("tif_external_overview_mask")
def _(d):
    path = d / "ortho.tif"
    gtiff(path, 64, 64, 1, "Byte", (10., .01, 0., 50., 0., -.01), srs_epsg(4326))
    gdal.SetConfigOption("GDAL_TIFF_INTERNAL_MASK", "NO")
    try:
        ds = gdal.Open(str(path), gdal.GA_Update)
        ds.CreateMaskBand(gdal.GMF_PER_DATASET)
        ds.GetRasterBand(1).GetMaskBand().Fill(255)
        ds = None
    finally:
        gdal.SetConfigOption("GDAL_TIFF_INTERNAL_MASK", None)
    ds = gdal.Open(str(path), gdal.GA_ReadOnly)
    ds.BuildOverviews("NEAREST", [2, 4]); ds = None
    (d / 'ortho.tif.msk.ovr').unlink(missing_ok=True)

@builder("tif_signed_pixel_scale")
def _(d):
    gtiff(d / "flipped.tif", 16, 16, 1, "Byte", (10., -.01, 0., 50., 0., .01), srs_epsg(4326))

@builder("gpkg_wkt2_preference")
def _(d):
    path = d / "modern.gpkg"
    ds = ogr.GetDriverByName("GPKG").CreateDataSource(str(path), options=["CRS_WKT_EXTENSION=YES"])
    write_vector(path, "GPKG", "points", srs_epsg(32633), ogr.wkbPoint, named(["POINT (500000 5000000)"]), ds=ds)
    ds = None
    gpkg_sql(path, ("UPDATE gpkg_spatial_ref_sys SET definition = 'undefined' WHERE srs_id = 32633", ()))
    normalise_gpkg(path)


@builder("gpkg_optional_zm")
def _(d):
    path = d / "optional.gpkg"
    write_vector(path, "GPKG", "points", srs_epsg(4326), ogr.wkbPointZM,
                 named(["POINT ZM (1 2 3 4)", "POINT ZM (3 4 5 6)"]))
    gpkg_sql(path, ("UPDATE gpkg_geometry_columns SET z=2, m=2", ()))
    normalise_gpkg(path)

@builder("gpkg_gridded_coverage")
def _(d):
    path = d / "elevation.gpkg"
    src = mem_raster(32, 32, 1, "Float32", (10., .01, 0., 50., 0., -.01), srs_epsg(4326))
    gdal.Translate(str(path), src, format="GPKG", creationOptions=["TILE_FORMAT=TIFF", "RASTER_TABLE=elevation", "BLOCKSIZE=256"])
    normalise_gpkg(path)

@builder("tif_custom_lcc_geokeys")
def _(d):
    gtiff(d / "custom.tif", 32, 32, 1, "Byte", (500000., 100., 0., 5000000., 0., -100.), srs_wkt(CUSTOM_LCC))
