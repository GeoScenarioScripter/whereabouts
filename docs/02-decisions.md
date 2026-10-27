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
| D-27 | Identities: `DatasetId` = (catalog-assigned `VolumeId`, anchor FRN); `LayerId` = dataset + reader-order ordinal | Agreed (2026-10-20) |
| D-28 | File names are stored losslessly as WTF-8 | Agreed (2026-10-20) |
| D-29 | `Source` change contract: `Changes::{Batch, FullRescanRequired}`, checkpoint taken before enumeration, idempotent replay | Agreed (2026-10-20) |
| D-30 | Core data conventions: frozen enum string codes, `Timestamp` = i64 ns, `ReadLimits` 16 MiB / 2 s, validated records | Agreed (2026-10-20) |
| D-31 | NTFS enumeration: V2 records, root FRN from the root handle, local fixed/removable drives only | Agreed (2026-10-20) |
| D-32 | CRS parsing: crate-local model, EPSG/PROJ vocabulary, GDAL's WKT1 conventions, bounded input, lenient datum shifts | Agreed (2026-10-20) |
| D-33 | Oracle comparison rules: tolerances, baseline, true-extent convention, pinned GDAL image, drift check on PRs | Agreed (2026-10-20) |
| D-34 | Gate G2: pure-Rust CRS stands, no PROJ fallback; Q-14 targets 0.98 / 0.98 | Agreed (2026-10-27, owner) |
| D-35 | Bundled SQLite (`rusqlite` `bundled`, built from C with `cc`) is accepted as part of the D-10 storage choice | Agreed (2026-10-27, owner) |
| D-36 | A transient extraction failure keeps the stored record; 3.3 retries via a queue (no core contract change) | Agreed (2026-10-27, owner) |
| D-37 | Name index conventions: 1:1 case folding, volume-relative paths, orphans by name only, snapshot v1, upsert semantics | Proposed |
| D-38 | USN journal source behaviours: per-file per-batch coalescing, carried renames, rescan on bad content, soft batch cap, no journal creation | Proposed |
| D-39 | Dataset grouper semantics: order-independent folding per batch, `GroupDelta` classes, hard-link and unknown-`Modified` handling | Proposed |
| D-40 | CRS layer: EPSG table generated from `proj.db`, `proj4rs` without default features, native inverses, GIS axis order, Helmert-only shifts | Proposed |
| D-41 | Catalog conventions: R*Tree row ids, exact f64 post-filter, CRS latest-write-wins, same-layer filters, `set_status` drops layers, `synchronous = NORMAL` | Proposed |
| D-42 | Shapefile reader conventions: `.shx`-then-`.dbf` count, `.shp` decides record presence, corrupt `.dbf` is `partial`, `measure` meter, `encoding_rs`, limits | Proposed |
| D-43 | Raw `$MFT` enumeration: same raw-record API, name rule, strict mode (`MftIncomplete` → USN fallback), torn retry, 1 MiB replay margin snapped to a record boundary, read caps | Proposed |

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
- **Update (2026-10-27):** gate G2 confirmed this choice and the PROJ fallback is not adopted. See D-34.

## D-10 — Storage
- **Status:** Agreed (2026-10-19 — owner confirmed the whole proposed stack)
- **Decision:**
  - Filename index: custom compact **in-memory** structure (string pool, `u32` indices, pre-folded names, extension offsets), persisted as a binary snapshot + last processed USN + journal ID.
  - Catalog: **SQLite** (WAL mode) via `rusqlite` — datasets, layers, CRS, raster info, files.
  - Spatial index: **SQLite R*Tree** on WGS84 bboxes; optional in-memory `rstar` tree if more speed is needed.
  - Tile cache: separate SQLite file (MBTiles-style), keyed by dataset + mtime.
- **Rationale:** Workload is millions of names + tens/hundreds of thousands of extents — in-process SQLite answers bbox queries in well under a millisecond.
- **Amendment (2026-10-27):** SQLite is compiled from source through `rusqlite`'s `bundled` feature (a C build). See D-35.

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

