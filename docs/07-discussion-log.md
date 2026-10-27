# 07 — Discussion Log (Story Threads)

How the project took shape, thread by thread, in the order it happened. Each thread lists the question, what was explored, and the conclusion.
First session: 2026-10-19.

---

## Thread 1 — "Do you know Everything?"
- **Question:** Familiarity with voidtools *Everything*.
- **Explored:** Everything is free Windows software that finds files/folders by name almost instantly. It reads the NTFS Master File Table directly to build its index in seconds, then follows the USN change journal for live updates. Name-first by default (content search is slow, on-demand). Rich syntax (wildcards, regex, boolean, `ext:`, `size:`, `dm:`, `dupe:`…), HTTP/ETP server, `es.exe` CLI, SDK. Windows-only.
- **Conclusion:** The model of "instant, machine-wide, name-first search" is the inspiration.

## Thread 2 — "Is it open source?"
- **Conclusion:** No — proprietary freeware. The SDK and the `es.exe` CLI source are public. Open-source analogues exist on Linux (FSearch, plocate/mlocate); none on Windows match its polish.

## Thread 3 — "Can we implement the fast indexing ourselves?"
- **Explored:** Yes — it's built on documented Windows APIs:
  1. Initial index via `FSCTL_ENUM_USN_DATA` (or raw `$MFT` parsing) → (FRN, parent FRN, name); store parent links, rebuild paths on demand.
  2. Live updates via `FSCTL_QUERY_USN_JOURNAL` / `FSCTL_READ_USN_JOURNAL`; persist last USN; rescan on journal wrap/recreate.
  3. Search is mostly a carefully engineered brute-force scan over a compact in-memory string pool.
  - Gotchas: admin rights (service), NTFS only, hard links, journal edge cases, memory layout.
  - Other OSes have no MFT shortcut (walk + fanotify/inotify; FSEvents/`getattrlistbulk`).
- **Conclusion:** Feasible; the core is small, the polish is the hard part.

## Thread 4 — The idea: a GIS-specific Everything
- **Proposal (owner):** A GIS-specific Everything, working name "Everywhere".
- **Assessment:** Real pain point (messy GIS file systems; "what data do I have for this area?"). The killer feature is **spatial search** plus GIS filters. Prior art: ArcMap/ArcGIS Desktop Search and search services (extent-filtered indexed search), Voyager (enterprise GIS data discovery). Gap: a fast, lightweight, vendor-neutral desktop tool.
- **Hard parts raised:** metadata requires opening files (slower than names); network shares have no USN; multi-file/folder datasets; project-file linkage; cloud.
- **Owner's response (Agreed):** The hard parts are the key features. Focus on **local files only**. **Dataset abstraction is a must**, and **indexing of CRS/bounds** is a must. **Network and cloud** are future features — maybe the first after local is done.

## Thread 5 — Naming
- **Owner's brief:** interesting and illustrative.
- **Candidates:** Whereabouts, Gazetteer, Dowser, Graticule (top picks); Everywhere, Wherever, Geodex; Intersects, Extent, Bbox; Cognita, Landfall, Sextant, Cairn.
- **Checks advised:** searchability, trademarks (class 9), domains, GitHub, winget/Store, crates/PyPI, CLI fit.
- **Conclusion (Agreed):** **Whereabouts** is the current pick. Availability checks still pending.

## Thread 6 — Tech stack
- **Recommendation (Proposed):** Rust core (`windows-rs`); elevated indexer service + unprivileged UI over named pipes; dataset grouper; metadata extraction; SQLite (WAL) + R*Tree; in-memory name index; Tauri + TypeScript + MapLibre GL; offline PMTiles basemap; GeoNames gazetteer; MSI installer; `Source` abstraction for future network/cloud/macOS/Linux.
- At this point GDAL/OGR + PROJ in sandboxed worker processes was the proposed metadata engine.
- **Alternative noted:** .NET 8 + WinUI/WPF + MaxRev.Gdal.Core + WebView2/MapLibre.
- **Suggested first milestone:** CLI-only indexer.

## Thread 7 — "Can we shake off GDAL/OGR?"
- **Key insight:** We need header readers, not a GIS library.
- **Explored:** Per-format effort table — Shapefile, GPKG, FlatGeobuf, GeoParquet, LAS/LAZ, world files, DXF easy; GeoTIFF moderate; GeoJSON/KML easy code but slow (full scan); FileGDB hard-ish (OpenFileGDB spec); ECW/MrSID/JP2/NetCDF/GRIB/etc. long tail (DWG needs ODA even in GDAL).
- **CRS is the real hard part:** parse ESRI WKT1/WKT2/PROJJSON/GeoKeys, identify EPSG, transform to WGS84. Low precision is acceptable for indexing → `proj4rs` + embedded EPSG table; or keep only PROJ via `proj` crate.
- **Trade-off:** Gains — speed, packaging, robustness, control. Costs — long tail, edge cases, later full-read features.
- **Conclusion (Proposed):** Native readers in the core; GDAL only as an optional out-of-process **Extended Format Pack** and as a **CI test oracle**. This supersedes the Thread 6 plan of GDAL in the core.

