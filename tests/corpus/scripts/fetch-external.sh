#!/usr/bin/env bash
# Fetch the large real-world samples listed in external-files.txt into tests/corpus/external/
# (gitignored) and verify each file's sha256. Files already present with the right hash are kept.
# Usage: tests/corpus/scripts/fetch-external.sh
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest_root="$script_dir/../external"
list="$script_dir/external-files.txt"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

failed=0
while read -r sha rel url; do
  [[ -z "${sha:-}" || "$sha" == \#* ]] && continue
  if [[ "$rel" == /* || "$rel" == *..* ]]; then
    echo "refusing unsafe path: $rel" >&2; failed=1; continue
  fi
  out="$dest_root/$rel"
  if [[ -f "$out" && "$(sha256_of "$out")" == "$sha" ]]; then
    echo "ok (cached)  $rel"; continue
  fi
  mkdir -p "$(dirname "$out")"
  tmp="$out.part"
  curl -sSfL --retry 3 -o "$tmp" "$url"
  got="$(sha256_of "$tmp")"
  if [[ "$got" != "$sha" ]]; then
    echo "SHA256 MISMATCH $rel: expected $sha, got $got" >&2
    rm -f "$tmp"; failed=1; continue
  fi
  mv "$tmp" "$out"
  echo "fetched      $rel"
done < "$list"

exit "$failed"
