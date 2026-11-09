# Accepted M0 gaps → stage02 handoff

Owner-approved 2026-11-09 (D-50). This is input to the stage02 sketch and session
planning, not a new implementation plan or a declaration that M0 is released.

## Resident-service performance — #72, supported by #48 and #47

[Issue #72](https://github.com/GeoScenarioScripter/whereabouts/issues/72) owns the
resident-state and end-to-end measurement work. #48 owns snapshot/name-index
optimization and #47 catalog costs. Assign concrete sessions when stage02 is planned.

Baseline: [phase measurements](../../benchmarks/m0-query-timings.json) show about
500 ms snapshot loading/reconstruction and 5 ms search on the existing owner
snapshot. This is a warm diagnostic, not a cold-start or instant-search pass.

Acceptance:
- Keep the name index, catalog access and dedicated query pool alive across requests;
  demonstrate that repeated requests do not reconstruct the snapshot or pool.
- Preserve a fresh consistent catalog read snapshot per query (D-47); never hold one
  SQLite read transaction for the lifetime of the service. Specify update/query
  concurrency and IPC contracts in stage02 rather than choosing them here.
- Measure service startup/readiness, reused search, IPC-to-result latency and total
  process/pipeline memory separately on the same terms, limits and dataset scope.
  Include owner-machine samples and state cache/background-load conditions.
- Obtain explicit numeric budget agreement before performance acceptance. D-48's
  candidate component budgets remain Proposed; no new numbers are imposed here.
- Profile snapshot reconstruction before bulk-loader changes; retain checksum,
  encoding, duplicate-identity and bounded-allocation validation. Report results
  and regressions for #48/#47 independently from service reuse gains.

## Complete hard-link path coverage — #52

[Issue #52](https://github.com/GeoScenarioScripter/whereabouts/issues/52) owns
multi-name support. [Investigation](sessions/m0-query-timings-followup.md) accounts
for the large count gap predominantly as extra hard-link paths, not absent file
identities. Both current enumerators select one name per FRN.

Acceptance:
- Specify the explicit contract change across source, name index, grouper and
  snapshot persistence for multiple `(parent, name)` links per identity.
- Preserve D-27 dataset identities and sidecar grouping; do not surface sidecars as
  standalone spatial results or accidentally duplicate a dataset through aliases.
- Enumerate supported long-name links and process link creation/deletion, including
  removal of the selected name when other links survive.
- Use isolated-volume tests for alternate-name search, link changes, restart and
  grouping/deduplication of linked anchors and sidecars.
- Compare path sets using matched inclusion rules and recorded observation windows.
  Classify churn, metadata and unresolvable parents separately. Equal totals alone
  do not establish parity; the M0 non-atomic reconciliation is not a substitute.

## Deferred cold enumeration evidence — #68

[Issue #68](https://github.com/GeoScenarioScripter/whereabouts/issues/68) owns the
six deferred scans: three boots per method, first scan and warm repeat, keeping USN
as the current default until a separately reviewed choice is supported.

Acceptance:
- Build in advance; coordinate six owner-controlled boots and record preceding disk
  activity, boot IDs, hardware/volume mapping, cache labels and background work.
- Retain every scan/repeat output and reconcile unexpected record-set differences;
  do not confuse file records with paths or datasets.
- Use the existing manual helper; no automatic reboots or journal/volume changes.
- Present USN/raw cold/warm evidence and tradeoffs for explicit default-selection
  review. A raw-first proposal does not change the default by itself.

## Other existing scope

#25 mount/GUID volume support was already outside M0 and is a stage02 handoff.
The three gaps above do not waive unrelated open issues. In particular #50
non-finite `RasterInfo` validation has a D-51 contract fix with a
[verification note](sessions/issue-50-raster-validation.md); it requires reviewed
integration and green final-head CI before closure, rather than a stage02 deferral;
#26/#29 are closed with the completed ingestion measurement and release-enumeration
CI evidence. Their closure does not approve a new source contract.

## Closure boundary

These items are accepted carry-forwards under D-50. Existing correctness and final-head
CI obligations remain; no universal file-path parity, cold-performance result or
instant CLI search is claimed. Stage02 must retain these issues when assigning
sessions, while release/main/tag/milestone actions remain a separate M0 step.
