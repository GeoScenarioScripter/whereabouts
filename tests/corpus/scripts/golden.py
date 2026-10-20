#!/usr/bin/env python3
"""Write one normalized GDAL *oracle record* per corpus case to ``tests/corpus/expected/<id>.json``.

Runs inside the pinned GDAL container (see ``generate.sh``). The record format is documented in
``tests/corpus/README.md`` and mirrored by the serde types in ``tests/oracle/src/record.rs``;
bump ``SCHEMA`` (both places) on any incompatible change.

Rules:
- Read-only: files are opened with ``OF_READONLY`` / SQLite ``mode=ro&immutable=1`` and nothing
  that writes PAM side files (statistics, histograms) is ever called.
- Deterministic: stable key order, floats rounded to ``DECIMALS`` places, ``-0.0`` folded to
  ``0.0``, messages stripped of absolute paths.
- External cases whose data has not been fetched keep their existing golden untouched.
"""

from __future__ import annotations

import json
import math
import re
import sqlite3
import sys
import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path

from osgeo import gdal, ogr, osr

gdal.UseExceptions()
ogr.UseExceptions()
osr.UseExceptions()

SCHEMA = 1
DECIMALS = 9
DENSIFY_POINTS = 21
MATCH_MIN_CONFIDENCE = 70  # FindMatches confidence accepted as an identification

CORPUS = Path(__file__).resolve().parent.parent
EXPECTED = CORPUS / "expected"

ORACLE = {
    "gdal": gdal.__version__,
    "proj": f"{osr.GetPROJVersionMajor()}.{osr.GetPROJVersionMinor()}.{osr.GetPROJVersionMicro()}",
}

DRIVERS = {"shapefile": "ESRI Shapefile", "geopackage": "GPKG", "geotiff": "GTiff"}

GEOM_NAMES = {
    ogr.wkbUnknown: "Unknown",
    ogr.wkbPoint: "Point",
    ogr.wkbLineString: "LineString",
    ogr.wkbPolygon: "Polygon",
    ogr.wkbMultiPoint: "MultiPoint",
    ogr.wkbMultiLineString: "MultiLineString",
    ogr.wkbMultiPolygon: "MultiPolygon",
    ogr.wkbGeometryCollection: "GeometryCollection",
}


# --------------------------------------------------------------------------------------------
# normalisation
# --------------------------------------------------------------------------------------------


def num(v):
    if v is None:
        return None
    v = float(v)
    if math.isnan(v):
        return "nan"
    if math.isinf(v):
        return "inf" if v > 0 else "-inf"
    return round(v, DECIMALS) + 0.0  # + 0.0 folds -0.0 into 0.0


def exact(v):
    """Like num() but without rounding: for values compared with a purely relative tolerance
    (raster resolution), where 9 decimals would be far coarser than the tolerance."""
    if v is None:
        return None
    v = float(v)
    if math.isnan(v) or math.isinf(v):
        return num(v)
    return v + 0.0


def bbox(minx, miny, maxx, maxy):
    return [num(minx), num(miny), num(maxx), num(maxy)]


class Messages:
    """Collects GDAL warnings/errors emitted while a case is processed (paths stripped)."""

    def __init__(self, root: Path):
        self.root = str(root).rstrip("/") + "/"
        self.items: list[str] = []

    def clean(self, msg: str) -> str:
        msg = msg.replace(self.root, "").replace(str(CORPUS) + "/", "")
        return re.sub(r"\s+", " ", msg).strip()

    def handler(self, err_class, err_no, msg):
        if err_class >= gdal.CE_Warning:
            m = self.clean(msg)
            if m not in self.items:
                self.items.append(m)


# --------------------------------------------------------------------------------------------
# CRS
# --------------------------------------------------------------------------------------------


