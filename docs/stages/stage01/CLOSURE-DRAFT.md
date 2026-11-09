# Stage01 closure record — v0.1.0 prerelease

Updated 2026-11-09. The historical filename is retained for existing links.
The owner requested the M0 release after accepting the gaps in D-50.
[Release notes and audit](RELEASE.md) record the delivered CLI, verification,
commands and known limitations. [PLAN.md](PLAN.md#stage-retro) contains the retro.

## Accepted disposition

- Retain USN as the current default; defer six boot-separated scans to stage02/#68.
- Carry one-shot CLI startup and resident-service performance to #72/#48/#47;
  numeric M1 budgets remain Open. Instant CLI startup is not a passing criterion.
- Carry complete hard-link path support to #52. Live path reconciliation explains
  the aggregate gap but does not establish atomic path parity.
- [STAGE02-HANDOFF.md](STAGE02-HANDOFF.md) retains acceptance criteria and evidence.

## Final audit

- All stage01 milestone issues are closed. #25 is assigned to stage02;
  #26/#29 closed with existing measurement/CI evidence.
- Correctness issue #50 closed after PR #74 merged as `8c74046`.
  [Final develop CI](https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42588045021)
  passed Windows/Linux, elevated NTFS/VHD tests, rustfmt and cargo-deny.
- [Owner evidence](sessions/5.1-exit-verification.md): 2,865,960 names and 5,209
  datasets; restart without rescan or extraction. Corpus agreement is 100% over
  62 available committed datasets, with one optional external case skipped.
- Remaining accepted gaps do not waive correctness checks or imply cold-speed,
  universal path parity or instant CLI startup acceptance.

## Release sequence

The owner's release request authorizes documentation integration through develop,
then a green develop-to-main PR merged with a merge commit, annotated v0.1.0 tag,
GitHub prerelease and stage01 milestone closure. Stage02 detailed planning is a
separate follow-up; it remains a sketch.

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

## Release integration

The develop-to-main release PR exposed main's historical CLI-stub formatting
commit (#18). The reconciliation preserves the completed develop CLI verbatim
and merges the main ancestry through a separate integration PR. Production
code and packaged binary inputs remain identical to the audited 8c74046 tree.
