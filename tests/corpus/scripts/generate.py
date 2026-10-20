#!/usr/bin/env python3
"""Build the synthetic part of the Whereabouts test corpus.

Runs *inside* the pinned GDAL container (see ``generate.sh``); never run it against a host GDAL,
or the output will not be byte-identical to the committed files.

For every ``origin = "synthetic"`` case in ``tests/corpus/manifest.toml`` this script deletes
``tests/corpus/data/<format>/<id>/``, rebuilds it with the builder registered under the case id,
and checks that the files produced are exactly the case's ``files`` list. Real-world cases are
never touched.

Determinism rules (a second run must produce byte-identical files):
- no wall-clock time anywhere: DBF header dates are pinned (``DBF_DATE_LAST_UPDATE``), GeoPackage
  ``last_change`` is pinned (``OGR_CURRENT_DATE`` + an explicit normalisation pass);
- all coordinates/pixels come from closed-form arithmetic, not RNGs;
- one image, one platform (``linux/amd64``), so floating point is reproducible.
"""

from __future__ import annotations

import math
import shutil
import sqlite3
import struct
import sys
import tomllib
from pathlib import Path

from osgeo import gdal, ogr, osr

gdal.UseExceptions()
ogr.UseExceptions()
osr.UseExceptions()

CORPUS = Path(__file__).resolve().parent.parent
DATA = CORPUS / "data"

FIXED_DATE = "2000-01-01"
FIXED_TIMESTAMP = "2000-01-01T00:00:00.000Z"
gdal.SetConfigOption("OGR_CURRENT_DATE", FIXED_TIMESTAMP)
gdal.SetConfigOption("GDAL_NUM_THREADS", "1")
gdal.SetConfigOption("CPL_DEBUG", "OFF")

SHP_OPTS = [f"DBF_DATE_LAST_UPDATE={FIXED_DATE}"]


# --------------------------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------------------------


def srs_epsg(code: int) -> osr.SpatialReference:
    s = osr.SpatialReference()
    s.ImportFromEPSG(code)
    s.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    return s


def srs_wkt(wkt: str) -> osr.SpatialReference:
    s = osr.SpatialReference()
    s.SetFromUserInput(wkt)
    s.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    return s


def project(code: int, pts):
    """Project (lon, lat) pairs from WGS84 into EPSG:code (traditional x/y order)."""
    if code == 4326:
        return [(float(x), float(y)) for x, y in pts]
    ct = osr.CoordinateTransformation(srs_epsg(4326), srs_epsg(code))
    return [tuple(ct.TransformPoint(lon, lat)[:2]) for lon, lat in pts]


def fmt(v: float) -> str:
    return repr(float(v))


def wkt_point(p, z=None, m=None) -> str:
    if z is not None and m is not None:
        return f"POINT ZM ({fmt(p[0])} {fmt(p[1])} {fmt(z)} {fmt(m)})"
    return f"POINT ({fmt(p[0])} {fmt(p[1])})"


def wkt_line(pts) -> str:
    return "LINESTRING (" + ", ".join(f"{fmt(x)} {fmt(y)}" for x, y in pts) + ")"


def wkt_polygon(ring) -> str:
    ring = list(ring)
    if ring[0] != ring[-1]:
        ring.append(ring[0])
    return "POLYGON ((" + ", ".join(f"{fmt(x)} {fmt(y)}" for x, y in ring) + "))"


def grid_points(lon0, lat0, lon1, lat1, nx, ny):
    pts = []
    for j in range(ny):
        for i in range(nx):
            lon = lon0 + (lon1 - lon0) * i / max(nx - 1, 1)
            lat = lat0 + (lat1 - lat0) * j / max(ny - 1, 1)
            pts.append((lon, lat))
    return pts


def box_ring(x0, y0, x1, y1, n=4):
    """Densified rectangle ring (n points per edge) — densified so projected boxes stay faithful."""
    ring = []
    for i in range(n):
        ring.append((x0 + (x1 - x0) * i / n, y0))
    for i in range(n):
        ring.append((x1, y0 + (y1 - y0) * i / n))
    for i in range(n):
        ring.append((x1 - (x1 - x0) * i / n, y1))
    for i in range(n):
        ring.append((x0, y1 - (y1 - y0) * i / n))
    ring.append(ring[0])
    return ring


