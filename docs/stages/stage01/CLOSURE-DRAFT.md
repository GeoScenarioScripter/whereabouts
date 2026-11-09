# Stage01 closure draft — pending owner decisions

This is reviewable closure preparation, not a claim that stage01 is closed.

## Evidence ready
- Session 5.1 and raw benchmark evidence demonstrate real-volume index/restart, native metadata status reporting and measured CLI behavior.
- PR #70 head 23e987a passed Windows/Linux tests, elevated NTFS/VHD acceptance, fmt and dependency policy: https://github.com/GeoScenarioScripter/whereabouts/actions/runs/42576242490
- Corpus strict agreement remains 100% over 62 committed datasets; optional external case excluded explicitly.
- Existing architecture doc already describes implemented catalog-first checkpoint recovery, extraction retry ledger and per-process query snapshot load.

## Decisions needed before close
1. Q-16: retain USN as current default and carry boot-separated USN/raw selection experiments into stage02 (#68), or collect six owner-coordinated boot runs before closure. Current evidence is uncontrolled, not cold.
2. Q-18: accept CLI startup latency as a known M0 gap and require resident service/IPC measurements in M1, with budgets chosen explicitly; or keep M0 open until the instant-search criterion is met. Existing #48 tracks snapshot/path performance; #47 tracks catalog costs.
3. Coverage: Everything paths and Whereabouts file records are not equal sets. D: directory counts are close but file-path counts differ substantially; raw scan identifies many hard-linked records. Hard-link completeness is already M1 scope (#52), but this run does not quantitatively reconcile every path. Accept explicit path-parity investigation as a carry-forward or investigate before closure.

No gap above is accepted merely by this draft. Q-14 stays resolved; Q-16/Q-18 remain Open until owner response.

## Prepared documentation changes after decision
- PLAN: attach evidence checklist, mark 5.1 completed only after PR merge, append retro below; label accepted gaps individually.
- Decisions/questions: record owner's Q-16/Q-18 choices, with actual measurement caveats; avoid declaring unmet speed target passed.
- Architecture: preserve USN default unless a separate implementation PR changes it; emphasize resident service is M1 and CLI reloads snapshot per invocation.
- Roadmap: check M0 implementation deliverables, annotate accepted performance/completeness gaps separately.
- Stage index/README: mark M0 complete with accepted gaps only after explicit acceptance and release; activate stage02 sketch.
- Discussion log: record owner decision, evidence links, accepted gaps and next-stage remediation.

## Retro ready to apply
**Keep:** contracts and crate boundaries; bounded readers and immutable data access; explicit V1–V4 levels; elevated isolated VHD resilience tests; raw benchmark outputs.

**Change:** prepare Rust, release executables and Everything/ES before owner V4; allocate reboot windows ahead of closure; benchmark while builds are idle; distinguish file records, hard-link paths and datasets; separate resident query and CLI startup targets.

**Plan deltas:** G1 added 2.7 raw MFT exploration; wave 4 transferred owner V4 to 5.1 without waiving exit criteria; 5.1 surfaced real CLI startup cost and incomplete path parity.

**Carried over (pending acceptance):** #68 boot-separated enumeration and Q-16; #48/#47 service-era performance work for Q-18; #52 hard-link completeness. Other already-open CRS/reader follow-ups remain unchanged.

**Numbers:** 18 numbered implementation/verification sessions across waves 1–5 plus bootstrap; Oct 19–Nov 9 (21 calendar days elapsed, not continuous work); total CI minutes unknown. Actual owner index: 2.866M names, 5,209 datasets. Oracle 62 committed cases. Benchmark tables contain actual samples rather than extrapolated performance claims.

## Release sequence after decisions
Finish docs sync on `docs/s01-close`, require CI, merge session/docs PRs into develop; open develop→main release PR, merge with merge commit after green CI; tag v0.1.0, create prerelease, close stage01 milestone; run plan-stage for stage02. Do not release while acceptance remains unresolved.


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

Each call saves first/repeat outputs to a unique `target/v4-boot-<method>-<repeat>` directory. The helper checks boot uniqueness, not actual absence of earlier disk activity; the operator must record that condition. Failed attempts reserve the boot too. This is ready if the owner chooses evidence completion rather than explicit carry-forward.
