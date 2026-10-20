//! Pure USN change-journal logic: `FSCTL_READ_USN_JOURNAL` input/output, and the mapping from
//! `USN_REASON_*` records to [`SourceEvent`]s.
//!
//! Everything here works on byte slices and parsed [`UsnRecord`]s, so it is unit-tested on every
//! platform from fixtures built with `record::build`. The Windows-only `NtfsVolumeSource`
//! drives it.
//!
//! # Reason → event mapping
//!
//! NTFS writes several records per file while a handle is open, OR-ing the reasons together, and
//! a final record with [`reason::CLOSE`]. A rename writes an *old name* record
//! ([`reason::RENAME_OLD_NAME`], carrying the old parent and name) followed by a *new name*
//! record ([`reason::RENAME_NEW_NAME`]). [`Coalescer`] folds all records of one file within one
//! batch into its final state and reports at most:
//!
//! | Reasons seen in the batch | Events |
//! |---|---|
//! | [`reason::FILE_DELETE`] | `Deleted(last state)` — also when the file was created in the same batch, because an enumeration that raced with the checkpoint may have reported it |
//! | [`reason::FILE_CREATE`] | `Created(final state)` (renames and writes in the same batch are folded in) |
//! | old + new name, final (parent, name) differs from the first old one | `Renamed { record, old_parent, old_name }` |
//! | any of [`reason::MODIFIED_MASK`] | `Modified(final state)`, after `Renamed` if both apply |
//! | only other reasons (security, object ID, indexing flags, close) | nothing |
//!
//! Events are ordered by each file's **first** record in the batch, so a directory created
//! before files are put into it is reported before them. Within one batch a file moved into a
//! directory created later in the batch can still precede that directory; consumers apply the
//! whole batch before resolving paths.
//!
//! Records of files whose handle is still open at the end of the batch are reported with what
//! is known so far (no cross-batch debouncing; that is the indexer's job). When the
//! close record arrives later its accumulated reasons are reported again, which consumers
//! absorb because they apply events idempotently (D-29).
//!
//! **Renames split across batches.** If a batch ends between a file's old-name and new-name
//! records, the rename is not reported yet: its pre-rename (parent, name) is carried in the
//! checkpoint ([`PendingRename`]) and paired with the new-name record in the next batch. A
//! new-name record that cannot be paired (its old half was purged, or a checkpoint was taken
//! between the two halves) is reported as `Created(final state)`, which consumers treat as an
//! upsert of a known FRN, as D-29 replay already requires.
//!
//! **Hard links.** [`reason::HARD_LINK_CHANGE`] is reported as `Modified` with the name in the
//! record, which is the link that was added or removed. The name index keeps one name per FRN,
//! so extra links are not tracked (documented M0 limitation).

use std::collections::HashMap;

use whereabouts_core::{FileAttributes, FileName, FileRecord, Frn};
use whereabouts_source::SourceEvent;

use crate::error::ParseError;
use crate::record::{EnumStep, UsnRecord, enum_step};