def circle_ring(cx, cy, r, n=24):
    ring = [(cx + r * math.cos(2 * math.pi * k / n), cy + r * math.sin(2 * math.pi * k / n)) for k in range(n)]
    ring.append(ring[0])
    return ring


FIELDS_BASIC = [("id", ogr.OFTInteger, 10), ("name", ogr.OFTString, 32)]


def write_vector(path: Path, driver: str, layer: str, srs, gtype, features, fields=FIELDS_BASIC,
                 layer_opts=(), ds=None):
    """Write one layer. ``features`` is a list of (wkt or None, {field: value})."""
    own = ds is None
    if own:
        ds = ogr.GetDriverByName(driver).CreateDataSource(str(path))
    lyr = ds.CreateLayer(layer, srs, gtype, options=list(layer_opts))
    for name, ftype, width in fields:
        fd = ogr.FieldDefn(name, ftype)
        if width:
            fd.SetWidth(width)
        lyr.CreateField(fd)
    defn = lyr.GetLayerDefn()
    for wkt, attrs in features:
        f = ogr.Feature(defn)
        if wkt is not None:
            f.SetGeometry(ogr.CreateGeometryFromWkt(wkt))
        for k, v in attrs.items():
            f.SetField(k, v)
        lyr.CreateFeature(f)
        f = None
    lyr = None
    if own:
        ds.FlushCache()
        ds = None
    return ds


def shp(case_dir: Path, name: str, srs, gtype, features, fields=FIELDS_BASIC, extra_opts=()):
    write_vector(case_dir / f"{name}.shp", "ESRI Shapefile", name, srs, gtype, features, fields,
                 layer_opts=[*SHP_OPTS, *extra_opts])


def named(features_wkt, prefix="f"):
    return [(w, {"id": i + 1, "name": f"{prefix}{i + 1}"}) for i, w in enumerate(features_wkt)]


def normalise_gpkg(path: Path):
    """Pin every timestamp GDAL may have written and compact the file deterministically."""
    con = sqlite3.connect(str(path))
    try:
        con.execute("UPDATE gpkg_contents SET last_change = ?", (FIXED_TIMESTAMP,))
        con.commit()
        con.execute("VACUUM")
    finally:
        con.close()


def gpkg_sql(path: Path, *statements):
    con = sqlite3.connect(str(path))
    try:
        for sql, params in statements:
            con.execute(sql, params)
        con.commit()
    finally:
        con.close()


def pack(dtype: str, values):
    code = {"Byte": "B", "UInt16": "H", "Int16": "h", "Float32": "f"}[dtype]
    return struct.pack(f"<{len(values)}{code}", *values)


GDT = {"Byte": gdal.GDT_Byte, "UInt16": gdal.GDT_UInt16, "Int16": gdal.GDT_Int16,
       "Float32": gdal.GDT_Float32}


def pattern(w, h, band, dtype):
    """Closed-form pixel pattern: smooth gradient + band offset, cheap to compress."""
    out = []
    for y in range(h):
        for x in range(w):
            v = (x + 2 * y + 17 * band)
            if dtype == "Byte":
                out.append(v % 256)
            elif dtype == "UInt16":
                out.append(1 + (v * 37) % 60000)
            elif dtype == "Int16":
                out.append((v * 13) % 30000 - 15000)
            else:
                out.append(float(v) * 0.5 - 20.0)
    return out


def mem_raster(w, h, bands, dtype, gt, srs):
    ds = gdal.GetDriverByName("MEM").Create("", w, h, bands, GDT[dtype])
    ds.SetGeoTransform(gt)
    ds.SetSpatialRef(srs)
    for b in range(1, bands + 1):
        ds.GetRasterBand(b).WriteRaster(0, 0, w, h, pack(dtype, pattern(w, h, b, dtype)), buf_type=GDT[dtype])
    return ds