## Thread 8 — Visualizing without GDAL; do we need PostgreSQL?
- **Visualization:** Possible without GDAL. Vectors are easy-to-moderate with Rust crates (`shapefile`, `geozero`, `flatgeobuf`, `geoarrow`) + reprojection. Rasters are the hard part (codecs, warping to Web Mercator, overviews, nodata/stretch/palettes). Point clouds via `las`/`laz` + deck.gl. FGDB geometry is hard.
  - **Design rule:** never ship raw data to the webview — run a **local tile server** behind a Tauri custom protocol (MVT for vectors, warped image tiles for rasters) with a tile cache.
  - **Fidelity bar:** faithful previews, not GIS cartography; hand off to QGIS/ArcGIS Pro. Optional `gdal3.js` (WASM) for long-tail previews.
- **PostgreSQL:** Not needed for the desktop app — likely slower for this workload than in-process SQLite (IPC + planner overhead), heavy to operate on a desktop, and tiles come from files. Speed comes from the in-memory name index, R*Tree, **stored simplified footprints with a two-stage query**, and the tile cache.
  - Postgres/PostGIS belongs (later) as an **indexable source** and in a **team/enterprise catalog server**. DuckDB noted for analytics, but its spatial extension bundles GDAL.
- **Conclusion (Proposed):** No PostgreSQL in the desktop product; local tile server for previews.

## Thread 9 — Documentation
- **Request:** Record discussions, conclusions, story threads, v1 goals and provisioned features as organized docs for humans and the coding agent.
- **Result:** This `docs/` set: README (index + agent guidance), vision & scope, decision log, architecture, formats & CRS, roadmap, open questions, and this log.

---

## Thread 10 — Bootstrapping the repository (2026-10-19)
- **Request (owner):** initialise the repo and a public GitHub repo under GeoScenarioScripter with a proper licence; set up agent skills/hooks/plugins; write CLAUDE.md/AGENTS.md; plan development as stages → waves → parallel sessions by sub-agents in worktrees, with only the first stage planned in detail.
- **Owner decisions:** licence MIT OR Apache-2.0 (D-21); the whole proposed stack confirmed (D-06…D-12, D-18…D-20 → Agreed; frontend framework still open); `main` + `develop` branching per org standard (D-22); the process itself (D-23). The owner will install the Rust toolchain; CI verifies until then.
- **Owner guidance:** the development convention should follow what is best practice for *this* project; the org's earlier conductor convention is only a reference. Result: D-26 / `08-development-workflow.md` — contracts-first waves, write-scope ownership, status in GitHub issues, verification levels V1–V4, ≤ 4 concurrent sessions.
- **Name check (Q-01):** crates.io `whereabouts` is taken and a Kubernetes CNI plugin shares the name; `whereabouts-*` crate names are free.
- **Result:** repo scaffold, CI, Claude Code setup, `AGENTS.md`/`CLAUDE.md`, and the detailed [stage01 plan](stages/stage01/PLAN.md) (M0: 17 sessions in 5 waves, gates G1/G2). See [the bootstrap session note](stages/stage01/sessions/0-bootstrap.md).

---

## Thread 11 — stage01 wave 1: contracts, spikes, oracle (2026-10-19)
- **What ran:** four parallel sessions: 1.1 core-contracts (#21), 1.2 oracle-corpus (#22), 1.3 ntfs-enumeration (#20) and 1.4 crs-wkt-parsing (#23). Each PR got a code review and one round of fixes before merging. The reviews found real bugs:
  - 1.1: a checkpoint that could not round-trip, case-sensitive GPKG geometry names, and no way to express a query box crossing the antimeridian.
  - 1.3: enumeration could end early on header-only replies with a USN filter, and network/SUBST drives passed the NTFS check.
  - 1.2: a ±180° span blind spot, and resolution rounding vs the tolerance.
  - 1.4: unit-factor overflow, and a Helmert scale heuristic.
- **Conclusions:** decisions D-27…D-33 recorded as Proposed (identities, WTF-8 names, `Source` change contract, core conventions, NTFS enumeration specifics, CRS parsing conventions, oracle rules).
  - One review instruction was overruled with evidence: 1.4 showed that ISO 19162/PROJ write `ABRIDGEDTRANSFORMATION` scale as `1 + dS`, so a unitless value there is not ppm (D-32).
- **Process lessons:**
  - `Closes #n` doesn't fire on merges into `develop` (it isn't the default branch), so the orchestrator closes session issues at wave close.
  - The scratchpad is shared between sessions, so PR-body drafts need session-specific names; two sessions collided on `pr-body.md`.
  - A forked `/code-review` checked the PR out in the main checkout, so only one PR review runs at a time.
  - `AGENTS.md` now allows printing in tests/examples/benches and read-only enumeration of the system volume in elevated tests.
