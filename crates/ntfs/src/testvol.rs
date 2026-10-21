//! A throw-away NTFS volume on a VHD, for tests that write files or create/delete a USN journal
//! (AGENTS.md: such tests must never touch a real volume).
//!
//! Only compiled for this crate's tests or with the `test-fixtures` feature (enable it from
//! `[dev-dependencies]`, e.g. for the indexer's and CLI's volume tests). Needs an **elevated**
//! process: `diskpart` attaches the disk, `fsutil` manages the journal. The CI Windows runner is
//! elevated; tests using this are `#[ignore]` like every other raw-volume test.
//!
//! ```no_run
//! # fn main() -> std::io::Result<()> {
//! use whereabouts_ntfs::testvol::TestVolume;
//! let vol = TestVolume::create(64)?; // 64 MiB, NTFS, mounted at a free drive letter
//! vol.create_journal(1 << 20, 64 << 10)?;
//! std::fs::write(vol.path("a.txt"), b"hello")?;
//! // ... open `vol.volume()` ("W:") with NtfsVolumeSource ...
//! # Ok(()) }
//! // Dropping `vol` detaches the VHD and deletes its file.
//! ```
//!
//! The VHD file lives in `RUNNER_TEMP` (CI) or the system temp directory. Volume creation is
//! serialised within the process (one `diskpart` at a time, no drive-letter races).

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Serialises `diskpart` runs and drive-letter selection within the process.
static DISKPART: Mutex<()> = Mutex::new(());
/// Makes VHD file names unique within the process.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// An attached, NTFS-formatted VHD with a drive letter. Detached and deleted on drop.
#[derive(Debug)]
pub struct TestVolume {
    letter: char,
    vhd: PathBuf,
}

impl TestVolume {
    /// Creates an expandable VHD of `size_mib` MiB (at least 16), formats it NTFS (quick, label
    /// `WBTEST`) and mounts it at the highest free drive letter from `Z:` down to `G:`.
    ///
    /// # Errors
    /// No free letter, `diskpart` failing (not elevated, no virtual disk service), or the volume
    /// not appearing within 60 s. The error carries `diskpart`'s output.
    pub fn create(size_mib: u32) -> io::Result<Self> {
        let guard = DISKPART
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::var_os("RUNNER_TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let vhd = dir.join(format!(
            "whereabouts-test-{}-{}.vhdx",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let letter = ('G'..='Z')
            .rev()
            .find(|l| !Path::new(&format!("{l}:\\")).exists())
            .ok_or_else(|| io::Error::other("no free drive letter for the test VHD"))?;
        let file = vhd.display();
        let script = format!(
            "create vdisk file=\"{file}\" maximum={} type=expandable\n\
             select vdisk file=\"{file}\"\n\
             attach vdisk\n\
             convert mbr noerr\n\
             create partition primary\n\
             format fs=ntfs quick label=WBTEST\n\
             assign letter={letter}\n",
            size_mib.max(16)
        );
        // From here on, drop cleans up whatever got created. Drop takes the lock itself, so
        // release it before an error can drop `vol`.
        let vol = Self { letter, vhd };
        let attached = run_diskpart(&script);
        drop(guard);
        attached?;
        let root = vol.root();
        let started = Instant::now();
        while !root.exists() {
            if started.elapsed() > Duration::from_secs(60) {
                return Err(io::Error::other(format!(
                    "test VHD did not appear at {}",
                    root.display()
                )));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(vol)
    }

    /// The drive letter, e.g. `'W'`.
    #[must_use]
    pub fn letter(&self) -> char {
        self.letter
    }

    /// The volume argument for `Volume::open`/`NtfsVolumeSource::open`, e.g. `"W:"`.
    #[must_use]
    pub fn volume(&self) -> String {
        format!("{}:", self.letter)
    }

    /// The root directory, e.g. `W:\`.
    #[must_use]
    pub fn root(&self) -> PathBuf {
        PathBuf::from(format!("{}:\\", self.letter))
    }

    /// `relative` resolved under the root.
    #[must_use]
    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root().join(relative)
    }

    /// The VHD file backing the volume.
    #[must_use]
    pub fn vhd_path(&self) -> &Path {
        &self.vhd
    }

    /// `fsutil usn createjournal m=<max_size> a=<allocation_delta>` on this volume. Also
    /// resizes an existing journal.
    ///
    /// # Errors
    /// `fsutil` failing.
    pub fn create_journal(&self, max_size: u64, allocation_delta: u64) -> io::Result<()> {
        run(
            "fsutil",
            &[
                "usn",
                "createjournal",
                &format!("m={max_size}"),
                &format!("a={allocation_delta}"),
                &self.volume(),
            ],
        )
    }

    /// `fsutil usn deletejournal /d` on this volume, then waits (up to 60 s) until
    /// `fsutil usn queryjournal` reports no journal, so the deletion has finished.
    ///
    /// # Errors
    /// `fsutil` failing, or the journal still present after 60 s.
    pub fn delete_journal(&self) -> io::Result<()> {
        let volume = self.volume();
        run("fsutil", &["usn", "deletejournal", "/d", &volume])?;
        let started = Instant::now();
        while run("fsutil", &["usn", "queryjournal", &volume]).is_ok() {
            if started.elapsed() > Duration::from_secs(60) {
                return Err(io::Error::other(format!(
                    "the USN journal on {volume} was still present 60 s after deletejournal"
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
}

impl Drop for TestVolume {
    fn drop(&mut self) {
        let _guard = DISKPART
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Test-only code: failures go to stderr so they show in the test output (tests have no
        // tracing subscriber), but never panic in drop.
        let file = self.vhd.display();
        let script = format!("select vdisk file=\"{file}\"\ndetach vdisk\n");
        if let Err(e) = run_diskpart(&script) {
            eprintln!("[testvol] could not detach the test VHD {file}: {e}");
        }
        if let Err(e) = std::fs::remove_file(&self.vhd)
            && e.kind() != io::ErrorKind::NotFound
        {
            eprintln!("[testvol] could not delete the test VHD {file}: {e}");
        }
    }
}

/// Runs a `diskpart /s` script (written to a temp file next to the VHDs).
fn run_diskpart(script: &str) -> io::Result<()> {
    let path = std::env::temp_dir().join(format!(
        "whereabouts-diskpart-{}-{}.txt",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, script)?;
    let result = run("diskpart", &["/s", &path.to_string_lossy()]);
    let _ = std::fs::remove_file(&path);
    result.map_err(|e| io::Error::other(format!("{e}\nscript:\n{script}")))
}

/// Runs a command; a non-zero exit becomes an error carrying its output.
fn run(program: &str, args: &[&str]) -> io::Result<()> {
    let out = Command::new(program).args(args).output()?;
    if out.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "`{program} {}` failed ({}):\n{}{}",
        args.join(" "),
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )))
}