def raw_kind(text: str | None) -> str | None:
    if text is None:
        return None
    t = text.lstrip()
    head = t.split("[", 1)[0].strip().upper()
    if head in ("PROJCRS", "GEOGCRS", "GEODCRS", "BOUNDCRS", "COMPOUNDCRS", "VERTCRS", "ENGCRS", "DERIVEDPROJCRS"):
        return "wkt2"
    if head in ("PROJCS", "GEOGCS", "GEOCCS", "COMPD_CS", "LOCAL_CS", "VERT_CS"):
        esri = ("AUTHORITY[" not in t.upper()) and bool(re.search(r'"(GCS|D)_', t))
        return "wkt1_esri" if esri else "wkt1"
    if t.startswith("{"):
        return "projjson"
    return "other"


def trad(srs):
    s = srs.Clone()
    s.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    return s


def identify(srs, raw):
    """EPSG identification, preferring the raw definition the file actually carries."""
    candidates_src = srs
    if raw:
        try:
            r = osr.SpatialReference()
            r.SetFromUserInput(raw)
            candidates_src = r
        except RuntimeError:
            candidates_src = srs
    s = candidates_src.Clone()
    if s.GetAuthorityName(None) == "EPSG" and s.GetAuthorityCode(None):
        return int(s.GetAuthorityCode(None)), "authority", 100, []
    try:
        s.AutoIdentifyEPSG()
        if s.GetAuthorityName(None) == "EPSG" and s.GetAuthorityCode(None):
            return int(s.GetAuthorityCode(None)), "auto_identify", 100, []
    except RuntimeError:
        pass
    try:
        matches = candidates_src.FindMatches() or []
    except RuntimeError:
        matches = []
    cands = []
    for m, conf in matches[:5]:
        cands.append({"authority": m.GetAuthorityName(None), "code": m.GetAuthorityCode(None), "confidence": int(conf)})
    for c in cands:
        if c["authority"] == "EPSG" and c["code"] and c["confidence"] >= MATCH_MIN_CONFIDENCE:
            return int(c["code"]), "find_matches", c["confidence"], cands
    return None, None, (cands[0]["confidence"] if cands else None), cands


def usable(srs) -> bool:
    """A CRS we can place on the globe: engineering/local CRSs ("Undefined Cartesian") are not."""
    return srs is not None and not srs.IsLocal()


def crs_record(srs, source, raw, declared_authority=None, kind=None):
    if not usable(srs):
        return {
            "status": "missing", "source": source, "raw_kind": kind or raw_kind(raw), "raw": raw,
            "declared_authority": declared_authority, "wkt2": None, "epsg": None,
            "epsg_method": None, "epsg_confidence": None, "candidates": [],
        }
    epsg, method, conf, cands = identify(srs, raw)
    return {
        "status": "identified" if epsg is not None else "unidentified",
        "source": source,
        "raw_kind": kind or raw_kind(raw),
        "raw": raw,
        "declared_authority": declared_authority,
        "wkt2": srs.ExportToWkt(["FORMAT=WKT2_2019"]),
        "epsg": epsg,
        "epsg_method": method,
        "epsg_confidence": conf,
        "candidates": cands,
    }


def wgs84(srs, extent):
    """Antimeridian-aware WGS84 bbox via OCTTransformBounds with edge densification."""
    if not usable(srs) or extent is None:
        return None, False, None
    try:
        dst = osr.SpatialReference()
        dst.ImportFromEPSG(4326)
        dst.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
        ct = osr.CoordinateTransformation(trad(srs), dst)
        w, s, e, n = ct.TransformBounds(extent[0], extent[1], extent[2], extent[3], DENSIFY_POINTS)
    except RuntimeError as exc:
        return None, False, re.sub(r"\s+", " ", str(exc))[:200]
    return bbox(w, s, e, n), bool(w > e), None


def read_text(p: Path) -> str:
    data = p.read_bytes()
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return data.decode("latin-1")


# --------------------------------------------------------------------------------------------
# layers
# --------------------------------------------------------------------------------------------


def geometry_info(gtype):
    if gtype == ogr.wkbNone:
        return None, False, False
    flat = ogr.GT_Flatten(gtype)
    return GEOM_NAMES.get(flat, f"wkb{flat}"), bool(ogr.GT_HasZ(gtype)), bool(ogr.GT_HasM(gtype))


