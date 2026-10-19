---
name: record-decision
description: Record a Whereabouts design decision ADR-style in docs/02-decisions.md (next D-number, status, context/decision/rationale/consequences), resolve the matching open question in docs/06-open-questions.md, and log it in docs/07-discussion-log.md. Use when the owner decides something, when folding a session's "Decisions proposed" at wave close, or when you are about to make a significant design choice.
---

# Record a decision

`docs/02-decisions.md` is the source of truth; if another doc disagrees, the log wins and the other doc gets fixed.

1. **Check first:** search `02-decisions.md` and `06-open-questions.md` for an existing entry. If the decision changes an existing one, mark the old one **Superseded by D-NN** — never renumber, never delete.
2. **Status:** **Agreed** only if the owner stated or confirmed it (quote or date it). Otherwise **Proposed**. Open questions stay **Open** until someone decides them; don't pick an answer silently.
3. **Append** an entry at the end (next free number) and a row to the Summary table:
   ```markdown
   ## D-NN — <short title>
   - **Status:** Proposed | Agreed (YYYY-MM-DD, owner) | Deferred | Superseded by D-MM
   - **Context:** what forced the decision (link the session note / issue)
   - **Decision:** one or two sentences
   - **Rationale:** why this over the alternatives (name them)
   - **Consequences:** what it constrains or costs; follow-up work
   ```
4. **Open question resolved?** Strike it in `06-open-questions.md` with `→ D-NN` (keep the row for history).
5. **Discussion log:** add a bullet to the current thread (or a new thread) in `07-discussion-log.md`: question → conclusion → D-NN.
6. If the decision changes architecture, formats/CRS handling or the roadmap, update `03`/`04`/`05` in the same PR.
