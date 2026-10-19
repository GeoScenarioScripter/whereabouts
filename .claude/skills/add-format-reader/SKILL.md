---
name: add-format-reader
description: How to implement a native Whereabouts metadata reader for a GIS format (Shapefile, GeoPackage, GeoTIFF, FlatGeobuf, GeoParquet, LAS/LAZ, GeoJSON, KML, DXF, FGDB, …) — bounded reads, FormatReader contract, CRS hand-off, corpus cases, GDAL oracle wiring. Use whenever writing or changing anything in crates/readers.
---

# Add or change a format reader

Readers answer a handful of questions about a dataset — extent, CRS, geometry type, counts, raster facts — by reading **only the bytes that hold them**. They are header readers, not a GIS library (D-07). Format notes: `docs/04-formats-and-crs.md` §2.

## Contract (`whereabouts_core::reader::FormatReader`)
- Input: a resolved dataset (format, anchor path, member paths by role) from the grouper. Never re-discover sidecars yourself; if a sidecar role is missing, fix the grouper (`crates/datasets`) — in a separate session if it's out of your scope.
- Output: `DatasetMetadata` with one `LayerMetadata` per layer. The CRS is returned **raw** (`CrsDefinition` + kind); parsing, EPSG identification and the WGS84 transform happen in `crates/crs` via `indexer::extract`. Readers don't transform coordinates.
- **Bounded reads:** fixed-size headers, explicit limits on metadata blocks; streaming formats (GeoJSON/KML) get a byte/time budget and return `Partial` when exceeded. Add a counting-reader test that asserts the budget.
- **Untrusted input:** every length/offset/count read from the file is validated before use; no `unwrap`/indexing panics on file data; corrupt → `Error(reason)`, truncated/partial → `Partial(reason)`. Add at least one corrupt/truncated corpus case.
- **Read-only, side-effect free:** never write, lock, or create sidecars (SQLite: read-only, no `-wal`/`-shm` creation); don't change atime/mtime expectations. Our own reads must not cause USN change events.
- No GDAL (a hook and cargo-deny block it). Prefer hand-written header parsing; a crate dependency must be pure Rust, permissively licensed, and justified in the session note.

## Steps
1. **Corpus first:** add cases under `tests/corpus/data/<format>/` via `tests/corpus/scripts/generate.sh` (or a provenance-recorded real sample) and regenerate goldens (`scripts/golden.py` locally, or the `oracle-golden` workflow on your branch: `gh workflow run oracle-golden.yml --ref <branch>` then `gh run download`). Cover: common CRS families, no CRS, empty dataset, multi-layer (if applicable), the format's known quirks (see `04` §2), one corrupt file.
2. **Reader module** `crates/readers/src/<format>.rs`; register it with **one line** in the registry in `crates/readers/src/lib.rs`.
3. **Unit tests** next to the module (byte-level fixtures for header parsing).
4. **Oracle adapter:** one registry line in `tests/oracle` mapping your output to the oracle record; run `cargo test -p whereabouts-oracle` and record per-format agreement. Raise `tests/oracle/baseline.toml` for your format to the measured value (never lower someone else's).
5. **Grouper check:** confirm the grouper produces the members you expect for every corpus case (`whereabouts inspect <path>` once 4.1 lands).
6. Session note: agreement numbers, bytes read per dataset, known disagreements with GDAL and why (some are GDAL quirks we choose not to copy — say so explicitly).