def data_extent(lyr):
    """Union of the envelopes of every non-empty geometry (ground truth, independent of headers)."""
    acc = None
    lyr.ResetReading()
    for f in lyr:
        g = f.GetGeometryRef()
        if g is None or g.IsEmpty():
            continue
        minx, maxx, miny, maxy = g.GetEnvelope()
        if acc is None:
            acc = [minx, miny, maxx, maxy]
        else:
            acc = [min(acc[0], minx), min(acc[1], miny), max(acc[2], maxx), max(acc[3], maxy)]
    return acc


def vector_layer(lyr, kind, crs, declared_extent):
    gname, has_z, has_m = geometry_info(lyr.GetGeomType())
    count = lyr.GetFeatureCount(1)
    extent = data_extent(lyr) if gname is not None else None
    srs = lyr.GetSpatialRef()
    w, crosses, werr = wgs84(srs, extent)
    rec = {
        "name": lyr.GetName(),
        "kind": kind,
        "geometry_type": gname,
        "has_z": has_z,
        "has_m": has_m,
        "feature_count": int(count) if count is not None and count >= 0 else None,
        "extent": bbox(*extent) if extent else None,
        "declared_extent": declared_extent,
        "raster": None,
        "fields": fields_of(lyr),
        "encoding": None,
        "crs": crs,
        "wgs84_extent": w,
        "crosses_antimeridian": crosses,
    }
    rec["wgs84_error"] = werr
    return rec


def fields_of(lyr):
    defn = lyr.GetLayerDefn()
    out = []
    for i in range(defn.GetFieldCount()):
        fd = defn.GetFieldDefn(i)
        out.append({"name": fd.GetName(), "type": fd.GetTypeName(), "width": fd.GetWidth()})
    return out


def shapefile_layers(case_dir: Path, anchor: Path):
    ds = gdal.OpenEx(str(anchor), gdal.OF_VECTOR | gdal.OF_READONLY, allowed_drivers=["ESRI Shapefile"])
    lyr = ds.GetLayer(0)
    prj = anchor.with_suffix(".prj")
    raw = read_text(prj) if prj.exists() else None
    srs = lyr.GetSpatialRef()
    crs = crs_record(srs, "prj" if raw is not None else "none", raw)
    minx, maxx, miny, maxy = lyr.GetExtent(force=0)
    rec = vector_layer(lyr, "vector", crs, bbox(minx, miny, maxx, maxy))
    md = lyr.GetMetadata("SHAPEFILE") or {}
    rec["encoding"] = {
        # What GDAL decodes the DBF with, and the two signals it chose from (.cpg wins over LDID).
        "source_encoding": md.get("SOURCE_ENCODING") or None,
        "cpg": md.get("CPG_VALUE"),
        "encoding_from_cpg": md.get("ENCODING_FROM_CPG"),
        "ldid": int(md["LDID_VALUE"]) if md.get("LDID_VALUE", "").isdigit() else None,
        "encoding_from_ldid": md.get("ENCODING_FROM_LDID"),
    }
    return [rec]


