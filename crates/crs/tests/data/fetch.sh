#!/usr/bin/env sh
# Re-fetches the authority-derived CRS fixtures in this directory from spatialreference.org
# (generated from the PROJ database, which embeds the EPSG Geodetic Parameter Dataset and
# Esri's projection-engine definitions). See README.md for provenance and attribution.
#
# Usage: sh crates/crs/tests/data/fetch.sh   (needs curl)
# The fixtures are committed; re-running this is only needed to refresh them. A refresh may
# change text (e.g. a newer EPSG release), so re-run `cargo test -p whereabouts-crs` afterwards.
set -eu

DATA_DIR=$(dirname "$0")
BASE=https://spatialreference.org/ref

# authority:code pairs. Keep in sync with the CASES table in tests/fixtures.rs.
CODES="
epsg:4326 epsg:4269 epsg:4267 epsg:4258 epsg:4283 epsg:7844 epsg:4167 epsg:4807 epsg:4277
epsg:4230 epsg:4978
epsg:32633 epsg:32755 epsg:26917 epsg:25832 epsg:27700 epsg:2193 epsg:28355 epsg:31467
epsg:3460 epsg:2046
epsg:2154 epsg:3347 epsg:27572 epsg:2227 epsg:2263 epsg:3034 epsg:31370 epsg:31300
epsg:5070 epsg:3577 epsg:3005
epsg:3857 epsg:3395 epsg:3832
epsg:3031 epsg:3413 epsg:32661 epsg:3995 epsg:5041
epsg:28992 epsg:3035 epsg:6931 epsg:4087 epsg:3068 epsg:2056 epsg:3375 epsg:5514 epsg:27200
epsg:8857 epsg:7405 epsg:5703 epsg:3855
esri:54009 esri:54030 esri:102003 esri:54034
"

fetch() {
  # $1 = url, $2 = output file. Missing variants are skipped (reported on stderr), including
  # PROJ's "This CRS cannot be written as …" placeholder bodies.
  if curl -fsSL --max-time 30 "$1" -o "$2.tmp"; then
    if grep -q '^[A-Z_0-9]*\[\|^{' "$2.tmp"; then
      mv "$2.tmp" "$2"
    else
      rm -f "$2.tmp"
      echo "skip (not WKT/JSON): $1" >&2
    fi
  else
    rm -f "$2.tmp"
    echo "skip: $1" >&2
  fi
}

for ac in $CODES; do
  auth=${ac%%:*}
  code=${ac#*:}
  dir="$DATA_DIR/$auth-$code"
  mkdir -p "$dir"
  fetch "$BASE/$auth/$code/esriwkt.txt" "$dir/esri.prj"
  fetch "$BASE/$auth/$code/ogcwkt/" "$dir/wkt1.txt"
  fetch "$BASE/$auth/$code/prettywkt2.txt" "$dir/wkt2_2019.txt"
  fetch "$BASE/$auth/$code/projjson.json" "$dir/projjson.json"
done
