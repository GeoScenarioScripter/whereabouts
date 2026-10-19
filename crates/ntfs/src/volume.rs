//! Raw NTFS volume access (Windows only): open, identify, query the journal, enumerate.
//!
//! Everything here is read-only. The volume is opened with `GENERIC_READ` only, and the crate
//! never issues `FSCTL_CREATE_USN_JOURNAL`/`FSCTL_DELETE_USN_JOURNAL` or any other write.

use std::fmt;

use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle, GetVolumeInformationW, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL};
use windows::core::HSTRING;

use crate::drive::{device_path, parse_drive_letter, root_path};
use crate::error::{Error, Result, win32};
use crate::record::{
    self, JournalInfo, MFT_ENUM_DATA_V1_SIZE, RawRecord, RecordIter, USN_JOURNAL_DATA_V2_SIZE,
};

/// What `GetVolumeInformationW` reports about a volume. Needs no elevation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// Upper-case drive letter.
    pub letter: char,
    /// Volume label (may be empty).
    pub label: String,
    /// Volume serial number (changes when the volume is re-formatted).
    pub serial_number: u32,
    /// File-system name, e.g. `NTFS`, `ReFS`, `FAT32`, `exFAT`.
    pub file_system: String,
    /// Maximum file-name component length.
    pub max_component_length: u32,
    /// `FILE_*` file-system flags.
    pub flags: u32,
}

impl VolumeInfo {
    /// `true` when the file system is NTFS.
    #[must_use]
    pub fn is_ntfs(&self) -> bool {
        self.file_system.eq_ignore_ascii_case("NTFS")
    }
}

/// Queries label, serial number and file system of a volume (e.g. `"C:"`). No elevation needed.
///
/// # Errors
/// [`Error::InvalidVolume`] for a malformed argument, [`Error::Os`] if the volume is missing or
/// not ready.
pub fn volume_info(volume: &str) -> Result<VolumeInfo> {
    let letter = parse_drive_letter(volume)?;
    let root = HSTRING::from(root_path(letter));
    let mut label = [0_u16; 261];
    let mut fs_name = [0_u16; 261];
    let mut serial = 0_u32;
    let mut max_component = 0_u32;
    let mut flags = 0_u32;
    // SAFETY: `root` is a NUL-terminated wide string that outlives the call; the two name
    // buffers are passed as slices (the wrapper passes their exact lengths); the three
    // out-pointers point to live, writable `u32` locals.
    unsafe {
        GetVolumeInformationW(
            &root,
            Some(&mut label),
            Some(&raw mut serial),
            Some(&raw mut max_component),
            Some(&raw mut flags),
            Some(&mut fs_name),
        )
    }
    .map_err(|e| os_error("GetVolumeInformationW", letter, &e))?;
    Ok(VolumeInfo {
        letter,
        label: wide_to_string(&label),
        serial_number: serial,
        file_system: wide_to_string(&fs_name),
        max_component_length: max_component,
        flags,
    })
}

/// Decodes a NUL-terminated wide buffer (lossy; labels and FS names are display-only).
fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn letter_str(letter: char) -> String {
    format!("{letter}:")
}

fn os_error(op: &'static str, letter: char, e: &windows::core::Error) -> Error {
    Error::Os {
        op,
        volume: letter_str(letter),
        code: win32::code_from_hresult(e.code().0),
        message: e.message(),
    }
}

/// A kernel handle closed on drop.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by a successful `CreateFileW`, is owned exclusively by
        // this value and is closed exactly once, here.
        if let Err(e) = unsafe { CloseHandle(self.0) } {
            tracing::warn!(error = %e, "CloseHandle failed");
        }
    }
}

impl fmt::Debug for OwnedHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OwnedHandle({:p})", self.0.0)
    }
}

/// An open, read-only handle to an NTFS volume (`\\.\C:`). Requires an elevated process.
#[derive(Debug)]
pub struct Volume {
    handle: OwnedHandle,
    info: VolumeInfo,
}

// SAFETY: a Win32 file handle is a process-wide kernel object reference, not tied to the thread
// that opened it; `Volume` owns it exclusively, so moving it to another thread is sound.
unsafe impl Send for Volume {}

