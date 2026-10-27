//! The NTFS source's [`Checkpoint`] payload, and the check that decides whether the change
//! journal still holds the history since a checkpoint. Pure and cross-platform.
//!
//! # Payload, version [`CHECKPOINT_VERSION`] (little-endian)
//!
//! | Bytes | Field |
//! |---|---|
//! | 4 | volume serial number |
//! | 1 | 1 = a journal was active, 0 = none (then the next 16 bytes are zero) |
//! | 3 | zero |
//! | 8 | journal ID |
//! | 8 | next USN to read |
//! | 2 | number of pending renames (≤ [`MAX_PENDING_RENAMES`]) |
//! | per rename | FRN (8), old parent FRN (8), name length (2), name (WTF-8, ≤ [`MAX_NAME_BYTES`]) |
//!
//! The largest payload is 26 + 64 × 783 bytes ≈ 49 KiB, inside the contract's 64 KiB limit.

use whereabouts_core::{FileName, Frn};
use whereabouts_source::{Checkpoint, CheckpointError, RescanReason};

use crate::journal::{MAX_PENDING_RENAMES, PendingRename};
use crate::record::JournalInfo;

/// Payload format version written into [`Checkpoint::version`].
pub const CHECKPOINT_VERSION: u32 = 1;

/// Longest name carried for a pending rename: 255 UTF-16 units, 3 WTF-8 bytes each.
pub const MAX_NAME_BYTES: usize = 765;

const FIXED_LEN: usize = 26;

/// Where to resume reading a volume's change journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalPosition {
    /// The journal instance the USN belongs to.
    pub journal_id: u64,
    /// The next USN to read.
    pub next_usn: i64,
}

/// The decoded payload of an NTFS source checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtfsCheckpoint {
    /// Serial number of the volume the checkpoint was taken on.
    pub volume_serial: u32,
    /// Journal position; `None` when the volume had no active journal.
    pub journal: Option<JournalPosition>,
    /// Renames whose new name had not been read yet (see [`crate::journal`]).
    pub pending_renames: Vec<PendingRename>,
}

/// A checkpoint payload that this source cannot use.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CheckpointDecodeError {
    /// Written by another payload version.
    #[error("unsupported NTFS checkpoint version {0}")]
    Version(u32),
    /// The payload does not have the documented layout.
    #[error("malformed NTFS checkpoint: {0}")]
    Malformed(&'static str),
}

impl NtfsCheckpoint {
    /// Encodes the payload. Pending renames beyond [`MAX_PENDING_RENAMES`] (the oldest) and
    /// names longer than [`MAX_NAME_BYTES`] are left out, so the result always fits.
    ///
    /// # Errors
    /// Only if the payload exceeded the contract's size limit, which the caps above prevent.
    pub fn to_checkpoint(&self) -> Result<Checkpoint, CheckpointError> {
        let renames: Vec<&PendingRename> = self
            .pending_renames
            .iter()
            .filter(|p| {
                let fits = p.old_name.len() <= MAX_NAME_BYTES;
                if !fits {
                    tracing::warn!(
                        frn = %p.frn, name_bytes = p.old_name.len(),
                        "pending rename's old name too long to carry; its new name will be \
                         reported as Created"
                    );
                }
                fits
            })
            .collect();
        let skip = renames.len().saturating_sub(MAX_PENDING_RENAMES);
        if skip > 0 {
            tracing::warn!(
                dropped = skip,
                "too many pending renames to carry; their new names will be reported as Created"
            );
        }
        let renames = &renames[skip..];
        let mut out = Vec::with_capacity(FIXED_LEN + renames.len() * 32);
        out.extend_from_slice(&self.volume_serial.to_le_bytes());
        let (flag, id, usn) = match self.journal {
            Some(j) => (1_u8, j.journal_id, j.next_usn),
            None => (0, 0, 0),
        };
        out.extend_from_slice(&[flag, 0, 0, 0]);
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&usn.to_le_bytes());
        // Lossless: at most MAX_PENDING_RENAMES (64) entries.
        out.extend_from_slice(&(renames.len() as u16).to_le_bytes());
        for p in renames {
            out.extend_from_slice(&p.frn.0.to_le_bytes());
            out.extend_from_slice(&p.old_parent.0.to_le_bytes());
            // Lossless: at most MAX_NAME_BYTES (765).
            out.extend_from_slice(&(p.old_name.len() as u16).to_le_bytes());
            out.extend_from_slice(p.old_name.as_wtf8());
        }
        Checkpoint::new(CHECKPOINT_VERSION, out)
    }

    /// Decodes a checkpoint produced by [`NtfsCheckpoint::to_checkpoint`], validating every
    /// length and the WTF-8 of every name.
    ///
    /// # Errors
    /// [`CheckpointDecodeError`] for another version or any malformed payload.
    pub fn from_checkpoint(cp: &Checkpoint) -> Result<Self, CheckpointDecodeError> {
        if cp.version() != CHECKPOINT_VERSION {
            return Err(CheckpointDecodeError::Version(cp.version()));
        }
        let mut r = Reader(cp.data());
        let volume_serial = r.u32()?;
        let flags = r.take(4)?;
        let journal_id = r.u64()?;
        let next_usn = r.u64()? as i64;
        let journal = match flags {
            [1, 0, 0, 0] if next_usn >= 0 => Some(JournalPosition {
                journal_id,
                next_usn,
            }),
            [0, 0, 0, 0] if journal_id == 0 && next_usn == 0 => None,
            _ => return Err(CheckpointDecodeError::Malformed("journal flags")),
        };
        let count = usize::from(r.u16()?);
        if count > MAX_PENDING_RENAMES {
            return Err(CheckpointDecodeError::Malformed("too many pending renames"));
        }
        let mut pending_renames = Vec::with_capacity(count);
        for _ in 0..count {
            let frn = Frn(r.u64()?);
            let old_parent = Frn(r.u64()?);
            let len = usize::from(r.u16()?);
            if len > MAX_NAME_BYTES {
                return Err(CheckpointDecodeError::Malformed("name too long"));
            }
            let old_name = FileName::from_wtf8(r.take(len)?)
                .map_err(|_| CheckpointDecodeError::Malformed("name is not WTF-8"))?;
            pending_renames.push(PendingRename {
                frn,
                old_parent,
                old_name,
            });
        }
        if !r.0.is_empty() {
            return Err(CheckpointDecodeError::Malformed("trailing bytes"));
        }
        Ok(Self {
            volume_serial,
            journal,
            pending_renames,
        })
    }
}

