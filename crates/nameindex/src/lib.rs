//! Compact in-memory filename index.
//!
//! One contiguous string pool per volume, `u32` offsets, pre-folded names, extension offsets;
//! stores (FRN, parent FRN, name) and reconstructs paths on demand. Search is a multithreaded
//! linear scan. Persisted as a binary snapshot plus the last processed USN and journal ID.
//!
//! Spec: `docs/03-architecture.md` §3.4, D-10. Filled in by stage01 session 2.1.