## D-27 — Dataset, layer and volume identities
- **Status:** Agreed (2026-10-20, owner — at gate G1)
- **Context:** stage01/1.1 had to choose stable IDs before wave 2 builds the catalog, name index and grouper on them ([1.1 note](stages/stage01/sessions/1.1-core-contracts.md)).
- **Decision:** `DatasetId` = (`VolumeId`, anchor FRN). `VolumeId` is a `u32` assigned by the catalog per registered source, matched by `SourceIdentity.key` (NTFS: volume GUID path). `LayerId` = (`DatasetId`, ordinal), where ordinal is the index in the reader's layer list; readers must order layers deterministically, and layers are replaced wholesale on re-extraction. The catalog keeps integer surrogate keys plus `UNIQUE(volume_id, anchor_frn)`.
- **Rationale:** An anchor FRN survives renames, moves and sidecar churn, and a delete + recreate correctly gives a new dataset. A hash or packed `u64` risks collisions. NTFS serials collide on cloned disks and VHDs, and non-NTFS sources have none.
- **Consequences:** 2.5 adds `anchor_frn` and `ordinal` columns. Hard-linked anchors collapse to one ID, which is the documented M0 hard-link limitation.

## D-28 — Lossless WTF-8 file names
- **Status:** Agreed (2026-10-20, owner — at gate G1)
- **Context:** NTFS names are UTF-16 and may contain unpaired surrogates. Proposed independently by stage01/1.1 (`FileName`) and 1.3 (`NtfsName`).
- **Decision:** File names are stored as WTF-8: identical to UTF-8 for valid names, with unpaired surrogates kept. `as_str()` is zero-cost for valid names. Serde writes a string, or a UTF-16 array for invalid names.
- **Rationale:** Every file stays openable and no two names collide, unlike lossy `String`. It's half the size of UTF-16 for ASCII, works on Linux unlike `OsString::from_wide`, and matches `OsStr::as_encoded_bytes` on Windows.
- **Consequences:** the name-index string pool (2.1) stores WTF-8 bytes directly. Types that hold `PathBuf` must not offer serialisation that fails on such names (`ResolvedDataset` has no serde for this reason).

## D-29 — `Source` change contract
- **Status:** Agreed (2026-10-20, owner — at gate G1)
- **Context:** D-18 requires a `Source` trait. stage01/1.1 defined it ([1.1 note](stages/stage01/sessions/1.1-core-contracts.md)).
- **Decision:** `Source { identity(), root(), enumerate(sink) -> Checkpoint, changes(&Checkpoint) -> Changes }`, synchronous and poll-based. `Changes` is either `Batch { events, checkpoint }` or `FullRescanRequired(reason)`; a rescan is not a `SourceEvent`. `enumerate` takes its checkpoint **before** enumerating, so replay covers the gap, and consumers apply events idempotently. `Deleted`/`Modified` carry the full `FileRecord`. `Checkpoint` is opaque versioned bytes (`WBCP` magic, ≤ 64 KiB, validated on construction and on decode).
- **Rationale:** A rescan can then never be committed together with a checkpoint, and event consumers (name index, grouper) never see a non-event.
- **Consequences:** this deviates from the stage01 spec wording (`SourceEvent::FullRescanRequired`), which is accepted here. 2.2 implements it for NTFS.

## D-30 — Core data conventions
- **Status:** Agreed (2026-10-20, owner — at gate G1)
- **Context:** stage01/1.1 ([note](stages/stage01/sessions/1.1-core-contracts.md)) and its review.
- **Decision:**
  - Every core enum has one frozen string code, used both as the catalog TEXT value and as the serde form. Codes are matched exactly, except `GeometryKind::parse_lenient`, which readers use for names read from files (GPKG writes them upper-case).
  - `Timestamp` is i64 nanoseconds since the Unix epoch.
  - `ReadLimits` defaults to 16 MiB and 2 s per dataset (starting values; 3.3 and 4.3 tune them).
  - `DatasetRecord::validate()` / `LayerRecord::validate()` enforce the record invariants before persisting.
  - An empty status detail equals none.
- **Rationale:** One spelling everywhere avoids mapping tables. Geometry/raster/field type names follow OGC and GDAL spelling, which simplifies oracle comparison.
- **Consequences:** codes can't change once released. 2.5 calls `validate()`.

