# M0 closure follow-up: query timings and path coverage

Date: 2026-11-09. Ad-hoc follow-up on `codex/m0-query-timings`, based on
`dffec7c`. Scope: CLI diagnostics/tests and this handoff. No frozen contracts,
enumeration defaults, schemas, or stage acceptance criteria change.

## Query diagnostics

Run `whereabouts query road --db <catalog> --json --timings`.
Query results remain on stdout; successful queries append one compact JSON object
to stderr with `kind: "query_timings"`, `phases_ms`, and `query_total_ms`.
Normal invocations take no phase clock readings and allocate no phase entries.
Existing tracing output may also appear on stderr when enabled through `RUST_LOG`.
Failed queries retain their usual errors and do not emit a success timing report.

The intervals are consecutive and include small instrumentation overhead:

| Phase | Work included |
|---|---|
| `snapshot_load` | Snapshot open/read, checksum and structural validation, in-memory reconstruction |
| `catalog_open` | Read-only catalog opening |
| `checkpoint_validation` | Checkpoint presence, entering the catalog read snapshot, volume/checkpoint selection |
| `pool_init` | Dedicated query engine / Rayon pool construction |
| `search` | Name/catalog filtering, ranking and result hydration |
| `query_cleanup` | Query engine destruction and leaving the catalog read snapshot |
| `output` | Serialization/table formatting and stdout flush |
| `teardown` | Dropping results, catalog and name snapshot |

`query_total_ms` is the sum of these intervals. It excludes process launch,
tracing setup, argument parsing, diagnostic serialization/emission, and final
process exit. It is neither startup-to-first-result nor resident query latency.
Snapshot I/O versus reconstruction is not separately instrumented.

## Resident-service handoff (stage02)

Keep the name index, catalog access and dedicated query pool alive across requests.
Preserve D-47's consistent per-query read snapshot; do not keep one SQLite read
transaction open for the lifetime of the service. Coordinate index updates and
query access under the stage02 concurrency/IPC design rather than choosing that
contract in this follow-up.

Acceptance for the service work:

- Record service startup/readiness separately from request-to-result latency.
- Measure repeated requests against one loaded index and pool; demonstrate no
  per-request snapshot reconstruction or pool creation.
- Include IPC and output costs in end-to-end measurements and retain separate
  search-engine measurements. Use the same corpus, terms and limits as the CLI.
- Profile snapshot reconstruction before considering a validated bulk loader.
  Preserve bounded allocation, checksum, encoding and duplicate-FRN checks.

## Hard-link path coverage handoff (stage02 / existing #52)

A read-only comparison on the owner's D: volume reconstructed all file paths from
the saved, checksum-validated snapshot and compared them case-insensitively with
an Everything export. Every unmatched exported path was checked with
`os.stat(..., follow_symlinks=False)` for its NTFS file identity and link count.

| Observation | Count |
|---|---:|
| Snapshot file records | 2,410,870 |
| Everything file paths | 3,866,925 |
| Matching paths | 2,410,863 |
| Additional paths with an identity already in the snapshot | 1,454,226 |
| Of those, current link count greater than one | 1,454,224 |
| Additional paths with an identity absent from the snapshot | 1,836 |
| Snapshot-only paths, now deleted | 3 |
| Snapshot orphan files (NTFS transaction metadata) | 4 |

All 1,836 absent identities had creation timestamps after the saved snapshot;
1,812 were under the repository. Creation time supports live-volume drift but is
not proof of journal history. Two alternate paths had a current link count of one
and could reflect renames. The observations were non-atomic, not a proof of exact
same-instant parity. The aggregate gap is explained predominantly by multiple
hard-link paths per identity, not missing identities. Switching to raw MFT cannot
provide path parity: both current enumeration implementations retain one name.

The full local diagnostic inputs/results are in the main checkout's ignored
`target/investigation-m0/` directory. They contain private file paths and are not
copied into tracked evidence. The benchmark summary above contains aggregates only.

Follow-up acceptance for #52:

- Represent multiple `(parent, name)` links per file identity while preserving
  D-27 dataset identity and sidecar grouping. Treat this as an explicit contract
  change spanning source, name index, grouper and snapshot persistence.
- Enumerate all supported long-name links and reconcile link creation/deletion,
  including deletion of the selected name while other links survive.
- Add isolated-volume tests for alternate-name searches, link changes, restart,
  and dataset deduplication/grouping across linked anchors and sidecars.
- Compare path sets under matched inclusion rules and record observation windows;
  classify churn and NTFS metadata separately instead of requiring totals alone.

USN remains the default. The six boot-separated scans remain deferred by the
owner; these diagnostics do not supply cold-boot evidence or change Q-16/Q-18.

## Verification

Local V1 checks passed: `cargo test -p whereabouts-cli` (8 tests passed, 3
explicitly ignored elevated/measurement tests), `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo build --release -p whereabouts-cli`. The new process-level regression
checks unchanged JSON stdout, opt-in stderr diagnostics, finite nonnegative
durations and phase-total consistency for both zero and normal result limits.
Independent code review found no actionable issues. Initial local verification
did not include Linux or elevated VHD tests. Subsequently, rebased PR #71 head
`f0dfb15` passed [Windows/Linux and elevated final-head CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42582146914)
and merged into develop as `8f14a45`; the final PR verification reaches V3, alongside
the targeted V4 warm diagnostics below.

Targeted V4 warm diagnostics used the existing 120,295,918-byte owner snapshot,
without enumeration. After one discarded warmup per mode, ten samples per mode
alternated diagnostics-on/off order; zero-limit diagnostics followed each pair.
All ten normal-query output pairs were byte-identical. Background activity was
uncontrolled. Raw samples, executable hash and method are in
[`m0-query-timings.json`](../../../benchmarks/m0-query-timings.json).

| Measurement | Median ms |
|---|---:|
| Normal query snapshot load/reconstruction | 499.589 |
| Normal query catalog open | 0.977 |
| Normal query checkpoint validation | 0.299 |
| Normal query pool construction | 0.322 |
| Normal query search | 5.174 |
| Normal query output | 0.355 |
| Normal query teardown | 18.916 |
| Normal query total inside diagnostic scope | 525.550 |
| Normal query process wall time, diagnostics on | 551.953 |
| Normal query process wall time, diagnostics off | 549.933 |
| Zero-limit snapshot load/reconstruction | 509.474 |

Per-phase medians do not sum to the median total. The approximately 2 ms
on/off wall-time difference is a small-sample observation, not a statistically
isolated overhead estimate. The direct phase measurements confirm that snapshot
loading/reconstruction dominates. Reusing the loaded index is the first service
priority; no faster-loader implementation or instant-search acceptance is claimed.