impl Volume {
    /// Opens a volume for enumeration, e.g. `Volume::open("C:")`.
    ///
    /// Checks the file system first (no elevation needed), then opens the raw device with
    /// read-only access.
    ///
    /// # Errors
    /// [`Error::InvalidVolume`], [`Error::NotNtfs`], [`Error::NotElevated`] (access denied), or
    /// [`Error::Os`].
    pub fn open(volume: &str) -> Result<Self> {
        let info = volume_info(volume)?;
        let letter = info.letter;
        if !info.is_ntfs() {
            return Err(Error::NotNtfs {
                volume: letter_str(letter),
                found: info.file_system,
            });
        }
        let path = HSTRING::from(device_path(letter));
        // SAFETY: `path` is a NUL-terminated wide string that outlives the call; no security
        // attributes or template handle are passed. Access is GENERIC_READ only, so the handle
        // cannot be used to write to the volume.
        let handle = unsafe {
            CreateFileW(
                &path,
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|e| {
            if win32::code_from_hresult(e.code().0) == win32::ERROR_ACCESS_DENIED {
                Error::NotElevated {
                    volume: letter_str(letter),
                }
            } else {
                os_error("CreateFileW(volume)", letter, &e)
            }
        })?;
        Ok(Self {
            handle: OwnedHandle(handle),
            info,
        })
    }

    /// The volume's identity as reported when it was opened.
    #[must_use]
    pub fn info(&self) -> &VolumeInfo {
        &self.info
    }

    /// Upper-case drive letter.
    #[must_use]
    pub fn letter(&self) -> char {
        self.info.letter
    }

    /// File reference number of the root directory (`C:\`), read via
    /// `GetFileInformationByHandle`. The root is MFT record 5; its sequence number varies.
    ///
    /// # Errors
    /// [`Error::Os`] if the root directory cannot be opened.
    pub fn root_frn(&self) -> Result<u64> {
        let letter = self.letter();
        let path = HSTRING::from(root_path(letter));
        // SAFETY: as in `open`; FILE_READ_ATTRIBUTES access only, and BACKUP_SEMANTICS is what
        // allows a directory handle to be opened at all.
        let handle = unsafe {
            CreateFileW(
                &path,
                FILE_READ_ATTRIBUTES.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(|e| os_error("CreateFileW(root directory)", letter, &e))?;
        let handle = OwnedHandle(handle);
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `handle` is a valid open handle; `info` is a live, writable struct of the type
        // the API expects.
        unsafe { GetFileInformationByHandle(handle.0, &raw mut info) }
            .map_err(|e| os_error("GetFileInformationByHandle(root)", letter, &e))?;
        Ok((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow))
    }

    /// Queries the USN change journal (`FSCTL_QUERY_USN_JOURNAL`).
    ///
    /// Returns `Ok(None)` when the volume has no active journal. A journal is **never** created
    /// implicitly — that is a write to the volume and a decision for the caller (2.2/3.3).
    ///
    /// # Errors
    /// [`Error::JournalDeleteInProgress`], [`Error::Os`], or [`Error::Parse`].
    pub fn query_journal(&self) -> Result<Option<JournalInfo>> {
        let letter = self.letter();
        let mut out = [0_u64; USN_JOURNAL_DATA_V2_SIZE / 8];
        let mut returned = 0_u32;
        // SAFETY: METHOD_BUFFERED control code with no input buffer; `out` is a live, 8-byte
        // aligned buffer of exactly the size passed; `returned` is a live `u32`. The call is
        // synchronous (no OVERLAPPED).
        let result = unsafe {
            DeviceIoControl(
                self.handle.0,
                FSCTL_QUERY_USN_JOURNAL,
                None,
                0,
                Some(out.as_mut_ptr().cast()),
                USN_JOURNAL_DATA_V2_SIZE as u32,
                Some(&raw mut returned),
                None,
            )
        };
        if let Err(e) = result {
            return match win32::code_from_hresult(e.code().0) {
                win32::ERROR_JOURNAL_NOT_ACTIVE => Ok(None),
                win32::ERROR_JOURNAL_DELETE_IN_PROGRESS => Err(Error::JournalDeleteInProgress {
                    volume: letter_str(letter),
                }),
                _ => Err(os_error(
                    "DeviceIoControl(FSCTL_QUERY_USN_JOURNAL)",
                    letter,
                    &e,
                )),
            };
        }
        let bytes = u64s_as_bytes(&out);
        let len = (returned as usize).min(bytes.len());
        record::parse_journal_data(&bytes[..len])
            .map(Some)
            .map_err(|source| Error::Parse {
                what: "FSCTL_QUERY_USN_JOURNAL output",
                volume: letter_str(letter),
                source,
            })
    }

    /// Enumerates every file and directory record on the volume with default options.
    #[must_use]
    pub fn enumerate(&self) -> Enumerator<'_> {
        self.enumerate_with(EnumOptions::default())
    }

    /// Enumerates with explicit options (buffer size, USN range).
    #[must_use]
    pub fn enumerate_with(&self, options: EnumOptions) -> Enumerator<'_> {
        let words = options.buffer_bytes().div_ceil(8);
        Enumerator {
            volume: self,
            options,
            buf: vec![0_u64; words],
            next_start: 0,
            pos: 0,
            end: 0,
            done: false,
            stats: EnumStats::default(),
        }
    }
}

/// Views a `u64` buffer as bytes (an 8-byte-aligned byte buffer without a dependency).
fn u64s_as_bytes(words: &[u64]) -> &[u8] {
    // SAFETY: `u8` has alignment 1 and every bit pattern is a valid `u8`; the byte length is
    // exactly the size of the `u64` slice, and the returned slice borrows `words`, so it cannot
    // outlive it or alias a mutable borrow.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), std::mem::size_of_val(words)) }
}

/// Options for [`Volume::enumerate_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumOptions {
    /// Output buffer size per `DeviceIoControl` call, in bytes. Clamped to 4 KiB ..= 64 MiB.
    /// Larger buffers mean fewer kernel round-trips; 1 MiB is the default.
    pub buffer_size: usize,
    /// Only records whose last USN is ≥ this (0 = all).
    pub low_usn: i64,
    /// Only records whose last USN is ≤ this (`i64::MAX` = all).
    pub high_usn: i64,
}