def gpkg_layers(case_dir: Path, anchor: Path):
    ds = gdal.OpenEx(str(anchor), gdal.OF_VECTOR | gdal.OF_RASTER | gdal.OF_READONLY, allowed_drivers=["GPKG"])
    con = sqlite3.connect(f"file:{anchor}?mode=ro&immutable=1", uri=True)
    try:
        rows = con.execute(
            "SELECT c.table_name, c.data_type, c.min_x, c.min_y, c.max_x, c.max_y, c.srs_id, "
            "s.organization, s.organization_coordsys_id, s.definition "
            "FROM gpkg_contents c LEFT JOIN gpkg_spatial_ref_sys s ON s.srs_id = c.srs_id "
            "ORDER BY c.table_name"
        ).fetchall()
    finally:
        con.close()
    layers = []
    for table, dtype, x0, y0, x1, y1, srs_id, org, org_id, definition in rows:
        declared = None if None in (x0, y0, x1, y1) else bbox(x0, y0, x1, y1)
        raw = definition if definition and definition.strip().lower() != "undefined" else None
        # org/org_id are NULL when srs_id dangles (no gpkg_spatial_ref_sys row): keep them null.
        authority = ({"name": org, "code": None if org_id is None else str(org_id), "srs_id": srs_id}
                     if srs_id is not None else None)
        if dtype in ("features", "attributes"):
            lyr = ds.GetLayerByName(table)
            srs = lyr.GetSpatialRef()
            crs = crs_record(srs, "gpkg_srs" if srs_id is not None else "none", raw, authority)
            layers.append(vector_layer(lyr, "vector" if dtype == "features" else "attributes", crs, declared))
        elif dtype in ("tiles", "2d-gridded-coverage"):
            rds = gdal.OpenEx(str(anchor), gdal.OF_RASTER | gdal.OF_READONLY, allowed_drivers=["GPKG"],
                              open_options=[f"TABLE={table}"])
            rec = raster_layer(rds, table, "tiles", anchor, crs_source="gpkg_srs", raw=raw, authority=authority)
            rec["declared_extent"] = declared
            layers.append(rec)
        else:
            layers.append({"name": table, "kind": f"other:{dtype}", "geometry_type": None, "has_z": False,
                           "has_m": False, "feature_count": None, "extent": None, "declared_extent": declared,
                           "raster": None, "fields": None, "encoding": None,
                           "crs": crs_record(None, "none", None), "wgs84_extent": None,
                           "crosses_antimeridian": False, "wgs84_error": None})
    return layers


def gt_extent(gt, w, h):
    xs, ys = [], []
    for px, py in ((0, 0), (w, 0), (0, h), (w, h)):
        xs.append(gt[0] + px * gt[1] + py * gt[2])
        ys.append(gt[3] + px * gt[4] + py * gt[5])
    return [min(xs), min(ys), max(xs), max(ys)]


def georef_with(anchor: Path, source: str):
    try:
        ds = gdal.OpenEx(str(anchor), gdal.OF_RASTER | gdal.OF_READONLY, allowed_drivers=["GTiff"],
                         open_options=[f"GEOREF_SOURCES={source}"])
    except RuntimeError:
        return None, None
    return ds.GetGeoTransform(can_return_null=True), ds.GetSpatialRef()


def aux_srs_text(anchor: Path):
    aux = anchor.with_name(anchor.name + ".aux.xml")
    if not aux.exists():
        return None
    try:
        node = ET.fromstring(aux.read_bytes()).find("SRS")
    except ET.ParseError:
        return None
    return node.text if node is not None else None


def raster_layer(ds, name, kind, anchor: Path, crs_source=None, raw=None, authority=None):
    w, h, n = ds.RasterXSize, ds.RasterYSize, ds.RasterCount
    gt = ds.GetGeoTransform(can_return_null=True)
    srs = ds.GetSpatialRef()
    georef_source = None
    if kind == "raster":
        # Which GTiff georeferencing source GDAL actually used (default priority order).
        for src, label in (("PAM", "aux_xml"), ("INTERNAL", "geotiff_tags"), ("WORLDFILE", "worldfile")):
            g, _ = georef_with(anchor, src)
            if gt is not None and g is not None and tuple(g) == tuple(gt):
                georef_source = label
                break
        if srs is not None:
            if georef_with(anchor, "INTERNAL")[1] is not None:
                crs_source, raw, rkind = "geokeys", None, "geokeys"
            else:
                crs_source, raw, rkind = "aux_xml", aux_srs_text(anchor), None
        else:
            prj = anchor.with_suffix(".prj")
            rkind = None
            if prj.exists():
                raw = read_text(prj)
                try:
                    srs = osr.SpatialReference()
                    srs.SetFromUserInput(raw)
                    crs_source = "prj"
                except RuntimeError:
                    srs, crs_source = None, "prj"
            else:
                crs_source = "none"
        crs = crs_record(srs, crs_source, raw, authority, kind=rkind)
    else:
        georef_source = "gpkg_tile_matrix"
        crs = crs_record(srs, crs_source, raw, authority)
    bands = [ds.GetRasterBand(i + 1) for i in range(n)]
    extent = gt_extent(gt, w, h) if gt is not None else None
    wext, crosses, werr = wgs84(srs, extent)
    head = anchor.read_bytes()[:4] if kind == "raster" else b""
    raster = {
        "width": w,
        "height": h,
        "band_count": n,
        "data_types": [gdal.GetDataTypeName(b.DataType) for b in bands],
        "nodata": [num(b.GetNoDataValue()) for b in bands],
        "geotransform": [num(v) for v in gt] if gt is not None else None,
        "resolution": [exact(math.hypot(gt[1], gt[4])), exact(math.hypot(gt[2], gt[5]))] if gt is not None else None,
        "georef_source": georef_source,
        "pixel_is_point": (ds.GetMetadataItem("AREA_OR_POINT") or "").lower() == "point",
        "overview_count": bands[0].GetOverviewCount() if bands else 0,
        "bigtiff": head in (b"II+\x00", b"MM\x00+"),
        "layout": ds.GetMetadataItem("LAYOUT", "IMAGE_STRUCTURE"),
        "compression": ds.GetMetadataItem("COMPRESSION", "IMAGE_STRUCTURE"),
    }
    rec = {
        "name": name,
        "kind": kind,
        "geometry_type": None,
        "has_z": False,
        "has_m": False,
        "feature_count": None,
        "extent": bbox(*extent) if extent else None,
        "declared_extent": None,
        "raster": raster,
        "fields": None,
        "encoding": None,
        "crs": crs,
        "wgs84_extent": wext,
        "crosses_antimeridian": crosses,
    }
    rec["wgs84_error"] = werr
    return rec


