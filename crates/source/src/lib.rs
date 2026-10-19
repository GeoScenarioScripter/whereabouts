//! The `Source` abstraction (D-18): enumerate a file system and stream change events.
//!
//! v1 ships only the NTFS implementation (`whereabouts-ntfs`); SMB, cloud, PostGIS, macOS and Linux
//! sources plug in here later without touching the catalog or query layers.
//!
//! Spec: `docs/03-architecture.md` §2, §9. Filled in by stage01 session 1.1 (core contracts).
