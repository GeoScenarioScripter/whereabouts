//! Binary snapshots: the index plus the source checkpoint it is consistent with.
//!
//! Layout (version 1, little-endian):
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 8 | magic `WBNISNAP` |
//! | 8 | 4 | format version (`1`) |
//! | 12 | 4 | flags (`0`) |
//! | 16 | 8 | root FRN |
//! | 24 | 4 | root attributes |
//! | 28 | 4 | checkpoint length `c` (0 = none) |
//! | 32 | 8 | entry count `n` (live entries, excluding the root) |
//! | 40 | c | `Checkpoint::to_bytes()` (opaque to the index) |
//! | … | | `n` × { frn u64, parent frn u64, attributes u32, name length u16, name (WTF-8) } |
//! | … | 4 | CRC-32 (IEEE) of every preceding byte |
//!
//! Entries are stored by FRN, not by index position, and loading re-inserts them through the
//! normal insert path, so a snapshot can never smuggle in an inconsistent in-memory layout; every
//! field is validated and allocations are bounded regardless of the header's claims. Any error
//! means "discard and rescan".

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use whereabouts_core::{FileAttributes, Frn};
use whereabouts_source::Checkpoint;

use crate::error::SnapshotError;
use crate::fold;
use crate::index::{MAX_NAME_BYTES, NameIndex};

const MAGIC: [u8; 8] = *b"WBNISNAP";
/// Current snapshot format version.
pub const SNAPSHOT_VERSION: u32 = 1;
const HEADER_LEN: usize = 40;
/// Largest serialised checkpoint (`Checkpoint` header + its 64 KiB payload limit).
const MAX_CHECKPOINT_BYTES: usize = 12 + Checkpoint::MAX_DATA_LEN;
/// Upper bound on up-front allocation while loading, whatever the header says.
const MAX_PREALLOC_ENTRIES: usize = 1 << 20;

/// A loaded snapshot.
#[derive(Debug)]
pub struct Snapshot {
    /// The index, compact (no tombstones).
    pub index: NameIndex,
    /// The checkpoint stored with it: replay source changes from here.
    pub checkpoint: Option<Checkpoint>,
}

