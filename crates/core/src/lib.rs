//! Shared domain types for Whereabouts.
//!
//! Datasets (not files) are the unit of search results (D-04). This crate holds the vocabulary every
//! other crate speaks: dataset/layer identities, bounds, CRS references, file identities (FRNs) and
//! extraction status. It has no dependency on any other workspace crate.
//!
//! Spec: `docs/03-architecture.md` §2, §4. Filled in by stage01 session 1.1 (core contracts).
