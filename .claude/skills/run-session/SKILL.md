---
name: run-session
description: Execute one planned Whereabouts stage session (e.g. "stage01/2.4") end to end inside its own git worktree — orient, branch, implement within write scope, verify to the required level, write the session note, open a PR to develop. Use when you are a session-worker sub-agent or the user says "run session X.Y".
---

# Run a session

You are executing exactly one session from `docs/stages/stageNN/PLAN.md`. The process is defined in `docs/08-development-workflow.md`; this is the checklist.

## 1. Orient (read, don't skim)
1. `AGENTS.md`.
2. Your session's spec in the stage `PLAN.md` — **Reads**, **Write scope**, **Deliverables**, **Acceptance** (note the required verification level V1–V4), **Out of scope** — plus the wave's *watch points* and the *dependency graph* row.
3. The session notes in `docs/stages/stageNN/sessions/` for every session you depend on (they describe the contracts you consume).
4. The docs your spec lists under **Reads**.
5. Confirm the worktree is clean and based on an up-to-date `develop`: `git status`, `git log -1 --oneline origin/develop`.

## 2. Branch
`git switch -c sNN/wW-N-<theme>` (e.g. `s01/w2-4-crs-epsg-transform`).

## 3. Implement — inside the write scope only
- Paths outside the write scope are off-limits. Exceptions (shared-file rules, workflow §8): you may *add* `[workspace.dependencies]` entries in the root `Cargo.toml` (alphabetical in their group); `Cargo.lock` changes as needed.
- Don't edit orchestrator-owned docs (`PLAN.md` tables, `02-decisions.md`, `03-architecture.md`, `06-open-questions.md`, `07-discussion-log.md`, `docs/stages/README.md`). Put proposals in your session note.
- Frozen contracts (types/traits from earlier waves) don't change silently. If you must change one: stop and explain in your report. Only proceed if the orchestrator told you to, and then update all dependants in the same PR.
- Follow `AGENTS.md` coding rules (bounded reads, no panics on file content, no GDAL, datasets-not-files, `// SAFETY:` comments).
- Something useful but out of scope → write it under *Follow-ups*; don't do it.

## 4. Verify
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Plus whatever the acceptance criteria name (elevated `--ignored` tests, oracle agreement, benchmarks).
- If `cargo` is missing locally (the SessionStart hook says so), push the branch and use CI as evidence: `gh run list --branch <branch>`, `gh run watch`, `gh run view --log-failed`.
- Record the verification level actually reached. If below the required level, say why.

## 5. Session note
Create `docs/stages/stageNN/sessions/W.N-<theme>.md` using the template in `docs/08-development-workflow.md` §9. Keep it short: what changed (file refs), decisions proposed, deviations/contract changes, follow-ups, verification with numbers.

## 6. Commit, push, PR
- Conventional Commits with the crate as scope (`feat(crs): …`, `test(oracle): …`). Several focused commits are fine; the PR is squash-merged.
- `git push -u origin <branch>`
- `gh pr create --base develop --title "[stageNN/W.N] <theme>" --body-file <filled template>`. Fill in every section of `.github/pull_request_template.md` and include `Closes #<issue>` (find it with `gh issue list --milestone stageNN --search "[stageNN/W.N]"`).
- If acceptance isn't met: `--draft`, and say exactly what's missing.

## 7. Report back (your final message to the orchestrator)
```
Session: stageNN/W.N — <theme>
PR: <url> (ready | draft)
Verification reached: V? (required V?) — evidence: <CI run url / numbers>
Acceptance: <each criterion → met / not met + why>
Contract changes: none | <details>
Decisions proposed: none | <list>
Follow-ups: none | <list>
Merge notes: <files likely to conflict with sibling sessions>
```
