# 06 — Open Questions

Undecided items. Coding agents: **do not silently pick an answer** for anything here — choose the default noted (if any) only when blocked, and flag it.
When resolved, record the answer in `02-decisions.md` and remove or strike it here.

## Product & business

| # | Question | Notes / default |
|---|---|---|
| Q-01 | Is the name **Whereabouts** available? | 2026-10-19 findings: GitHub repo `GeoScenarioScripter/whereabouts` created; crates.io `whereabouts` is **taken** (an offline place-names/IP-lookup crate) — our crates use `whereabouts-*` and are unpublished; a Kubernetes CNI IPAM plugin of the same name exists (`k8snetworkplumbingwg/whereabouts`, searchability). Still to check: trademarks (software class 9: USPTO, EUIPO), domains (`.app`/`.io`), GitHub org, winget/Microsoft Store, crates.io. Fallbacks discussed: Gazetteer, Dowser, Graticule, Geodex, Cognita |
| Q-02 | Business model | Licensing part resolved → D-21 (open source, MIT OR Apache-2.0). Business model still open: free core, network/cloud/team features as a paid tier was the idea |
| Q-03 | Validate the competitive landscape | Known prior art: ArcGIS Desktop Search / search services, Voyager. Check what currently exists before launch |
| ~~Q-04~~ | ~~Confirm the Proposed tech stack~~ | **Resolved 2026-10-19** → D-06…D-12, D-18…D-20 Agreed. Frontend framework stays open (Q-12) |

## Scope

| # | Question | Notes / default |
|---|---|---|
| Q-05 | Windows-only for v1? | Proposed yes (D-19) |
| Q-06 | Include non-NTFS local drives (FAT/exFAT/ReFS, USB) in v1? | Would need a walker + `ReadDirectoryChangesW` fallback. Since stage01/1.3: `Volume::open` rejects non-NTFS and non-local drives (D-31). ReFS's 128-bit file IDs don't fit `Frn(u64)` (#24) |
| Q-07 | FileGDB in v1.0 or v1.x? | Currently M4 (end of v1); previews later |
| Q-08 | Ship the GDAL Extended Format Pack in v1 or later? | Currently post-v1 |
| Q-09 | Point-cloud previews in v1? | May slip to v1.x |
| Q-10 | DXF and other CRS-less data: allow users to assign a CRS? | Default: name/attribute searchable, excluded from spatial index |

## Technical

| # | Question | Notes / default |
|---|---|---|
| Q-11 | Search syntax design | Default: Everything-compatible operators + GIS filters (`crs:`, `geom:`, `res:`, `format:`, `in:"place"`) |
| Q-12 | Frontend framework | Solid / Svelte (light) vs React (ecosystem) |
| Q-13 | Where does the tile server run? | Default: Tauri protocol handler forwards to the service over IPC |
| Q-14 | Is pure-Rust CRS (`proj4rs` + EPSG table) coverage sufficient? | Validate on corpus; fallback = PROJ via `proj` crate. Answered in stage01 (session 2.4 → gate G2, final in 4.3). The oracle's EPSG/WGS84 minimum rates are 0.0 until this is answered (D-33) |
| Q-15 | Large GeoJSON/KML extent policy | Size threshold + timeout; mark extent "pending/approximate" |
| Q-16 | Raw `$MFT` parsing vs `FSCTL_ENUM_USN_DATA` | Start with the USN API; raw MFT as later optimization. Measured in stage01 (session 1.3 → gate G1, final in 4.3). **1.3 CI numbers (debug build, cloud OS disk, 1.34M records):** cold 36–61 s, about 90% of it kernel/MFT I/O; warm 2.8–3.4 s (≈ 400k rec/s); enumeration memory = its 1 MiB buffer. Raw `$MFT` would read the same bytes. Its possible wins are larger sequential reads on a cold cache, and sizes/timestamps in the same pass. Proposed rule: keep USN if a cold run on the owner's largest volume stays under about 10 s. **Owner V4 (2026-10-20, NVMe, release build):** C: 812,684 records in 4.86 s; D: 1,442,867 records in 40.98 s (35k rec/s; 40.79 s inside the FSCTL, 0.19 s in our code). D: misses the rule. **G1 outcome:** add session 2.7 (raw `$MFT` reader behind the same raw-record API); 4.3 decides on cold/warm numbers for both methods. Still to measure: whether D:'s 41 s is cold-cache I/O or a fixed kernel cost (warm D: run). |
| Q-17 | Ranking model for results | Name relevance + spatial overlap + recency — weights TBD |
| Q-18 | Performance targets (index time, memory per million files, query latency) | Not yet set — proposed in stage01 session 4.3 from benchmarks against Everything |
| Q-19 | Auto-update and crash reporting approach | M4 |
| Q-20 | Which project-file formats to parse for linkage (`.mxd` is binary/legacy) | Post-v1 |