def gtiff(path: Path, w, h, bands, dtype, gt, srs, options=(), nodata=None, metadata=None):
    ds = gdal.GetDriverByName("GTiff").Create(str(path), w, h, bands, GDT[dtype], options=list(options))
    if metadata:
        for k, v in metadata.items():
            ds.SetMetadataItem(k, v)
    if gt is not None:
        ds.SetGeoTransform(gt)
    if srs is not None:
        ds.SetSpatialRef(srs)
    for b in range(1, bands + 1):
        band = ds.GetRasterBand(b)
        values = pattern(w, h, b, dtype)
        if nodata is not None:
            values[0] = nodata
            band.SetNoDataValue(nodata)
        band.WriteRaster(0, 0, w, h, pack(dtype, values), buf_type=GDT[dtype])
    ds.FlushCache()
    ds = None


def truncate(path: Path, size: int):
    data = path.read_bytes()
    path.write_bytes(data[:size])


def esri_wkt(srs) -> str:
    return srs.ExportToWkt(["FORMAT=WKT1_ESRI"])


# --------------------------------------------------------------------------------------------
# builders — one per synthetic case id (keep in manifest order)
# --------------------------------------------------------------------------------------------

BUILDERS = {}


def builder(case_id):
    def deco(fn):
        BUILDERS[case_id] = fn
        return fn
    return deco


# ---- Shapefile -----------------------------------------------------------------------------

@builder("shp_utm33n_points")
def _(d):
    pts = project(32633, grid_points(13.0, 45.5, 16.5, 48.0, 5, 5))
    shp(d, "points", srs_epsg(32633), ogr.wkbPoint, named([wkt_point(p) for p in pts], "pt"))


@builder("shp_lcc_2154_lines")
def _(d):
    lines = []
    for k in range(6):
        lonlat = [(-1.5 + 0.9 * i, 43.5 + 0.8 * k + 0.15 * (i % 2)) for i in range(8)]
        lines.append(wkt_line(project(2154, lonlat)))
    shp(d, "roads", srs_epsg(2154), ogr.wkbLineString, named(lines, "road"))


@builder("shp_albers_5070_polygons")
def _(d):
    polys = []
    for k in range(4):
        lon0, lat0 = -120.0 + 12.0 * k, 32.0 + 3.0 * k
        polys.append(wkt_polygon(project(5070, box_ring(lon0, lat0, lon0 + 8.0, lat0 + 5.0, 8))))
    shp(d, "counties", srs_epsg(5070), ogr.wkbPolygon, named(polys, "county"))


@builder("shp_webmerc_3857_points")
def _(d):
    pts = project(3857, grid_points(-150.0, -60.0, 150.0, 70.0, 6, 4))
    shp(d, "cities", srs_epsg(3857), ogr.wkbPoint, named([wkt_point(p) for p in pts], "city"))


@builder("shp_polar_3031_polygons")
def _(d):
    # Projected-space rings around the South Pole: the first one contains the pole itself.
    rings = [circle_ring(0.0, 0.0, 1_200_000.0, 32), circle_ring(-1_500_000.0, 900_000.0, 300_000.0, 16),
             circle_ring(2_000_000.0, -400_000.0, 250_000.0, 16)]
    shp(d, "ice_shelves", srs_epsg(3031), ogr.wkbPolygon, named([wkt_polygon(r) for r in rings], "shelf"))


@builder("shp_geographic_4326_polygons")
def _(d):
    polys = [wkt_polygon(box_ring(-10.0 + 15.0 * k, -5.0 + 7.0 * k, -2.0 + 15.0 * k, 3.0 + 7.0 * k)) for k in range(5)]
    shp(d, "parcels", srs_epsg(4326), ogr.wkbPolygon, named(polys, "parcel"))


@builder("shp_nztm_2193_lines")
def _(d):
    lines = []
    for k in range(5):
        lonlat = [(168.0 + 1.3 * i, -46.0 + 2.2 * k + 0.2 * (i % 3)) for i in range(7)]
        lines.append(wkt_line(project(2193, lonlat)))
    shp(d, "rivers", srs_epsg(2193), ogr.wkbLineString, named(lines, "river"))


@builder("shp_stateplane_2227_ftus")
def _(d):
    pts = project(2227, grid_points(-122.6, 37.2, -121.6, 38.1, 4, 4))
    shp(d, "hydrants", srs_epsg(2227), ogr.wkbPoint, named([wkt_point(p) for p in pts], "h"))


