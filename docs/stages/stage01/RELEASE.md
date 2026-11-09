# Whereabouts v0.1.0 — M0 CLI indexer prototype

Pre-alpha prerelease for Windows x64. Stage01 is complete with the owner-accepted
limitations in D-50; the desktop UI and resident service are future work.

## Delivered

- Local NTFS enumeration, USN journal catch-up, durable restart and rescan recovery.
- Filename substring/wildcard indexing and GIS dataset/sidecar grouping.
- Bounded native Shapefile, GeoPackage and GeoTIFF metadata readers, pure-Rust
  CRS identification and WGS84 bounds, SQLite catalog and R*Tree queries.
- `index`, `inspect` and `query`, including bbox/format/EPSG filters, JSON output
  and opt-in query phase timings.
- Non-finite raster metadata is rejected before persistence; invalid extraction
  remains a dataset-local error without aborting the indexing batch (D-51/#50).

## Quick start

Extract the Windows x64 archive into a directory, then run PowerShell there.
Indexing requires an elevated shell and a local NTFS drive. Store the catalog
in a writable directory; keep its snapshot and retry-state files together.

```powershell
.\whereabouts.exe --version
.\whereabouts.exe index --volume D: --db .\whereabouts.db
.\whereabouts.exe query roads --db .\whereabouts.db
.\whereabouts.exe query "" --bbox 116,39,117,40 --format shp,gpkg,gtiff --json --db .\whereabouts.db
.\whereabouts.exe inspect 'D:\GIS\roads.shp' --json
# Keep following the journal until Ctrl+C:
.\whereabouts.exe index --volume D: --db .\whereabouts.db --watch
```

Query and inspect do not require an elevated volume handle. Use `--help` on each
command for details. Building from source requires the pinned Rust toolchain
and MSVC build tools on Windows: `cargo build --release --locked`.
Linux builds/tests exercise the portable core; NTFS indexing requires Windows.

## Verification and release audit

- Final code integration `8c74046101d2fc32014731b677bd2575116ff2da` passed
  [develop CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42588045021):
  Windows/Linux clippy and tests, elevated NTFS/VHD acceptance, rustfmt and
  cargo-deny. The release documentation and develop-to-main PR also require
  green CI before merge.
- Raster validation [PR #74](https://github.com/GeoScenarioScripter/whereabouts/pull/74)
  is merged; issue #50 is closed. Stage01 has no open milestone issues at audit.
  Unrelated dependency update PRs remain outside the release.
- Strict oracle agreement is 100% across 62 available committed datasets
  (63 manifest entries; one optional external case explicitly skipped).
- Owner indexing/restart exercised 2,865,960 names and 5,209 datasets, with no
  restart rescan or extraction. Private owner data was not independently
  GDAL-validated and exact inventory completeness is not established.
- [Exit evidence](sessions/5.1-exit-verification.md),
  [raster regression evidence](sessions/issue-50-raster-validation.md),
  [retro](PLAN.md#stage-retro) and [stage02 handoff](STAGE02-HANDOFF.md).

## Known limitations carried to stage02

- One-shot CLI startup is not instant: warm diagnostics isolate roughly 500 ms
  snapshot loading/reconstruction and 5 ms search. Resident service/IPC performance
  and numeric budgets remain #72, supported by #48/#47.
- One selected name per file identity; complete hard-link path search remains #52.
- USN stays the default. Six boot-separated cold enumeration scans are deferred
  to #68; existing timings do not establish cold performance.
- Drive-letter local NTFS volumes only; mount/GUID support remains #25. Remaining
  Tier 1 readers, footprints, UI and previews are outside M0. Classified formats
  without a reader report `unsupported`.

No GDAL runtime, network/cloud indexing, installer or background Windows service
is included. Licensed MIT OR Apache-2.0; bundled notices accompany the binary.