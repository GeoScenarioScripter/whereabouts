# 02 — Decision Log

ADR-style log. Each entry: **Status**, **Context**, **Decision**, **Rationale**, **Consequences**.
Status vocabulary: **Agreed** (owner-confirmed) · **Proposed** (recommended, not yet explicitly confirmed) · **Open** · **Deferred** · **Superseded**.

New decisions are appended with status **Proposed**. Do not renumber.

---

## Summary

| ID | Decision | Status |
|---|---|---|
| D-01 | Build a GIS-specific, Everything-style instant search tool | Agreed |
| D-02 | Product name: **Whereabouts** | Agreed (pending availability checks) |
| D-03 | v1 scope is local files only; network & cloud are future (likely first after v1) | Agreed |
| D-04 | Dataset abstraction layer is mandatory | Agreed |
| D-05 | Index geospatial metadata (CRS, bounds, …) | Agreed |
| D-06 | Rust core | Agreed (2026-10-19) |
| D-07 | No GDAL in the core; native header readers | Agreed (2026-10-19) |
| D-08 | GDAL only as optional out-of-process fallback plugin + test oracle | Agreed (2026-10-19) |
| D-09 | Pure-Rust CRS layer (proj4rs + embedded EPSG table); low-precision bbox transforms are acceptable | Agreed (2026-10-19) |
| D-10 | Storage: in-memory name index + SQLite (WAL) catalog + SQLite R*Tree spatial index | Agreed (2026-10-19) |
| D-11 | No PostgreSQL in the desktop product | Agreed (2026-10-19) |
| D-12 | UI: Tauri + TypeScript + MapLibre GL JS | Agreed (2026-10-19) |
| D-13 | Preview fidelity bar: faithful placement + sensible default styling, not GIS cartography | Proposed |
| D-14 | Previews via a local tile server (Tauri custom protocol), never raw data to the webview | Proposed |
| D-15 | Process split: elevated indexer service + unprivileged UI over named pipes | Proposed |
| D-16 | Spatial matching is two-stage: R*Tree bbox → exact test against stored simplified footprint | Proposed |
| D-17 | Offline-first: bundled PMTiles basemap + GeoNames-based gazetteer | Proposed |
| D-18 | `Source` abstraction from day one (NTFS source in v1; others later) | Agreed (2026-10-19) |
| D-19 | Windows first (NTFS MFT/USN) | Agreed (2026-10-19) |
| D-20 | First milestone is a CLI-only indexer (no UI) | Agreed (2026-10-19) |
| D-21 | License: MIT OR Apache-2.0 | Agreed (2026-10-19) |
| D-22 | Public GitHub repo `GeoScenarioScripter/whereabouts`; `main` + `develop` branching | Agreed (2026-10-19) |
| D-23 | Process: stages → waves → parallel sessions by sub-agents in worktrees; only the current stage planned in detail | Agreed (2026-10-19) |
| D-24 | Workspace layout, pinned toolchain, lints, CI matrix | Proposed |
| D-25 | GDAL golden files are committed; GDAL needed only to regenerate them | Proposed |
| D-26 | Workflow practices: contracts-first waves, write-scope ownership, GitHub-tracked status, verification levels V1–V4 | Proposed |

---

## D-01 — Build a GIS-specific instant search tool
- **Status:** Agreed
- **Context:** voidtools Everything gives near-instant filename search on Windows but has no notion of geography. GIS users' key question is "what data covers this area?"
- **Decision:** Build a desktop tool with Everything-like speed plus spatial search over GIS datasets.
- **Rationale:** Real pain point; existing solutions are either geography-blind (Everything) or heavyweight/enterprise/vendor-tied (ArcGIS search, Voyager).
- **Consequences:** Speed is a product requirement, not a nice-to-have.

## D-02 — Name: Whereabouts
- **Status:** Agreed (current pick; availability not yet checked)
- **Context:** Original working name "Everywhere" is a common word — hard to search for and to trademark. Owner wanted something interesting and illustrative.
- **Decision:** **Whereabouts** ("the whereabouts of your data").
- **Rationale:** Warm, plain English; double meaning (where the file is / where the data is on Earth).
- **Consequences:** Run trademark/domain/store/crate checks before public use (see open questions). Crate/binary prefix: `whereabouts` (CLI name TBD).

