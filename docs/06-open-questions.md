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
| ~~Q-14~~ | ~~Is pure-Rust CRS (`proj4rs` + EPSG table) coverage sufficient?~~ | **Resolved 2026-10-27 (gate G2, owner)** → D-34: pure Rust, no PROJ fallback. Targets 0.98 for EPSG identification and 0.98 for WGS84 agreement, enforced from 4.3 via `baseline.toml` (replacing the 0.0 of D-33). Evidence: corpus 40/40 and 38/38; fixtures 99.2% / 99.1%; EPSG-wide sweep 99.0% (D-34, [2.4 note](stages/stage01/sessions/2.4-crs-epsg-transform.md)). Missing methods (Polyconic, AEQD first) only if real data needs them (#45) |
| Q-15 | Large GeoJSON/KML extent policy | Size threshold + timeout; mark extent "pending/approximate" |
| Q-16 | Raw `$MFT` parsing vs `FSCTL_ENUM_USN_DATA` | Start with the USN API; raw MFT as later optimization. Measured in stage01 (session 1.3 → gate G1, final in 4.3). **1.3 CI numbers (debug build, cloud OS disk, 1.34M records):** cold 36–61 s, about 90% of it kernel/MFT I/O; warm 2.8–3.4 s (≈ 400k rec/s); enumeration memory = its 1 MiB buffer. Raw `$MFT` would read the same bytes. Its possible wins are larger sequential reads on a cold cache, and sizes/timestamps in the same pass. Proposed rule: keep USN if a cold run on the owner's largest volume stays under about 10 s. **Owner V4 (2026-10-20, NVMe, release build):** C: 812,684 records in 4.86 s; D: 1,442,867 records in 40.98 s (35k rec/s; 40.79 s inside the FSCTL, 0.19 s in our code). D: misses the rule. **G1 outcome:** add session 2.7 (raw `$MFT` reader behind the same raw-record API); 4.3 decides on cold/warm numbers for both methods. Still to measure: whether D:'s 41 s is cold-cache I/O or a fixed kernel cost (warm D: run). **Wave 2 numbers (CI runner C:, 1.35M records, 1.34 GB of `$MFT`, throttled cloud disk, release; 2.7, D-43):** USN API cold (first access in the job) 36–62 s, warm 2.0–2.7 s; raw `$MFT` about 6 s cold and warm (5.4 s reading, 0.44 s of our parsing, flat at about 245 MB/s across 1/4/32 MiB chunks, so it is the disk), peak memory about 161–164 MiB either way. The record sets are identical (1,355,125 identical; 17 raw-only NTFS metadata records). Raw is not served from the file system's `$MFT` cache, which is why USN wins warm. On NVMe the raw read should approach parse speed (about 0.6 s per 1.35M records). 2.1 gives the consumer side: ingesting 1M records into the name index takes 0.27 s (0.18 s with a borrowed view, #26). 2.7 proposes **raw for the first (cold) index and USN for changes**, with the USN API as fallback on any `Error::Mft`/`MftIncomplete`, pending the owner's D: run (cold raw and cold USN in separate boots, instructions in #31). **Q-16 stays Open; 4.3 decides.** |
| Q-17 | Ranking model for results | Name relevance + spatial overlap + recency — weights TBD |
| Q-18 | Performance targets (index time, memory per million files, query latency) | Not yet set — proposed in stage01 session 4.3 from benchmarks against Everything. **Inputs from 2.1 (synthetic volume, release, 8 threads):** 90 B/entry compacted (92 B at 5M), snapshot 46 B/entry, snapshot load 1.1 s at 5M; name-mode substring p50 2–5 ms at 5M for terms of 2+ characters, 25 ms for one character; path mode 76–190 ms at 5M. **From 2.5 (100k datasets):** bbox query p50 62 µs, member lookup 12 µs, first insert 7.9 s. Options if targets demand them: #48, #47 |
| Q-19 | Auto-update and crash reporting approach | M4 |
| Q-20 | Which project-file formats to parse for linkage (`.mxd` is binary/legacy) | Post-v1 |
