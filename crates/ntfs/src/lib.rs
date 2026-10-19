//! NTFS volume source: `FSCTL_ENUM_USN_DATA` enumeration and USN change-journal tailing.
//!
//! This is the only crate allowed to opt out of the workspace `unsafe_code = "deny"` lint (raw
//! `DeviceIoControl` calls); every `unsafe` block must carry a `// SAFETY:` comment.
//!
//! Spec: `docs/03-architecture.md` §3.1–3.3. Filled in by stage01 sessions 1.3 and 2.2.