## D-03 — v1 is local files only
- **Status:** Agreed
- **Decision:** v1 indexes local drives only. Network shares and cloud sources are future features, likely the first features after local is done.
- **Consequences:** v1 can rely on NTFS MFT + USN journal. Architecture must not hard-wire "local NTFS" assumptions into the catalog/query layers (see D-18).

## D-04 — Dataset abstraction is mandatory
- **Status:** Agreed
- **Context:** One logical dataset can be many files (shapefile + sidecars), a folder (File Geodatabase, Esri Grid), or one file with many layers (GeoPackage).
- **Decision:** The unit of indexing and results is a **dataset** (with **layers**), sitting above files.
- **Consequences:** A dataset grouper component is required; file changes must map to dataset re-extraction. See `04-formats-and-crs.md` for grouping rules.

## D-05 — Index geospatial metadata
- **Status:** Agreed
- **Decision:** For each dataset/layer, index at least CRS and bounds; also geometry type, counts, raster dimensions/resolution/bands, and a simplified footprint where feasible.
- **Consequences:** Metadata extraction runs in the background after the instant filename index; it reads file headers, so it is slower per file than name indexing.

## D-06 — Rust core
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Rationale:** `windows-rs` gives first-class access to `DeviceIoControl`/USN/named pipes; C++-level performance for compact in-memory structures; memory safety in a long-running service; one language shared with the Tauri backend.
- **Alternative considered:** .NET 8 (WinUI 3/WPF, MaxRev.Gdal.Core, WebView2 + MapLibre) — faster iteration if the team is C#-oriented. Qt/C++ only if tight QGIS integration is wanted.

## D-07 — No GDAL in the core
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Context:** Whereabouts needs *header facts* (extent, CRS, geometry type, counts), not full read/write of 200+ formats. Bundling GDAL on Windows (vcpkg, DLL sets, `proj.db`) is the biggest packaging pain in the stack. GDAL's open path (driver probing, sibling listing, `.aux.xml` checks) adds per-file overhead.
- **Decision:** Write native Rust readers for the top formats.
- **Rationale:** Speed (bounded reads of a few hundred bytes), packaging (single static binary, small installer), robustness (no segfaults in the core), control over dataset semantics.
- **Costs accepted:** Long-tail formats not covered natively; must re-learn edge cases GDAL already handles; later full-data features need more work.

## D-08 — GDAL as optional fallback plugin and test oracle
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Decision:** (a) An optional "Extended Format Pack": GDAL loaded lazily in a separate worker process, used only for files no native reader claims; shipped as a separate download. (b) In CI, run `gdalinfo`/`ogrinfo` over a real-world corpus and diff extents/CRS against native readers.
- **Possible later addition:** `gdal3.js` (WASM) for on-demand front-end previews of long-tail formats (large download — optional).

## D-09 — Pure-Rust CRS layer
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Decision:** Parse ESRI WKT1, WKT2, PROJJSON and GeoTIFF keys; identify EPSG (name/parameter matching for ESRI `.prj` without authority codes); transform bounds to WGS84 using `proj4rs` + an embedded EPSG definitions table.
- **Rationale:** For a search index, bbox error of metres (ignoring datum grid shifts) is irrelevant. Projection math is enough; full PROJ machinery is not needed.
- **Fallback option:** Keep PROJ via the `proj` crate (C++ dependency, lighter than GDAL) if pure-Rust coverage proves insufficient.
- **Consequences:** Densify bbox edges before transforming; handle antimeridian-crossing extents explicitly; validate projection coverage against real data.

## D-10 — Storage
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Decision:**
  - Filename index: custom compact **in-memory** structure (string pool, `u32` indices, pre-folded names, extension offsets), persisted as a binary snapshot + last processed USN + journal ID.
  - Catalog: **SQLite** (WAL mode) via `rusqlite` — datasets, layers, CRS, raster info, files.
  - Spatial index: **SQLite R*Tree** on WGS84 bboxes; optional in-memory `rstar` tree if more speed is needed.
  - Tile cache: separate SQLite file (MBTiles-style), keyed by dataset + mtime.