ESRI_UTM33N_NOAUTH = (
    'PROJCS["WGS_1984_UTM_Zone_33N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",'
    'SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],'
    'UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],'
    'PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],'
    'PARAMETER["Central_Meridian",15.0],PARAMETER["Scale_Factor",0.9996],'
    'PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]'
)

ESRI_CUSTOM_TM = (
    'PROJCS["Site_Grid_Custom",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",'
    'SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],'
    'UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],'
    'PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],'
    'PARAMETER["Central_Meridian",15.0],PARAMETER["Scale_Factor",0.9996],'
    'PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]'
)


@builder("shp_esri_wkt_noauth")
def _(d):
    # ArcGIS-style .prj: ESRI names, no AUTHORITY nodes; must be identified by name/parameters.
    pts = project(32633, grid_points(14.0, 46.0, 16.0, 47.5, 3, 3))
    shp(d, "wells", srs_epsg(32633), ogr.wkbPoint, named([wkt_point(p) for p in pts], "well"))
    (d / "wells.prj").write_bytes(ESRI_UTM33N_NOAUTH.encode("ascii"))


@builder("shp_esri_custom_name_tm")
def _(d):
    # Unrecognisable PROJCS name with parameters identical to UTM 33N: parameter matching only.
    pts = project(32633, grid_points(14.5, 46.5, 15.5, 47.0, 3, 2))
    shp(d, "survey", srs_epsg(32633), ogr.wkbPoint, named([wkt_point(p) for p in pts], "s"))
    (d / "survey.prj").write_bytes(ESRI_CUSTOM_TM.encode("ascii"))


ATTR_FIELDS = [("id", ogr.OFTInteger, 10), ("name", ogr.OFTString, 40)]


@builder("shp_cpg_utf8")
def _(d):
    names = ["Zürich", "Ørsted", "東京", "Saint-Étienne", "Kraków"]
    pts = grid_points(5.0, 45.0, 25.0, 55.0, 5, 1)
    feats = [(wkt_point(p), {"id": i + 1, "name": n}) for i, (p, n) in enumerate(zip(pts, names))]
    shp(d, "places", srs_epsg(4326), ogr.wkbPoint, feats, ATTR_FIELDS, extra_opts=["ENCODING=UTF-8"])


@builder("shp_ldid_cp1251_nocpg")
def _(d):
    # Encoding signalled only by the DBF language driver id (LDID 0xC9 = Windows-1251); no .cpg.
    # (GDAL's default, LDID 0x57 = Windows-1252 without .cpg, is what most other cases carry.)
    names = ["Москва", "Київ", "София", "Минск"]
    pts = grid_points(23.0, 42.0, 38.0, 56.0, 4, 1)
    feats = [(wkt_point(p), {"id": i + 1, "name": n}) for i, (p, n) in enumerate(zip(pts, names))]
    shp(d, "shops", srs_epsg(4326), ogr.wkbPoint, feats, ATTR_FIELDS, extra_opts=["ENCODING=LDID/201"])
    cpg = d / "shops.cpg"
    if cpg.exists():
        cpg.unlink()


@builder("shp_cpg_cp936")
def _(d):
    names = ["北京", "上海", "广州", "深圳"]
    pts = grid_points(113.0, 22.0, 117.0, 40.0, 4, 1)
    feats = [(wkt_point(p), {"id": i + 1, "name": n}) for i, (p, n) in enumerate(zip(pts, names))]
    shp(d, "cities", srs_epsg(4326), ogr.wkbPoint, feats, ATTR_FIELDS, extra_opts=["ENCODING=CP936"])


@builder("shp_empty")
def _(d):
    shp(d, "nothing", srs_epsg(32633), ogr.wkbPolygon, [])


@builder("shp_pointzm")
def _(d):
    pts = project(32633, grid_points(14.0, 46.0, 15.0, 47.0, 3, 3))
    feats = [(wkt_point(p, z=100.0 + 10 * i, m=float(i)), {"id": i + 1, "name": f"z{i + 1}"}) for i, p in enumerate(pts)]
    shp(d, "soundings", srs_epsg(32633), ogr.wkbPointZM, feats)