/// `USN_REASON_*` flags (`winioctl.h`).
pub mod reason {
    /// Main data stream overwritten.
    pub const DATA_OVERWRITE: u32 = 0x0000_0001;
    /// Main data stream extended.
    pub const DATA_EXTEND: u32 = 0x0000_0002;
    /// Main data stream truncated.
    pub const DATA_TRUNCATION: u32 = 0x0000_0004;
    /// A named data stream overwritten.
    pub const NAMED_DATA_OVERWRITE: u32 = 0x0000_0010;
    /// A named data stream extended.
    pub const NAMED_DATA_EXTEND: u32 = 0x0000_0020;
    /// A named data stream truncated.
    pub const NAMED_DATA_TRUNCATION: u32 = 0x0000_0040;
    /// The file or directory was created.
    pub const FILE_CREATE: u32 = 0x0000_0100;
    /// The file or directory was deleted.
    pub const FILE_DELETE: u32 = 0x0000_0200;
    /// Extended attributes changed.
    pub const EA_CHANGE: u32 = 0x0000_0400;
    /// Security descriptor changed.
    pub const SECURITY_CHANGE: u32 = 0x0000_0800;
    /// Rename/move: this record carries the **old** parent and name.
    pub const RENAME_OLD_NAME: u32 = 0x0000_1000;
    /// Rename/move: this record carries the **new** parent and name.
    pub const RENAME_NEW_NAME: u32 = 0x0000_2000;
    /// The content-indexed attribute changed.
    pub const INDEXABLE_CHANGE: u32 = 0x0000_4000;
    /// Attributes or time stamps changed.
    pub const BASIC_INFO_CHANGE: u32 = 0x0000_8000;
    /// A hard link was added or removed.
    pub const HARD_LINK_CHANGE: u32 = 0x0001_0000;
    /// Compression state changed.
    pub const COMPRESSION_CHANGE: u32 = 0x0002_0000;
    /// Encryption state changed.
    pub const ENCRYPTION_CHANGE: u32 = 0x0004_0000;
    /// Object ID changed.
    pub const OBJECT_ID_CHANGE: u32 = 0x0008_0000;
    /// Reparse point changed.
    pub const REPARSE_POINT_CHANGE: u32 = 0x0010_0000;
    /// A named stream was added, removed or renamed.
    pub const STREAM_CHANGE: u32 = 0x0020_0000;
    /// Transactional change (TxF).
    pub const TRANSACTED_CHANGE: u32 = 0x0040_0000;
    /// Integrity stream setting changed.
    pub const INTEGRITY_CHANGE: u32 = 0x0080_0000;
    /// Desired storage class changed.
    pub const DESIRED_STORAGE_CLASS_CHANGE: u32 = 0x0100_0000;
    /// The handle was closed: the last record of an open/modify/close session.
    pub const CLOSE: u32 = 0x8000_0000;

    /// Reasons reported as `SourceEvent::Modified`: content, attributes/time stamps, streams,
    /// links, compression, encryption, reparse data. Security descriptor, object ID, indexing
    /// and storage-class changes don't affect what Whereabouts indexes and are ignored.
    pub const MODIFIED_MASK: u32 = DATA_OVERWRITE
        | DATA_EXTEND
        | DATA_TRUNCATION
        | NAMED_DATA_OVERWRITE
        | NAMED_DATA_EXTEND
        | NAMED_DATA_TRUNCATION
        | EA_CHANGE
        | BASIC_INFO_CHANGE
        | HARD_LINK_CHANGE
        | COMPRESSION_CHANGE
        | ENCRYPTION_CHANGE
        | REPARSE_POINT_CHANGE
        | STREAM_CHANGE;
}

/// Size of `READ_USN_JOURNAL_DATA_V1` (44 bytes of fields, padded to the 8-byte alignment).
pub const READ_USN_JOURNAL_DATA_V1_SIZE: usize = 48;

/// Serialises `READ_USN_JOURNAL_DATA_V1`, the input of `FSCTL_READ_USN_JOURNAL`, for a
/// non-blocking read of every reason from `start_usn`.
///
/// Fixed choices: `ReasonMask` = all reasons, `ReturnOnlyOnClose` = 0 (the old-name half of a
/// rename is only visible this way), `Timeout` = 0 and `BytesToWaitFor` = 0 (return at once,
/// even with no records), record versions 2..=2 (D-31: NTFS file references are 64-bit).
#[must_use]
pub fn encode_read_usn_journal_data_v1(
    start_usn: i64,
    journal_id: u64,
) -> [u8; READ_USN_JOURNAL_DATA_V1_SIZE] {
    let mut out = [0_u8; READ_USN_JOURNAL_DATA_V1_SIZE];
    out[0..8].copy_from_slice(&start_usn.to_le_bytes());
    out[8..12].copy_from_slice(&u32::MAX.to_le_bytes()); // ReasonMask
    // 12..16 ReturnOnlyOnClose = 0; 16..24 Timeout = 0; 24..32 BytesToWaitFor = 0.
    out[32..40].copy_from_slice(&journal_id.to_le_bytes());
    out[40..42].copy_from_slice(&2_u16.to_le_bytes()); // MinMajorVersion
    out[42..44].copy_from_slice(&2_u16.to_le_bytes()); // MaxMajorVersion
    // 44..48: structure padding.
    out
}