impl Default for EnumOptions {
    fn default() -> Self {
        Self {
            buffer_size: 1 << 20,
            low_usn: 0,
            high_usn: i64::MAX,
        }
    }
}

impl EnumOptions {
    /// The effective buffer size after clamping.
    #[must_use]
    pub fn buffer_bytes(&self) -> usize {
        self.buffer_size.clamp(4 << 10, 64 << 20)
    }
}

/// Counters collected while enumerating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EnumStats {
    /// `DeviceIoControl` calls that returned data.
    pub batches: u64,
    /// Records parsed successfully.
    pub records: u64,
    /// Records skipped because of a record-local parse error (logged at `warn`).
    pub skipped: u64,
    /// Bytes of output returned by the kernel.
    pub bytes: u64,
}

/// Streaming enumeration of a volume via `FSCTL_ENUM_USN_DATA`.
///
/// Two ways to consume it:
/// - as an `Iterator<Item = Result<RawRecord>>` (owned records, one small allocation per name);
/// - with [`Enumerator::next_batch`], which lends the records of each kernel batch without
///   copying (fastest; what the name index should use).
///
/// Records come in MFT order, one per file record (a hard-linked file appears once, under one of
/// its names). Only `USN_RECORD_V2` is requested; V3 is parsed if the system returns it anyway.
/// After an error the enumerator is finished.
pub struct Enumerator<'v> {
    volume: &'v Volume,
    options: EnumOptions,
    buf: Vec<u64>,
    next_start: u64,
    /// Unconsumed record bytes of the current batch: `bytes()[pos..end]`.
    pos: usize,
    end: usize,
    done: bool,
    stats: EnumStats,
}

