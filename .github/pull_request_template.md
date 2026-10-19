## Summary

<!-- One or two sentences: what this PR does and why. -->

## Session

<!-- For stage/wave/session work (docs/08-development-workflow.md). Delete for ad-hoc PRs. -->

Closes #
- Session: `stage01/W.N` — <theme>
- Session note: `docs/stages/stage01/sessions/W.N-<theme>.md`
- Verification level reached: V1 / V2 / V3 / V4 (see docs/08-development-workflow.md §5)
- Contract change (frozen types/traits modified)? no / yes — describe and list updated dependants

## Type of change

- [ ] Feature
- [ ] Bug fix
- [ ] Refactor (no behaviour change)
- [ ] Performance
- [ ] Tests / corpus
- [ ] Docs / decisions
- [ ] Build / CI

## Changes

-

## Decisions

<!-- New or changed decisions must be appended to docs/02-decisions.md (status Proposed) and noted
     in docs/07-discussion-log.md. Did you touch anything listed in docs/06-open-questions.md? -->

- [ ] No decision changes
- [ ] Decision(s) added/updated: D-

## Verification

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Oracle diff (if readers/CRS touched): agreement rate unchanged or better
- [ ] Manual / elevated checks (describe):

## Guardrails

- [ ] No GDAL linked into any crate (D-07)
- [ ] Results are datasets, not files (D-04)
- [ ] No network/cloud/PostgreSQL code paths in v1 (D-03, D-11)
- [ ] Hot path (typing → results) not made slower, or the cost is justified below

## Notes for reviewers

-