/// What to do after one `FSCTL_READ_USN_JOURNAL` call (see [`read_step`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStep {
    /// The reply holds records; read again from `next_usn` once they are consumed.
    Records {
        /// USN to start the next read from.
        next_usn: i64,
    },
    /// Header-only reply whose USN moved forward (records were filtered out): read again from
    /// `next_usn`.
    Advance {
        /// USN to start the next read from.
        next_usn: i64,
    },
    /// Header-only reply that did not move: everything up to the journal's end has been read.
    CaughtUp,
    /// The returned USN did not move forward although records came back, or went backwards.
    Stalled {
        /// The USN the kernel returned.
        next_usn: i64,
    },
}

/// Decides how a journal read proceeds after a call made from `start_usn` returned `reply`
/// (the bytes the kernel wrote: the next USN, then records). Pure; the same termination rules
/// as enumeration ([`enum_step`]), because both replies start with an 8-byte cursor.
///
/// # Errors
/// [`ParseError::Truncated`] when the reply is shorter than 8 bytes,
/// [`ParseError::UsnOutOfRange`] for a negative `start_usn` or a returned USN above `i64::MAX`.
pub fn read_step(start_usn: i64, reply: &[u8]) -> Result<ReadStep, ParseError> {
    let start = u64::try_from(start_usn).map_err(|_| ParseError::UsnOutOfRange {
        value: start_usn as u64,
    })?;
    let usn = |value: u64| i64::try_from(value).map_err(|_| ParseError::UsnOutOfRange { value });
    Ok(match enum_step(start, reply)? {
        EnumStep::Records { next_start } => ReadStep::Records {
            next_usn: usn(next_start)?,
        },
        EnumStep::Advance { next_start } => ReadStep::Advance {
            next_usn: usn(next_start)?,
        },
        EnumStep::End => ReadStep::CaughtUp,
        EnumStep::Stalled { next_start } => ReadStep::Stalled {
            next_usn: usn(next_start)?,
        },
    })
}

/// Converts a parsed record into the `Source` contract's [`FileRecord`] (lossless name).
#[must_use]
pub fn file_record(rec: &UsnRecord<'_>) -> FileRecord {
    let units: Vec<u16> = rec.name_units().collect();
    FileRecord {
        frn: Frn(rec.frn),
        parent: Frn(rec.parent_frn),
        name: FileName::from_utf16(&units),
        attrs: FileAttributes(rec.file_attributes),
    }
}

/// The pre-rename location of a file whose old-name record was read but whose new-name record
/// was not (yet). Carried from one batch to the next in the checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRename {
    /// The file.
    pub frn: Frn,
    /// Parent directory before the rename.
    pub old_parent: Frn,
    /// Name before the rename.
    pub old_name: FileName,
}

/// Most pending renames carried in one checkpoint; beyond this the oldest are dropped (their
/// new-name halves are then reported as `Created`, see the module docs).
pub const MAX_PENDING_RENAMES: usize = 64;

/// Per-file state while a batch is folded.
#[derive(Debug)]
struct FileState {
    /// OR of every reason seen in this batch.
    reasons: u32,
    /// State from the latest record.
    last: FileRecord,
    /// (parent, name) before the first rename in this batch (or carried from the previous one).
    old: Option<(Frn, FileName)>,
    /// The latest rename record was an old-name record: the new name is not known yet.
    awaiting_new_name: bool,
    /// A new-name record arrived with no old name to pair it with.
    unpaired_new_name: bool,
}

/// Folds the journal records of one batch into [`SourceEvent`]s (see the module docs for the
/// rules). Feed records in journal order with [`Coalescer::push`], then call
/// [`Coalescer::finish`].
#[derive(Debug, Default)]
pub struct Coalescer {
    files: HashMap<u64, FileState>,
    /// FRNs in order of their first record in the batch.
    order: Vec<u64>,
    /// Renames left open by the previous batch, by FRN.
    carried: HashMap<u64, (Frn, FileName)>,
    /// Carried renames in their original order, for re-carrying unresolved ones.
    carried_order: Vec<u64>,
    records: u64,
}

impl Coalescer {
    /// Starts a batch, with the renames the previous batch left open (from the checkpoint).
    #[must_use]
    pub fn new(carried: Vec<PendingRename>) -> Self {
        let mut this = Self::default();
        for p in carried {
            if this
                .carried
                .insert(p.frn.0, (p.old_parent, p.old_name))
                .is_none()
            {
                this.carried_order.push(p.frn.0);
            }
        }
        this
    }

