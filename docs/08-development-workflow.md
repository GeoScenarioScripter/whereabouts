# 08 — Development Workflow: Stages → Waves → Sessions

Status: the three-level structure (stage → wave → parallel sessions by sub-agents in worktrees; only the current stage planned in detail) is **Agreed** (D-23). The specific practices below are **Proposed** (D-26) — designed for this project, with `GeoScenarioScripter/conductor`'s stage/session convention used only as a reference (its retro lessons are folded in, see §10).

---

## 1. Why this shape

Whereabouts is a Rust workspace whose architecture already splits into small crates with clean boundaries (`core`, `source`, `ntfs`, `nameindex`, `datasets`, `readers`, `crs`, `catalog`, `query`, `indexer`, `cli`). That makes parallel work cheap **if** (a) the shared vocabulary is fixed before the parallel work starts and (b) each parallel session owns different crates. Its riskiest code (elevated NTFS access, CRS math) is also the code most likely to "pass unit tests but fail on a real machine", so evidence of verification has to be explicit. The practices below follow from those three facts.

## 2. Units

| Unit | Definition | Run by | Lives in |
|---|---|---|---|
| **Stage** | One roadmap milestone (v1: stage01 = M0 … stage05 = M4) with an objective, exit criteria and gates | Orchestrator (main Claude session) + owner | `docs/stages/stageNN/PLAN.md`; GitHub milestone `stageNN` |
| **Wave** | Sessions with no dependency on each other, run concurrently. The next wave opens when every session in the current one has merged into `develop` | Orchestrator | Wave table in `PLAN.md` |
| **Session** | One agent-sized unit: orient → implement → verify → note → PR. Own git worktree, own branch, executed by a `session-worker` sub-agent | Sub-agent | Spec in `PLAN.md`; GitHub issue; PR; session note |

**Planning horizon:** only the current stage is planned in detail. The next stage has a short sketch in `docs/stages/README.md`; it gets its detailed plan (`/plan-stage`) only after the current stage closes and its retro is written.

## 3. Principles

1. **Contracts first, then fan out.** Wave 1 of a stage defines the shared types/traits the rest of the stage consumes. After wave 1 merges, a contract is *frozen* for the stage: changing it is allowed only as an explicitly flagged "contract change" in the PR (orchestrator approval, all dependants updated in the same PR).
2. **Write scope = ownership.** Every session spec lists the paths it may write. Parallel sessions in a wave have disjoint write scopes; overlaps are listed as *watch points* with a merge order.
3. **Spec is the source of truth for scope; GitHub is the source of truth for status.** `PLAN.md` says *what* each session must deliver; the session's GitHub issue/PR says *where it is*. Sessions never edit shared status tables, so parallel PRs don't conflict on bookkeeping.
4. **Evidence over assertion.** Every session states the verification level it reached (§5). Stage exit criteria require the highest level.
5. **Small, reviewable diffs.** Target ≲ 1,500 changed lines per session (excluding fixtures/generated data). If a session grows past that, split it and record the split in the plan.
6. **Stop, don't cut.** A session that can't meet its acceptance criteria opens a *draft* PR and documents why. It never silently reduces scope or changes a frozen contract.

## 4. Names and identifiers

- Stage: `stage01`… (folder `docs/stages/stage01/`, milestone `stage01`).
- Session ID: `W.N` within the stage (wave W, session N), e.g. `2.4`; fully qualified `stage01/2.4`. Wave 0 = the orchestrator's planning/bootstrap work.
- Branch: `s01/w2-4-<theme>` (stage-namespaced so a stage's branches can be listed and cleaned together), e.g. `s01/w2-4-crs-epsg-transform`. Ad-hoc work: `fix/<theme>`, `docs/<theme>`, `chore/<theme>`.
- Issue title: `[stage01/2.4] CRS: EPSG identification + bounds transform`; labels `stage01`, `wave-2`, crate labels.
- Session note: `docs/stages/stage01/sessions/2.4-crs-epsg-transform.md` (sorted by ID, not date; the date goes inside).
- Commits: Conventional Commits, crate as scope — `feat(crs): identify EPSG by parameter matching`.

## 5. Verification levels

| Level | Meaning | Typical evidence |
|---|---|---|
| **V1 — unit** | `cargo test` for the crate, fmt + clippy clean | CI green on Windows + Linux |
| **V2 — integration** | Cross-crate tests, golden fixtures, oracle agreement on the committed corpus | Test names + agreement numbers in the note |
| **V3 — elevated CI** | Windows-only tests that need admin / a real NTFS volume, run on the (elevated) GitHub Windows runner, incl. VHD-based test volumes | Link to the CI run |
| **V4 — real machine** | Run on the owner's GIS workstation against real data / volumes | Command + output summary pasted by the owner or the session |

Each session spec states its *required* level. "Not verified at the required level, because …" is an acceptable note; silence is not. Until the owner has a local Rust toolchain, V1–V3 come from CI.

## 6. Branching and merging (D-22)

- `main`: stage-complete states only — one `develop → main` PR per stage (merge commit), tagged `v0.<stage>.0`.
- `develop`: integration branch. Session PRs squash-merge here.
- Session branches start from `develop` at wave open. No direct pushes to `main`/`develop`, no `--no-verify`, no plain `--force` (branch protection + the `guard-git` hook).
- Merge one PR at a time in watch-point order; rebase the rest onto the new `develop` and let CI re-run before the next merge.
- `Cargo.lock` is committed. On conflict: take `develop`'s, run `cargo check`, commit.
- Watch-point order exists to keep conflicts trivial. When the only shared file is `Cargo.lock`, the orchestrator may merge a ready PR ahead of a PR still in review fixes; record the actual order at wave close.
- `Closes #<issue>` doesn't fire on merges into `develop` (it isn't the default branch). The orchestrator closes session issues by hand, linking the PR.
- Changes to `tests/corpus/` (scripts, data, goldens, manifest) trigger the `oracle-golden` drift check on the PR. Regenerating needs Docker or that workflow's artifact (D-33).

