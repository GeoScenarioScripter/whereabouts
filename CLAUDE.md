# CLAUDE.md

@AGENTS.md

## Claude Code specifics

### Orchestrator vs session worker
- **Main session = orchestrator.** It plans stages (`/plan-stage`), runs waves (`/run-wave`), reviews and merges PRs, folds decisions into the docs, and closes stages (`/close-stage`). It does not implement session work itself.
- **Sessions = `session-worker` sub-agents** (`.claude/agents/session-worker.md`), spawned with `isolation: "worktree"` (worktrees live in `.claude/worktrees/`, gitignored), at most 4 at a time. Each follows `/run-session`.
- Talk to the owner before deciding anything listed as Open, at every stage gate, and before anything outward-facing that isn't in the plan.

### Project skills (`.claude/skills/`)
| Skill | Use it to |
|---|---|
| `/plan-stage` | Write the next stage's detailed plan, pre-create crates, open milestone + issues |
| `/run-wave` | Spawn a wave's sessions, review and merge in order, close the wave, run gates |
| `/run-session` | Execute one session in a worktree (session workers) |
| `/close-stage` | Exit-criteria evidence, retro, docs sync, `develop → main`, tag |
| `/add-format-reader` | Implement or change a reader in `crates/readers` (bounded reads, oracle wiring) |
| `/record-decision` | Add or update an ADR in `docs/02-decisions.md` and related docs |

### Hooks (`.claude/settings.json`, scripts in `.claude/hooks/`, Node-based for Windows + POSIX)
- `session-context` (SessionStart): prints branch/worktree, active stage, and warns if `cargo` is missing.
- `guard-no-gdal` (PreToolUse on edits): blocks adding `gdal`/`gdal-sys` to any `Cargo.toml` (D-07).
- `guard-git` (PreToolUse on shell): blocks pushes to `main`/`develop`, plain `--force`, and `--no-verify`.
- `rustfmt-on-edit` (PostToolUse on edits): formats edited `.rs` files; silent no-op without rustfmt.

If a hook blocks you, fix the cause; never work around it.

### Plugins (project scope)
`rust-analyzer-lsp` (code intelligence; needs the `rust-analyzer` rustup component), `commit-commands`, `pr-review-toolkit` (PR review agents for wave merges), `security-guidance` (the service runs elevated and parses untrusted files), `claude-md-management`, `context7` (current docs for `windows-rs`, `rusqlite`, `proj4rs`, …).

### Environment notes
- Windows 11 is the dev OS. Use the Bash tool (Git Bash) for POSIX scripts and PowerShell for Windows admin tasks (`fsutil`, `diskpart`, elevation).
- Elevated work (raw volume handles) needs an elevated shell — if you're not elevated, rely on the `#[ignore]` tests in CI and say so.
- The owner's machine may not have the Rust toolchain yet; the SessionStart hook tells you. Without it, CI is your build/test environment (`gh run watch`, `gh run view --log-failed`).
- Each worktree has its own `target/`. `sccache` (`RUSTC_WRAPPER=sccache`) is recommended when running several sessions at once.
