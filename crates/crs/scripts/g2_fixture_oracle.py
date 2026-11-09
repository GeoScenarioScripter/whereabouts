"""GDAL oracle records for the CRS fixtures in crates/crs/tests/data (gate G2 evidence).

Runs inside the pinned GDAL image (see run-in-gdal.sh) and writes
crates/crs/tests/g2/fixture-oracle.json:

- one `identify` record per fixture file: the EPSG code GDAL identifies from the raw text, with
  the same rule as the corpus goldens (tests/corpus/scripts/golden.py: authority →
  AutoIdentifyEPSG → first EPSG FindMatches candidate with confidence >= 70);
- per fixture directory with a horizontal CRS and an area of use, two boxes: `area` (the whole
  area of use) and `inner` (a box of 20% of its size around its centre), each given as the native
  bbox GDAL gets by projecting the lon/lat box (TransformBounds, 21 points) and the WGS84 extent
  GDAL gets back from that native bbox (TransformBounds, 21 points, traditional GIS order) —
  exactly what tests/corpus/expected records for real files.
"""

import json
import math
import sys
from pathlib import Path

from osgeo import gdal, osr

gdal.UseExceptions()
osr.UseExceptions()

DATA = Path("crates/crs/tests/data")
OUT = Path("crates/crs/tests/g2/fixture-oracle.json")
DENSIFY = 21
MATCH_MIN_CONFIDENCE = 70


def num(v):
    return None if v is None or not math.isfinite(v) else float(f"{v:.9f}")


def identify(raw):
    s = osr.SpatialReference()
    try:
        s.SetFromUserInput(raw)
    except RuntimeError as exc:
        return {"parsed": False, "epsg": None, "method": None, "confidence": None,
                "error": str(exc)[:200]}
    base = s.Clone()
    if s.IsCompound():
        # Match the horizontal part, as whereabouts-crs does.
        try:
            h = s.Clone()
            h.StripVertical()
            s = h
        except RuntimeError:
            pass
    if s.GetAuthorityName(None) == "EPSG" and s.GetAuthorityCode(None):
        return {"parsed": True, "epsg": int(s.GetAuthorityCode(None)), "method": "authority",
                "confidence": 100}
    t = s.Clone()
    try:
        t.AutoIdentifyEPSG()
        if t.GetAuthorityName(None) == "EPSG" and t.GetAuthorityCode(None):
            return {"parsed": True, "epsg": int(t.GetAuthorityCode(None)),
                    "method": "auto_identify", "confidence": 100}
    except RuntimeError:
        pass
    try:
        matches = s.FindMatches() or []
    except RuntimeError:
        matches = []
    for m, conf in matches[:5]:
        if m.GetAuthorityName(None) == "EPSG" and m.GetAuthorityCode(None) and conf >= MATCH_MIN_CONFIDENCE:
            return {"parsed": True, "epsg": int(m.GetAuthorityCode(None)), "method": "find_matches",
                    "confidence": int(conf)}
    del base
    return {"parsed": True, "epsg": None, "method": None, "confidence": None}


def trad(srs):
    s = srs.Clone()
    s.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    return s


def wgs84():
    s = osr.SpatialReference()
    s.ImportFromEPSG(4326)
    s.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    return s