impl NameIndex {
    /// Writes a snapshot of the live entries plus `checkpoint`; returns the bytes written.
    ///
    /// # Errors
    /// [`SnapshotError::Io`].
    pub fn write_snapshot<W: Write>(
        &self,
        writer: W,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<u64, SnapshotError> {
        let mut w = CrcWriter {
            inner: BufWriter::with_capacity(1 << 20, writer),
            crc: crc32fast::Hasher::new(),
            written: 0,
        };
        let cp = checkpoint.map(Checkpoint::to_bytes).unwrap_or_default();
        w.write_all(&MAGIC)?;
        w.write_all(&SNAPSHOT_VERSION.to_le_bytes())?;
        w.write_all(&0_u32.to_le_bytes())?;
        w.write_all(&self.root_frn.0.to_le_bytes())?;
        w.write_all(&self.attrs.first().copied().unwrap_or(0).to_le_bytes())?;
        w.write_all(&(cp.len() as u32).to_le_bytes())?;
        w.write_all(&(self.len() as u64).to_le_bytes())?;
        w.write_all(&cp)?;
        for id in self.entries() {
            let i = id.index();
            let name = self.name_bytes(i);
            w.write_all(&self.frn.get(i).map_or(0, |f| f.0).to_le_bytes())?;
            w.write_all(&self.parent_frn(i).0.to_le_bytes())?;
            w.write_all(&self.attrs.get(i).copied().unwrap_or(0).to_le_bytes())?;
            w.write_all(&(name.len() as u16).to_le_bytes())?;
            w.write_all(name)?;
        }
        let crc = w.crc.clone().finalize();
        w.inner.write_all(&crc.to_le_bytes())?;
        w.inner.flush()?;
        Ok(w.written + 4)
    }

    /// Reads a snapshot written by [`NameIndex::write_snapshot`].
    ///
    /// # Errors
    /// Any [`SnapshotError`]; the index must then be rebuilt by a full enumeration.
    pub fn read_snapshot<R: Read>(reader: R) -> Result<Snapshot, SnapshotError> {
        let mut r = CrcReader {
            inner: BufReader::with_capacity(1 << 20, reader),
            crc: crc32fast::Hasher::new(),
        };
        let mut header = [0_u8; HEADER_LEN];
        r.read_exact(&mut header).map_err(eof)?;
        let field = |at: usize, len: usize| header.get(at..at + len).unwrap_or_default();
        if field(0, 8) != MAGIC {
            return Err(SnapshotError::BadMagic);
        }
        let version = le_u32(field(8, 4));
        if version != SNAPSHOT_VERSION {
            return Err(SnapshotError::UnsupportedVersion(version));
        }
        if le_u32(field(12, 4)) != 0 {
            return Err(SnapshotError::Corrupt("unknown flags".into()));
        }
        let root = Frn(le_u64(field(16, 8)));
        let root_attrs = le_u32(field(24, 4));
        let cp_len = le_u32(field(28, 4)) as usize;
        let count = le_u64(field(32, 8));
        if cp_len > MAX_CHECKPOINT_BYTES {
            return Err(SnapshotError::Corrupt("checkpoint too large".into()));
        }
        let checkpoint = if cp_len == 0 {
            None
        } else {
            let mut cp = vec![0_u8; cp_len];
            r.read_exact(&mut cp).map_err(eof)?;
            Some(Checkpoint::from_bytes(&cp)?)
        };
        let prealloc = usize::try_from(count)
            .unwrap_or(usize::MAX)
            .min(MAX_PREALLOC_ENTRIES);
        let mut index = NameIndex::with_capacity(root, prealloc, prealloc.saturating_mul(24));
        index
            .upsert(root, root, &[], FileAttributes(root_attrs))
            .map_err(corrupt)?;
        let mut fixed = [0_u8; 22];
        let mut name = Vec::with_capacity(256);
        for _ in 0..count {
            r.read_exact(&mut fixed).map_err(eof)?;
            let f = |at: usize, len: usize| fixed.get(at..at + len).unwrap_or_default();
            let frn = Frn(le_u64(f(0, 8)));
            let parent = Frn(le_u64(f(8, 8)));
            let attrs = FileAttributes(le_u32(f(16, 4)));
            let len = usize::from(le_u16(f(20, 2)));
            if len > MAX_NAME_BYTES {
                return Err(SnapshotError::Corrupt(format!("name of {len} bytes")));
            }
            name.resize(len, 0);
            r.read_exact(&mut name).map_err(eof)?;
            if !fold::is_wtf8(&name) {
                return Err(SnapshotError::Corrupt(format!("invalid name for {frn}")));
            }
            if frn == root || index.lookup(frn).is_some() {
                return Err(SnapshotError::Corrupt(format!("duplicate entry {frn}")));
            }
            index.upsert(frn, parent, &name, attrs).map_err(corrupt)?;
        }
        let computed = r.crc.clone().finalize();
        let mut trailer = [0_u8; 4];
        r.inner.read_exact(&mut trailer).map_err(eof)?;
        let stored = u32::from_le_bytes(trailer);
        if stored != computed {
            return Err(SnapshotError::ChecksumMismatch { stored, computed });
        }
        let mut probe = [0_u8; 1];
        if r.inner.read(&mut probe)? != 0 {
            return Err(SnapshotError::TrailingBytes);
        }
        Ok(Snapshot { index, checkpoint })
    }

    /// Writes a snapshot to `path` atomically: a sibling `.tmp` file is written, flushed to disk
    /// and renamed over `path`.
    ///
    /// # Errors
    /// [`SnapshotError::Io`].
    pub fn save_snapshot(
        &self,
        path: &Path,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<u64, SnapshotError> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        let result = (|| {
            let file = File::create(&tmp)?;
            let written = self.write_snapshot(&file, checkpoint)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, path)?;
            Ok(written)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Loads a snapshot from `path`.
    ///
    /// # Errors
    /// As [`NameIndex::read_snapshot`].
    pub fn load_snapshot(path: &Path) -> Result<Snapshot, SnapshotError> {
        Self::read_snapshot(File::open(path)?)
    }
}

fn eof(e: io::Error) -> SnapshotError {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        SnapshotError::Truncated
    } else {
        SnapshotError::Io(e)
    }
}

fn corrupt(e: crate::IndexError) -> SnapshotError {
    SnapshotError::Corrupt(e.to_string())
}

fn le_u16(b: &[u8]) -> u16 {
    let mut a = [0_u8; 2];
    a.copy_from_slice(b.get(..2).unwrap_or(&[0; 2]));
    u16::from_le_bytes(a)
}

fn le_u32(b: &[u8]) -> u32 {
    let mut a = [0_u8; 4];
    a.copy_from_slice(b.get(..4).unwrap_or(&[0; 4]));
    u32::from_le_bytes(a)
}

fn le_u64(b: &[u8]) -> u64 {
    let mut a = [0_u8; 8];
    a.copy_from_slice(b.get(..8).unwrap_or(&[0; 8]));
    u64::from_le_bytes(a)
}

struct CrcWriter<W: Write> {
    inner: W,
    crc: crc32fast::Hasher,
    written: u64,
}

impl<W: Write> Write for CrcWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.crc.update(buf.get(..n).unwrap_or_default());
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct CrcReader<R: Read> {
    inner: R,
    crc: crc32fast::Hasher,
}

impl<R: Read> Read for CrcReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.crc.update(buf.get(..n).unwrap_or_default());
        Ok(n)
    }
}
