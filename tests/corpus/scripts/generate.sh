#!/usr/bin/env bash
# Regenerate the synthetic corpus and the GDAL oracle records inside the pinned GDAL image.
#
#   tests/corpus/scripts/generate.sh                 # rebuild synthetic data + all goldens
#   tests/corpus/scripts/generate.sh --goldens-only  # keep data, only re-run golden.py
#   tests/corpus/scripts/generate.sh <case-id>...    # rebuild only these synthetic cases (+ goldens)
#
# Needs Docker (Linux containers). GDAL is never installed on the host and never linked into any
# crate (D-07/D-08): it only runs here, in an image pinned by digest, on one platform so the
# floating-point output is reproducible. Bumping GDAL = change GDAL_IMAGE, re-run, review the diff.
set -euo pipefail

GDAL_IMAGE="ghcr.io/osgeo/gdal:ubuntu-small-3.13.3@sha256:64250faf833c06d4b21afce4c27190039ba7ab58d70f0eebc87cf77d929c0b40"
GDAL_PLATFORM="linux/amd64"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"

if [[ "${WHEREABOUTS_IN_GDAL_CONTAINER:-}" != "1" ]]; then
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
    -e WHEREABOUTS_IN_GDAL_CONTAINER=1 -e HOME=/tmp -e PYTHONDONTWRITEBYTECODE=1 \
    -v "$mount_src:/work" -w /work \
    "$GDAL_IMAGE" bash tests/corpus/scripts/generate.sh "$@"
fi

# ---- inside the container -------------------------------------------------------------------
cd /work
goldens_only=0
cases=()
for arg in "$@"; do
  case "$arg" in
    --goldens-only) goldens_only=1 ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) cases+=("$arg") ;;
  esac
done

if [[ "$goldens_only" == "0" ]]; then
  python3 -I tests/corpus/scripts/generate.py "${cases[@]}"
fi
python3 -I tests/corpus/scripts/golden.py "${cases[@]}"
