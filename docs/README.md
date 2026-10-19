# Whereabouts — Project Docs

> **Whereabouts** is a GIS-specific, instant file search tool for the desktop — in the spirit of voidtools *Everything*, but it knows *where on Earth* your data is, not just where it is on disk.
> Working name: **Whereabouts** (originally "Everywhere").

These documents record the design discussions, decisions, and plans made so far (as of 2026-10-19). They are written to be read by humans **and** by coding agents working on this repository.

## Document map

| Doc | What it covers | Read it when… |
|---|---|---|
| [01-vision-and-scope.md](01-vision-and-scope.md) | Problem, users, positioning, prior art, v1 scope and non-goals | You need to know *what* we are building and *what we are not* |
| [02-decisions.md](02-decisions.md) | Decision log (ADR-style) with status, rationale, consequences | You are about to make a design choice — check here first |
| [03-architecture.md](03-architecture.md) | Processes, components, indexing pipeline, storage, query and preview pipelines | You are writing or changing code |
| [04-formats-and-crs.md](04-formats-and-crs.md) | Format support tiers, where metadata lives per format, dataset grouping rules, CRS strategy, testing with GDAL as oracle | You are writing a reader or touching CRS logic |
| [05-roadmap.md](05-roadmap.md) | Milestones for v1, and provisioned (post-v1) features | You are planning work or picking the next task |
| [06-open-questions.md](06-open-questions.md) | Undecided items that need an answer | You hit something that isn't decided — check before guessing |
| [07-discussion-log.md](07-discussion-log.md) | The story threads of how we got here, in order | You want the reasoning and context behind a decision |
| [08-development-workflow.md](08-development-workflow.md) | Stages → waves → sessions, branching, verification levels, session notes | You are planning, running or reviewing work |
| [stages/](stages/README.md) | Stage index; detailed plan of the active stage; session notes | You want to know what is being built right now |

## Status vocabulary (used across all docs)

- **Agreed** — stated or confirmed by the project owner. Treat as a requirement.
- **Proposed** — recommended during design discussion and not yet explicitly confirmed by the owner. Follow it by default, but it can be revisited; flag it if you find a reason to deviate.
- **Open** — undecided. Do not silently pick an answer; raise it (see `06-open-questions.md`).
- **Deferred** — intentionally out of v1; provisioned for later.

## Guidance for coding agents

The operational guide (commands, layout, conventions) is [`../AGENTS.md`](../AGENTS.md). The design-level rules:

1. **Precedence:** `02-decisions.md` is the source of truth for decisions. If another doc disagrees with it, the decision log wins — and the mismatch should be fixed.
2. **Scope discipline:** v1 is **local files only**. Do not add network-share, cloud, or PostgreSQL code paths to v1 — but *do* keep the `Source` abstraction (see architecture) so they can be added later without rework.
3. **No GDAL in the core.** Core readers are native Rust. GDAL may only appear (a) as an optional, out-of-process fallback plugin and (b) in tests as a correctness oracle. See D-07/D-08.
4. **Datasets, not files.** The unit of search results is a *dataset* (which may span many files or be one of many layers inside a file). Never surface a `.dbf` or `.shx` as a standalone result.
5. **When you make a new significant decision,** append it to `02-decisions.md` with status **Proposed** and note it in `07-discussion-log.md`.
6. **Speed is the product.** Typing in the search box must feel instant. Any feature that makes the hot path slower needs justification.
