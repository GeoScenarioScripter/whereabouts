---
name: plan-stage
description: Write the detailed plan for the next Whereabouts stage (objective, exit criteria, gates, waves of parallel sessions with write scopes and acceptance criteria) after the previous stage has closed, pre-create its crates, and open the GitHub milestone and session issues. Use when the user says "plan stage N", "plan the next stage", or "/plan-stage".
---

# Plan a stage

Only one stage is planned in detail at a time (`docs/08-development-workflow.md` §2). The template is `docs/stages/stage01/PLAN.md` — copy its structure, not its content.

## 1. Inputs (read all)
- The previous stage's `PLAN.md` **retro** and its session notes' follow-ups; open `follow-up` issues for this stage's milestone.
- `docs/05-roadmap.md` (this stage's milestone), `docs/02-decisions.md`, `docs/06-open-questions.md` (which questions must this stage answer?), `docs/03-architecture.md`, `docs/stages/README.md` sketch.
- The current code: crate APIs that this stage builds on (`cargo doc` or read `lib.rs` summaries).

## 2. Discuss open points with the owner first
List the open questions and design forks this stage depends on, give a recommendation for each, and ask (AskUserQuestion). Don't plan around an undecided Open question silently.

## 3. Design the waves
- **Wave 1 = contracts** (types, traits, protocols, schemas the rest consumes) **+ the riskiest spikes**, so later waves fan out against frozen interfaces and risks surface early.
- Sessions are agent-sized: one crate or module, ≲ 1,500 changed lines, one clear acceptance test. Split anything bigger.
- Parallel sessions in a wave have **disjoint write scopes**; list unavoidable overlaps as watch points with a merge order.
- ≤ 4 concurrent sessions in practice; if a wave has more, say which start first.
- Put a **gate** wherever a result could change the architecture (performance feasibility, coverage, owner review of contracts).
- Every session spec: Reads · Write scope · Deliverables · Acceptance (with required verification level V1–V4) · Out of scope. Add a dependency-graph row with the reason for each dependency.
- Exit criteria are measurable and at least one is V4 (owner's real machine) when the stage touches the file system, service or UI.

## 4. Write it down (wave 0 PR from `docs/sNN-plan`)
- `docs/stages/stageNN/PLAN.md` and `docs/stages/stageNN/sessions/0-plan.md` (the wave-0 note).
- Pre-create every new crate as a stub workspace member (Cargo.toml + `lib.rs` doc comment naming the session that fills it) with its internal dependency edges, so sessions never edit `members`.
- Update `docs/stages/README.md` (active line, stage row, sketch of the stage after this one), `docs/05-roadmap.md` if scope moved, decisions/open questions resolved during planning.

## 5. GitHub
- `gh api repos/GeoScenarioScripter/whereabouts/milestones -f title=stageNN -f description="<objective>"`
- One issue per session: title `[stageNN/W.N] <theme>`, body = the spec + link to PLAN.md, labels `stageNN`, `wave-W`, milestone `stageNN`. Put the issue numbers in the status table.

## 6. Hand back
Summarize the plan for the owner (waves, gates, exit criteria, what you need from them — e.g. V4 runs) and get a go before `/run-wave 1`.
