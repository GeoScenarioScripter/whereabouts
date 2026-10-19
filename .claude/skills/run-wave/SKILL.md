---
name: run-wave
description: Orchestrate one wave of the active Whereabouts stage — spawn a session-worker sub-agent per session in isolated worktrees (max 4 concurrent), review and merge their PRs into develop in watch-point order, then close the wave with one docs PR and run any gate. Use when the user says "run wave N", "start the next wave", or "/run-wave".
---

# Run a wave

Process: `docs/08-development-workflow.md` §10. You are the orchestrator; sessions are done by sub-agents, not by you.

## 0. Preconditions
- Read `docs/stages/README.md` (active stage) and the stage `PLAN.md`: the wave table, every session spec in this wave, the dependency graph, the watch points (merge order), and any gate before/after this wave.
- The previous wave is closed (status table updated) and its gate (if any) has an owner go.
- `git switch develop && git pull --ff-only`; CI on `develop` is green (`gh run list --branch develop -L 3`).
- Every session in the wave has an open issue in the stage milestone (`gh issue list --milestone stageNN`). Create missing ones.
- The main checkout stays on `develop` while sub-agents run (their worktrees branch from it).

## 1. Spawn sessions
For each session, call the Agent tool with:
- `subagent_type: "session-worker"`, `isolation: "worktree"`, `description: "stageNN/W.N <theme>"`
- prompt: `Run session stageNN/W.N (<theme>). Follow .claude/skills/run-session/SKILL.md exactly. Issue: #<n>. <wave-specific notes: sibling sessions running in parallel and the files you must not touch>.`

Run **at most 4 at once** (Rust builds per worktree are heavy). Start the ones the plan names first; start the rest as earlier ones finish. Don't predict or fabricate results while they run.

## 2. Review each PR as it arrives
- CI green on Windows + Linux, cargo-deny green.
- Acceptance criteria evidenced at the required verification level; anything not verified is explained.
- Diff stays inside the write scope (`gh pr diff <n> --name-only`). Shared-file edits follow the rules.
- No contract change, or a flagged one you agree with.
- Guardrails: no GDAL; datasets not files; untrusted-input handling (no panics/unbounded reads); hot path not slowed.
- Code review: run `/code-review` against the PR, or the `pr-review-toolkit` agents for larger diffs. Request fixes by continuing that session's agent (SendMessage) rather than fixing it yourself.

## 3. Merge in watch-point order
For each PR in order: `gh pr merge <n> --squash --delete-branch`. Then for each remaining PR, have its agent (or do it yourself, if trivial) rebase onto `develop`, resolve conflicts (`Cargo.lock`: take develop's, then `cargo check`), push, and wait for green CI before merging.

## 4. Close the wave (one docs PR from `docs/sNN-wave-W-close`)
- `PLAN.md` status table: ✅ + date + note link for each session; edit later waves' specs if scope moved (record the deviation).
- Fold every *Decisions proposed* into `docs/02-decisions.md` with the next free D-numbers (status Proposed unless the owner confirmed), plus a line in `docs/07-discussion-log.md`. Use `/record-decision`.
- Update `docs/06-open-questions.md`, `docs/03-architecture.md` (deltas sessions reported), `docs/stages/README.md` (active line).
- Turn follow-ups into issues (label `follow-up`, milestone = the stage that should do them).
- Merge it like any PR, then `git pull` on `develop`.

## 5. Gate (if the plan defines one after this wave)
Summarize the gate's evidence for the owner (numbers, links, your recommendation) and **wait for their go/no-go**. Don't open the next wave until you have it.

## 6. Report to the owner
Wave summary: sessions merged (PR links), verification levels reached, decisions proposed, contract changes, follow-ups created, gate status, what opens next.
