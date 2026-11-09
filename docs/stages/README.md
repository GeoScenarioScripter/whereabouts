# Stages — v1 Delivery Plan

Stage01 (M0) is complete for the v0.1.0 prerelease with owner-accepted gaps (D-50). Next: stage02 (M1), awaiting detailed planning; its scope remains a sketch.

How stages, waves and sessions work: [`../08-development-workflow.md`](../08-development-workflow.md).
Only the active stage is planned in detail; the next one is sketched here and detailed when the active stage closes.

| Stage | Milestone ([roadmap](../05-roadmap.md)) | Objective | Plan | Status |
|---|---|---|---|---|
| [stage01](stage01/PLAN.md) | M0 | CLI indexer prototype: NTFS enumeration + USN, name index, dataset grouping, SHP/GPKG/GeoTIFF readers, CRS → WGS84, SQLite catalog, GDAL oracle, benchmarks | Detailed | ✅ complete — v0.1.0 prerelease with accepted gaps (D-50) |
| stage02 | M1 | Elevated indexer service + named-pipe IPC; remaining Tier 1 readers; footprints + two-stage spatial query; extraction queue polish; hard links | Sketch below | ⏳ |
| stage03 | M2 | Desktop UI: Tauri + MapLibre, search syntax (Q-11), results list, map footprints, offline basemap + gazetteer, hand-off actions | Roadmap only | ⏳ |
| stage04 | M3 | Previews via the local tile server: vector MVT, raster warping, point clouds | Roadmap only | ⏳ |
| stage05 | M4 | FGDB reader, performance passes, MSI + service registration, auto-update/crash reporting, release | Roadmap only | ⏳ |

## stage02 sketch (to be detailed at stage01 close)

Likely shape: wave 1 freezes the IPC protocol (`whereabouts-ipc` message types) and footprint representation; wave 2 runs the service host, the seven remaining Tier 1 readers (FlatGeobuf, GeoParquet, LAS/LAZ, world-file rasters, GeoJSON, KML/KMZ, DXF — readily parallel, one session each or paired) and footprint generation in parallel; wave 3 integrates the two-stage query and the CLI-over-IPC client. Inputs from the stage01 retro and Q-06/Q-10/Q-15 decide the rest.

The owner-approved [M0 → M1 handoff](stage01/STAGE02-HANDOFF.md) carries cold enumeration (#68), resident performance (#72 with #48/#47) and hard-link completeness (#52), with explicit acceptance criteria. The stage02 plan must assign sessions and resolve contract choices; numeric budgets must be approved before performance acceptance; this sketch does not assert stage02 has started.
