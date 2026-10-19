# Stage 01 — M0: CLI Indexer Prototype

**Objective:** prove the risky core before any UI (D-20). A command-line `whereabouts` that enumerates an NTFS volume, keeps up with the USN journal, groups files into datasets, extracts extent/CRS metadata for Shapefile, GeoPackage and GeoTIFF with native readers, stores it in a SQLite catalog with an R*Tree, and answers name + bbox queries — with correctness measured against GDAL as an oracle and speed measured against Everything.

**Status:** planned (2026-10-19). Wave 0 (bootstrap + this plan) done; milestone [`stage01`](https://github.com/GeoScenarioScripter/whereabouts/milestone/1) with issues #1–#17. Next: wave 1.

Read first: `AGENTS.md`, `docs/03-architecture.md`, `docs/04-formats-and-crs.md`, `docs/08-development-workflow.md`.

---

## Scope

**In scope**
- Crates: `core`, `source`, `ntfs`, `nameindex`, `datasets`, `readers` (SHP, GPKG, GeoTIFF), `crs`, `catalog`, `query` (minimal), `indexer`, `cli`, `tests/oracle`.
- One NTFS volume at a time; CLI runs elevated.
- Real-world + synthetic test corpus; GDAL golden files; oracle agreement tracking in CI.
- Benchmarks (enumeration, query latency, memory) and the answers to Q-14, Q-16, Q-18.

**Out of scope (later stages)**
- Windows service, named-pipe IPC (M1 / stage02).
- Remaining Tier 1 readers: FlatGeobuf, GeoParquet, LAS/LAZ, world-file rasters, GeoJSON, KML/KMZ, DXF (M1). The grouper *classifies* them now; extraction marks them `unsupported`.
- Simplified footprints and two-stage spatial query (D-16, M1). Stage01 stores WGS84 bboxes only.
- Search syntax design (Q-11), ranking model (Q-17), UI, previews, FGDB reader, non-NTFS drives (Q-06).
- Hard-link completeness (documented limitation; M1).

## Exit criteria

1. **Indexes a real volume (V4):** `whereabouts index --volume <X:>` completes on the owner's GIS workstation volume; the catalog holds the SHP/GPKG/GeoTIFF datasets found there, each with status `ok`/`partial`/`error:<reason>` — no panics.
2. **Stays live and survives restarts (V3):** after a restart it resumes from the saved checkpoint without a full rescan; journal wrap and journal re-creation trigger a correct full rescan. Proven in CI on a VHD test volume.
3. **Instant name search (V4):** query latency and index memory meet the Q-18 targets set in session 4.3, measured on the owner's volume alongside Everything.
4. **Correct metadata (V2):** on the committed corpus, oracle agreement meets the committed baseline: native bbox, geometry type and counts agree on ≥ 98% of cases; EPSG identification and WGS84 bbox agreement meet the targets recorded for Q-14. Enforced in CI.
5. **Clean build (V1):** CI green on Windows + Linux; `cargo deny` green (no GDAL).
6. **Docs:** Q-14, Q-16, Q-18 resolved in `02-decisions.md`; architecture doc matches the code; retro written; stage02 sketch updated.

## Gates

| Gate | After | Question | Evidence | If it fails |
|---|---|---|---|---|
| **G1 — contracts + enumeration** | wave 1 | Are the core contracts (1.1) right for everything downstream? Does `FSCTL_ENUM_USN_DATA` enumerate fast enough to keep the USN-API approach (Q-16)? | 1.1 API summary; 1.3 enumeration timing and memory on CI + owner volume | Contract fixes before wave 2; or add a raw-`$MFT` session to wave 2 |
| **G2 — CRS coverage** | wave 2 | Is pure-Rust CRS (`proj4rs` + embedded EPSG table) good enough (Q-14)? | 2.4 identification rate + WGS84 agreement on corpus + fixtures | Owner decides: PROJ via the `proj` crate as fallback (D-09), or widen 2.4's tables |

Gates are owner go/no-go points: the orchestrator presents evidence and waits.

---

## Wave plan

| Wave | Sessions (parallel) | Opens after | Theme |
|---|---|---|---|
| 0 | bootstrap (orchestrator) | — | Repo, tooling, docs, this plan ✅ |
| 1 | **1.1** core-contracts · **1.2** oracle-corpus · **1.3** ntfs-enumeration · **1.4** crs-wkt-parsing | wave 0 | Contracts + the two riskiest spikes + test oracle |
| — | **Gate G1** | wave 1 merged | |
| 2 | **2.1** nameindex · **2.2** ntfs-journal-source · **2.3** dataset-grouper · **2.4** crs-epsg-transform · **2.5** catalog · **2.6** reader-shapefile | G1 | Build every component against the frozen contracts (≤ 4 concurrent; see order below) |
| — | **Gate G2** | 2.4 merged | |
| 3 | **3.1** reader-geopackage · **3.2** reader-geotiff · **3.3** index-pipeline | wave 2 + G2 | Remaining readers + end-to-end `index` |
| 4 | **4.1** query-and-inspect-cli · **4.2** resilience · **4.3** benchmarks-and-coverage | wave 3 | Usable CLI, failure modes, numbers |
| 5 | **5.1** exit-verification | wave 4 | Exit criteria evidence, docs sync, retro draft |

Wave 2 has six sessions; with the 4-concurrent cap, start **2.2, 2.4, 2.1, 2.6** first (longest / gate-relevant), then **2.3, 2.5** as slots free.

### Dependency graph

| Session | Depends on | Why |
|---|---|---|
| 1.1 | — | Defines the shared vocabulary |
| 1.2 | — | Corpus + golden files use their own record format (no core types needed) |
| 1.3 | — | Raw NTFS records are local to `ntfs`; the `Source` adapter comes in 2.2 |
| 1.4 | — | CRS model is local to `crs` |
| 2.1 | 1.1 | Indexes `FileRecord`s |
| 2.2 | 1.1, 1.3 | Implements `Source` on top of 1.3's enumeration |
| 2.3 | 1.1 | Produces `Dataset`s from file records |
| 2.4 | 1.1, 1.4 (+1.2 for corpus checks) | Identification/transform on 1.4's model, `Bounds` from core |
| 2.5 | 1.1 | Persists core types |
| 2.6 | 1.1, 1.2 | Implements `FormatReader`; oracle wiring needs the corpus |
| 3.1, 3.2 | 2.6 (reader pattern), 2.4 (3.2 adds GeoKeys to `crs`) | Same pattern, same registry |
| 3.3 | 2.1–2.6 | Wires everything |
| 4.1 | 3.3 | Needs a populated catalog + snapshot |
| 4.2 | 3.3 | Exercises the pipeline's failure paths |
| 4.3 | 3.1–3.3 | Measures the finished pipeline and all three readers |
| 5.1 | all | |

### Watch points (shared files within a wave)

- **Wave 1:** disjoint crates. Each session may add `[workspace.dependencies]` lines → trivial conflicts; merge order 1.1 → 1.4 → 1.3 → 1.2.
- **Wave 2:** 2.6 adds the reader registry in `readers/src/lib.rs` and the oracle adapter registry in `tests/oracle` — nobody else in wave 2 touches them. 2.2 is the only `ntfs` writer, 2.4 the only `crs` writer. Merge order: 2.5 → 2.3 → 2.1 → 2.4 → 2.2 → 2.6.
- **Wave 3:** 3.1 and 3.2 both add one line to the reader registry and one to the oracle adapter registry → merge 3.1 first, rebase 3.2. 3.2 adds `crs/src/geokeys.rs` (+ one `mod` line). 3.3 owns `indexer` and `cli`.
- **Wave 4:** 4.1 owns `query` and `cli`; 4.2 owns `indexer` (+ `ntfs` tests); 4.3 owns `tests/oracle`, `benches/`, `docs/benchmarks/` and only *reads* `indexer`. If 4.2 needs a CLI flag, it goes to 4.1 as a follow-up. Merge order: 4.2 → 4.1 → 4.3.

---

## Sessions

Each spec gives: **Reads**, **Write scope**, **Deliverables**, **Acceptance** (with required verification level), **Out of scope**. Anything not listed in the write scope is off-limits except the shared-file rules in `08-development-workflow.md` §8.

### Wave 1 — contracts, spikes, oracle

#### 1.1 — core-contracts
- **Reads:** `03-architecture.md` §2–5, `04-formats-and-crs.md` §3, D-04, D-05, D-10, D-18.
- **Write scope:** `crates/core/**`, `crates/source/**`.
- **Deliverables:**
  - `core`: identities — `VolumeId`, `Frn` (64-bit file reference incl. sequence number), `DatasetId`, `LayerId` (choose and document the derivation of stable dataset IDs, e.g. volume + anchor FRN); `FileRecord { frn, parent, name, is_dir, attrs }`; `Format` enum (all Tier 1 + FGDB + `NameOnly`), `DatasetKind`; `Dataset { id, volume, kind, format, anchor, members: Vec<Member { frn, role }> }` with `MemberRole`; `Bounds` (native, with validity checks) and `Wgs84Extent` (one or more parts, for antimeridian splits); `CrsDefinition { kind: Wkt1Esri | Wkt1 | Wkt2 | ProjJson | GeoKeys | Epsg, text }`; `GeometryType`; `RasterInfo`; `LayerMetadata` and `DatasetMetadata` (what a reader returns: raw CRS definition, native bounds, counts, fields, raster facts); `ExtractionStatus { Pending, Ok, Partial(reason), Unsupported, Error(reason) }`; error type.
  - `core::reader`: the `FormatReader` trait (input: a resolved dataset = format + anchor path + member paths by role; output: `DatasetMetadata`), with the documented contract that readers do bounded reads, never write, never panic on malformed input.
  - `source`: the `Source` trait — synchronous, poll-based: `enumerate(sink)` → `Checkpoint`; `changes(&Checkpoint)` → batch of `SourceEvent { Created, Deleted, Renamed { old_parent, old_name }, Modified, FullRescanRequired(reason) }` + new `Checkpoint`. `Checkpoint` is opaque, versioned, serializable bytes.
  - `serde` derives on all public data types; rustdoc on every public item; a `CONTRACTS.md`-style summary at the top of `core/src/lib.rs` for downstream sessions.
- **Acceptance (V1):** serde round-trip and invariant tests (e.g. `Bounds` rejects NaN/inverted); `cargo doc` clean; every catalog column in `03-architecture.md` §4 maps to a field (mapping table in the note); the note includes a ≤ 1-page API summary for G1.
- **Out of scope:** any I/O, any implementation of the traits.

#### 1.2 — oracle-corpus
- **Reads:** `04-formats-and-crs.md` §2, §4, §5; D-08.
- **Write scope:** `tests/corpus/**`, `tests/oracle/**`, `.github/workflows/oracle-golden.yml`.
- **Deliverables:**
  - Corpus layout: `tests/corpus/data/<format>/<case>/…` (committed, ≤ 10 MB total), `tests/corpus/manifest.toml` (per case: id, format, files, edge-case tags, provenance, licence), `tests/corpus/external/` (gitignored) with `scripts/fetch-external.{sh,ps1}` + sha256 manifest for larger real-world samples.
  - `scripts/generate.sh`: builds synthetic cases inside a **pinned** GDAL container image (`ghcr.io/osgeo/gdal:<tag>@sha256:…`). Shapefile, GeoPackage, GeoTIFF only, covering at least: UTM, LCC, Albers, Web Mercator, polar stereographic, geographic, NZTM (2193); ESRI WKT without authority codes; `.cpg` vs DBF language-driver encodings; empty shapefile; GPKG with several layers, stale `gpkg_contents` bounds, missing bounds, attribute-only table, tiles; GeoTIFF north-up, rotated (ModelTransformation), PixelIsPoint, BigTIFF, COG with overviews, georef only in `.aux.xml`, world file + `.prj`, nodata; an antimeridian-crossing dataset (e.g. EPSG:3460 Fiji); a few deliberately corrupt/truncated files.
  - `scripts/golden.py` (runs in the same container, GDAL Python bindings) → one normalized **oracle record** per case in `tests/corpus/expected/<case>.json`: per layer — name, geometry type, feature count, native extent, raster size/bands/dtype/resolution/nodata, CRS as WKT2 + identified EPSG (`AutoIdentifyEPSG`/`FindMatches`) + `crs_status`, WGS84 extent via `TransformBounds` with 21-point densification (antimeridian-aware).
  - `.github/workflows/oracle-golden.yml` (`workflow_dispatch`): regenerates corpus + goldens and uploads them as an artifact, so sessions without Docker can still regenerate.
  - `tests/oracle` crate: oracle record types (serde), tolerance-aware comparison (bbox abs/rel tolerance, WGS84 in degrees, exact counts, EPSG equality), an adapter registry (`format → fn(case) -> Option<NativeRecord>`, empty for now), per-format agreement report, and `tests/agreement.rs` that fails if agreement drops below `tests/oracle/baseline.toml`.
- **Acceptance (V2):** ≥ 30 cases covering every edge case listed; goldens committed and reproducible (re-running the workflow yields no diff); `cargo test -p whereabouts-oracle` passes with an empty adapter registry (all cases "skipped", report printed).
- **Out of scope:** native readers; Tier 1 formats beyond SHP/GPKG/GeoTIFF (the layout must make adding them trivial).

#### 1.3 — ntfs-enumeration
- **Reads:** `03-architecture.md` §3.1–3.3; Q-16; Microsoft docs for `FSCTL_ENUM_USN_DATA`, `FSCTL_QUERY_USN_JOURNAL`, `USN_RECORD_V2/V3`.
- **Write scope:** `crates/ntfs/**`.
- **Deliverables:**
  - `windows` crate as a `cfg(windows)` target dependency; on other targets the crate compiles to an empty API (so Linux CI stays green).
  - Open a volume handle; detect NTFS (`GetVolumeInformationW`); clear error when not elevated or not NTFS.
  - `query_journal()` → journal ID, first/next USN, max size (do **not** create a journal implicitly; report absence).
  - `enumerate()` → iterator of raw records `(frn, parent_frn, name, attributes)` via `FSCTL_ENUM_USN_DATA`, large reusable buffer, USN_RECORD_V2 and V3 parsing; names kept lossless (UTF-16 → `OsString`/WTF-8; document the choice).
  - Record parsing as pure functions over byte buffers, unit-tested cross-platform with captured fixtures.
  - `examples/enum_volume.rs`: prints record count, elapsed time, peak working set.
  - `// SAFETY:` comment on every `unsafe` block; `#![allow(unsafe_code)]` only in this crate.
- **Acceptance (V3):** `#[ignore]` elevated test on the CI Windows runner enumerates `C:` (> 100k records; root FRN present; ≥ 99.9% of parent chains resolve to the root); timing and memory for the CI runner recorded in the note; owner asked to run the example on their workstation (V4 numbers feed G1 and Q-16).
- **Out of scope:** journal tailing, the `Source` trait implementation (2.2), raw `$MFT` parsing (only if G1 says so).

#### 1.4 — crs-wkt-parsing
- **Reads:** `04-formats-and-crs.md` §4; D-09; OGC 01-009 (WKT1), OGC 18-010 (WKT2), PROJJSON schema.
- **Write scope:** `crates/crs/**`.
- **Deliverables:**
  - Tokenizer + parser for OGC WKT1, the ESRI WKT1 dialect, and WKT2 (2015/2019) — `[]` and `()` brackets, case-insensitive keywords, quoted names.
  - Internal model: `Crs { kind (geographic | projected | geocentric | compound | vertical | engineering | unknown), name, datum { name, ellipsoid { a, inv_f }, prime_meridian, towgs84 }, projection { method, params }, linear_unit, angular_unit, axis_order, authority }`; canonical method/parameter names with ESRI → canonical normalisation (`Transverse_Mercator`, `False_Easting`, `GCS_*`, `D_*`, …).
  - PROJJSON → same model.
  - Fixtures in `crates/crs/tests/data/`: ≥ 60 definitions — ESRI `.prj` and WKT1/WKT2 for the projection families listed in `04` §4, sourced from public references (provenance noted; EPSG-derived text attributed).
- **Acceptance (V1):** 100% of the fixtures parse into the expected kind/method/units (table-driven tests); malformed input returns an error and never panics (fuzz-style test over truncated and mutated fixtures); no `unwrap` on input-derived data.
- **Out of scope:** EPSG identification, coordinate transforms (2.4), GeoKeys (3.2).

### Wave 2 — components on frozen contracts

#### 2.1 — nameindex
- **Reads:** `03-architecture.md` §3.4; D-10; 1.1 note.
- **Write scope:** `crates/nameindex/**`.
- **Deliverables:** contiguous string pool per volume; entry arrays (`frn`, parent index, name offset/len, extension offset, flags); case-folded copy; FRN → entry map; path reconstruction with cycle/orphan guards; insert/delete/rename with tombstones and compaction; search: case-insensitive substring, `*`/`?` wildcards, extension filter, optional path match; multithreaded scan; binary snapshot (versioned header, checksum, embedded opaque `Checkpoint`); criterion benchmarks on synthetic 1M/5M-entry indexes with a realistic name distribution.
- **Acceptance (V1):** property tests (path reconstruction equals a naive implementation; search equals a naive filter); snapshot round-trip; corrupt snapshot detected. Benchmark numbers recorded (they are inputs to Q-18, not pass/fail yet).
- **Out of scope:** regex, ranking, the query syntax.

#### 2.2 — ntfs-journal-source
- **Reads:** 1.1 and 1.3 notes; `03-architecture.md` §3.2.
- **Write scope:** `crates/ntfs/**`.
- **Deliverables:** `FSCTL_READ_USN_JOURNAL` polling; reason → `SourceEvent` mapping coalesced per file at `USN_REASON_CLOSE` (rename old/new pairing); `ERROR_JOURNAL_ENTRY_DELETED`, journal-ID change and missing journal → `FullRescanRequired`; `NtfsVolumeSource: Source` (enumerate + changes) with a versioned `Checkpoint` (volume serial, journal ID, next USN); CI helper that creates, formats and mounts a small **VHD test volume** (diskpart) for isolated tests.
- **Acceptance (V3):** on the VHD volume in CI — create/modify/rename/delete produce the expected events; a checkpoint older than `FirstUsn` → `FullRescanRequired`; `fsutil usn deletejournal` + recreate → `FullRescanRequired`. Reason-mapping unit tests run cross-platform from fixtures.
- **Out of scope:** debouncing across batches (3.3), hard links beyond documenting behaviour.

#### 2.3 — dataset-grouper
- **Reads:** `04-formats-and-crs.md` §3; D-04; 1.1 note.
- **Write scope:** `crates/datasets/**`.
- **Deliverables:** grouping over a directory view (`children(parent) → (frn, name, is_dir)`), so it's testable without a volume; rules for shapefile (case-insensitive stems, multi-dot names, `.shp.xml`), GeoPackage (ignore `-wal`/`-shm`/`-journal`), GeoTIFF + sidecars (`.aux.xml`, `.ovr`, `.msk`, `.tfw`/`.tifw`/`.wld`, `.prj`), FGDB folder signature (internal files never surfaced), single-file Tier 1 formats (classified, extraction later), lone `.dbf` → non-spatial table, DWG → name-only; incremental API: given a `SourceEvent`, return affected dataset IDs (dirty set), and anchor renames that need no re-extraction.
- **Acceptance (V1):** table-driven tests for ≥ 40 messy directory listings (mixed-case extensions, orphan sidecars, `a.b.shp`, a GeoTIFF with both internal georef and a world file, a GPKG with `-wal`, nested `.gdb`); incremental tests for every event type.
- **Out of scope:** reading file contents.

#### 2.4 — crs-epsg-transform
- **Reads:** 1.4 note; `04-formats-and-crs.md` §4; D-09; Q-14.
- **Write scope:** `crates/crs/**`, plus a `NOTICE` entry for EPSG attribution.
- **Deliverables:** embedded EPSG table (choose: an existing crate such as `crs-definitions`, or generated from the EPSG dataset — record licence + attribution as a *Decision proposed*); `identify(&Crs) → Option<(epsg, confidence)>`: authority code → normalised name match → parameter match (method, params within tolerance, ellipsoid, units); model → `proj4rs` projection built from parameters (so unidentified-but-parseable CRSs still transform); `transform_bounds(&Bounds, &Crs) → Wgs84Extent`: 21 points per edge, axis-order aware, antimeridian detection and split, polar clamping, out-of-domain → partial.
- **Acceptance (V2):** over the 1.2 corpus (using each case's raw CRS text + native bbox from the goldens) and the 1.4 fixtures: identification rate and WGS84 agreement reported per projection family; this report is the evidence for **G2**.
- **Out of scope:** datum grid shifts (accepted error per D-09), GeoKeys (3.2).

#### 2.5 — catalog
- **Reads:** `03-architecture.md` §4; D-10; 1.1 note.
- **Write scope:** `crates/catalog/**`.
- **Deliverables:** schema per §4 adapted to the frozen core types (record the delta for the architecture doc); migrations via `PRAGMA user_version`; WAL, foreign keys; `crs` de-duplicated by definition hash; R*Tree of WGS84 extent parts with an auxiliary `layer_id` column (one row per antimeridian part); `volumes` table holding the last committed `Checkpoint`; API: upsert/delete dataset with files and layers in one transaction, status updates, lookup by member FRN, bbox query, filter by format/EPSG; batch-insert benchmark (100k datasets).
- **Acceptance (V1):** tests on temporary DBs — idempotent upsert, cascade delete, bbox queries incl. antimeridian parts, migration from an empty DB; benchmark numbers recorded.
- **Out of scope:** footprints (M1), tile cache.

#### 2.6 — reader-shapefile
- **Reads:** `04-formats-and-crs.md` §2; 1.1 and 1.2 notes.
- **Write scope:** `crates/readers/**`, `tests/oracle/**` (adapter registration only).
- **Deliverables:** hand-written bounded header readers: `.shp` (100-byte header: shape type, bbox, Z/M ranges), `.dbf` (record count, fields, language driver), `.shx` length fallback for the count, `.prj` → `CrsDefinition` (ESRI vs OGC WKT1 detection), `.cpg`; the **reader registry** (`format → &dyn FormatReader`) in `readers/src/lib.rs` that 3.1/3.2 extend; a counting-reader test utility that asserts bytes read stay bounded; the shapefile oracle adapter (native fields only — WGS84/EPSG comparison arrives with 3.3's `extract`).
- **Acceptance (V2):** oracle agreement on all non-corrupt shapefile cases for geometry type, count and native bbox; corrupt cases → `Error` status, no panic; bounded-read test shows < 64 KiB read per dataset.
- **Out of scope:** geometry reading (previews, M3).

### Wave 3 — remaining readers, end-to-end pipeline

#### 3.1 — reader-geopackage
- **Write scope:** `crates/readers/**` (gpkg module + one registry line), `tests/oracle/**` (one adapter line).
- **Deliverables:** read-only open that **never creates `-wal`/`-shm` files or changes mtime** (verify; document the read-only-directory edge case); layers from `gpkg_contents` (features, tiles, attributes, 2D gridded coverage); CRS from `gpkg_spatial_ref_sys` (prefer `definition_12_063` WKT2 when present, else WKT1, plus organisation/ID); geometry type and Z/M from `gpkg_geometry_columns`; bounds: contents → R-tree min/max → mark `partial`; feature count: `gpkg_ogr_contents` → R-tree count → bounded `count(*)` with a time budget → `partial`.
- **Acceptance (V2):** oracle agreement on all non-corrupt GPKG cases (including stale/missing bounds); a test proving the file's bytes, mtime and directory listing are unchanged after reading.

#### 3.2 — reader-geotiff
- **Write scope:** `crates/readers/**` (tiff module + one registry line), `crates/crs/src/geokeys.rs` (+ one `mod` line), `tests/oracle/**` (one adapter line).
- **Deliverables:** bounded TIFF/BigTIFF IFD parser (first IFD + overview count); tags: dimensions, samples, bits, sample format, compression, ModelPixelScale/Tiepoint/Transformation, GeoKeyDirectory/Double/Ascii params, GDAL_NODATA, GDAL_METADATA; native bounds from the affine (all four corners for rotated rasters); PixelIsPoint handled the way GDAL does by default; georeferencing precedence mirrors GDAL's default `GDAL_GEOREF_SOURCES` (PAM `.aux.xml`, internal, world file); `.ovr` detection; GeoKeys → `Crs` (direct EPSG codes + user-defined projection keys).
- **Acceptance (V2):** oracle agreement on all non-corrupt GeoTIFF cases incl. rotated, PixelIsPoint, BigTIFF, aux.xml-only and world-file cases; bounded-read test.

#### 3.3 — index-pipeline
- **Write scope:** `crates/indexer/**`, `crates/cli/**`.
- **Deliverables:** `indexer::extract(resolved dataset) → extracted metadata` (reader → `crs` parse/identify/transform → status), the single function the oracle and the pipeline share; pipeline: `NtfsVolumeSource` enumerate → name index → grouper → extraction queue (worker threads, priority = recently modified first) → catalog; change loop: poll journal → name-index update → dirty datasets → coalesce per batch → re-extract; checkpoint commit order (catalog first, then snapshot) and restart replay; CLI `whereabouts index --volume X: [--db PATH] [--watch]` with progress output.
- **Acceptance (V3):** on the CI VHD volume populated with corpus files: full index → catalog matches expectations; restart → no full rescan, picks up changes made while stopped; editing a `.prj` re-extracts only that dataset.

### Wave 4 — usable CLI, failure modes, numbers

#### 4.1 — query-and-inspect-cli
- **Write scope:** `crates/query/**`, `crates/cli/**`.
- **Deliverables:** `whereabouts inspect <path>` (group + extract a single path without an index — the debugging tool); `whereabouts query "<text>" [--bbox minx,miny,maxx,maxy] [--format …] [--epsg N] [--limit N] [--json]`: name scan → member FRN → dataset, ∩ R*Tree bbox, simple ordering (exact > prefix > substring, then mtime); loads the snapshot instead of re-enumerating; table and JSON output.
- **Acceptance (V3):** CLI integration tests on the VHD volume; startup-to-first-result time recorded.

#### 4.2 — resilience
- **Write scope:** `crates/indexer/**`, `crates/ntfs/**` (tests only).
- **Deliverables + acceptance (V3), each as a CI test on the VHD volume:** journal wrap (tiny journal via `fsutil usn createjournal m=… a=…` plus file churn) → full rescan, catalog consistent; journal deleted/recreated → rescan; process killed mid-index → restart converges; corrupt snapshot → detected → rescan; catalog/snapshot checkpoint mismatch → reconcile; Whereabouts' own reads produce no USN events for indexed files; hard-link behaviour documented.

#### 4.3 — benchmarks-and-coverage
- **Write scope:** `tests/oracle/**`, `benches/**` (new, if needed), `docs/benchmarks/**`, `scripts/bench/**`.
- **Deliverables:** oracle switched to the full `indexer::extract` path (EPSG + WGS84 comparisons), with `baseline.toml` raised to the measured numbers; `scripts/bench/bench-vs-everything.ps1` (uses `es.exe` if installed) for the owner's machine; `docs/benchmarks/stage01.md` with enumeration time, index memory per million entries, query latency p50/p95/p99 by term length, catalog throughput, oracle agreement per format; **proposed answers** to Q-14, Q-16, Q-18 (as *Decisions proposed*).
- **Acceptance (V2 in CI + V4 numbers from the owner's run).**

### Wave 5 — exit

#### 5.1 — exit-verification
- **Write scope:** `docs/stages/stage01/**`, `docs/benchmarks/**`, small fixes anywhere (each listed in the note; anything larger becomes a follow-up).
- **Deliverables:** an exit-criteria checklist with evidence links (CI runs, benchmark doc, the owner's V4 run output); a proposed docs sync (architecture deltas, decisions, open questions) for the orchestrator to apply; a retro draft (what worked, what to change for stage02).
- **Acceptance:** every exit criterion is evidenced, or explicitly marked failed with a proposed remedy.

Then the orchestrator runs `/close-stage` (retro, docs sync, `develop → main`, tag `v0.1.0`) and `/plan-stage` for stage02.

---

## Risks

| Risk | Mitigation |
|---|---|
| Core contracts turn out wrong after the fan-out | Wave 1 freezes them deliberately; G1 review before wave 2; flagged contract-change process |
| USN enumeration too slow / memory-heavy | Measured in 1.3 at G1; raw `$MFT` parsing kept as a planned fallback (Q-16) |
| Pure-Rust CRS coverage insufficient | Measured in 2.4 at G2; PROJ via `proj` crate is the documented fallback (D-09) |
| No local Rust toolchain on the owner's machine yet | CI is the verification source (V1–V3); V4 waits for the toolchain — the owner installs it before wave 4 |
| Elevated tests are hard to run safely | Isolated VHD test volumes in CI; tests never write outside their own volume/temp dir |
| Our own reads dirty the user's files (SQLite `-wal`, mtime) and trigger re-index loops | Read-only opens + explicit tests in 3.1 and 4.2 |
| Corpus licensing | Synthetic data generated by us; real-world samples only with recorded provenance and a redistributable licence, large ones not committed |
| Worktree builds exhaust disk/CPU | ≤ 4 concurrent sessions; `sccache` recommended (AGENTS.md) |

## Status

Session status is tracked in GitHub (milestone `stage01`, one issue per session). This table is updated at each wave close.

| Session | Theme | Issue | Status | Note |
|---|---|---|---|---|
| 0 | Bootstrap: repo, tooling, docs, stage plan | — | ✅ done (2026-10-19) | [note](sessions/0-bootstrap.md) |
| 1.1 | core-contracts | #1 | ⏳ planned | |
| 1.2 | oracle-corpus | #2 | ⏳ planned | |
| 1.3 | ntfs-enumeration | #3 | ⏳ planned | |
| 1.4 | crs-wkt-parsing | #4 | ⏳ planned | |
| 2.1 | nameindex | #5 | ⏳ planned | |
| 2.2 | ntfs-journal-source | #6 | ⏳ planned | |
| 2.3 | dataset-grouper | #7 | ⏳ planned | |
| 2.4 | crs-epsg-transform | #8 | ⏳ planned | |
| 2.5 | catalog | #9 | ⏳ planned | |
| 2.6 | reader-shapefile | #10 | ⏳ planned | |
| 3.1 | reader-geopackage | #11 | ⏳ planned | |
| 3.2 | reader-geotiff | #12 | ⏳ planned | |
| 3.3 | index-pipeline | #13 | ⏳ planned | |
| 4.1 | query-and-inspect-cli | #14 | ⏳ planned | |
| 4.2 | resilience | #15 | ⏳ planned | |
| 4.3 | benchmarks-and-coverage | #16 | ⏳ planned | |
| 5.1 | exit-verification | #17 | ⏳ planned | |

## Stage retro

_(written at stage close)_
