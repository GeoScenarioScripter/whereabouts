# Stage01 benchmark evidence

Status: local synthetic/component evidence; **V4 acceptance is not met**. No new owner-volume
cold/warm enumeration or Everything comparison has been run in this session. Q-16 and Q-18
remain Open pending those measurements and an owner decision. Q-14 is already Agreed in D-34.
The orchestrator explicitly transferred owner V4 collection to session 5.1, preserving the
stage-exit requirement. See the [exact preparation and run commands](stage01-v4-handoff.md).

## Reproduce

Use a release build, record the commit, CPU, RAM, storage model, power mode and idle conditions.
Do not compare debug CI times with release workstation times. The script writes a new output
directory, leaves indexed datasets untouched and does not create/delete a journal or a VHD.

```powershell
pwsh -File scripts/bench/bench-vs-everything.ps1 -PrepareOnly -Output target/v4-preparation
# Separate boots are necessary: run USN first after one reboot, raw MFT first after another.
pwsh -File scripts/bench/bench-vs-everything.ps1 -Volume D: -Enumeration usn -CacheState first-after-reboot -EnumerationOnly -SkipBuild -Output evidence-usn
# Reboot before this separate run; the script never reboots the machine itself.
pwsh -File scripts/bench/bench-vs-everything.ps1 -Volume D: -Enumeration mft -CacheState first-after-reboot -EnumerationOnly -SkipBuild -Output evidence-mft
# After session 4.1 has merged; use the existing owner catalog/snapshot and an idle Everything index.
pwsh -File scripts/bench/bench-vs-everything.ps1 -Volume D: -Database D:/bench/catalog.sqlite -Everything C:/Tools/es.exe -SkipBuild -SkipComponents -Output evidence-query
```

Enumeration requires an elevated terminal and is strictly read-only. Each run retains the
first scan and immediate warm repeat separately. `first-after-reboot` is an operator label,
not a claim of a purged cache: record boot time and background activity. Run both methods
three times in separate boots to judge variance. Neither the script nor this document
chooses the enumeration default.

