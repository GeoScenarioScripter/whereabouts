#!/usr/bin/env bash
# Runs one of the whereabouts-crs generator scripts inside the pinned GDAL image.
#
#   crates/crs/scripts/run-in-gdal.sh gen_epsg.py           # regenerate crates/crs/data/epsg.txt
#   crates/crs/scripts/run-in-gdal.sh g2_fixture_oracle.py  # regenerate crates/crs/tests/g2/fixture-oracle.json
#
# Needs Docker (Linux containers). GDAL/PROJ are never installed on the host and never linked into
# any crate (D-07/D-08): they only run here, in the same image the oracle corpus is built with
# (tests/corpus/scripts/generate.sh), pinned by digest on one platform so the output is
# reproducible. Bumping the image = change GDAL_IMAGE here and there, re-run both, review the diff.
set -euo pipefail

GDAL_IMAGE="ghcr.io/osgeo/gdal:ubuntu-small-3.13.3@sha256:64250faf833c06d4b21afce4c27190039ba7ab58d70f0eebc87cf77d929c0b40"
GDAL_PLATFORM="linux/amd64"

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <script.py> [args...]" >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"
script="$1"
shift
case "$script" in
  gen_epsg.py | g2_fixture_oracle.py) ;;
  *) echo "unknown script: $script" >&2; exit 2 ;;
esac

mount_src="$repo_root"
# Git Bash on Windows: hand Docker a Windows path and stop MSYS from rewriting /work.
if pwd -W >/dev/null 2>&1; then
  mount_src="$(cd "$repo_root" && pwd -W)"
  export MSYS_NO_PATHCONV=1
fi
user_args=()
if [[ "$(uname -s)" == "Linux" ]]; then
  user_args=(--user "$(id -u):$(id -g)")
fi
exec docker run --rm --platform "$GDAL_PLATFORM" \
  ${user_args[@]+"${user_args[@]}"} \
  -e HOME=/tmp -e PYTHONDONTWRITEBYTECODE=1 \
  -v "$mount_src:/work" -w /work \
  "$GDAL_IMAGE" python3 -I "crates/crs/scripts/$script" "$@"
