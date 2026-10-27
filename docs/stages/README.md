# Stages — v1 Delivery Plan

Active stage: stage01 — M0 CLI indexer prototype (wave 2 merged except 2.7 (in review); G2 go 2026-10-27; wave 3 next)

How stages, waves and sessions work: [`../08-development-workflow.md`](../08-development-workflow.md).
Only the active stage is planned in detail; the next one is sketched here and detailed when the active stage closes.

| Stage | Milestone ([roadmap](../05-roadmap.md)) | Objective | Plan | Status |
|---|---|---|---|---|
| [stage01](stage01/PLAN.md) | M0 | CLI indexer prototype: NTFS enumeration + USN, name index, dataset grouping, SHP/GPKG/GeoTIFF readers, CRS → WGS84, SQLite catalog, GDAL oracle, benchmarks | Detailed | 🟡 active — waves 1–2 done (2.7 in review), G1 and G2 go; wave 3 next |
| stage02 | M1 | Elevated indexer service + named-pipe IPC; remaining Tier 1 readers; footprints + two-stage spatial query; extraction queue polish; hard links | Sketch below | ⏳ |
| stage03 | M2 | Desktop UI: Tauri + MapLibre, search syntax (Q-11), results list, map footprints, offline basemap + gazetteer, hand-off actions | Roadmap only | ⏳ |
| stage04 | M3 | Previews via the local tile server: vector MVT, raster warping, point clouds | Roadmap only | ⏳ |
| stage05 | M4 | FGDB reader, performance passes, MSI + service registration, auto-update/crash reporting, release | Roadmap only | ⏳ |

## stage02 sketch (to be detailed at stage01 close)

Likely shape: wave 1 freezes the IPC protocol (`whereabouts-ipc` message types) and footprint representation; wave 2 runs the service host, the seven remaining Tier 1 readers (FlatGeobuf, GeoParquet, LAS/LAZ, world-file rasters, GeoJSON, KML/KMZ, DXF — readily parallel, one session each or paired) and footprint generation in parallel; wave 3 integrates the two-stage query and the CLI-over-IPC client. Inputs from the stage01 retro and Q-06/Q-10/Q-15 decide the rest.