ES flags follow the [official command-line reference](https://www.voidtools.com/support/everything/command_line_interface/).
Both CLIs limit output to 100 and discard rendering output during timing. Everything searches
files/folders in the specified volume; Whereabouts returns grouped datasets. These are useful
end-to-end user timings, not equivalent result sets or an algorithm-only comparison. Everything
already has a running resident service, whereas each Whereabouts CLI invocation loads its
snapshot/catalog. Record Everything version, index scope and background indexing state.
Missing `es.exe` is reported as not measured, never a zero-latency result. `query-samples.csv`
preserves raw samples and `query-summary.csv` uses nearest-rank p50/p95/p99 by exact term/length.

The component commands can run without elevation or Everything:

```powershell
cargo run --release -p whereabouts-nameindex --example measure -- 1000000
cargo run --release -p whereabouts-catalog --example bulk_insert -- 100000 1000
cargo run --release -p whereabouts-oracle --example pipeline_costs -- 1000000
cargo test -p whereabouts-oracle --test agreement
```

`measure` reports exact name-index heap bytes, in-memory snapshot cost and name scan percentiles
by term length. `pipeline_costs` adds the actual grouping tree/grouper, temporary fsynced snapshot
save and warm reload, fresh-thread versus warm CRS cache extraction and 1/2/4/8-worker throughput.
Fresh-thread timing starts inside the thread and excludes spawn cost; file-cache effects remain.
Worker runs use repeated corpus files with warm OS caches, not independent disks or real datasets.
Windows memory counters are sampled through `ntfs::process_memory`; process private-byte differences
include allocator overhead and retained capacity, not exact Rust object sizes. Compare name-only,
name+tree, and name+tree+grouper on the same run. Linux builds print memory unavailable and need
external RSS tooling. Catalog throughput includes per-batch transactions in an isolated temp DB.

## Previous enumeration evidence (not a new run)

| Source | USN first access | USN warm | raw MFT | Conditions |
|---|---:|---:|---:|---|
| 1.3 owner C: | 4.86 s | not recorded | not measured | 812,684 records, release NVMe |
| 1.3 owner D: | 40.98 s | not recorded | not measured | 1,442,867 records, release NVMe |
| 2.7 elevated CI C: | 36–62 s | 2.0–2.7 s | about 6 s cold/warm | 1.35M records, 1.34 GB MFT, throttled cloud disk |

Sources: session notes [1.3](../stages/stage01/sessions/1.3-ntfs-enumeration.md) and
[2.7](../stages/stage01/sessions/2.7-ntfs-mft-reader.md). The owner D: warm cost is still unknown.

## Oracle agreement

62 committed datasets, one optional unfetched external case. GDAL 3.13.3 / PROJ 9.8.1, unchanged
D-33 tolerances (native 1e-6 absolute or 1e-9 relative; WGS84 0.01 degrees with span check).

| Format | status | layer sets | geometry | counts | native bbox | raster | EPSG | WGS84 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Shapefile | 31/31 | 27/27 | 26/26 | 27/27 | 26/26 | n/a | 26/26 | 26/26 |
| GeoPackage | 14/14 | 12/12 | 11/11 | 13/13 | 14/14 | not extracted | 14/14 | 14/14 |
| GeoTIFF | 17/17 | 15/15 | n/a | n/a | 15/15 | 15/15 | 15/15 | 15/15 |

Rates include absent CRS/extent matching absent expected values. Corrupt files are status-only;
feature and spatial counts are per layer, not per dataset. The report retains that distinction.
All adapters use the shared extraction path; measured native and CRS rates are frozen at 1.0
per format. The global EPSG/WGS84 floor is 0.98 as required by D-34. Fixture additions and remaining
coverage are listed in [WAVE4-COVERAGE](../../tests/corpus/WAVE4-COVERAGE.md).

## Proposed decisions

Q-16: carry forward the evidence-based candidate of raw MFT for a first cold scan with strict
USN fallback, USN for changes. Do **not** adopt it yet: cold/warm owner D: measurements and
raw/USN count agreement are still needed. The current default remains unchanged.

Q-18: use name-only compacted heap and warm scan percentiles as component budgets, and report
CLI startup separately. A name-index target around 100 B/entry and p95 below 50 ms for terms
of at least two characters at 5M entries is a candidate based on session 2.1's 90–92 B/entry
and 2–5 ms median measurements. These are proposals, not accepted targets; this session's local
run and Everything comparison must be reviewed first. Grouping, extraction caches and catalog
memory must be reported separately from the compact name index. A total-process memory target
and CLI latency target require the owner-volume run.

## Local component run (2026-11-08)

Intel Core Ultra 7 258V, 33,796,481,024 bytes RAM, WD PC SN560 SDDPNQE-1T00-1102 SSD, Windows 10.0.26300; Rust 1.99.1 MSVC, release, `RAYON_NUM_THREADS=8`, 1,000,000 synthetic name records;
100,000 synthetic catalog datasets in batches of 1,000. This shared workstation was also
running parallel session builds; the catalog run overlapped the pipeline rerun. These are
reproducible diagnostic measurements, not uncontended performance acceptance or V4 evidence.
Raw outputs: [names](stage01-local-nameindex.txt), [pipeline](stage01-local-pipeline.txt),
[catalog](stage01-local-catalog.txt). A sandboxed snapshot rename was denied; the recorded
pipeline run is the successful rerun outside the sandbox using a fresh temporary directory.

Historical quantile estimators are retained honestly: this stored pipeline run used sorted
index `ceil((N-1)*p)`; stored nameindex and catalog rows used `round((N-1)*p)`. Raw individual
samples were not retained for recalculation. New pipeline and comparison-script runs use
nearest rank `max(0, ceil(N*p)-1)`, with 20- and 50-sample boundary tests. These historical
rows must not be described as nearest-rank results.

| Component | Observed |
|---|---:|
| Name insert from owned records | 0.21 s, 4.73M records/s |
| Compacted name-index heap | 89,787,104 bytes; 89.79 B/entry |
| Snapshot serialized size | 45,763,657 bytes; 45.76 B/entry |
| In-memory snapshot write / load | 158 / 244 ms |
| Snapshot fsync+rename save / warm file load | 300 / 390 ms |
| Group tree build (includes record regeneration) | 1,466 ms |
| Group full scan | 1,671 ms; 18,873 datasets |
| Process private bytes, initial / names / names+tree / names+tree+grouper | 0.72 / 91.61 / 248.94 / 274.51 MB |
| Group tree incremental private bytes | about 157.33 MB per million records |
| Grouper incremental private bytes | about 25.57 MB; 18,873 datasets |
| Catalog first insert / same-record upsert | 4,867 / 4,310 datasets/s |
| Catalog bbox p50 / p95 / p99 | 0.163 / 0.361 / 0.601 ms |
| Catalog DB / WAL | 65.1 / 8.1 MiB |

| Name substring length | p50 ms | p95 ms | p99 ms |
|---|---:|---:|---:|
| 1 | 16.07 | 36.12 | 54.78 |
| 2 | 2.46 | 7.52 | 15.22 |
| 3 | 1.60 | 4.08 | 5.57 |
| 4 | 1.49 | 2.88 | 3.60 |
| 5 | 1.85 | 4.12 | 8.51 |
| 6 | 2.15 | 5.64 | 7.75 |
| 7 | 1.12 | 2.60 | 3.57 |
| 8 | 1.00 | 2.83 | 8.36 |

| Shared extraction | fresh-thread p50 / p95 / p99 ms | warm p50 / p95 / p99 ms |
|---|---:|---:|
| GeoPackage | 0.767 / 28.662 / 28.662 | 0.256 / 0.747 / 1.453 |
| GeoTIFF | 0.462 / 3.176 / 3.176 | 0.130 / 0.597 / 2.169 |
| Shapefile | 0.638 / 1.532 / 2.423 | 0.323 / 0.974 / 2.295 |

5,100 shared extraction jobs: 1 / 2 / 4 / 8 workers produced approximately
2,249 / 5,187 / 7,463 / 12,479 datasets/s. This mixes formats and repeatedly reads 51 usable
corpus datasets. Fresh-thread versus warm figures include file-cache and thread-local cache
effects; they do not isolate CRS preparation. The frozen cache is capped at 128 definitions
and 1 MiB of definition text per worker, plus prepared transformer structures. These measurements
show the practical cost without adding a production cache-control API.

