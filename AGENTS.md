# AGENTS.md

Guidance for any coding agent (Claude Code, Codex, Copilot, …) and for human contributors working in this repository. `CLAUDE.md` imports this file and adds Claude-Code-specific tooling.

## What this is

**Whereabouts** is a GIS-aware instant file search tool for the Windows desktop — voidtools *Everything*, plus *where*. It indexes NTFS volumes via the MFT/USN journal, groups files into GIS **datasets**, extracts extent/CRS metadata with native Rust header readers, and answers "what data do I have for this area?" by name and by place.

Status: pre-alpha. Stage 01 (M0 — CLI indexer prototype) is in progress. See `docs/stages/README.md`.

## Read before you change anything

| Doc | Why |
|---|---|
| `docs/README.md` | Map of the design docs, status vocabulary (Agreed / Proposed / Open / Deferred) |
| `docs/02-decisions.md` | **Source of truth for decisions.** Check it before any design choice |
| `docs/03-architecture.md` | Processes, crates, pipeline, catalog schema |
| `docs/04-formats-and-crs.md` | Before touching readers, grouping or CRS logic |
| `docs/06-open-questions.md` | Undecided items: never pick an answer silently; raise it |
| `docs/08-development-workflow.md` | Stage → wave → session process, branches, verification levels |
| `docs/stages/<active>/PLAN.md` | What the current stage is building; your session spec if you're running one |

## Repository layout

```
crates/
  core/       whereabouts-core       shared types: Frn, Dataset, Layer, Bounds, CrsDefinition, FormatReader trait
  source/     whereabouts-source     Source trait (enumerate + changes) — D-18
  ntfs/       whereabouts-ntfs       FSCTL_ENUM_USN_DATA + USN journal (only crate allowed `unsafe`)
  nameindex/  whereabouts-nameindex  in-memory filename index + snapshots
  datasets/   whereabouts-datasets   dataset grouper (sidecars, GPKG layers, FGDB folders)
  readers/    whereabouts-readers    native bounded-read metadata readers
  crs/        whereabouts-crs        WKT/PROJJSON/GeoKeys parsing, EPSG identification, bounds → WGS84
  catalog/    whereabouts-catalog    SQLite (WAL) + R*Tree catalog
  query/      whereabouts-query      query planning (name scan ∩ catalog filters ∩ bbox)
  indexer/    whereabouts-indexer    pipeline: source → name index → grouper → extract → catalog
  cli/        whereabouts-cli        `whereabouts` binary (index / inspect / query)
tests/
  oracle/     GDAL-as-oracle comparison harness (dev/CI only)
  corpus/     test datasets + committed GDAL golden files
docs/         design docs, stage plans, session notes
```

Later stages add `ipc`, `service`, `tiles` crates and the Tauri `app/`.

## Commands

```sh
cargo build                                          # default member: the CLI
cargo fmt --all                                      # format (CI runs --check)
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                               # unit + integration (non-elevated)
cargo test -p whereabouts-ntfs -- --ignored          # raw-volume tests: needs an elevated shell on Windows
cargo test -p whereabouts-oracle                     # GDAL oracle agreement report
cargo deny check                                     # licences, advisories, GDAL ban
cargo run -- --help                                  # the CLI
```

Toolchain is pinned by `rust-toolchain.toml` (Rust 1.99, edition 2024, MSVC on Windows). CI (`.github/workflows/ci.yml`) runs everything on Windows and Linux; the Windows runner is elevated, so `--ignored` NTFS tests run there.

## Guardrails (non-negotiable unless `02-decisions.md` changes)

1. **v1 is local files only** (D-03). No network-share, cloud or PostgreSQL code paths — but keep everything behind the `Source` trait (D-18) so they can come later.
2. **No GDAL in any crate** (D-07/D-08). GDAL appears only as a CLI that regenerates oracle golden files (`tests/corpus/scripts`) and, later, as an optional out-of-process plugin. `cargo-deny` and a Claude hook enforce this.
3. **Datasets, not files** (D-04). Results are datasets and layers. Never surface a `.dbf`, `.shx` or FGDB internal file as a standalone spatial result.
4. **Speed is the product** (D-01). The typing → results path must stay instant. Anything that adds work there needs a benchmark and a justification in the PR.
5. **File contents are untrusted input.** Readers do bounded reads, validate every length/offset/count, never panic on malformed data (`Error`/`Partial` status instead), and never write to, lock or create sidecars for user files.
6. **Windows-specific code stays in `ntfs`** (and later `service`/`ipc`), behind `cfg(windows)`. Every other crate must build and test on Linux.

## Coding conventions

- Rust 2024 edition, `rustfmt.toml` formatting, clippy clean with `-D warnings`. Workspace lints: `unsafe_code = "deny"` (only `crates/ntfs` opts out, with `// SAFETY:` on every block), `unwrap_used` warns (allowed in tests).
- Errors: `thiserror` enums in library crates; `anyhow` only in the CLI binary and test helpers.
- Logging: `tracing`; no `println!` outside the CLI.
- Public items have rustdoc. The crate-level doc comment states the crate's responsibility and links its spec.
- Dependencies: one used by more than one crate goes in root `[workspace.dependencies]` (alphabetical) and is referenced with `{ workspace = true }`. New dependencies must be permissively licensed (see `deny.toml`) and justified in the PR. Prefer small pure-Rust crates; no C/C++ build dependencies without a decision.
- Tests: unit tests next to the code; cross-crate tests in `crates/<x>/tests/`; corpus-driven tests go through `tests/oracle`. Tests that need admin or a raw volume are `#[ignore]` with a comment saying so, and must use an isolated test volume or temp dir.
- Commit messages: Conventional Commits with the crate as scope — `feat(nameindex): add wildcard scan`, `fix(readers): handle empty .dbf`.

## Workflow

- Work is organised as **stages → waves → sessions** (`docs/08-development-workflow.md`). Each session runs on its own branch (`s01/w2-4-<theme>`) in its own worktree and lands as a squash-merged PR into `develop`. `main` only receives stage-complete merges.
- Never push to `main`/`develop` directly, never `--no-verify`, never plain `--force`.
- Running a session? Stay inside its **write scope**, don't edit orchestrator-owned docs (`PLAN.md` tables, `02`/`03`/`06`/`07`), put proposed decisions in your session note, and report your verification level (V1–V4) honestly.
- Making a significant design decision outside a session? Append it to `docs/02-decisions.md` as **Proposed** and note it in `docs/07-discussion-log.md`.
- Hit an Open question? Use its documented default only if blocked, and flag it in the PR.