## 7. Session lifecycle (sub-agent in its worktree)

Defined in `.claude/skills/run-session/SKILL.md`; summary:

1. **Orient:** `AGENTS.md` → the session's spec in `PLAN.md` (plus the wave's watch points) → notes of the sessions it depends on → the docs the spec lists.
2. **Branch:** `git switch -c s01/wW-N-<theme>`.
3. **Implement within write scope.** Shared-file rules: §8.
4. **Verify** to the required level; collect evidence.
5. **Write the session note** (template §9). Decisions go under *Decisions proposed* — the orchestrator numbers and records them.
6. **PR** to `develop` with the template filled, `Closes #<issue>`; report back PR URL, verification level reached, deviations, follow-ups.

## 8. Shared files

- **Root `Cargo.toml`:** stage crates are pre-created as workspace members in wave 0, so sessions never edit `members`. Sessions may *add* `[workspace.dependencies]` entries (alphabetical in their group); changing an existing entry needs a line in the PR.
- **Registries in `lib.rs`** (e.g. reader registry): add one line per item; watch points give merge order.
- **Orchestrator-owned docs** (edited only at wave close, in one docs PR): `PLAN.md` status/wave tables, `02-decisions.md`, `03-architecture.md`, `06-open-questions.md`, `07-discussion-log.md`, `docs/stages/README.md`.

## 9. Session note template

Short — a changelog for the next agent, not a design doc.

```markdown
# stageNN/W.N — <theme>
Date: YYYY-MM-DD · Issue: #N · PR: #M · Verification reached: V?

**Context**: one or two lines (depends on …; what the spec asked).

## What changed
- … with file refs

## Decisions proposed
- (none) | <title>: <decision> — <why>

## Deviations / contract changes
- (none) | …

## Follow-ups
- …

## Verification
- Commands + results (test counts, oracle agreement, benchmark numbers, CI run link).
- Not verified: … because …
```

## 10. Wave and stage lifecycle (orchestrator)

**Wave** (`/run-wave`):
1. Preconditions: previous wave closed, `develop` green, issues exist for the wave's sessions.
2. Spawn one `session-worker` per session (`isolation: "worktree"`), **at most 4 concurrently** — Rust builds are heavy per worktree; queue the rest.
3. Review each PR: CI, required verification level, write scope, guardrails (no GDAL, datasets-not-files, hot path), code review (`/code-review` or `pr-review-toolkit`). Run one forked PR review at a time, because it may check the PR out in the main checkout.
4. Merge in order (§6).
5. Close with one docs PR `docs/s01-wave-W-close`: tick status, fold decisions/open questions/architecture deltas, adjust later waves if scope moved.
6. Gate (if defined): present evidence to the owner; wait for go/no-go.

**Stage:** `/plan-stage` → waves → `/close-stage` (exit criteria evidence incl. V4, retro in `PLAN.md`, docs sync, `develop → main` PR, tag) → `/plan-stage` for the next one.

**Lessons taken from conductor's stage retros:** bookkeeping edited by parallel sessions caused avoidable merge friction → status moved to GitHub, docs folded at wave close. "Live smoke test recommended" was deferred nine sessions in a row → verification levels are explicit and V4 is a stage exit criterion. Cross-surface gaps slipped when a counterpart session wasn't named → every session spec lists its out-of-scope items and follow-ups explicitly.
