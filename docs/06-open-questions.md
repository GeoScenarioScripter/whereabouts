# 06 — Open Questions

Undecided items. Coding agents: **do not silently pick an answer** for anything here — choose the default noted (if any) only when blocked, and flag it.
When resolved, record the answer in `02-decisions.md` and remove or strike it here.

## Current M0 closure disposition (2026-11-09, D-50)

The owner retained USN, deferred six cold-boot scans, and accepted CLI startup and hard-link completeness as M0 gaps carried to stage02. Q-16's current default and Q-18's M0 acceptance disposition are resolved; future cold-method selection and numeric M1 performance budgets remain Open. [Handoff and acceptance criteria](stages/stage01/STAGE02-HANDOFF.md). The wave-close entries below are historical and superseded on these points by D-50.

## Wave 3 close (2026-11-08)

The reader and pipeline implementation conventions are recorded as Proposed in D-44–D-46. No Open product question is silently resolved by these changes. Q-14 remains resolved by D-34; wave 4.3 is tasked with enforcing its EPSG/WGS84 targets through the shared extraction path. Q-16 (default enumeration method) and Q-18 (performance targets) remain Open until the wave 4.3 evidence and owner decision. Wave 3 adds GeoPackage/GeoTIFF bounded-read and full-pipeline validation, not owner-volume performance evidence.
## Wave 4 engineering close (2026-11-09)

D-47 records consistent query snapshots and temporary inspect identities. D-48 records proposed performance budgets without resolving Q-16/Q-18; D-49 corrects the MultiPatch declared-layer convention. Q-14 is enforced through the common extractor, with strict measured per-format baselines. The owner confirmed there are no existing V4 logs. Required owner-volume cold/warm enumeration and Everything comparisons are carried explicitly to 5.1 / [#68](https://github.com/GeoScenarioScripter/whereabouts/issues/68); stage exit criteria remain unchanged and V4 is **not verified**. CI and synthetic component results are not substitutes.
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
| Q-16 | Raw `$MFT` parsing vs `FSCTL_ENUM_USN_DATA` | **M0 default resolved by D-50: retain USN.** Six owner-coordinated, boot-separated scans (three per method with warm repeats) are deferred to stage02 / #68. Raw-first remains a candidate, not the selected default. Existing uncontrolled measurements do not establish cold performance. Future optimization choice remains Open pending recorded cache conditions, record-set reconciliation and owner review. |
| Q-17 | Ranking model for results | Name relevance + spatial overlap + recency — weights TBD |
| Q-18 | Performance targets (index time, memory per million files, query latency) | **M0 disposition resolved by D-50: accept the measured CLI startup gap and carry service/IPC performance to stage02 / #72, supported by #48 and #47.** Numeric M1/end-to-end budgets remain Open; D-48 component budgets remain Proposed. Warm diagnostics isolate about 500 ms snapshot loading/reconstruction and 5 ms search. Measure service readiness, reused search, IPC-to-result latency and total-process memory separately; obtain explicit budget agreement in stage02. |
| Q-19 | Auto-update and crash reporting approach | M4 |
| Q-20 | Which project-file formats to parse for linkage (`.mxd` is binary/legacy) | Post-v1 |