- **Rationale:** Workload is millions of names + tens/hundreds of thousands of extents — in-process SQLite answers bbox queries in well under a millisecond.

## D-11 — No PostgreSQL in the desktop product
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Context:** Owner asked whether PostgreSQL would speed up indexing/search/tiles.
- **Decision:** Not for the desktop app.
- **Rationale:** In-process SQLite avoids IPC/planner overhead (Postgres would likely be *slower* for this pattern); a database server is heavy to install/operate on a desktop; tiles come from files, not from a DB.
- **Where Postgres belongs (Deferred):** (1) user PostGIS databases as an indexable *source*; (2) a future team/enterprise catalog server.
- **Note:** DuckDB is an embedded option for later catalog analytics, but its spatial extension bundles GDAL (conflicts with D-07).

## D-12 — UI stack
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Decision:** Tauri (WebView2 on Windows), TypeScript frontend, **MapLibre GL JS** for the map, virtualized result list. Frontend framework (Solid/Svelte/React) is **Open** (Q-12) — the owner's 2026-10-19 confirmation covers Tauri + TypeScript + MapLibre, not the framework.
- **Rationale:** Small binary vs Electron; Rust backend shared with the core; MapLibre is the best open renderer for footprints, box-draw queries, and vector tiles. deck.gl may be added for point clouds.

## D-13 — Preview fidelity bar
- **Status:** Proposed
- **Decision:** Previews must place data correctly and use sensible default styling, and appear quickly. Cartographic work is handed off ("Open in QGIS / ArcGIS Pro").

## D-14 — Local tile server for previews
- **Status:** Proposed
- **Decision:** MapLibre requests `whereabouts://tiles/{dataset}/{z}/{x}/{y}` via a Tauri custom protocol. The backend reads, clips, simplifies, reprojects to Web Mercator and encodes **MVT** on demand (vectors), or warps and encodes image tiles (rasters); results are cached.
- **Rationale:** Raw data (e.g., a 2 GB shapefile) must never be shipped to the webview.

## D-15 — Process split
- **Status:** Proposed
- **Decision:** An **elevated indexer service** (raw volume handles require admin) and an **unprivileged UI** communicating over **named pipes**. GDAL fallback (if installed) runs in separate, killable worker processes with per-file timeouts.

## D-16 — Two-stage spatial matching
- **Status:** Proposed
- **Decision:** Query = R*Tree bbox filter → exact intersection test (`geo` crate) against a stored simplified footprint (convex hull or simplified outline).
- **Rationale:** A bbox alone is misleading for diagonal or L-shaped data (rivers, survey corridors).

## D-17 — Offline-first
- **Status:** Proposed
- **Decision:** Bundle a low-zoom vector basemap as a single **PMTiles** file (Natural Earth / low-zoom OSM) and a trimmed **GeoNames** cities/admin dataset as an offline gazetteer for place-name search.

## D-18 — `Source` abstraction from day one
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Decision:** All file discovery goes through a `Source` trait (enumerate + watch changes). v1 ships `NtfsVolumeSource` (and possibly a generic walker for non-NTFS local drives — Open). Future: SMB share source, S3/`object_store` source, STAC source, PostGIS source, macOS/Linux sources.

## D-19 — Windows first
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Decision:** v1 targets Windows 10/11 and NTFS. Tauri and the `Source` abstraction keep macOS/Linux ports feasible later (filesystem walk + FSEvents / fanotify/inotify).

## D-20 — First milestone: CLI-only indexer
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Decision:** Before any UI, build a CLI that enumerates one volume, groups datasets, extracts extents/CRS into SQLite.
- **Rationale:** Proves the riskiest pieces (USN handling, dataset grouping, readers/CRS) before investing in the frontend.

