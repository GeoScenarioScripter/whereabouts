# Stage01 closure record — owner-accepted gaps; release pending

Updated 2026-11-09. The historical filename is retained for existing links. Owner
acceptance is recorded in D-50; this record does not claim the release is complete.

## Accepted disposition

1. **Q-16:** retain USN as the current default. Defer six owner-coordinated
   boot-separated scans (three USN, three raw MFT, with warm repeats) to stage02/#68.
   Current timings are uncontrolled/warm diagnostics, not cold evidence.
2. **Q-18:** accept one-shot CLI startup as a known M0 gap. Stage02/#72 owns
   resident-service/IPC latency and explicit numeric budgets, with #48/#47 covering
   snapshot/name and catalog optimization. The instant-search criterion remains
   unmet; D-48's proposed budgets are not approved by this disposition.
3. **Path coverage:** the requested investigation explains the large numerical gap
   predominantly through additional hard-link paths to already indexed identities.
   Complete multi-name support remains stage02/#52; exact simultaneous path parity
   is not claimed. Raw MFT also keeps one name per FRN and is not a fix for this gap.

## Evidence and integration

- Owner index/restart: approximately 2.866M names and 5,209 datasets, no restart
  rescan or extractions; [session 5.1 checklist](sessions/5.1-exit-verification.md).
- PR #70 head `0440a41` passed [final-head CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42579220164), including Windows/Linux, elevated VHD/NTFS acceptance, fmt and dependency policy. It merged into develop as `6cd518a`.
- Strict corpus agreement: 100% over 62 committed datasets; optional external case
  excluded explicitly. Private owner datasets were not independently GDAL-validated.
- [Query timing and path investigation](sessions/m0-query-timings-followup.md):
  approximately 500 ms snapshot load/reconstruction and 5 ms search. Opt-in
  `query --timings` keeps results on stdout and emits phase JSON on stderr.
- Path reconciliation: 2,410,863 matching paths; 1,454,226 extra paths with known
  identities (1,454,224 currently multi-link, two possible renames); 1,836 absent
  identities all with post-snapshot creation times; three deleted snapshot paths;
  four orphaned NTFS transaction metadata records. Live observations are non-atomic.
- Timing PR #71 head `f0dfb15` passed [final-head CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42582146914), including Windows/Linux and elevated tests, and merged as `8f14a45`.
- This documentation update completes the closure sync and handoffs when merged
  after final-head CI. Release audit and the separate release sequence remain;
  the evidence and timing implementation are already integrated into develop.

## Stage02 handoffs

[STAGE02-HANDOFF.md](STAGE02-HANDOFF.md) records issue ownership, evidence,
acceptance criteria and unresolved future budgets/contracts. The handoffs are
accepted M0 gaps, not silently weakened tests or proof of completeness.

## Documentation and retro

D-50 and Q-16/Q-18 record the current disposition; architecture preserves USN and
per-process CLI loading; roadmap and stage index distinguish delivered prototype
from release. The retro is in [PLAN.md](PLAN.md#stage-retro). Stage02 remains a
sketch until the separate stage planning step.

## Remaining milestone audit

The three accepted gaps are not blanket acceptance of every open issue. Existing
#25 mount/GUID volume support is outside M0 and has moved to stage02. #26 borrowed
record ingestion measurement and #29 release-enumeration CI are closed with existing
session/CI evidence; no borrowed-record contract change was adopted.
#50 non-finite `RasterInfo` validation is addressed by the D-51 core/catalog/indexer
contract fix ([verification note](sessions/issue-50-raster-validation.md)). Close
the issue only after reviewed integration into develop and green final-head CI.
It is not an accepted gap. Audit remaining stage01 issues before closing the milestone.

## Separate release sequence (not performed by this work)

After final-head CI and all session/docs PRs merge into develop, open the
develop→main release PR; merge with a merge commit after green CI; tag v0.1.0,
create the prerelease, close the stage01 milestone, then plan stage02. No tag,
main merge or milestone closure is authorized by this documentation-only release
preparation. The owner's current request covers documentation, integration into
develop and handoffs.

## Prepared manual cold-run helper
`docs/benchmarks/run-owner-boot.ps1` checks elevation and reserves one measurement per boot in a local ledger. It does not reboot, schedule tasks, build, or alter a journal. Prebuild executables before the first reboot. After each separate owner-controlled reboot, before opening GIS directories or compiling, run one command in an elevated shell:

```powershell
# Three pairs, each command on a different boot:
./docs/benchmarks/run-owner-boot.ps1 -Method usn -Repeat 1
./docs/benchmarks/run-owner-boot.ps1 -Method mft -Repeat 1
./docs/benchmarks/run-owner-boot.ps1 -Method usn -Repeat 2
./docs/benchmarks/run-owner-boot.ps1 -Method mft -Repeat 2
./docs/benchmarks/run-owner-boot.ps1 -Method usn -Repeat 3
./docs/benchmarks/run-owner-boot.ps1 -Method mft -Repeat 3
```

Each call saves first/repeat outputs to a unique `target/v4-boot-<method>-<repeat>` directory. The helper checks boot uniqueness, not actual absence of earlier disk activity; the operator must record that condition. Failed attempts reserve the boot too. The owner deferred these runs to stage02/#68; the helper is retained for that work. It is not an M0 closure prerequisite under D-50.