def geotiff_layers(case_dir: Path, anchor: Path):
    ds = gdal.OpenEx(str(anchor), gdal.OF_RASTER | gdal.OF_READONLY, allowed_drivers=["GTiff"])
    return [raster_layer(ds, anchor.stem, "raster", anchor)]


READERS = {"shapefile": shapefile_layers, "geopackage": gpkg_layers, "geotiff": geotiff_layers}


# --------------------------------------------------------------------------------------------


def case_root(case) -> Path:
    base = "external" if case.get("location") == "external" else "data"
    return CORPUS / base / case["format"] / case["id"]


def golden(case):
    root = case_root(case)
    msgs = Messages(root)
    rec = {
        "schema": SCHEMA,
        "case": case["id"],
        "format": case["format"],
        "oracle": ORACLE,
        "status": "ok",
        "error": None,
        "messages": msgs.items,
        "layers": [],
    }
    gdal.PushErrorHandler(msgs.handler)
    try:
        rec["layers"] = READERS[case["format"]](root, root / case["anchor"])
    except Exception as exc:  # noqa: BLE001 — any GDAL failure is the oracle's "error" verdict
        rec["status"] = "error"
        rec["error"] = msgs.clean(str(exc))
        rec["layers"] = []
    finally:
        gdal.PopErrorHandler()
    rec["messages"] = msgs.items
    return rec


def main(argv):
    only = set(argv[1:])
    manifest = tomllib.loads((CORPUS / "manifest.toml").read_text(encoding="utf-8"))
    EXPECTED.mkdir(exist_ok=True)
    ids = set()
    for case in manifest["case"]:
        ids.add(case["id"])
        if only and case["id"] not in only:
            continue
        # Same rule as Corpus::is_present: a partial fetch must never overwrite a committed golden.
        root = case_root(case)
        if not all((root / f).is_file() for f in case["files"]):
            print(f"{case['id']:36s} skipped (data not present; golden kept)")
            continue
        rec = golden(case)
        text = json.dumps(rec, indent=2, ensure_ascii=False, allow_nan=False) + "\n"
        (EXPECTED / f"{case['id']}.json").write_text(text, encoding="utf-8", newline="\n")
        summary = rec["status"] if rec["status"] == "ok" else f"error: {rec['error'][:60]}"
        print(f"{case['id']:36s} {len(rec['layers'])} layer(s)  {summary}")
    if not only:
        for stale in sorted(EXPECTED.glob("*.json")):
            if stale.stem not in ids:
                print(f"removing stale golden {stale.name}")
                stale.unlink()


if __name__ == "__main__":
    main(sys.argv)