/// Bounds-checked little-endian reader.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CheckpointDecodeError> {
        if self.0.len() < n {
            return Err(CheckpointDecodeError::Malformed("truncated"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CheckpointDecodeError> {
        let mut out = [0_u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16, CheckpointDecodeError> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, CheckpointDecodeError> {
        self.array().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64, CheckpointDecodeError> {
        self.array().map(u64::from_le_bytes)
    }
}

/// Decides whether the journal still holds everything since `checkpoint`, given the volume's
/// current serial number and journal state. Returns where to resume reading, or why a full
/// rescan is required:
///
/// | Situation | Result |
/// |---|---|
/// | volume serial differs | [`RescanReason::CheckpointMismatch`] |
/// | no active journal now | [`RescanReason::JournalMissing`] |
/// | no journal at checkpoint time, one now | [`RescanReason::JournalRecreated`] |
/// | journal ID differs | [`RescanReason::JournalRecreated`] |
/// | checkpoint USN < the journal's `FirstUsn` | [`RescanReason::JournalWrapped`] |
/// | checkpoint USN > the journal's `NextUsn` | [`RescanReason::CheckpointMismatch`] |
///
/// # Errors
/// The [`RescanReason`] when the history is unavailable.
pub fn assess(
    checkpoint: &NtfsCheckpoint,
    volume_serial: u32,
    journal: Option<&JournalInfo>,
) -> Result<JournalPosition, RescanReason> {
    if checkpoint.volume_serial != volume_serial {
        return Err(RescanReason::CheckpointMismatch);
    }
    let Some(journal) = journal else {
        return Err(RescanReason::JournalMissing);
    };
    let Some(pos) = checkpoint.journal else {
        return Err(RescanReason::JournalRecreated);
    };
    if pos.journal_id != journal.journal_id {
        return Err(RescanReason::JournalRecreated);
    }
    if pos.next_usn < journal.first_usn {
        return Err(RescanReason::JournalWrapped);
    }
    if pos.next_usn > journal.next_usn {
        return Err(RescanReason::CheckpointMismatch);
    }
    Ok(pos)
}

/// Default journal margin replayed after a raw `$MFT` scan (1 MiB of journal).
pub const RAW_SCAN_REPLAY_MARGIN: u64 = 1 << 20;

/// The USN to put in the checkpoint of a raw `$MFT` scan: `next_usn` moved back by `margin`
/// bytes of journal, but never before `first_usn`.
///
/// Raw volume reads can miss changes NTFS has not written to disk yet, so changes from just
/// before the scan are replayed by the next `changes` call. Replay is idempotent for consumers
/// (upserts, delete wins, D-29). The result is arithmetic only: it need **not** be a record
/// boundary, and `FSCTL_READ_USN_JOURNAL` rejects a start USN that is not one
/// (`ERROR_INVALID_PARAMETER`, seen on a VHD), so the source snaps it to a record first.
#[must_use]
pub fn replay_start(first_usn: i64, next_usn: i64, margin: u64) -> i64 {
    let available = u64::try_from(next_usn.saturating_sub(first_usn).max(0)).unwrap_or(0);
    let back = i64::try_from(margin.min(available)).unwrap_or(i64::MAX);
    next_usn.saturating_sub(back).max(first_usn)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn replay_start_backs_off_but_stays_inside_the_journal() {
        // Plenty of journal: back by exactly the margin.
        assert_eq!(replay_start(100, 5_000_000, 1 << 20), 5_000_000 - (1 << 20));
        // Less journal than the margin: start at the first record.
        assert_eq!(replay_start(100, 500, 1 << 20), 100);
        // Empty journal and zero margin leave the position alone.
        assert_eq!(replay_start(700, 700, 1 << 20), 700);
        assert_eq!(replay_start(0, 4096, 0), 4096);
        // Nonsense input (next before first) never goes below `first`/panics.
        assert_eq!(replay_start(1000, 10, 50), 1000);
        // Extreme input must not panic.
        let _ = replay_start(i64::MIN, i64::MAX, u64::MAX);
        // The result is always acceptable to `assess`.
        let j = journal(1, 100, 5_000_000);
        let cp = NtfsCheckpoint {
            volume_serial: 7,
            journal: Some(JournalPosition {
                journal_id: 1,
                next_usn: replay_start(j.first_usn, j.next_usn, 1 << 20),
            }),
            pending_renames: vec![],
        };
        assert!(assess(&cp, 7, Some(&j)).is_ok());
    }

    fn journal(id: u64, first: i64, next: i64) -> JournalInfo {
        JournalInfo {
            journal_id: id,
            first_usn: first,
            next_usn: next,
            lowest_valid_usn: 0,
            max_usn: i64::MAX,
            maximum_size: 32 << 20,
            allocation_delta: 8 << 20,
            min_supported_major_version: Some(2),
            max_supported_major_version: Some(4),
        }
    }

    fn sample() -> NtfsCheckpoint {
        NtfsCheckpoint {
            volume_serial: 0xDEAD_BEEF,
            journal: Some(JournalPosition {
                journal_id: 0x01D9_0000_0000_0001,
                next_usn: 123_456,
            }),
            pending_renames: vec![
                PendingRename {
                    frn: Frn(0x0002_0000_0000_0050),
                    old_parent: Frn(0x0005_0000_0000_0005),
                    old_name: FileName::from("roads.shp"),
                },
                PendingRename {
                    frn: Frn(0x51),
                    old_parent: Frn(0x40),
                    old_name: FileName::from_utf16(&[0xD800, 0x41]),
                },
            ],
        }
    }

    #[test]
    fn round_trips_through_checkpoint_bytes() {
        let cp = sample();
        let encoded = cp.to_checkpoint().unwrap();
        assert_eq!(encoded.version(), CHECKPOINT_VERSION);
        let again = Checkpoint::from_bytes(&encoded.to_bytes()).unwrap();
        assert_eq!(NtfsCheckpoint::from_checkpoint(&again).unwrap(), cp);

        let none = NtfsCheckpoint {
            volume_serial: 7,
            journal: None,
            pending_renames: Vec::new(),
        };
        let encoded = none.to_checkpoint().unwrap();
        assert_eq!(encoded.data().len(), FIXED_LEN);
        assert_eq!(NtfsCheckpoint::from_checkpoint(&encoded).unwrap(), none);
    }

    #[test]
    fn worst_case_fits_the_contract_limit() {
        let long = FileName::from_utf16(&[0x6771; 255]); // 255 × 3-byte characters
        assert_eq!(long.len(), MAX_NAME_BYTES);
        let cp = NtfsCheckpoint {
            pending_renames: (0..MAX_PENDING_RENAMES as u64 + 10)
                .map(|i| PendingRename {
                    frn: Frn(i),
                    old_parent: Frn(5),
                    old_name: long.clone(),
                })
                .collect(),
            ..sample()
        };
        let encoded = cp.to_checkpoint().unwrap();
        assert!(encoded.data().len() <= Checkpoint::MAX_DATA_LEN);
        let decoded = NtfsCheckpoint::from_checkpoint(&encoded).unwrap();
        assert_eq!(decoded.pending_renames.len(), MAX_PENDING_RENAMES);
        assert_eq!(decoded.pending_renames[0].frn, Frn(10)); // the oldest were dropped
    }

    #[test]
    fn rejects_other_versions_and_malformed_payloads() {
        let good = sample().to_checkpoint().unwrap();
        let other = Checkpoint::new(2, good.data().to_vec()).unwrap();
        assert_eq!(
            NtfsCheckpoint::from_checkpoint(&other),
            Err(CheckpointDecodeError::Version(2))
        );
        // Every truncation, and trailing bytes.
        for cut in 0..good.data().len() {
            let cp = Checkpoint::new(1, good.data()[..cut].to_vec()).unwrap();
            assert!(NtfsCheckpoint::from_checkpoint(&cp).is_err(), "cut {cut}");
        }
        let mut long = good.data().to_vec();
        long.push(0);
        assert!(NtfsCheckpoint::from_checkpoint(&Checkpoint::new(1, long).unwrap()).is_err());
        // Bad flags, negative USN, invalid WTF-8, count over the cap, name over the cap.
        let mutate = |at: usize, bytes: &[u8]| {
            let mut d = good.data().to_vec();
            d[at..at + bytes.len()].copy_from_slice(bytes);
            NtfsCheckpoint::from_checkpoint(&Checkpoint::new(1, d).unwrap())
        };
        assert!(mutate(4, &[2]).is_err());
        assert!(mutate(5, &[1]).is_err());
        assert!(mutate(16, &(-5_i64).to_le_bytes()).is_err());
        assert!(mutate(24, &65_u16.to_le_bytes()).is_err());
        assert!(mutate(42, &766_u16.to_le_bytes()).is_err());
        assert!(mutate(44, &[0xFF]).is_err());
        // A "no journal" payload with a non-zero USN.
        let mut d = vec![0_u8; FIXED_LEN];
        d[16] = 1;
        assert!(NtfsCheckpoint::from_checkpoint(&Checkpoint::new(1, d).unwrap()).is_err());
    }

    #[test]
    fn random_payloads_never_panic() {
        let good = sample().to_checkpoint().unwrap().data().to_vec();
        let mut x = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..20_000 {
            let mut d = good.clone();
            for _ in 0..3 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let at = (x as usize) % d.len();
                d[at] = (x >> 32) as u8;
            }
            let _ = NtfsCheckpoint::from_checkpoint(&Checkpoint::new(1, d).unwrap());
        }
    }

    #[test]
    fn assess_decides_rescans() {
        let cp = sample();
        let id = 0x01D9_0000_0000_0001;
        let ok = journal(id, 1_000, 200_000);
        assert_eq!(
            assess(&cp, 0xDEAD_BEEF, Some(&ok)),
            Ok(JournalPosition {
                journal_id: id,
                next_usn: 123_456
            })
        );
        // Caught up exactly.
        assert!(assess(&cp, 0xDEAD_BEEF, Some(&journal(id, 1_000, 123_456))).is_ok());
        assert_eq!(
            assess(&cp, 1, Some(&ok)),
            Err(RescanReason::CheckpointMismatch)
        );
        assert_eq!(
            assess(&cp, 0xDEAD_BEEF, None),
            Err(RescanReason::JournalMissing)
        );
        assert_eq!(
            assess(&cp, 0xDEAD_BEEF, Some(&journal(id + 1, 0, 200_000))),
            Err(RescanReason::JournalRecreated)
        );
        assert_eq!(
            assess(&cp, 0xDEAD_BEEF, Some(&journal(id, 123_457, 200_000))),
            Err(RescanReason::JournalWrapped)
        );
        assert_eq!(
            assess(&cp, 0xDEAD_BEEF, Some(&journal(id, 0, 100_000))),
            Err(RescanReason::CheckpointMismatch)
        );
        let no_journal = NtfsCheckpoint {
            journal: None,
            ..sample()
        };
        assert_eq!(
            assess(&no_journal, 0xDEAD_BEEF, Some(&ok)),
            Err(RescanReason::JournalRecreated)
        );
        assert_eq!(
            assess(&no_journal, 0xDEAD_BEEF, None),
            Err(RescanReason::JournalMissing)
        );
    }
}