@builder("shp_no_prj")
def _(d):
    pts = [(1000.0 + 50.0 * i, 2000.0 + 25.0 * i) for i in range(6)]
    shp(d, "local", None, ogr.wkbPoint, named([wkt_point(p) for p in pts], "l"))
    prj = d / "local.prj"
    if prj.exists():
        prj.unlink()


@builder("shp_fiji_3460_antimeridian")
def _(d):
    # Islands on both sides of 180°: lon 177°E … 179°W (= 181°E), Fiji Map Grid (EPSG:3460).
    polys = []
    for k, (lon, lat) in enumerate([(177.2, -17.8), (178.4, -18.1), (179.6, -16.6), (-179.4, -17.2), (-178.6, -18.4)]):
        ring = [(lon + 0.3 * math.cos(2 * math.pi * t / 12), lat + 0.2 * math.sin(2 * math.pi * t / 12)) for t in range(12)]
        polys.append(wkt_polygon(project(3460, ring)))
    shp(d, "islands", srs_epsg(3460), ogr.wkbPolygon, named(polys, "island"))


@builder("shp_corrupt_truncated_header")
def _(d):
    pts = project(32633, grid_points(14.0, 46.0, 15.0, 47.0, 3, 3))
    shp(d, "broken", srs_epsg(32633), ogr.wkbPoint, named([wkt_point(p) for p in pts]))
    truncate(d / "broken.shp", 60)  # shorter than the 100-byte main file header


@builder("shp_corrupt_truncated_records")
def _(d):
    pts = project(32633, grid_points(14.0, 46.0, 15.0, 47.0, 4, 4))
    shp(d, "halfway", srs_epsg(32633), ogr.wkbPoint, named([wkt_point(p) for p in pts]))
    p = d / "halfway.shp"
    truncate(p, 100 + 28 * 7 + 10)  # header intact, file-length field now lies, 8th record cut


@builder("shp_corrupt_bad_dbf")
def _(d):
    pts = project(32633, grid_points(14.0, 46.0, 15.0, 47.0, 3, 3))
    shp(d, "baddbf", srs_epsg(32633), ogr.wkbPoint, named([wkt_point(p) for p in pts]))
    p = d / "baddbf.dbf"
    data = bytearray(p.read_bytes())
    data[4:8] = struct.pack("<I", 0x7FFFFFF0)  # absurd record count
    data[8:10] = struct.pack("<H", 0xFFFF)  # header length beyond EOF
    p.write_bytes(bytes(data))


# ---- GeoPackage ----------------------------------------------------------------------------

def gpkg_create(path: Path):
    return ogr.GetDriverByName("GPKG").CreateDataSource(str(path))


@builder("gpkg_multi_layer")
def _(d):
    path = d / "multi.gpkg"
    ds = gpkg_create(path)
    write_vector(path, "GPKG", "poi", srs_epsg(4326), ogr.wkbPoint,
                 named([wkt_point(p) for p in grid_points(10.0, 50.0, 12.0, 52.0, 3, 3)], "poi"), ds=ds)
    lines = [wkt_line(project(32633, [(13.0 + 0.5 * i, 46.0 + 0.3 * k) for i in range(5)])) for k in range(4)]
    write_vector(path, "GPKG", "tracks", srs_epsg(32633), ogr.wkbLineString, named(lines, "trk"), ds=ds)
    polys = [wkt_polygon(project(3857, box_ring(-74.1 + 0.05 * k, 40.6, -74.06 + 0.05 * k, 40.65))) for k in range(3)]
    write_vector(path, "GPKG", "blocks", srs_epsg(3857), ogr.wkbMultiPolygon,
                 named([w.replace("POLYGON ((", "MULTIPOLYGON (((").replace("))", ")))") for w in polys], "blk"), ds=ds)
    ds = None
    normalise_gpkg(path)


