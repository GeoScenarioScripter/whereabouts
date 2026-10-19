//! Query planning: name terms → name-index scan; spatial term → R*Tree filter; attribute filters →
//! catalog SQL; intersect, rank, page.
//!
//! The full Everything-style search syntax is open question Q-11 (decided in a later stage); stage01
//! only needs plain name terms, `--bbox` and `--format`.
//!
//! Spec: `docs/03-architecture.md` §5. Filled in by stage01 session 4.1.
