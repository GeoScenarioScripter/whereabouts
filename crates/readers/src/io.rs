//! Bounded, read-only file access shared by the readers, and a per-thread I/O meter.
//!
//! Every byte a reader reads goes through [`SourceFile`], which
//! - opens files read-only (std's default share mode on Windows allows other readers, writers and
//!   deletes, so we never lock a user's file),
//! - charges each read against the dataset's [`Budget`] (from [`ReadLimits::max_bytes`]) *before*
//!   reading, so a reader can never read more than it was allowed, and
//! - records what it read in a thread-local meter that [`measure`] exposes. Tests use it to assert
//!   that reads stay bounded; benchmarks use it to report bytes read per dataset.
//!
//! [`ReadLimits::max_bytes`]: whereabouts_core::ReadLimits::max_bytes

use std::cell::Cell;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use whereabouts_core::ReadError;

/// What the readers on one thread did while [`measure`] ran.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoStats {
    /// Bytes read from member files.
    pub bytes_read: u64,
    /// Files opened.
    pub files_opened: u32,
    /// Read calls (each is one seek + one bounded read).
    pub reads: u32,
}

impl IoStats {
    const ZERO: IoStats = IoStats {
        bytes_read: 0,
        files_opened: 0,
        reads: 0,
    };

    fn since(self, earlier: IoStats) -> IoStats {
        IoStats {
            bytes_read: self.bytes_read.saturating_sub(earlier.bytes_read),
            files_opened: self.files_opened.saturating_sub(earlier.files_opened),
            reads: self.reads.saturating_sub(earlier.reads),
        }
    }
}

thread_local! {
    static METER: Cell<IoStats> = const { Cell::new(IoStats::ZERO) };
}

fn meter(update: impl FnOnce(&mut IoStats)) {
    METER.with(|m| {
        let mut stats = m.get();
        update(&mut stats);
        m.set(stats);
    });
}

/// Runs `f` and returns its result with the I/O the readers performed on this thread meanwhile.
///
/// Readers are synchronous and stateless, so wrapping one [`FormatReader::read`] call measures
/// exactly one dataset. Nested calls work (each sees its own delta).
///
/// [`FormatReader::read`]: whereabouts_core::FormatReader::read
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, IoStats) {
    let before = METER.with(Cell::get);
    let result = f();
    let after = METER.with(Cell::get);
    (result, after.since(before))
}

/// The byte budget of one dataset read ([`ReadLimits::max_bytes`]), shared by all its members.
///
/// [`ReadLimits::max_bytes`]: whereabouts_core::ReadLimits::max_bytes
#[derive(Debug)]
pub(crate) struct Budget {
    limit: u64,
    used: Cell<u64>,
}

impl Budget {
    /// A budget of `limit` bytes.
    pub(crate) fn new(limit: u64) -> Self {
        Budget {
            limit,
            used: Cell::new(0),
        }
    }

    /// Reserves `n` bytes, or fails without reserving anything if that would exceed the limit.
    fn charge(&self, n: u64) -> io::Result<()> {
        let used = self.used.get().saturating_add(n);
        if used > self.limit {
            return Err(io::Error::other(format!(
                "read budget of {} bytes exhausted",
                self.limit
            )));
        }
        self.used.set(used);
        Ok(())
    }
}

/// A member file opened read-only, with its length taken at open time.
#[derive(Debug)]
pub(crate) struct SourceFile<'a> {
    file: File,
    path: &'a Path,
    len: u64,
    budget: &'a Budget,
}

impl<'a> SourceFile<'a> {
    /// Opens `path` read-only. Fails for anything that is not a regular file.
    pub(crate) fn open(path: &'a Path, budget: &'a Budget) -> Result<Self, ReadError> {
        let io_err = |source| ReadError::Io {
            path: path.to_owned(),
            source,
        };
        let file = File::open(path).map_err(io_err)?;
        let meta = file.metadata().map_err(io_err)?;
        if !meta.is_file() {
            return Err(io_err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            )));
        }
        meter(|m| m.files_opened = m.files_opened.saturating_add(1));
        Ok(SourceFile {
            file,
            path,
            len: meta.len(),
            budget,
        })
    }

    /// File length in bytes when it was opened.
    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    /// Reads exactly `len` bytes at `offset`. Callers bound `len` by a constant and check it
    /// against [`len`](Self::len) first; a file that shrank since opening gives an I/O error.
    pub(crate) fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, ReadError> {
        let mut buf = vec![0; len];
        self.read_into(offset, &mut buf)?;
        Ok(buf)
    }

    /// Reads up to `max` bytes from the start of the file (fewer if the file is shorter).
    pub(crate) fn read_prefix(&mut self, max: usize) -> Result<Vec<u8>, ReadError> {
        let n = usize::try_from(self.len).map_or(max, |len| len.min(max));
        self.read_at(0, n)
    }

    fn read_into(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), ReadError> {
        let n = buf.len() as u64;
        let result = self
            .budget
            .charge(n)
            .and_then(|()| self.file.seek(SeekFrom::Start(offset)))
            .and_then(|_| self.file.read_exact(buf));
        meter(|m| m.reads = m.reads.saturating_add(1));
        result.map_err(|source| ReadError::Io {
            path: self.path.to_owned(),
            source,
        })?;
        meter(|m| m.bytes_read = m.bytes_read.saturating_add(n));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn budget_is_charged_before_reading_and_meter_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, [7u8; 300]).unwrap();

        let budget = Budget::new(250);
        let (result, stats) = measure(|| {
            let mut f = SourceFile::open(&path, &budget).unwrap();
            assert_eq!(f.len(), 300);
            assert_eq!(f.read_at(10, 100).unwrap(), vec![7u8; 100]);
            assert_eq!(f.read_prefix(100).unwrap().len(), 100);
            // 200 used; another 100 would exceed 250 and must not be read.
            f.read_at(0, 100)
        });
        assert!(result.unwrap_err().to_string().contains("budget"));
        assert_eq!(
            stats,
            IoStats {
                bytes_read: 200,
                files_opened: 1,
                reads: 3
            }
        );
    }

    #[test]
    fn prefix_of_short_file_and_eof_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, [1u8, 2, 3]).unwrap();
        let budget = Budget::new(1 << 20);
        let mut f = SourceFile::open(&path, &budget).unwrap();
        assert_eq!(f.read_prefix(100).unwrap(), vec![1, 2, 3]);
        assert!(f.read_at(2, 5).is_err());
    }

    #[test]
    fn directories_and_missing_files_are_io_errors() {
        let dir = tempfile::tempdir().unwrap();
        let budget = Budget::new(1 << 20);
        assert!(matches!(
            SourceFile::open(dir.path(), &budget),
            Err(ReadError::Io { .. })
        ));
        let missing = dir.path().join("missing");
        assert!(matches!(
            SourceFile::open(&missing, &budget),
            Err(ReadError::Io { .. })
        ));
    }

    #[test]
    fn nested_measure_reports_deltas() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, [0u8; 64]).unwrap();
        let budget = Budget::new(1 << 20);
        let (((), inner), outer) = measure(|| {
            let mut f = SourceFile::open(&path, &budget).unwrap();
            f.read_at(0, 8).unwrap();
            measure(|| {
                f.read_at(0, 16).unwrap();
            })
        });
        assert_eq!(inner.bytes_read, 16);
        assert_eq!(outer.bytes_read, 24);
    }
}
