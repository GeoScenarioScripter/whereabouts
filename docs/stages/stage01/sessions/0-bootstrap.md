# stage01/0 — Bootstrap: repo, tooling, docs, stage plan
Date: 2026-10-19 · Run by: orchestrator (main session) · Verification reached: V1 via CI (no local toolchain)

**Context**: first working session after the design discussions recorded in `docs/01`–`07`. Owner asked to initialise the repo and GitHub project, set up agent tooling, write `CLAUDE.md`/`AGENTS.md`, define the stage → wave → session convention and plan stage01 in detail.

## What changed
- **Owner decisions** (recorded in `02-decisions.md`): licence MIT OR Apache-2.0 (D-21); the whole proposed stack confirmed → D-06…D-12, D-18…D-20 now Agreed; `main` + `develop` branching (D-22); stage/wave/session process (D-23).
- **Repo**: `git init`; public repo `GeoScenarioScripter/whereabouts`; `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, `README.md`, `CONTRIBUTING.md`, `.editorconfig` (from org dev-standards), `.gitattributes` (byte-exact corpus), `.gitignore`.
- **Rust workspace**: `Cargo.toml` with all stage01 crates pre-created as stubs under `crates/` + `tests/oracle` (D-24), so wave sessions never edit `members`; internal dependency edges declared; shared dependency versions pinned; workspace lints (`unsafe_code = deny` except `ntfs`); toolchain pinned to 1.99 (edition 2024); `rustfmt.toml`; `deny.toml` (licence allow-list, GDAL banned per D-07).
- **CI**: `.github/workflows/ci.yml` — rustfmt; clippy + test on Windows and Linux; elevated NTFS tests on the Windows runner; cargo-deny. Dependabot (cargo + actions → `develop`).
- **Claude Code**: `.claude/settings.json` with hooks (`session-context`, `guard-no-gdal`, `guard-git`, `rustfmt-on-edit`), safe-command allow-list, project plugins (rust-analyzer-lsp, commit-commands, pr-review-toolkit, security-guidance, claude-md-management, context7); skills `plan-stage`, `run-wave`, `run-session`, `close-stage`, `add-format-reader`, `record-decision`; agent `session-worker`.
- **Docs**: `AGENTS.md` (canonical agent guide), `CLAUDE.md` (imports it + Claude specifics), `docs/08-development-workflow.md`, `docs/stages/README.md`, `docs/stages/stage01/PLAN.md`; `docs/README.md` map, decisions, open questions, roadmap and discussion log updated.
- **GitHub**: milestone `stage01`, one issue per session, labels; branch protection on `main` and `develop`.

## Decisions proposed
- D-24 workspace layout & toolchain; D-25 committed golden files (GDAL only to regenerate); D-26 workflow practices — recorded directly (orchestrator session).

## Follow-ups
- Owner: install Rust (rustup, MSVC toolchain + VS Build Tools C++ workload) before wave 4 at the latest — V4 verification and local builds depend on it.
- Commit `Cargo.lock` from the first session that builds locally or from CI.
- Q-01 findings (2026-10-19): the name "Whereabouts" is already used by a Kubernetes CNI IPAM plugin (`k8snetworkplumbingwg/whereabouts`, ~400★) and the crates.io name `whereabouts` is taken (an active "offline place names and IP lookup" crate). `whereabouts-*` crate names are free; all crates are `publish = false` for now. Trademark/domain/store checks still pending.

## Verification
- Hook scripts exercised with sample inputs (deny/allow paths) via Node.
- Workspace compiled only in CI (no local toolchain) — see the first CI run on `main`.