@builder("gpkg_stale_bounds")
def _(d):
    path = d / "stale.gpkg"
    pts = project(32633, grid_points(13.0, 45.5, 16.5, 48.0, 4, 4))
    write_vector(path, "GPKG", "samples", srs_epsg(32633), ogr.wkbPoint, named([wkt_point(p) for p in pts], "s"))
    # Bounds left over from an earlier, smaller version of the layer: the R-tree has the truth.
    gpkg_sql(path, ("UPDATE gpkg_contents SET min_x = ?, min_y = ?, max_x = ?, max_y = ? WHERE table_name = 'samples'",
                    (400000.0, 5100000.0, 450000.0, 5150000.0)))
    normalise_gpkg(path)


@builder("gpkg_missing_bounds")
def _(d):
    path = d / "nobounds.gpkg"
    lines = [wkt_line(project(2154, [(2.0 + 0.4 * i, 46.0 + 0.5 * k) for i in range(6)])) for k in range(4)]
    write_vector(path, "GPKG", "canals", srs_epsg(2154), ogr.wkbLineString, named(lines, "canal"))
    gpkg_sql(path, ("UPDATE gpkg_contents SET min_x = NULL, min_y = NULL, max_x = NULL, max_y = NULL", ()))
    normalise_gpkg(path)


@builder("gpkg_attributes_only")
def _(d):
    path = d / "table.gpkg"
    rows = [(None, {"id": i + 1, "name": f"row{i + 1}"}) for i in range(7)]
    write_vector(path, "GPKG", "owners", None, ogr.wkbNone, rows)
    normalise_gpkg(path)


def tiles_source(w=256, h=256):
    # Web Mercator area around Wellington, NZ; 3-band RGB.
    x0, y0 = project(3857, [(174.70, -41.25)])[0]
    x1, y1 = project(3857, [(174.85, -41.35)])[0]
    gt = (x0, (x1 - x0) / w, 0.0, y0, 0.0, (y1 - y0) / h)
    return mem_raster(w, h, 3, "Byte", gt, srs_epsg(3857))


@builder("gpkg_tiles")
def _(d):
    path = d / "tiles.gpkg"
    gdal.Translate(str(path), tiles_source(), format="GPKG",
                   creationOptions=["TILE_FORMAT=PNG", "RASTER_TABLE=imagery", "BLOCKSIZE=128"])
    normalise_gpkg(path)


@builder("gpkg_mixed_content")
def _(d):
    path = d / "mixed.gpkg"
    gdal.Translate(str(path), tiles_source(128, 128), format="GPKG",
                   creationOptions=["TILE_FORMAT=PNG", "RASTER_TABLE=basemap", "BLOCKSIZE=128"])
    ds = gdal.OpenEx(str(path), gdal.OF_VECTOR | gdal.OF_UPDATE)
    pts = project(3857, grid_points(174.72, -41.33, 174.83, -41.27, 3, 2))
    write_vector(path, "GPKG", "stops", srs_epsg(3857), ogr.wkbPoint, named([wkt_point(p) for p in pts], "stop"), ds=ds)
    write_vector(path, "GPKG", "timetable", None, ogr.wkbNone,
                 [(None, {"id": i + 1, "name": f"run{i + 1}"}) for i in range(4)], ds=ds)
    ds = None
    normalise_gpkg(path)


CUSTOM_LCC = (
    'PROJCS["Custom LCC",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],'
    'PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Lambert_Conformal_Conic_2SP"],'
    'PARAMETER["latitude_of_origin",40],PARAMETER["central_meridian",-97.5],'
    'PARAMETER["standard_parallel_1",33.25],PARAMETER["standard_parallel_2",46.75],'
    'PARAMETER["false_easting",1000000],PARAMETER["false_northing",500000],UNIT["metre",1]]'
)


@builder("gpkg_custom_srs")
def _(d):
    path = d / "custom.gpkg"
    srs = srs_wkt(CUSTOM_LCC)
    ct = osr.CoordinateTransformation(srs_epsg(4326), srs)
    polys = []
    for k in range(3):
        ring = [tuple(ct.TransformPoint(lon, lat)[:2]) for lon, lat in box_ring(-104.0 + 5 * k, 36.0, -100.0 + 5 * k, 40.0, 6)]
        polys.append(wkt_polygon(ring))
    write_vector(path, "GPKG", "fields", srs, ogr.wkbPolygon, named(polys, "field"))
    normalise_gpkg(path)