## D-21 — License: MIT OR Apache-2.0
- **Status:** Agreed (2026-10-19, owner)
- **Context:** The repository is public from day one. Business model (Q-02) is still open; the idea floated is a free core with paid network/cloud/team features.
- **Decision:** Dual-license the code under **MIT OR Apache-2.0** (Rust-ecosystem convention): `LICENSE-MIT`, `LICENSE-APACHE`, contributions inbound under the same terms.
- **Rationale:** Maximally compatible with the Rust ecosystem and with embedding; the Apache half adds an explicit patent grant. An open-core paid tier remains possible as separately licensed components.
- **Consequences:** Dependencies must be permissively licensed — enforced by `cargo-deny` (`deny.toml`). Bundled data (EPSG dataset, GeoNames, Natural Earth) needs attribution in `NOTICE` and the about screen. Anyone may fork commercially (accepted).

## D-22 — Repo hosting and branching
- **Status:** Agreed (2026-10-19, owner)
- **Decision:** Public repository `GeoScenarioScripter/whereabouts`. Branches per the org's dev-standards: `main` (stage-complete states, tagged `v0.<stage>.0` pre-1.0) and `develop` (integration); work branches merge into `develop` by squash-merged PR; `develop → main` by merge commit at stage close. Branch protection: PRs + green CI required, no force pushes, no required human approvals yet (agents merge after review).
- **Consequences:** Direct pushes to `main`/`develop` are blocked (protection + `guard-git` hook).

## D-23 — Stage → wave → session process
- **Status:** Agreed (2026-10-19, owner)
- **Decision:** A **stage** (≈ one roadmap milestone) contains **waves**; a wave contains **sessions** that run in parallel, each done by a sub-agent in its own git worktree. Only the current stage is planned in detail; the next is planned when the previous closes.
- **Consequences:** See `08-development-workflow.md` and D-26 for the practices.

## D-24 — Workspace layout, toolchain, lints, CI
- **Status:** Proposed
- **Decision:** Crates live in `crates/<short-name>` with package names `whereabouts-<name>` (all `publish = false` until Q-01 is settled); every crate of the current stage is pre-created as a workspace member when the stage is planned. Toolchain pinned via `rust-toolchain.toml` (Rust 1.99, edition 2024). Workspace lints: `unsafe_code = "deny"` except `whereabouts-ntfs` (with `// SAFETY:` comments), clippy warnings are errors in CI. CI runs fmt, clippy and tests on **Windows and Linux** (non-NTFS crates must stay portable, D-18/D-19), elevated NTFS tests on the Windows runner, and `cargo-deny` (licences, advisories, GDAL ban). Adds a crate `whereabouts-indexer` (pipeline shared by the CLI and the future service), not in the original architecture list.
- **Rationale:** Pre-created members and disjoint crate ownership keep parallel sessions from colliding; Linux CI protects the future macOS/Linux ports cheaply.

## D-25 — Committed GDAL golden files
- **Status:** Proposed
- **Context:** D-08 uses GDAL as a correctness oracle in CI.
- **Decision:** `gdalinfo`/`ogrinfo`-derived expectations are generated by scripts in a pinned GDAL container and **committed** (`tests/corpus/expected/`). Normal CI compares native readers against these files and needs no GDAL; a manual workflow regenerates them.
- **Rationale:** Fast, deterministic CI on Windows and Linux; GDAL version changes become explicit, reviewable diffs.

## D-26 — Workflow practices
- **Status:** Proposed
- **Decision:** Wave 1 of each stage freezes the stage's contracts; later changes are flagged "contract changes". Each session has a written **write scope**; parallel sessions own disjoint paths. Session status lives in GitHub (milestone per stage, issue per session); `PLAN.md` holds the specs and is updated by the orchestrator at wave close together with the decision/open-question/architecture docs. Every session reports a verification level (V1 unit, V2 integration/oracle, V3 elevated CI, V4 owner's real machine); stage exit needs V4 where the stage touches real systems. At most 4 concurrent sessions.
- **Rationale:** Designed for a crate-structured Rust workspace and elevated Windows code; folds in lessons from the org's earlier stage/session work (bookkeeping conflicts, never-run live tests). Details in `08-development-workflow.md`.