def boxes(srs):
    if srs.IsCompound():
        srs = srs.Clone()
        srs.StripVertical()
    if not (srs.IsProjected() or srs.IsGeographic()):
        return []
    area = srs.GetAreaOfUse()
    if area is None:
        return []
    w, s, e, n = area.west_lon_degree, area.south_lat_degree, area.east_lon_degree, area.north_lat_degree
    width = (e - w) % 360 or (360 if e != w else 0)
    cx = w + width / 2
    cy = (s + n) / 2
    inner = (cx - width * 0.1, cy - (n - s) * 0.1, cx + width * 0.1, cy + (n - s) * 0.1)
    inner = tuple(((v + 180) % 360) - 180 if i in (0, 2) else v for i, v in enumerate(inner))
    out = []
    to_native = osr.CoordinateTransformation(wgs84(), trad(srs))
    to_wgs = osr.CoordinateTransformation(trad(srs), wgs84())
    for name, (bw, bs, be, bn) in (("area", (w, s, e, n)), ("inner", inner)):
        rec = {"box": name, "lonlat": [num(bw), num(bs), num(be), num(bn)]}
        try:
            native = to_native.TransformBounds(bw, bs, be, bn, DENSIFY)
            rec["native"] = [num(v) for v in native]
            back = to_wgs.TransformBounds(native[0], native[1], native[2], native[3], DENSIFY)
            rec["wgs84"] = [num(v) for v in back]
            rec["crosses_antimeridian"] = bool(back[0] > back[2])
            rec["error"] = None
        except RuntimeError as exc:
            rec.update({"native": None, "wgs84": None, "crosses_antimeridian": False,
                        "error": str(exc)[:200]})
        out.append(rec)
    return out


def epsg_sweep():
    """--epsg-sweep: every non-deprecated EPSG geographic 2D / projected CRS, one inner box each,
    plus its ESRI WKT export and GDAL's identification of that export. Written as JSON lines to
    crates/crs/tests/g2/epsg-sweep.jsonl (gitignored: ~6k records; regenerate locally)."""
    import sqlite3

    con = sqlite3.connect("/usr/share/proj/proj.db")
    codes = [int(r[0]) for r in con.execute(
        "SELECT code FROM geodetic_crs WHERE auth_name = 'EPSG' AND deprecated = 0 AND type = 'geographic 2D'"
        " UNION SELECT code FROM projected_crs WHERE auth_name = 'EPSG' AND deprecated = 0")]
    codes.sort()
    out = OUT.parent / "epsg-sweep.jsonl"
    n = 0
    with out.open("w", encoding="utf-8") as fh:
        for code in codes:
            srs = osr.SpatialReference()
            try:
                srs.ImportFromEPSG(code)
            except RuntimeError:
                continue
            rec = {"epsg": code, "projected": bool(srs.IsProjected())}
            try:
                rec["boxes"] = [b for b in boxes(srs) if b["box"] == "inner"]
            except RuntimeError as exc:
                rec["boxes"] = []
                rec["box_error"] = str(exc)[:200]
            try:
                esri = srs.ExportToWkt(["FORMAT=WKT1_ESRI"])
            except RuntimeError:
                esri = None
            rec["esri"] = esri
            rec["esri_identify"] = identify(esri) if esri else None
            fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
            n += 1
    print(f"wrote {out}: {n} CRSs", file=sys.stderr)


def main():
    if "--epsg-sweep" in sys.argv[1:]:
        epsg_sweep()
        return
    files, dirs = [], []
    for d in sorted(p for p in DATA.iterdir() if p.is_dir()):
        for f in sorted(p for p in d.iterdir() if p.is_file()):
            raw = f.read_bytes().decode("utf-8", errors="replace")
            files.append({"fixture": f"{d.name}/{f.name}", **identify(raw)})
        if d.name == "handwritten":
            continue
        source = next((d / n for n in ("wkt2_2019.txt", "projjson.json", "wkt1.txt", "esri.prj")
                       if (d / n).exists()), None)
        srs = osr.SpatialReference()
        srs.SetFromUserInput(source.read_text(encoding="utf-8"))
        b = boxes(srs)
        if b:
            dirs.append({"fixture": d.name, "boxes": b})
    OUT.parent.mkdir(parents=True, exist_ok=True)
    doc = {
        "schema": 1,
        "oracle": {"gdal": gdal.__version__,
                   "proj": f"{osr.GetPROJVersionMajor()}.{osr.GetPROJVersionMinor()}."
                           f"{osr.GetPROJVersionMicro()}"},
        "identify": files,
        "transform": dirs,
    }
    OUT.write_text(json.dumps(doc, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT}: {len(files)} files, {len(dirs)} CRSs", file=sys.stderr)


main()