## D-31 — NTFS enumeration specifics
- **Status:** Agreed (2026-10-20, owner — at gate G1)
- **Context:** stage01/1.3 ([note](stages/stage01/sessions/1.3-ntfs-enumeration.md)).
- **Decision:**
  - Request `USN_RECORD_V2` only (`MFT_ENUM_DATA_V1`, min = max = 2). V3 is still parsed and narrowed to 64 bits, or rejected.
  - Take the root FRN from `GetFileInformationByHandle(X:\)`, because enumeration never returns the root.
  - `Volume::open` accepts only drive letters of `DRIVE_FIXED`/`DRIVE_REMOVABLE` drives backed by a `\Device\` object. Network redirectors, `SUBST` aliases, optical and RAM disks get `NotLocal` (D-03).
  - Never create a USN journal implicitly.
- **Rationale:** NTFS file references are 64-bit, and V2 records are 16 bytes smaller. Explicit local-only checks keep D-03 enforceable.
- **Consequences:** 2.1/2.2 seed the root entry. Mount points and GUID paths are #25; 128-bit file IDs (ReFS) are #24.

## D-32 — CRS parsing model and conventions
- **Status:** Agreed (2026-10-20, owner — at gate G1)
- **Context:** stage01/1.4 ([note](stages/stage01/sessions/1.4-crs-wkt-parsing.md)). Readers hand over raw text (`CrsDefinition`, D-30).
- **Decision:**
  - The parsed `Crs` model stays inside `whereabouts-crs`; 2.4 wires `CrsDefinition → Crs`.
  - Methods use EPSG names and codes; parameters use PROJ semantics (`lat_0`, `lon_0`, `k_0`, `x_0`, …), with method-dependent renames done at parse time.
  - WKT1 follows GDAL's conventions (prime meridian in degrees, angular parameters in the GEOGCS unit, GDAL's ESRI method refinements).
  - Input limits: 256 KiB, nesting depth 32, 16,384 nodes. Non-UTF-8 input is decoded as Windows-1252.
  - Every number in a successful parse is finite.
  - Helmert scale is decided by its unit. With no unit: ppm in PROJJSON, and `1 + dS` in a WKT2 `ABRIDGEDTRANSFORMATION`, as ISO 19162 and PROJ write it.
  - An unreadable `TOWGS84`/BOUNDCRS transformation drops the shift but keeps the CRS.
  - Derived, temporal and parametric CRSs parse as `Unknown`.
- **Rationale:** `core` stays free of CRS semantics. 2.4 can build `proj4rs` projections directly. Agreeing with GDAL's importer avoids spurious oracle mismatches.
- **Consequences:** 2.4 decides axis order for WKT1/ESRI and compares angles modulo 360. EPSG and Esri attribution goes into `NOTICE` (#27).

## D-33 — Oracle comparison rules
- **Status:** Agreed (2026-10-20, owner — at gate G1)
- **Context:** D-08/D-25 use GDAL as the correctness oracle. stage01/1.2 built the corpus and harness ([note](stages/stage01/sessions/1.2-oracle-corpus.md)).
- **Decision:**
  - **Tolerances:** native extent `|Δ| ≤ max(1e-6, 1e-9·|v|)`. WGS84: each edge within 0.01°, longitude compared modulo 360, **and** longitude span within 0.02°. Raster resolution relative 1e-9 (goldens unrounded). Counts and EPSG exact.
  - **Baseline:** status and layers 1.0; geometry type, feature count, native extent and raster 0.98 (exit criterion 4); EPSG and WGS84 0.0 until Q-14 sets targets.
  - Adapters declare the checks they are held to.
  - Corrupt cases, and cases GDAL refused, are compared on status only.
  - Readers are compared against the **true** data extent, not the declared one.
  - Layer name = file stem (SHP/GeoTIFF) or `table_name` (GPKG: every `gpkg_contents` row is a layer).
  - EPSG in goldens is identified from the raw definition (authority → `AutoIdentifyEPSG` → `FindMatches` ≥ 70).
  - GDAL is pinned to `ghcr.io/osgeo/gdal:ubuntu-small-3.13.3@sha256:64250faf…` (GDAL 3.13.3, PROJ 9.8.1). Bumping it is a reviewed diff.
  - `oracle-golden.yml` also runs on PRs touching corpus scripts, data, goldens or the manifest, and fails on drift.
- **Rationale:** 0.01° leaves room for proj4rs vs PROJ without grid shifts (D-09). The span check stops a collapsed box at ±180° matching the full globe. The true-extent convention measures what users need, not what files claim.
- **Consequences:** stale GPKG bounds need the R-tree fallback (3.1); PixelIsPoint needs a half-pixel shift (3.2). Changing corpus scripts needs Docker or the workflow artifact. 4.3 raises the EPSG/WGS84 rates once Q-14 is set.

## D-34 — Gate G2: pure-Rust CRS, no PROJ fallback
- **Status:** Agreed (2026-10-27, owner — at gate G2)
- **Context:** D-09 left a PROJ fallback open until the pure-Rust CRS layer was measured (Q-14). stage01/2.4 ([note](stages/stage01/sessions/2.4-crs-epsg-transform.md)) reports against GDAL 3.13.3 / PROJ 9.8.1 (D-33 tolerances):
  - corpus: identification 40/40, WGS84 agreement 38/38;
  - 246 CRS fixtures: identification 241/243 (99.2%), WGS84 426/430 (99.1%);
  - all non-deprecated EPSG geographic and projected CRSs (5,947): WGS84 of an inner box 99.0%, own code from the Esri `.prj` export 99.9%.

  Every implemented projection method agrees with PROJ on every CRS. The 60 failures are rare national methods with no implementation (Colombia Urban, AEQD, Polyconic, LCC Michigan and 1SP variant B, Krovak Modified, Laborde, …).
- **Decision:**
  - Gate G2 is **go**: the CRS layer stays pure Rust (`proj4rs`, the embedded EPSG table and native inverses). D-09 stands and the PROJ fallback is not adopted.
  - Missing projection methods (Polyconic and AEQD first) are added natively only if real data needs them (#45). Until then the dataset stays name-searchable and the CRS is identified by name (`UnsupportedMethod`).
  - Q-14 targets, enforced from 4.3 through `tests/oracle/baseline.toml`: **EPSG identification 0.98** and **WGS84 agreement 0.98** on the corpus.
- **Rationale:** The evidence covers every family in `04` §4 and the whole EPSG registry. PROJ would bring a C++ dependency and `proj.db` packaging (D-07's motive) for about 1% of CRSs that are rare. Native inverses are small, pure and checkable against PROJ.
- **Consequences:** 4.3 raises the baseline from 0.0 to 0.98 for both rates (D-33) and may raise it further as measured. The EPSG-wide sweep stays a local tool for now, because it needs Docker (#49).

## D-35 — Bundled SQLite is part of the storage choice
- **Status:** Agreed (2026-10-27, owner)
- **Context:** D-10 chose SQLite through `rusqlite`. The root `Cargo.toml` has used `features = ["bundled"]` since bootstrap, so `libsqlite3-sys` compiles the SQLite amalgamation (3.53.2, `SQLITE_ENABLE_RTREE`) with `cc`. That is a C build dependency, and AGENTS.md rules those out without a decision. stage01/2.5 asked ([note](stages/stage01/sessions/2.5-catalog.md)).
- **Decision:** The bundled SQLite build is accepted as part of D-10, as the one sanctioned C build dependency of the storage layer. It amends D-10 and does not replace it.
- **Rationale:** The SQLite version and R*Tree support are the same on every platform, and Windows has no system SQLite to link. A C toolchain (MSVC) is required anyway. A system SQLite would make R*Tree support and version depend on the machine.
- **Consequences:** Builds need a C compiler (already true for the MSVC toolchain). `cargo-deny` allows the new crates (`rusqlite`, `libsqlite3-sys`, `cc`; MIT or Apache-2.0). This decision covers SQLite only; `proj4rs`'s `aeqd` feature stays off (D-40).

## D-36 — Transient extraction failures keep the stored record
- **Status:** Agreed (2026-10-27, owner)
- **Context:** The core `DatasetRecord` invariant says only an `ok` or `partial` dataset carries layers. The catalog therefore drops layers on `set_status(Pending | Unsupported | Error)` ([2.5 note](stages/stage01/sessions/2.5-catalog.md)). A transient failure on a re-read (a file locked by the tool that is writing it) or a `Pending` re-queue would hide a previously good dataset from spatial search, and `error` datasets are never re-queued.
- **Decision:** The pipeline (3.3) keeps the **stored record** when a re-extraction fails transiently, and retries through a queue. The `DatasetRecord` invariant stays; there is no core contract change. Permanent failures (malformed content, unsupported format) are recorded as today.
- **Rationale:** Relaxing the invariant to keep last-good layers beside a `last_error` would be a core change that every consumer must understand. Holding the record is local to 3.3.
- **Consequences:** 3.3 needs a retry queue and a rule that separates transient from permanent errors (`ReadError::Io` against `Malformed`). It is in 3.3's spec as a wave-2 hand-off ([PLAN.md](stages/stage01/PLAN.md)). 4.2 should test it.

## D-37 — Name index conventions
- **Status:** Proposed
- **Context:** stage01/2.1 ([note](stages/stage01/sessions/2.1-nameindex.md)).
- **Decision:**
  - **Case folding** is a 1:1 code-point map, `lower(upper(c))` with single-character fallbacks. It is not the NTFS upcase table and not full Unicode folding (which turns `ß` into `ss` and would make `?` count characters that are not there). Queries are folded the same way.
  - **Paths** are volume-relative, with a leading `\` and `\` separators (`\data\roads.shp`, root `\`). Consumers add the drive. In path mode `*` also matches `\`.
  - **Orphans** (entries whose parent chain does not reach the root) are searchable by name and never by path; `path()` reports `Orphan`. The query layer (4.1) decides whether to hide them.
  - **Snapshot v1** stores entries by FRN and reloads them through the normal upsert path, instead of dumping columns. Loading validates everything and costs about 1.1 s at 5M entries.
  - **Every event is an upsert** (Created, Modified, Renamed). `EntryId`s are stable until `compact()`; FRNs are the durable identity. Hard links keep one name per FRN (last upsert wins), the M0 limitation.
  - **#26:** the `RecordSink` contract does not change now. A borrowed record view saves about 82 ns per record (31% of ingestion, roughly 0.1 s per 1.34M records). Revisit if 2.7 brings enumeration down to about 1 s per million records, as an additive provided method.
- **Rationale:** Full Unicode folding changes string lengths, which breaks `?` and the offset maps. A reload through upsert gives validation for free and survives layout changes, at a load-time cost that a column dump (v2, #48) can reduce if Q-18 demands it.
- **Consequences:** 4.1 hides or marks orphans and loads the snapshot at startup. 3.3 calls `compact()` when `needs_compaction()` and treats `EntryId`s as invalid afterwards. Numbers for Q-18 are in the note (5M entries: 92 B/entry compacted, 2–5 ms for terms of 2+ characters, 25 ms for one character).

## D-38 — USN journal source behaviours
- **Status:** Proposed
- **Context:** stage01/2.2 ([note](stages/stage01/sessions/2.2-ntfs-journal-source.md)); contract in D-29, enumeration specifics in D-31. Verified at V3 on VHD volumes.
- **Decision:**
  - **Coalescing is per file per batch**, not per close session. Precedence: delete (even for a file created in the same batch), create, rename (first old name to final name), modify (after rename), with security, object ID and indexing reasons ignored. A file still open at a batch end is reported with what is known, and again later.
  - **Renames split across batches** are carried in the checkpoint (up to 64 pending). A new-name record that cannot be paired becomes `Created(final)`, which D-29 consumers apply as an upsert.
  - **Bad journal content** (malformed or truncated records, stalled cursors) returns `FullRescanRequired(Other)`. Skipping would silently corrupt the index, and an error would repeat on every poll.
  - **The batch cap is soft** (`max_batch_bytes`, 32 MiB). A batch ends past the cap only once it holds an event, so polling until a batch is empty drains the journal.
  - **`StartUsn = 0` is special in the kernel:** it reads from the first record instead of failing. The post-check that asks again for `FirstUsn` applies only to reads that start at 0.
  - **No journal, no history.** Enumeration works without a journal. The checkpoint records the absence and `changes` asks for a rescan until one exists. The source never creates a journal (D-31).
  - **Hard links:** adding or removing one link of a file reports nothing. Removing the last link is `Deleted`. A file whose only records in a batch are link records is not reported in that batch.
- **Rationale:** Per-batch coalescing matches the `Modified` contract ("once per batch") and keeps the source stateless apart from the checkpoint. Rescanning on bad data is safe because a rescan's checkpoint is taken after the bad data.
- **Consequences:** 3.3 polls until empty, commits the checkpoint only after applying a batch, debounces across batches and decides when to create a journal. A rename whose records straddle batches is reported as `Created`, pinned by a unit test. #25 (mount points) is unchanged.

## D-39 — Dataset grouper semantics
- **Status:** Proposed
- **Context:** stage01/2.3 ([note](stages/stage01/sessions/2.3-dataset-grouper.md)); dataset identity in D-27, USN behaviour in D-38.
- **Decision:**
  - Events are folded **per FRN per batch** and are order-independent. A child may arrive before its parent directory. `Created` for a known FRN is an upsert, which covers an unpaired rename.
  - `GroupDelta` separates three actions: `changed` (membership changed, re-extract), `modified` (content changed on an existing member, re-extract) and `renamed` (anchor renamed or moved only, update the catalog row, no extraction). `added`, `removed` and `moved_dirs` complete it.
  - A `Created` for a known directory with a new parent or name is a move (`moved_dirs`). Deleting a directory drops every dataset below it, whatever the order of deletions.
  - A `Modified` for an unknown relevant file is treated as a missed `Created`.
  - One FRN is one name (the first by sort order), matching the source's hard-link behaviour (D-38). `*.vat.dbf` is not a lone table.
- **Rationale:** The USN source can deliver events in any order, and 3.3 needs a direct mapping from delta to action.
- **Consequences:** 3.3 calls `is_relevant_name` before listing directories and maps `GroupDelta` to actions. GPKG layers are not enumerated here; readers produce them. Grouper memory at 10M files is not benchmarked (4.3).

## D-40 — CRS layer implementation choices
- **Status:** Proposed
- **Context:** stage01/2.4 ([note](stages/stage01/sessions/2.4-crs-epsg-transform.md)); policy in D-09 and D-34, parsing model in D-32.
- **Decision:**
  - **EPSG table** generated from PROJ's `proj.db` (EPSG v12.029) by `crates/crs/scripts/gen_epsg.py` in the pinned GDAL image, committed as `crates/crs/data/epsg.txt` (1.16 MB, parsed once on first use). It is not the `crs-definitions` crate, which lacks Esri aliases, datum aliases and Helmert parameters. Licences (EPSG terms of use, Esri names Apache-2.0, PROJ MIT) are in `NOTICE`. Bumping the GDAL image bumps EPSG; the diff is reviewed.
  - **`proj4rs` without default features.** Its `aeqd` feature builds C (`proj4rs-geodesic`, `cc`), which needs its own decision (D-35 covers SQLite only). Cost: no Azimuthal Equidistant (7 EPSG CRSs).
  - **Native inverses** for methods `proj4rs` lacks or has only spherically: Cassini-Soldner, Hotine Oblique Mercator A/B, NZMG, Equal Earth, Robinson, Sinusoidal, ellipsoidal Equidistant Cylindrical. They follow PROJ's formulas and are checked against it.
  - **Axis order for file data is GDAL's traditional GIS order** (`AxisPolicy::TraditionalGis`, default); `AxisPolicy::Authority` is available.
  - **Datum shifts are Helmert only**, from the definition or else the table's most accurate one to WGS 84. No grids (D-09).
  - **Identification confidence** follows PROJ (100 authority or name, 70 parameters), plus 90 for name-only matches on methods that cannot be transformed. Ambiguity means unidentified, never a guess. A bare code outside the table is `Unidentified`, unless it is a known vertical or geocentric code.
  - **Partial extents:** only boundary points outside the domain make an extent partial, and the extent is widened to the domain edge by bisection. Interior domain holes are not detected (#46).
  - API shape: `identify` returns `Option<Identification>` and `transform_bounds` returns `Result<TransformedBounds, TransformError>`, both richer than the spec's wording.
- **Rationale:** Agreeing with GDAL on axis order and identification avoids spurious oracle mismatches. The generated table gives name-based identification of Esri `.prj` files, which the `crs-definitions` crate cannot.
- **Consequences:** 3.2 feeds GeoKey codes into `resolve`. 3.3 caches `resolve` and `Transformer` per definition hash. `proj4rs` upstream issues (spherical `eqc`, hard-coded `krovak`) are worth filing (#45).

## D-41 — Catalog conventions
- **Status:** Proposed
- **Context:** stage01/2.5 ([note](stages/stage01/sessions/2.5-catalog.md)); schema deltas are in `03-architecture.md` §4, storage choice in D-10 and D-35.
- **Decision:**
  - **R*Tree row ids encode the layer:** `id = layer_id * 4 + part`, at most 4 parts per layer. A delete trigger and the read-back are rowid lookups. An extent with more than 4 parts is stored as the union of its parts (a superset), with a `warn`.
  - **Exact bbox test in SQL.** R*Tree coordinates are f32 rounded outwards; auxiliary `west/south/east/north` columns keep exact f64 values and each probe compares them. A query box that crosses the antimeridian is a `UNION` of probes, not an `OR`.
  - **A CRS resolution belongs to its definition and the latest write wins.** `crs` rows are keyed by (kind, text, authority) through a stable FNV-1a hash plus a full compare, so better identification upgrades every layer that shares the definition.
  - **`epsg` and `bbox` in `DatasetFilter` must hold for the same layer.**
  - **`set_status` keeps stored records valid:** `pending`, `unsupported` and `error` drop the dataset's layers (the `DatasetRecord` invariant). D-36 keeps 3.3 from using it on transient failures.
  - **`synchronous = NORMAL` in WAL mode.** Commits stay atomic, but the last commits can be lost on power failure. The plan reconciles a catalog/snapshot checkpoint mismatch (3.3, 4.2). Also `temp_store = MEMORY`, `foreign_keys = ON`, writers `BEGIN IMMEDIATE`.
  - **Non-finite raster floats are stored as absent** (#50 proposes rejecting them in core). An out-of-range integer is an `OutOfRange` error.
  - Writes (`upsert_dataset`, `delete_dataset`, `delete_volume`, `set_status`) are atomic through a savepoint; `volumes.id` is `AUTOINCREMENT`, so ids are never reused.
- **Rationale:** Rowid arithmetic avoids a scan on delete. Exact f64 edges avoid f32 false positives and negatives at box boundaries. A stable hash is needed because `std::hash` changes between Rust releases.
- **Consequences:** Re-upsert costs about 2x a first insert, and planner statistics need `PRAGMA optimize` (#47). Benchmarks (100k datasets): first insert 7.9 s, bbox p50 62 µs, member lookup 12 µs.

## D-42 — Shapefile reader conventions
- **Status:** Proposed (the count rule is the orchestrator's review decision)
- **Context:** stage01/2.6 ([note](stages/stage01/sessions/2.6-reader-shapefile.md)); oracle rules in D-33.
- **Decision:**
  - **Feature count** comes from the `.shx`, then the `.dbf`, as in GDAL. A disagreement is `partial` and reports the `.shx` count. **Record presence** is decided by the `.shp` alone (declared length above 100), so an empty `.shp` has no extent and a sidecar can never erase a valid bbox. Orchestrator decision at review (2026-10-27).
  - **A corrupt `.dbf` with an intact `.shp` is `partial`**, not `error`: geometry type, extent and CRS stay usable, the count falls back to the `.shx` and the fields are dropped. Any `.shp` header problem, including a declared length beyond EOF, stays `error`.
  - **The counting reader is a thread-local meter** (`readers::measure`), not a wrapper type. Readers are synchronous and stateless, so a per-thread delta equals one dataset.
  - **`encoding_rs`** decodes DBF field names and non-UTF-8 `.prj` text. DOS code pages that the Encoding Standard lacks (437, 850, 852, …) fall back to UTF-8 or Windows-1252, and are flagged `partial` when a name has non-ASCII bytes (#42).
  - **Limits:** `.prj` up to 16 KiB (larger: CRS ignored, `partial`), up to 1,024 DBF fields (more: list truncated, `partial`), `.cpg` up to 64 B. Worst case about 49 KB read per dataset.
  - **DBF version allow-list:** dBase III, FoxPro and Visual FoxPro flavours. Others are not parsed (`Unsupported`, or fields dropped with `partial` inside a shapefile).
  - All file access goes through `io::SourceFile`, which checks the path is a regular file before opening, validates offset and length before allocating, and charges the per-dataset budget (`ReadLimits::max_bytes`).
- **Rationale:** GDAL opens the corrupt-`.dbf` corpus case as `ok`, so `partial` keeps the usable metadata without claiming perfection. Validating before allocating means untrusted lengths cannot cause large allocations (guardrail 5).
- **Consequences:** 3.1 and 3.2 add one `READERS` line and one oracle `Adapter` line each, and reuse `SourceFile`, `measure` and `adapters::native`. `ReadLimits::time_budget` is not checked by this reader.

## D-43 — Raw `$MFT` enumeration
- **Status:** Proposed
- **Context:** Session 2.7 was added at gate G1 after the owner's D: run (41 s, almost all inside `FSCTL_ENUM_USN_DATA`) ([note](stages/stage01/sessions/2.7-ntfs-mft-reader.md), Q-16). Verified at V3 on the CI runner's `C:`.
- **Decision:**
  - **Same raw-record API.** The reader yields 1.3's `RawRecord` (FRN with sequence number, parent, name, attributes), so `Source` and its consumers do not change. It is selected by the internal option `SourceOptions::enumeration` (`EnumerationMethod::{UsnApi (default), RawMft}`); it is not a recommendation. The reserved system records 0..=15 (except the root), the children of `$Extend` (except `$RmMetadata`) and `$RmMetadata\$Repair` are left out, as the USN API omits them.
  - **Name rule:** the first `$FILE_NAME` in the Win32 or Win32+DOS namespace, else the first POSIX one. DOS-only names are never reported. Attributes are `$STANDARD_INFORMATION` masked to the low 16 bits plus the directory bit.
  - **Strict mode:** if the scan cannot account for every in-use record (torn, malformed, not-`FILE` in-use slots, extension errors, truncated runlists), it returns `Error::MftIncomplete`. The caller discards what the sink received and falls back to the USN API. A record without a reportable name is counted but is not a loss.
  - **Torn-record retry:** each torn record is read once more after the sequential pass.
  - **Replay margin:** the raw checkpoint starts journal replay `raw_replay_margin` (default 1 MiB) before the position taken at the start, because `$MFT` pages may be unflushed. `FSCTL_READ_USN_JOURNAL` rejects a start USN that is not a record boundary, so the source walks the journal from `first_usn` and snaps to the last record at or before the target (any trouble falls back to `first_usn`, which only replays more).
  - **No `$MFT` order guarantee:** files whose extension records come later are delivered after the pass; consumers must not rely on order.
  - **Sizes and timestamps** can come from the same pass at no extra I/O (`$STANDARD_INFORMATION`, the unnamed `$DATA` header); the USN API gives neither. Input to stage02.
  - **Read caps:** 4 MiB chunks, free chunks skipped through `$BITMAP`, at most 2^20 runs, 4096 extension records per file, `$ATTRIBUTE_LIST` up to 1 MiB, an extension cache of 128 MiB, deferred files capped at 64 MiB. `FILE_FLAG_NO_BUFFERING` and overlapped read-ahead were not adopted.
- **Rationale:** On the CI runner raw reads take about 6 s cold and warm, against USN 36–62 s cold and 2.0–2.7 s warm; the record sets agree (1,355,125 identical, 17 raw-only metadata records). Strictness avoids a silently incomplete index; the margin covers metadata that is not yet on disk.
- **Consequences:** Q-16 stays **Open** and is decided in 4.3 with the owner's D: numbers. The session proposes raw for the first (cold) index and USN for changes, with USN as the fallback. Not handled: `$BITMAP`/`$DATA` pieces in extension records, compressed or sparse `$MFT`, 4Kn hardware (#53), ReFS. Hard-link names are counted but not all reported (#52).