@builder("gpkg_undefined_srs")
def _(d):
    path = d / "nocrs.gpkg"
    pts = [(10.0 * i, 5.0 * i) for i in range(5)]
    write_vector(path, "GPKG", "sketch", None, ogr.wkbPoint, named([wkt_point(p) for p in pts], "p"))
    # GDAL >= 3.x registers its own "Undefined SRS" (srs_id 99999); older writers and the spec use
    # the mandatory srs_id -1 "Undefined Cartesian SRS" row. Exercise the spec form.
    gpkg_sql(path,
             ("UPDATE gpkg_geometry_columns SET srs_id = -1 WHERE table_name = 'sketch'", ()),
             ("UPDATE gpkg_contents SET srs_id = -1 WHERE table_name = 'sketch'", ()),
             ("DELETE FROM gpkg_spatial_ref_sys WHERE srs_id NOT IN (-1, 0, 4326)", ()))
    normalise_gpkg(path)


@builder("gpkg_empty_layer")
def _(d):
    path = d / "empty.gpkg"
    write_vector(path, "GPKG", "planned", srs_epsg(3413), ogr.wkbPolygon, [])
    normalise_gpkg(path)


@builder("gpkg_corrupt_truncated")
def _(d):
    path = d / "cut.gpkg"
    pts = project(32633, grid_points(13.0, 45.5, 16.5, 48.0, 4, 4))
    write_vector(path, "GPKG", "samples", srs_epsg(32633), ogr.wkbPoint, named([wkt_point(p) for p in pts], "s"))
    normalise_gpkg(path)
    truncate(path, 3000)  # keeps the SQLite header, loses most pages


@builder("gpkg_corrupt_not_sqlite")
def _(d):
    (d / "fake.gpkg").write_bytes(b"This is not a GeoPackage, just text with a .gpkg extension.\n" * 4)


# ---- GeoTIFF -------------------------------------------------------------------------------

def utm_gt(x0=500000.0, y0=5200000.0, px=30.0):
    return (x0, px, 0.0, y0, 0.0, -px)


@builder("tif_northup_utm")
def _(d):
    gtiff(d / "dem.tif", 64, 48, 1, "Byte", utm_gt(), srs_epsg(32633))


@builder("tif_rotated")
def _(d):
    a = math.radians(15.0)
    px = 20.0
    gt = (480000.0, px * math.cos(a), -px * math.sin(a), 5150000.0, -px * math.sin(a), -px * math.cos(a))
    gtiff(d / "scan.tif", 48, 40, 1, "Byte", gt, srs_epsg(32633))


@builder("tif_pixelispoint")
def _(d):
    gt = (-3.0, 0.01, 0.0, 52.0, 0.0, -0.01)
    gtiff(d / "grid.tif", 40, 30, 1, "Int16", gt, srs_epsg(4326), metadata={"AREA_OR_POINT": "Point"})


@builder("tif_bigtiff")
def _(d):
    gt = (1740000.0, 50.0, 0.0, 5430000.0, 0.0, -50.0)
    gtiff(d / "big.tif", 64, 64, 1, "UInt16", gt, srs_epsg(2193), options=["BIGTIFF=YES"])


@builder("tif_cog_overviews")
def _(d):
    x0, y0 = project(3857, [(-0.20, 51.55)])[0]
    src = mem_raster(512, 512, 1, "Byte", (x0, 20.0, 0.0, y0, 0.0, -20.0), srs_epsg(3857))
    gdal.Translate(str(d / "cog.tif"), src, format="COG",
                   creationOptions=["BLOCKSIZE=128","COMPRESS=DEFLATE", "OVERVIEWS=AUTO", "RESAMPLING=NEAREST"])


@builder("tif_aux_xml_only")
def _(d):
    # Baseline TIFF: no GeoTIFF tags at all; georeferencing lives only in the PAM .aux.xml.
    gt = (650000.0, 25.0, 0.0, 6860000.0, 0.0, -25.0)
    gtiff(d / "ortho.tif", 48, 48, 1, "Byte", gt, srs_epsg(2154), options=["PROFILE=BASELINE"])