impl fmt::Debug for Enumerator<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Enumerator")
            .field("volume", &self.volume.letter())
            .field("options", &self.options)
            .field("next_start", &self.next_start)
            .field("done", &self.done)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl Enumerator<'_> {
    /// Counters so far.
    #[must_use]
    pub fn stats(&self) -> EnumStats {
        self.stats
    }

    /// Lends the next batch of records (the rest of the current batch first, if the iterator
    /// interface was used before). `Ok(None)` when enumeration is complete.
    ///
    /// The returned iterator yields per-record parse errors as `Err` items; they are not
    /// counted in [`EnumStats::skipped`].
    ///
    /// # Errors
    /// Any error from the underlying `DeviceIoControl` call.
    pub fn next_batch(&mut self) -> Result<Option<RecordIter<'_>>> {
        if self.pos >= self.end && !self.fill()? {
            return Ok(None);
        }
        let (pos, end) = (self.pos, self.end);
        self.pos = end;
        Ok(Some(RecordIter::new(
            &u64s_as_bytes(&self.buf)[pos..end],
            pos,
        )))
    }

    /// Fetches the next kernel batch into the buffer. Returns `false` at the end.
    fn fill(&mut self) -> Result<bool> {
        loop {
            if self.done {
                return Ok(false);
            }
            let letter = self.volume.letter();
            let input = AlignedInput(record::encode_mft_enum_data_v1(
                self.next_start,
                self.options.low_usn,
                self.options.high_usn,
                2,
                2,
            ));
            let out_len = std::mem::size_of_val(self.buf.as_slice());
            let mut returned = 0_u32;
            // SAFETY: METHOD_NEITHER control code: the kernel reads `input` (a live, 8-byte
            // aligned MFT_ENUM_DATA_V1 of the size passed) and writes at most `out_len` bytes
            // into `self.buf` (live, 8-byte aligned, exactly `out_len` bytes, not borrowed
            // elsewhere during the call). `returned` is a live `u32`. Synchronous call.
            let result = unsafe {
                DeviceIoControl(
                    self.volume.handle.0,
                    FSCTL_ENUM_USN_DATA,
                    Some(input.0.as_ptr().cast()),
                    MFT_ENUM_DATA_V1_SIZE as u32,
                    Some(self.buf.as_mut_ptr().cast()),
                    u32::try_from(out_len).unwrap_or(u32::MAX),
                    Some(&raw mut returned),
                    None,
                )
            };
            if let Err(e) = result {
                self.done = true;
                return match win32::code_from_hresult(e.code().0) {
                    win32::ERROR_HANDLE_EOF => Ok(false),
                    _ => Err(os_error("DeviceIoControl(FSCTL_ENUM_USN_DATA)", letter, &e)),
                };
            }
            let len = (returned as usize).min(out_len);
            let bytes = &u64s_as_bytes(&self.buf)[..len];
            let (next, _) = record::parse_enum_output(bytes).map_err(|source| {
                self.done = true;
                Error::Parse {
                    what: "FSCTL_ENUM_USN_DATA output",
                    volume: letter_str(letter),
                    source,
                }
            })?;
            self.stats.batches += 1;
            self.stats.bytes += len as u64;
            if len == 8 {
                // A header without records: nothing left (some systems return this instead of
                // ERROR_HANDLE_EOF).
                self.done = true;
                return Ok(false);
            }
            if next == self.next_start {
                self.done = true;
                return Err(Error::Stalled {
                    volume: letter_str(letter),
                    frn: next,
                });
            }
            self.next_start = next;
            self.pos = 8;
            self.end = len;
            if self.pos < self.end {
                return Ok(true);
            }
        }
    }
}

/// `MFT_ENUM_DATA_V1` bytes with the alignment of the C struct (it starts with a `u64`).
#[repr(C, align(8))]
struct AlignedInput([u8; MFT_ENUM_DATA_V1_SIZE]);

impl Iterator for Enumerator<'_> {
    type Item = Result<RawRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.pos >= self.end {
                match self.fill() {
                    Ok(true) => {}
                    Ok(false) => return None,
                    Err(e) => return Some(Err(e)),
                }
            }
            let (pos, end) = (self.pos, self.end);
            let rest = &u64s_as_bytes(&self.buf)[pos..end];
            match record::parse_record(rest, pos) {
                Ok((rec, len)) => {
                    self.pos += len;
                    self.stats.records += 1;
                    return Some(Ok(rec.to_raw()));
                }
                Err(e) if e.is_record_local() => {
                    // Length already validated by `parse_record` before any record-local check.
                    let len = rest
                        .get(..4)
                        .and_then(|b| <[u8; 4]>::try_from(b).ok())
                        .map_or(0, |b| u32::from_le_bytes(b) as usize);
                    self.stats.skipped += 1;
                    tracing::warn!(volume = %self.volume.letter(), error = %e, "skipping USN record");
                    self.pos += len.max(8);
                }
                Err(source) => {
                    self.done = true;
                    self.pos = self.end;
                    return Some(Err(Error::Parse {
                        what: "FSCTL_ENUM_USN_DATA record",
                        volume: letter_str(self.volume.letter()),
                        source,
                    }));
                }
            }
        }
    }
}