- **Open:** Q-16 waits on the owner's V4 `enum_volume` run. Gate G1 is next. Follow-ups #24–#29.
- **Gate G1 (2026-10-20):**
  - **Owner's V4 run (NVMe, release build):** C: 812k records in 4.9 s. D: 1.44M records in 41.0 s, of which 40.8 s was inside `FSCTL_ENUM_USN_DATA` and 0.19 s in our code. That's about 35 MB/s of MFT, consistent with small synchronous reads.
  - **Owner decision:** go for wave 2; D-27…D-33 → Agreed; add session 2.7 (raw `$MFT` reader, #31) behind the same raw-record API, after 2.2. Q-16 is decided in 4.3 on cold/warm numbers for both methods.

---

## Thread 12 — stage01 wave 2 close and gate G2 (2026-10-27)
- **What ran:** six sessions, all merged on 2026-10-27: 2.1 nameindex (#35, V1), 2.2 ntfs-journal-source (#34, V3), 2.6 reader-shapefile (#33, V2), 2.3 dataset-grouper (#38, V1), 2.5 catalog (#36, V1) and 2.4 crs-epsg-transform (#37, V2). 2.7 (raw `$MFT`, #31, PR #39) is still in review. Each PR got a review and fixes before merging; the reviews found, among others, swapped LCC parallels and a geographic box at ±90° (2.4), an unpaired directory rename (2.3), a path-mode depth disagreement with `path()` (2.1), a savepoint cost that doubled bulk insert time (2.5), and an unbounded read on untrusted offsets (2.6).
- **Merge order:** 2.1 → 2.2 → 2.6 → 2.3 → 2.5 → 2.4, as each became ready, against the planned 2.5 → 2.3 → 2.1 → 2.4 → 2.2 → 2.6. The only shared file was `Cargo.lock`, so the order made no difference (as in wave 1).
- **Gate G2 (owner, 2026-10-27): go.** Pure Rust, no PROJ fallback → D-34 (Q-14 resolved; targets 0.98 / 0.98 enforced from 4.3). Evidence: corpus 40/40 identification and 38/38 WGS84; 246 fixtures 99.2% / 99.1%; EPSG-wide sweep 99.0% WGS84 and 99.9% own-code identification. The 60 misses are rare methods (Polyconic, AEQD, …), added natively only if real data needs them (#45).
- **Other owner answers (2026-10-27):**
  - `rusqlite` `bundled` (SQLite compiled from C) is part of storage → D-35, amending D-10.
  - On the 2.5 design concern (non-ok statuses drop layers, so a transient failure would hide a dataset from spatial search): 3.3 keeps the stored record and retries through a queue, with no core change → D-36 and a 3.3 hand-off.
- **Orchestrator review decision:** shapefile feature count takes the `.shx`, then the `.dbf`, and record presence is decided by the `.shp` alone, matching GDAL → D-42 (Proposed).
- **Decisions proposed by the sessions:** D-37 (name index, incl. #26: no `RecordSink` change now), D-38 (journal source), D-39 (grouper), D-40 (CRS layer), D-41 (catalog), D-42 (shapefile reader), all Proposed.
- **Docs:** schema delta table folded into `03` §4, USN behaviours into §3.2, crate rows updated. `proj4rs`'s workspace entry was already fixed in 2.4's review (`default-features = false`).
- **Follow-up issues:** #40 oracle error/partial reasons, #41 oracle `cargo doc` clash, #42 CP437/CP850 tables, #43 shapefile corpus cases, #44 CRS-family corpus cases, #45 unsupported CRS methods, #46 partial-extent interior holes, #47 catalog re-upsert/`PRAGMA optimize`, #48 name index path memo and snapshot v2, #49 EPSG sweep in CI, #50 core `LayerRecord::validate` and non-finite raster floats. Items already owned by a planned session (3.1–3.3, 4.1–4.3) went into the wave 2 hand-offs in the stage plan. A new `stage02` milestone holds the items beyond M0. #25 stays open.
- **Process lessons:**
  - API rate limits stopped all running agents several times; they were resumed with their context and no work was lost.
  - Sub-agents now run on Sonnet 5.5, at the owner's request (2026-10-27).
  - A PR whose CI never starts usually conflicts with `develop`; rebase before waiting on checks.
  - The merge order was relaxed for PRs that share only `Cargo.lock`: merge as they become ready, rebase the rest.
- **Open:** wave 3 opens now that G2 is a go; 2.7 must merge first or run alongside (it only touches `crates/ntfs`). Q-16 still waits on the owner's D: runs and 2.7. Q-18 numbers are accumulating in `06`.

---

## Suggested next steps (from the discussion — superseded by the stage01 plan)
- Scaffold the Rust workspace and M0 CLI indexer (ntfs, nameindex, datasets, readers for SHP/GPKG/GeoTIFF, crs, catalog, GDAL-diff harness).
- Sketch the local tile server (Tauri custom protocol, MVT encoder, tile cache schema).
- Run name availability checks for Whereabouts.