@builder("tif_worldfile_prj")
def _(d):
    # Baseline TIFF + .tfw (geotransform) + ESRI .prj (CRS) — the classic ArcGIS export.
    gt = (-2000000.0, 100.0, 0.0, 2500000.0, 0.0, -100.0)
    srs = srs_epsg(5070)
    gtiff(d / "landcover.tif", 64, 48, 1, "Byte", gt, srs, options=["PROFILE=BASELINE", "TFW=YES"])
    aux = d / "landcover.tif.aux.xml"
    if aux.exists():
        aux.unlink()
    (d / "landcover.prj").write_bytes(esri_wkt(srs).encode("ascii"))


@builder("tif_nodata_float32")
def _(d):
    gt = (-800000.0, 25000.0, 0.0, 800000.0, 0.0, -25000.0)  # spans the South Pole
    gtiff(d / "thickness.tif", 64, 64, 1, "Float32", gt, srs_epsg(3031), nodata=-9999.0)


@builder("tif_nodata_nan")
def _(d):
    gt = (-1200000.0, 20000.0, 0.0, 400000.0, 0.0, -20000.0)
    ds = gdal.GetDriverByName("GTiff").Create(str(d / "seaice.tif"), 40, 40, 1, gdal.GDT_Float32)
    ds.SetGeoTransform(gt)
    ds.SetSpatialRef(srs_epsg(3413))
    band = ds.GetRasterBand(1)
    values = pattern(40, 40, 1, "Float32")
    values[0] = float("nan")
    band.WriteRaster(0, 0, 40, 40, pack("Float32", values), buf_type=gdal.GDT_Float32)
    band.SetNoDataValue(float("nan"))
    ds.FlushCache()
    ds = None


@builder("tif_multiband_uint16")
def _(d):
    gt = (-75.5, 0.005, 0.0, 40.0, 0.0, -0.005)
    gtiff(d / "rgbn.tif", 40, 40, 4, "UInt16", gt, srs_epsg(4326), options=["INTERLEAVE=PIXEL"], nodata=0)


@builder("tif_fiji_3460_antimeridian")
def _(d):
    gt = (1800000.0, 5000.0, 0.0, 4100000.0, 0.0, -5000.0)  # ~177°E … ~179°W
    gtiff(d / "fiji.tif", 96, 48, 1, "Byte", gt, srs_epsg(3460))


@builder("tif_no_georef")
def _(d):
    gtiff(d / "photo.tif", 32, 24, 3, "Byte", None, None, options=["PROFILE=BASELINE", "PHOTOMETRIC=RGB"])


@builder("tif_corrupt_truncated")
def _(d):
    gtiff(d / "cut.tif", 64, 64, 1, "Byte", utm_gt(), srs_epsg(32633))
    truncate(d / "cut.tif", 200)


@builder("tif_corrupt_not_tiff")
def _(d):
    (d / "fake.tif").write_bytes(b"II*\x00" + b"\xff\xff\xff\x7f" + b"garbage, not a real IFD" * 3)


# --------------------------------------------------------------------------------------------


def main(argv):
    only = set(argv[1:])
    manifest = tomllib.loads((CORPUS / "manifest.toml").read_text(encoding="utf-8"))
    cases = [c for c in manifest["case"] if c.get("origin") == "synthetic"]
    ids = [c["id"] for c in cases]
    unknown = sorted(set(BUILDERS) - set(ids))
    missing = sorted(set(ids) - set(BUILDERS))
    if unknown or missing:
        sys.exit(f"manifest/builder mismatch: builders without case {unknown}, cases without builder {missing}")
    failures = []
    for case in cases:
        if only and case["id"] not in only:
            continue
        d = DATA / case["format"] / case["id"]
        if d.exists():
            shutil.rmtree(d)
        d.mkdir(parents=True)
        BUILDERS[case["id"]](d)
        produced = sorted(p.name for p in d.iterdir())
        declared = sorted(case["files"])
        status = "ok" if produced == declared else "FILES MISMATCH"
        print(f"{case['id']:36s} {status}  {produced}")
        if produced != declared:
            failures.append(case["id"])
    if failures:
        sys.exit(f"files produced differ from manifest for: {failures}")


if __name__ == "__main__":
    main(sys.argv)
