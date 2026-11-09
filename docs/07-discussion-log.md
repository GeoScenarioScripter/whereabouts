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
- **What ran:** seven sessions, all merged on 2026-10-27: 2.1 nameindex (#35, V1), 2.2 ntfs-journal-source (#34, V3), 2.6 reader-shapefile (#33, V2), 2.3 dataset-grouper (#38, V1), 2.5 catalog (#36, V1) and 2.4 crs-epsg-transform (#37, V2). 2.7 ntfs-mft-reader (#39, V3, merged last; owner V4 on D: requested in #31). Each PR got a review and fixes before merging; the reviews found, among others, swapped LCC parallels and a geographic box at ±90° (2.4), an unpaired directory rename (2.3), a path-mode depth disagreement with `path()` (2.1), a savepoint cost that doubled bulk insert time (2.5), and an unbounded read on untrusted offsets (2.6).
- **Merge order:** 2.1 → 2.2 → 2.6 → 2.3 → 2.5 → 2.4 → 2.7, as each became ready, against the planned 2.5 → 2.3 → 2.1 → 2.4 → 2.2 → 2.6. The only shared files were `Cargo.lock` and 2.7's small `ci.yml` change, so the order made no difference (as in wave 1).
- **2.7 raw `$MFT` reader → D-43 (Proposed); Q-16 stays Open for 4.3.** On the CI runner's `C:` (1.35M records) raw reads take about 6 s cold and warm against USN 36–62 s cold and 2.0–2.7 s warm. The session proposes raw for the first index and USN for changes, pending the owner's D: run. The review made `RawMft` strict (any loss → `Error::MftIncomplete`, fall back to USN) and found that `FSCTL_READ_USN_JOURNAL` rejects start USNs that are not record boundaries.
- **CI notes from 2.7:** the release `enum_volume` step now runs under bash with `set -e` (pwsh had swallowed failures), and the flaky 2.2 VHD test (`checkpoint_older_than_first_usn_requires_rescan`, the journal filled the 64 MiB VHD) was fixed with a 512 MiB VHD.
- **Gate G2 (owner, 2026-10-27): go.** Pure Rust, no PROJ fallback → D-34 (Q-14 resolved; targets 0.98 / 0.98 enforced from 4.3). Evidence: corpus 40/40 identification and 38/38 WGS84; 246 fixtures 99.2% / 99.1%; EPSG-wide sweep 99.0% WGS84 and 99.9% own-code identification. The 60 misses are rare methods (Polyconic, AEQD, …), added natively only if real data needs them (#45).
- **Other owner answers (2026-10-27):**
  - `rusqlite` `bundled` (SQLite compiled from C) is part of storage → D-35, amending D-10.
  - On the 2.5 design concern (non-ok statuses drop layers, so a transient failure would hide a dataset from spatial search): 3.3 keeps the stored record and retries through a queue, with no core change → D-36 and a 3.3 hand-off.
- **Orchestrator review decision:** shapefile feature count takes the `.shx`, then the `.dbf`, and record presence is decided by the `.shp` alone, matching GDAL → D-42 (Proposed).
- **Decisions proposed by the sessions:** D-43 (raw `$MFT`), D-37 (name index, incl. #26: no `RecordSink` change now), D-38 (journal source), D-39 (grouper), D-40 (CRS layer), D-41 (catalog), D-42 (shapefile reader), all Proposed.
- **Docs:** schema delta table folded into `03` §4, USN behaviours into §3.2, crate rows updated. `proj4rs`'s workspace entry was already fixed in 2.4's review (`default-features = false`).
- **Follow-up issues:** #40 oracle error/partial reasons, #41 oracle `cargo doc` clash, #42 CP437/CP850 tables, #43 shapefile corpus cases, #44 CRS-family corpus cases, #45 unsupported CRS methods, #46 partial-extent interior holes, #47 catalog re-upsert/`PRAGMA optimize`, #48 name index path memo and snapshot v2, #49 EPSG sweep in CI, #50 core `LayerRecord::validate` and non-finite raster floats, #52 hard-link-only changes not reported by `changes`, #53 `$BITMAP` continuation / 4Kn hardware test. #29 got a comment (the release step now runs the two count-only enumerations and asserts no loss) and stays open for 4.3. Items already owned by a planned session (3.1–3.3, 4.1–4.3) went into the wave 2 hand-offs in the stage plan. A new `stage02` milestone holds the items beyond M0. #25 stays open.
- **Process lessons:**
  - API rate limits stopped all running agents several times; they were resumed with their context and no work was lost.
  - Sub-agents now run on Sonnet 5.5, at the owner's request (2026-10-27).
  - A PR whose CI never starts usually conflicts with `develop`; rebase before waiting on checks.
  - The merge order was relaxed for PRs that share only `Cargo.lock`: merge as they become ready, rebase the rest.
- **Open:** wave 3 opens now that G2 is a go and wave 2 is complete. Q-16 waits on the owner's D: runs (cold raw, cold USN, per 2.7's follow-up instructions in #31) and is decided in 4.3. Q-18 numbers are accumulating in `06`.

---

## Thread 13 — stage01 wave 3 review, merge and wave 4 hand-off (2026-11-08)
- **Request (owner):** merge wave 3 properly, then start wave 4 with parallel sub-agents in isolated worktrees.
- **Merged in watch-point order:** [3.1 / #57](https://github.com/GeoScenarioScripter/whereabouts/pull/57) (V2, `47a1770`) → [3.2 / #59](https://github.com/GeoScenarioScripter/whereabouts/pull/59) (V2, `73e1588`) → [3.3 / #58](https://github.com/GeoScenarioScripter/whereabouts/pull/58) (V3, `0e5ba34`), rebasing and obtaining fresh CI between merges. Session issues #11–#13 closed explicitly.
- **Final-head evidence:** [GeoPackage CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42557522475), [GeoTIFF CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42558523773), [pipeline CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42559141794) all passed Windows/Linux clippy/tests, rustfmt and cargo-deny. Pipeline CI additionally passed the elevated four-dataset SHP/GPKG/TIFF VHD test (golden facts, EPSG/WGS84 presence, restart, isolated `.prj` edit and idle restart). No V4 claim is made.
- **Review fixes:** GeoPackage now accepts inactive SQLite PERSIST rollback journals (zeroed headers), with a regression that failed before the fix. GeoTIFF honors signed pixel scales for flipped rasters, with four-sign-combination and nonzero-tiepoint regression coverage. GeoKeys now route through the common CRS resolver instead of an adapter-only path.
- **Pipeline integration:** authorized the CI scope addition needed to run elevated CLI/indexer VHD tests. Acceptance covers the combined readers, restart without enumeration, and one-dataset `.prj` re-extraction. Required V3 evidence is collected before promoting and merging the pipeline PR.
- **Proposed decisions:** bounded native SQLite page reads and transaction deferral → D-44; GeoTIFF caps and raw GeoKeys JSON → D-45; durable retry ledger and conservative mismatch recovery → D-46. No frozen core/source contract changes. Q-14 stays resolved (D-34); Q-16/Q-18 stay Open for wave 4.3 evidence and owner decisions.
- **Follow-ups:** #60 broader bounded SQLite schema support (stage02), #61 committed GPKG/TIFF edge cases (stage01/4.3), #62 additional TIFF georeferencing sources (stage02). Existing #28/#43/#44 corpus assignments are explicitly included in 4.3's write scope; corpus drift checks remain required.
- **Wave 4 ownership:** 4.1 owns query/CLI and consumes the snapshot plus shared extraction; 4.2 owns indexer and NTFS tests, including retry-ledger/crash paths; 4.3 owns oracle/corpus, benchmarks and benchmark scripts. Merge order remains 4.2 → 4.1 → 4.3. No additional gate is defined between waves 3 and 4.

---

## Thread 14 — stage01 wave 4 engineering close (2026-11-09)
- **Request (owner):** close the gaps, merge 4.2 → 4.1 → 4.3 and finish wave 4. The orchestrator continued independent reviews and authorized the narrow cross-crate fixes needed for complete integration.
- **Resilience:** PR #65 independently reviewed with no remaining findings; real tiny-journal wrap/recreation, synchronized killed-child scenarios, nine persistent-state faults, retry preservation, raw-USN no-write checks and hard-link behavior are evidenced at V3. Merge `43d35e6`; final-head CI: <https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42563077533>.
- **Query merge:** PR #66 merged after #65 as `d80ba38`; final-head CI <https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42570083707> passed Windows/Linux, rustfmt, deny and elevated acceptance. Issue #14 closed explicitly.
- **Query review:** checking the source checkpoint alone cannot detect metadata-only retry commits. The additive catalog read-snapshot API keeps checkpoint matching, filter selection and returned rows in one view; nested/error/panic cleanup and concurrent-writer regressions cover it (D-47). A larger 100k-dataset benchmark exposed seconds of work from full layer hydration before limit; lightweight summaries now support ranking first and loading only selected records. Baseline and updated measurements are recorded separately in the 4.1 note: at 100k GIS datasets the broad median fell from about 6.4 s to 0.19 s, and empty-query median from about 1.34 s to 0.31 s (five samples, diagnostic warm queries; not V4 or an instant-search acceptance claim).
- **Benchmark/oracle merge:** PR #67 merged after #66 as `c957017`, completing #65 → #66 → #67. Exact reviewed head `f0b27d6` passed [Windows/Linux and elevated CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42571127730) and [pinned GDAL regeneration](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42571127732). All measured strict checks pass across 62 committed datasets (one optional external case unfetched); session 4.3 reaches V2. Session issues #14–#16 and completed coverage follow-ups #28/#40/#41/#43/#44/#61 closed explicitly. #42 (CP437 tables), #47/#48 (remaining performance work) and #68 (V4) remain open.
- **Oracle/corpus review:** authorized explicit corpus-expectation updates rather than weakening tests. Review also corrected nearest-rank quantile indexing and a raster check that could overlook missing native resolution. Lone DBF and MultiPatch coverage close their previously reported fixture gaps. Final inspection rejected dummy upstream SBN/SBX sentinels; they were replaced with the small pinned CoHI_GCS12 dataset, whose actual index headers, counts, bounds, upstream spatial-filter tests and public-domain USDA/Census provenance were independently checked. Attribution is retained with the fixture. MultiPatch's over-specific PolyhedralSurface label is corrected to the unknown declared-layer convention, with semantic Geometry/Unknown naming normalization and unchanged strict geometry gates (D-49).
- **Decisions:** D-47 (query consistency/identity conventions), D-48 (separate component budgets and unverified owner targets), D-49 (MultiPatch declared layer) are Proposed. Existing core/source type signatures and storage schema are unchanged; catalog read-snapshot/summary methods are explicitly flagged API additions. Q-11/Q-17 remain deferred and Q-16/Q-18 remain Open; Q-14's agreed thresholds are enforced through shared extraction.
- **Verification boundary:** the owner confirmed no existing V4 logs. The session is not Windows-elevated and found no Everything CLI/process or populated benchmark catalog in checked locations; boot-separated cold scans cannot be inferred from current runs. The original 4.3 V4 obligation is explicitly carried to 5.1 / [#68](https://github.com/GeoScenarioScripter/whereabouts/issues/68). This closes software delivery, not real-machine acceptance. Stage exit requirements are unchanged; no V4 pass, enumeration-default decision or Q-18 target approval is claimed.
- **Next:** stage01/5.1 verifies the full exit checklist, collects owner evidence, resolves Q-16/Q-18 with the owner and prepares the stage retro. No additional owner gate is defined immediately after wave 4. `main` and the stage release remain untouched until stage closure.

---

## Thread 15 — M0 closure disposition and stage02 handoffs (2026-11-09)

- **Owner direction:** defer six cold-boot scans, retain USN, carry CLI startup performance into resident-service work, and investigate the file-path count difference. After investigation and timing implementation, the owner requested closure-doc sync, reviewed integration and finalized handoffs; release is a separate step.
- **Evidence:** 5.1 owner index/restart and Everything comparison are recorded. PR #70 head `0440a41` passed [final-head Windows/Linux, elevated acceptance and policy checks](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42579220164), then merged as `6cd518a`. The timing follow-up isolates approximately 500 ms snapshot load/reconstruction versus 5 ms search.
- **Count investigation:** 1,454,226 additional Everything paths resolve to snapshot identities; 1,454,224 have multiple current hard links, two may be renames. All 1,836 absent identities have post-snapshot creation times; three snapshot paths are now deleted and four entries are NTFS metadata orphans. Live observations account for the aggregate gap but do not prove atomic parity.
- **Decision:** D-50 records owner-accepted M0 gaps. Q-16's current default and Q-18's M0 disposition are resolved; future method selection and numeric M1 budgets remain Open. Stage02 #68/#72/#48/#47/#52 retain concrete acceptance criteria in [the handoff](stages/stage01/STAGE02-HANDOFF.md).
- **Boundary:** final integration/CI is tracked separately from owner acceptance. No main merge, release tag, prerelease or stage milestone closure is claimed by this docs sync.

---

## Thread 16 — Release-audit raster validation (#50, 2026-11-09)

- **Owner direction:** fix #50 before release; use Bash for GitHub access. Live issue inspection confirmed the requested core contract change and stage01 milestone.
- **Contract change:** D-51 proposes rejecting non-finite raster resolutions and geotransform components in `LayerRecord::validate`, replacing D-41's silent store-as-absent behavior. Nodata text such as `"nan"` remains valid.
- **Dependants:** catalog writes stop filtering these values; the shared extractor converts invalid facts to a dataset-local error before the catalog transaction. A real TIFF regression covers finite coefficients/corners whose derived resolution overflows, continued indexing of other datasets, and restart persistence.
- **Release boundary:** #50 can close after the fix is reviewed, final-head CI passes and the PR lands in develop. This change does not release M0, waive other release checks, or close the milestone. Verification is recorded in [the fix note](stages/stage01/sessions/issue-50-raster-validation.md).

---

## Suggested next steps (from the discussion — superseded by the stage01 plan)
- Scaffold the Rust workspace and M0 CLI indexer (ntfs, nameindex, datasets, readers for SHP/GPKG/GeoTIFF, crs, catalog, GDAL-diff harness).
- Sketch the local tile server (Tauri custom protocol, MVT encoder, tile cache schema).
- Run name availability checks for Whereabouts.

- **Integration and issue reconciliation:** PR #71 head `f0dfb15` passed [final-head CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42582146914) and merged as `8f14a45`, following PR #70 (`6cd518a`). #26 (ingestion measurement) and #29 (release-enumeration CI) closed with existing evidence; #25 moved to its original stage02 scope. #50 remains an outstanding release-audit item, outside the three accepted gaps. This documentation PR is the final closure-sync integration gate; no main merge, tag, release or stage-milestone closure was performed.