    /// Records folded so far.
    #[must_use]
    pub fn records(&self) -> u64 {
        self.records
    }

    /// Folds one journal record (records must be pushed in journal order).
    pub fn push(&mut self, rec: &UsnRecord<'_>) {
        self.records += 1;
        let record = file_record(rec);
        let state = match self.files.entry(rec.frn) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                self.order.push(rec.frn);
                // A rename left open by the previous batch: the consumer still has the file
                // under its pre-rename name, so that is the "old" side of any rename now. If
                // this batch has no new-name record for it, the final state decides.
                let old = self.carried.remove(&rec.frn);
                e.insert(FileState {
                    reasons: 0,
                    last: record.clone(),
                    old,
                    awaiting_new_name: false,
                    unpaired_new_name: false,
                })
            }
        };
        state.reasons |= rec.reason;
        let is_new_name = rec.reason & reason::RENAME_NEW_NAME != 0;
        let is_old_name = rec.reason & reason::RENAME_OLD_NAME != 0 && !is_new_name;
        if is_old_name {
            if state.old.is_none() {
                state.old = Some((record.parent, record.name.clone()));
            }
            state.awaiting_new_name = true;
        } else if is_new_name {
            if state.old.is_none() && state.reasons & reason::FILE_CREATE == 0 {
                state.unpaired_new_name = true;
            }
            state.awaiting_new_name = false;
        }
        state.last = record;
    }

    /// Ends the batch: the events in order, and the renames still waiting for their new-name
    /// record (to carry in the next checkpoint).
    ///
    /// Carried renames that this batch did not touch are kept only if the batch read no
    /// records at all: the new-name half follows the old-name half immediately in the journal,
    /// so once newer records exist it is never coming.
    #[must_use]
    pub fn finish(mut self) -> (Vec<SourceEvent>, Vec<PendingRename>) {
        let mut events = Vec::with_capacity(self.order.len());
        let mut pending = Vec::new();
        if self.records == 0 {
            for frn in &self.carried_order {
                if let Some((old_parent, old_name)) = self.carried.remove(frn) {
                    pending.push(PendingRename {
                        frn: Frn(*frn),
                        old_parent,
                        old_name,
                    });
                }
            }
        } else {
            for (frn, (parent, name)) in &self.carried {
                tracing::debug!(
                    frn = %Frn(*frn), old_parent = %parent, old_name = %name,
                    "dropping a rename whose new name never appeared in the journal"
                );
            }
        }
        for frn in self.order {
            let Some(st) = self.files.remove(&frn) else {
                continue;
            };
            let r = st.reasons;
            if r & reason::FILE_DELETE != 0 {
                events.push(SourceEvent::Deleted(st.last));
                continue;
            }
            if r & reason::FILE_CREATE != 0 {
                if st.awaiting_new_name {
                    // Created, then an old-name record: the consumer learns the file under the
                    // name it had before this (unfinished) rename.
                    pending.push(PendingRename {
                        frn: st.last.frn,
                        old_parent: st.last.parent,
                        old_name: st.last.name.clone(),
                    });
                }
                events.push(SourceEvent::Created(st.last));
                continue;
            }
            if st.awaiting_new_name {
                // The consumer still has the file at its pre-rename location; report nothing
                // about the rename until the new name is known.
                let (old_parent, old_name) = st
                    .old
                    .unwrap_or_else(|| (st.last.parent, st.last.name.clone()));
                if r & reason::MODIFIED_MASK != 0 {
                    events.push(SourceEvent::Modified(FileRecord {
                        frn: st.last.frn,
                        parent: old_parent,
                        name: old_name.clone(),
                        attrs: st.last.attrs,
                    }));
                }
                pending.push(PendingRename {
                    frn: st.last.frn,
                    old_parent,
                    old_name,
                });
                continue;
            }
            if st.unpaired_new_name {
                tracing::debug!(
                    frn = %st.last.frn, name = %st.last.name,
                    "rename without its old name in this batch; reporting as Created (upsert)"
                );
                events.push(SourceEvent::Created(st.last));
                continue;
            }
            let modified = r & reason::MODIFIED_MASK != 0;
            match st.old {
                Some((old_parent, old_name))
                    if old_parent != st.last.parent || old_name != st.last.name =>
                {
                    let record = st.last;
                    let modified_record = modified.then(|| record.clone());
                    events.push(SourceEvent::Renamed {
                        record,
                        old_parent,
                        old_name,
                    });
                    if let Some(rec) = modified_record {
                        events.push(SourceEvent::Modified(rec));
                    }
                }
                _ => {
                    if modified {
                        events.push(SourceEvent::Modified(st.last));
                    }
                }
            }
        }
        if pending.len() > MAX_PENDING_RENAMES {
            let excess = pending.len() - MAX_PENDING_RENAMES;
            tracing::warn!(
                dropped = excess,
                "too many unfinished renames to carry; their new names will be reported as Created"
            );
            pending.drain(..excess);
        }
        (events, pending)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::reason::*;
    use super::*;
    use crate::record::build::{RecordSpec, record};
    use crate::record::{RecordIter, parse_record};

    const ROOT: u64 = 0x0005_0000_0000_0005;
    const DIR: u64 = 0x0001_0000_0000_0040;
    const SUB: u64 = 0x0001_0000_0000_0041;
    const F: u64 = 0x0002_0000_0000_0050;
    const G: u64 = 0x0003_0000_0000_0051;

    fn spec(frn: u64, parent: u64, name: &str, reason: u32) -> RecordSpec {
        RecordSpec {
            major: 2,
            frn,
            parent_frn: parent,
            usn: 0,
            timestamp: 0,
            reason,
            file_attributes: 0x20,
            name: name.encode_utf16().collect(),
        }
    }

    fn rec(frn: u64, parent: u64, name: &str) -> FileRecord {
        FileRecord {
            frn: Frn(frn),
            parent: Frn(parent),
            name: FileName::from(name),
            attrs: FileAttributes(0x20),
        }
    }

    /// Runs one batch of synthetic records through the real parser and the coalescer.
    fn fold(
        carried: Vec<PendingRename>,
        specs: &[RecordSpec],
    ) -> (Vec<SourceEvent>, Vec<PendingRename>) {
        let bytes: Vec<u8> = specs.iter().flat_map(record).collect();
        let mut c = Coalescer::new(carried);
        for item in RecordIter::new(&bytes, 0) {
            c.push(&item.unwrap());
        }
        c.finish()
    }

    fn events(specs: &[RecordSpec]) -> Vec<SourceEvent> {
        let (events, pending) = fold(Vec::new(), specs);
        assert!(pending.is_empty(), "{pending:?}");
        events
    }

    #[test]
    fn create_write_close_is_one_created() {
        let ev = events(&[
            spec(F, DIR, "a.shp", FILE_CREATE),
            spec(F, DIR, "a.shp", FILE_CREATE | DATA_EXTEND),
            spec(F, DIR, "a.shp", FILE_CREATE | DATA_EXTEND | CLOSE),
        ]);
        assert_eq!(ev, [SourceEvent::Created(rec(F, DIR, "a.shp"))]);
    }

    #[test]
    fn data_change_is_modified() {
        for r in [
            DATA_OVERWRITE,
            DATA_EXTEND,
            DATA_TRUNCATION,
            NAMED_DATA_EXTEND,
            BASIC_INFO_CHANGE,
            STREAM_CHANGE,
            HARD_LINK_CHANGE,
        ] {
            let ev = events(&[spec(F, DIR, "a.prj", r), spec(F, DIR, "a.prj", r | CLOSE)]);
            assert_eq!(ev, [SourceEvent::Modified(rec(F, DIR, "a.prj"))], "{r:#x}");
        }
    }

    #[test]
    fn ignored_reasons_produce_nothing() {
        for r in [
            SECURITY_CHANGE,
            OBJECT_ID_CHANGE,
            INDEXABLE_CHANGE,
            TRANSACTED_CHANGE,
            INTEGRITY_CHANGE,
            DESIRED_STORAGE_CLASS_CHANGE,
            CLOSE,
        ] {
            assert!(
                events(&[spec(F, DIR, "a", r), spec(F, DIR, "a", r | CLOSE)]).is_empty(),
                "{r:#x}"
            );
        }
    }

    #[test]
    fn rename_pairs_old_and_new_names() {
        let ev = events(&[
            spec(F, DIR, "a.shp", RENAME_OLD_NAME),
            spec(F, DIR, "b.shp", RENAME_NEW_NAME),
            spec(F, DIR, "b.shp", RENAME_NEW_NAME | CLOSE),
        ]);
        assert_eq!(
            ev,
            [SourceEvent::Renamed {
                record: rec(F, DIR, "b.shp"),
                old_parent: Frn(DIR),
                old_name: FileName::from("a.shp"),
            }]
        );
    }

    #[test]
    fn move_reports_old_parent() {
        let ev = events(&[
            spec(F, DIR, "a.shp", RENAME_OLD_NAME),
            spec(F, SUB, "a.shp", RENAME_NEW_NAME),
            spec(F, SUB, "a.shp", RENAME_NEW_NAME | CLOSE),
        ]);
        assert_eq!(
            ev,
            [SourceEvent::Renamed {
                record: rec(F, SUB, "a.shp"),
                old_parent: Frn(DIR),
                old_name: FileName::from("a.shp"),
            }]
        );
    }

    #[test]
    fn rename_with_cumulative_old_flag_still_pairs() {
        // Whether or not NTFS keeps RENAME_OLD_NAME set on later records of the same handle, a
        // record with the new-name flag is the new name.
        let ev = events(&[
            spec(F, DIR, "a", RENAME_OLD_NAME),
            spec(F, DIR, "b", RENAME_OLD_NAME | RENAME_NEW_NAME),
            spec(F, DIR, "b", RENAME_OLD_NAME | RENAME_NEW_NAME | CLOSE),
        ]);
        assert_eq!(
            ev,
            [SourceEvent::Renamed {
                record: rec(F, DIR, "b"),
                old_parent: Frn(DIR),
                old_name: FileName::from("a"),
            }]
        );
    }

    #[test]
    fn chained_renames_fold_to_first_old_and_last_new() {
        let ev = events(&[
            spec(F, DIR, "a", RENAME_OLD_NAME),
            spec(F, DIR, "b", RENAME_NEW_NAME | CLOSE),
            spec(F, DIR, "b", RENAME_OLD_NAME),
            spec(F, SUB, "c", RENAME_NEW_NAME | CLOSE),
        ]);
        assert_eq!(
            ev,
            [SourceEvent::Renamed {
                record: rec(F, SUB, "c"),
                old_parent: Frn(DIR),
                old_name: FileName::from("a"),
            }]
        );
        // There and back again: no net rename.
        let ev = events(&[
            spec(F, DIR, "a", RENAME_OLD_NAME),
            spec(F, DIR, "b", RENAME_NEW_NAME | CLOSE),
            spec(F, DIR, "b", RENAME_OLD_NAME),
            spec(F, DIR, "a", RENAME_NEW_NAME | CLOSE),
        ]);
        assert!(ev.is_empty(), "{ev:?}");
    }

    #[test]
    fn rename_and_write_gives_renamed_then_modified() {
        let ev = events(&[
            spec(F, DIR, "a", DATA_EXTEND),
            spec(F, DIR, "a", DATA_EXTEND | CLOSE),
            spec(F, DIR, "a", RENAME_OLD_NAME),
            spec(F, DIR, "b", RENAME_NEW_NAME | CLOSE),
        ]);
        assert_eq!(
            ev,
            [
                SourceEvent::Renamed {
                    record: rec(F, DIR, "b"),
                    old_parent: Frn(DIR),
                    old_name: FileName::from("a"),
                },
                SourceEvent::Modified(rec(F, DIR, "b")),
            ]
        );
    }

    #[test]
    fn delete_wins_and_carries_last_state() {
        let ev = events(&[
            spec(F, DIR, "a", RENAME_OLD_NAME),
            spec(F, DIR, "b", RENAME_NEW_NAME | CLOSE),
            spec(F, DIR, "b", FILE_DELETE | CLOSE),
        ]);
        assert_eq!(ev, [SourceEvent::Deleted(rec(F, DIR, "b"))]);
    }

    #[test]
    fn create_then_delete_in_one_batch_is_deleted() {
        let ev = events(&[
            spec(F, DIR, "tmp", FILE_CREATE),
            spec(F, DIR, "tmp", FILE_CREATE | CLOSE),
            spec(F, DIR, "tmp", FILE_DELETE | CLOSE),
        ]);
        assert_eq!(ev, [SourceEvent::Deleted(rec(F, DIR, "tmp"))]);
    }

    #[test]
    fn create_then_rename_is_created_under_final_name() {
        let ev = events(&[
            spec(F, DIR, "~tmp", FILE_CREATE | DATA_EXTEND | CLOSE),
            spec(F, DIR, "~tmp", RENAME_OLD_NAME),
            spec(F, SUB, "a.tif", RENAME_NEW_NAME | CLOSE),
        ]);
        assert_eq!(ev, [SourceEvent::Created(rec(F, SUB, "a.tif"))]);
    }

    #[test]
    fn events_follow_each_files_first_record() {
        let ev = events(&[
            spec(SUB, DIR, "sub", FILE_CREATE),
            spec(F, SUB, "a", FILE_CREATE | CLOSE),
            spec(G, DIR, "g", DATA_EXTEND | CLOSE),
            spec(SUB, DIR, "sub", FILE_CREATE | CLOSE),
        ]);
        let frns: Vec<u64> = ev.iter().map(|e| e.record().frn.0).collect();
        assert_eq!(frns, [SUB, F, G]);
        assert!(matches!(ev[0], SourceEvent::Created(_)));
        assert!(matches!(ev[2], SourceEvent::Modified(_)));
    }

    #[test]
    fn rename_split_across_batches_is_carried_and_paired() {
        // Batch 1 ends between the two halves of the rename.
        let (ev, pending) = fold(
            Vec::new(),
            &[
                spec(G, DIR, "other", DATA_EXTEND | CLOSE),
                spec(F, DIR, "a.shp", RENAME_OLD_NAME),
            ],
        );
        assert_eq!(ev, [SourceEvent::Modified(rec(G, DIR, "other"))]);
        assert_eq!(
            pending,
            [PendingRename {
                frn: Frn(F),
                old_parent: Frn(DIR),
                old_name: FileName::from("a.shp"),
            }]
        );
        // Batch 2 starts with the new-name half.
        let (ev, pending) = fold(
            pending,
            &[
                spec(F, SUB, "b.shp", RENAME_NEW_NAME),
                spec(F, SUB, "b.shp", RENAME_NEW_NAME | CLOSE),
            ],
        );
        assert!(pending.is_empty());
        assert_eq!(
            ev,
            [SourceEvent::Renamed {
                record: rec(F, SUB, "b.shp"),
                old_parent: Frn(DIR),
                old_name: FileName::from("a.shp"),
            }]
        );
    }

    #[test]
    fn carried_renames_survive_empty_batches_only() {
        let carried = vec![PendingRename {
            frn: Frn(F),
            old_parent: Frn(DIR),
            old_name: FileName::from("a"),
        }];
        let (ev, pending) = fold(carried.clone(), &[]);
        assert!(ev.is_empty());
        assert_eq!(pending, carried);
        let (ev, pending) = fold(carried, &[spec(G, DIR, "g", FILE_CREATE | CLOSE)]);
        assert_eq!(ev.len(), 1);
        assert!(pending.is_empty(), "{pending:?}");
    }

    #[test]
    fn unpaired_new_name_is_reported_as_created() {
        let ev = events(&[
            spec(F, SUB, "b.shp", RENAME_NEW_NAME),
            spec(F, SUB, "b.shp", RENAME_NEW_NAME | CLOSE),
        ]);
        assert_eq!(ev, [SourceEvent::Created(rec(F, SUB, "b.shp"))]);
    }

    #[test]
    fn write_during_unfinished_rename_reports_the_known_name() {
        let (ev, pending) = fold(
            Vec::new(),
            &[
                spec(F, DIR, "a", DATA_EXTEND),
                spec(F, DIR, "a", DATA_EXTEND | RENAME_OLD_NAME),
            ],
        );
        assert_eq!(ev, [SourceEvent::Modified(rec(F, DIR, "a"))]);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].old_name, FileName::from("a"));
    }

    #[test]
    fn created_then_unfinished_rename_carries_the_created_name() {
        let (ev, pending) = fold(
            Vec::new(),
            &[
                spec(F, DIR, "new", FILE_CREATE | CLOSE),
                spec(F, DIR, "new", RENAME_OLD_NAME),
            ],
        );
        assert_eq!(ev, [SourceEvent::Created(rec(F, DIR, "new"))]);
        assert_eq!(pending[0].old_name, FileName::from("new"));
    }

    #[test]
    fn pending_renames_are_capped() {
        let specs: Vec<RecordSpec> = (0..MAX_PENDING_RENAMES as u64 + 5)
            .map(|i| spec(0x100 + i, DIR, "x", RENAME_OLD_NAME))
            .collect();
        let (ev, pending) = fold(Vec::new(), &specs);
        assert!(ev.is_empty());
        assert_eq!(pending.len(), MAX_PENDING_RENAMES);
        // The newest are kept.
        assert_eq!(
            pending.last().map(|p| p.frn),
            Some(Frn(0x100 + MAX_PENDING_RENAMES as u64 + 4))
        );
    }

    #[test]
    fn directories_and_lossless_names() {
        let mut s = spec(SUB, ROOT, "", FILE_CREATE | CLOSE);
        s.file_attributes = 0x10;
        s.name = vec![0xD800, u16::from(b'x')];
        let ev = events(&[s]);
        let SourceEvent::Created(r) = &ev[0] else {
            panic!("{ev:?}")
        };
        assert!(r.is_dir());
        assert_eq!(r.name.to_utf16(), [0xD800, u16::from(b'x')]);
    }

    #[test]
    fn file_record_matches_the_parsed_record() {
        let bytes = record(&spec(F, DIR, "Ortho 2024.tif", CLOSE));
        let (r, _) = parse_record(&bytes, 0).unwrap();
        assert_eq!(file_record(&r), rec(F, DIR, "Ortho 2024.tif"));
    }

    #[test]
    fn read_request_layout() {
        let b = encode_read_usn_journal_data_v1(0x0102_0304_0506_0708, 0x1122_3344_5566_7788);
        assert_eq!(&b[0..8], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(&b[8..12], &[0xFF; 4]);
        assert_eq!(&b[12..32], &[0; 20]);
        assert_eq!(&b[32..40], &0x1122_3344_5566_7788_u64.to_le_bytes());
        assert_eq!(&b[40..44], &[2, 0, 2, 0]);
        assert_eq!(&b[44..48], &[0; 4]);
    }

    #[test]
    fn read_steps() {
        use crate::record::build::enum_output;
        let r = record(&spec(F, DIR, "a", CLOSE));
        assert_eq!(
            read_step(100, &enum_output(200, std::slice::from_ref(&r))),
            Ok(ReadStep::Records { next_usn: 200 })
        );
        assert_eq!(
            read_step(100, &enum_output(150, &[])),
            Ok(ReadStep::Advance { next_usn: 150 })
        );
        assert_eq!(
            read_step(100, &enum_output(100, &[])),
            Ok(ReadStep::CaughtUp)
        );
        assert_eq!(
            read_step(100, &enum_output(100, std::slice::from_ref(&r))),
            Ok(ReadStep::Stalled { next_usn: 100 })
        );
        assert!(matches!(
            read_step(100, &[0; 5]),
            Err(ParseError::Truncated { .. })
        ));
        assert!(matches!(
            read_step(100, &enum_output(u64::MAX, &[])),
            Err(ParseError::UsnOutOfRange { .. })
        ));
        assert!(matches!(
            read_step(-1, &enum_output(0, &[])),
            Err(ParseError::UsnOutOfRange { .. })
        ));
    }

    /// Deterministic xorshift (no extra dependency).
    struct XorShift(u64);
    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn random_reason_sequences_never_panic_and_stay_bounded() {
        let mut rng = XorShift(0xDEAD_BEEF_CAFE_F00D);
        let frns = [F, G, SUB, DIR];
        for _ in 0..2_000 {
            let n = (rng.next() % 40) as usize;
            let specs: Vec<RecordSpec> = (0..n)
                .map(|_| {
                    let frn = frns[(rng.next() % 4) as usize];
                    let parent = frns[(rng.next() % 4) as usize];
                    let name = ["a", "b", "c"][(rng.next() % 3) as usize];
                    spec(frn, parent, name, rng.next() as u32)
                })
                .collect();
            let (ev, pending) = fold(Vec::new(), &specs);
            // At most two events (Renamed + Modified) per distinct file.
            assert!(ev.len() <= 2 * frns.len());
            assert!(pending.len() <= frns.len());
        }
    }
}
