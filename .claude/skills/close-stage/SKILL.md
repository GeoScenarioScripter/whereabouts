---
name: close-stage
description: Close the active Whereabouts stage — verify every exit criterion with evidence, write the stage retro, sync the docs, merge develop into main and tag v0.N.0, then hand over to plan-stage. Use when the last wave of a stage has merged, or the user says "close the stage" / "/close-stage".
---

# Close a stage

## 1. Exit criteria
For each exit criterion in `docs/stages/stageNN/PLAN.md`: link the evidence (CI run, benchmark doc, test name, owner's V4 output). Anything unmet → either fix it (small, via a normal PR) or ask the owner to accept it explicitly as a known gap carried into the next stage. Never mark a criterion met without evidence.

## 2. Retro (append to the bottom of PLAN.md)
- **Keep** — practices and designs that worked, with concrete examples.
- **Change** — what cost time or caused rework (merge friction, wrong contracts, unverified claims, session sizing), and the concrete change for the next stage.
- **Plan deltas** — sessions that split/merged/moved and why.
- **Carried over** — open follow-ups and accepted gaps, each with an issue.
- **Numbers** — sessions, waves, wall-clock time, CI minutes if known, final oracle/benchmark figures.

## 3. Docs sync (one PR `docs/sNN-close`)
- `03-architecture.md` matches the code (crates, types, schema, pipeline).
- `02-decisions.md`: open questions answered this stage become decisions; statuses current. `06-open-questions.md` pruned. `07-discussion-log.md`: one thread summarizing the stage.
- `05-roadmap.md`: tick the milestone's items; move anything deferred.
- `docs/stages/README.md`: stage ✅, active line → next stage.
- `README.md` status section, if user-visible capability changed.

## 4. Release to main
- `gh pr create --base main --head develop --title "Stage NN complete: <objective>"` with the exit-criteria evidence and retro link. Merge with a **merge commit** (not squash) once CI is green and the owner approves.
- `git fetch && git tag -a v0.N.0 origin/main -m "Stage NN: <objective>" && git push origin v0.N.0`
- `gh release create v0.N.0 --notes-file <summary> --prerelease` (pre-1.0).
- Close the `stageNN` milestone.

## 5. Next
Invoke `/plan-stage` for stage NN+1 — the retro you just wrote is its main input.
