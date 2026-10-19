//! Volume-level tests (Windows only).
//!
//! The `#[ignore]` tests need an elevated shell (raw volume handle) and read the system volume
//! `C:` — read-only: the volume is opened with `GENERIC_READ`, no journal is created and nothing
//! is written. CI runs them on the elevated Windows runner with
//! `cargo test -p whereabouts-ntfs -- --ignored`.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::io::Write as _;
use std::time::{Duration, Instant};

use whereabouts_ntfs::{
    EnumOptions, EnumStats, Error, ParentChainReport, RawRecord, Volume, process_memory, record,
    volume_info,
};

/// Writes a measurement line straight to the process's stderr.
///
/// libtest captures `print!`/`eprint!` output of passing tests, but not direct writes to the
/// stderr handle — so these lines show up in the CI log, where the session note reads them.
fn report(line: &str) {
    let _ = writeln!(std::io::stderr(), "[ntfs-measure] {line}");
}

#[test]
fn system_volume_is_ntfs_without_elevation() {
    let info = volume_info("C:").unwrap();
    assert_eq!(info.letter, 'C');
    assert!(info.is_ntfs(), "C: reports {}", info.file_system);
    assert!(info.max_component_length >= 255);
}

#[test]
fn malformed_volume_arguments_are_rejected() {
    for bad in ["", "CC:", r"C:\Windows", r"\\server\share"] {
        assert!(
            matches!(volume_info(bad), Err(Error::InvalidVolume(_))),
            "{bad:?}"
        );
        assert!(
            matches!(Volume::open(bad), Err(Error::InvalidVolume(_))),
            "{bad:?}"
        );
    }
}

#[test]
fn missing_volume_is_an_error_not_a_panic() {
    // Find a drive letter with nothing mounted (B: is almost always free).
    let free = ('B'..='Z').find(|l| volume_info(&format!("{l}:")).is_err());
    if let Some(letter) = free {
        assert!(Volume::open(&format!("{letter}:")).is_err());
    }
}

#[test]
#[ignore = "needs an elevated shell: opens the raw C: volume (read-only)"]
fn query_journal_on_system_volume() {
    let volume = Volume::open("C:").unwrap();
    match volume.query_journal().unwrap() {
        Some(j) => {
            assert!(j.next_usn >= j.first_usn, "{j:?}");
            assert!(j.first_usn >= 0, "{j:?}");
            report(&format!(
                "journal C: id={:#x} first_usn={} next_usn={} max_size={} versions={:?}..={:?}",
                j.journal_id,
                j.first_usn,
                j.next_usn,
                j.maximum_size,
                j.min_supported_major_version,
                j.max_supported_major_version
            ));
        }
        None => report("journal C: not active (absence reported, nothing created)"),
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// One line of measurements for an enumeration pass.
fn report_pass(label: &str, stats: EnumStats, elapsed: Duration) {
    let mem = process_memory().unwrap();
    report(&format!(
        "pass {label}: records={} skipped={} batches={} kernel_mib={:.1} elapsed_ms={} \
         kernel_ms={} user_ms={} records_per_s={:.0} working_set_mib={:.1} peak_mib={:.1}",
        stats.records,
        stats.skipped,
        stats.batches,
        mib(stats.bytes),
        elapsed.as_millis(),
        stats.kernel_time.as_millis(),
        elapsed.saturating_sub(stats.kernel_time).as_millis(),
        stats.records as f64 / elapsed.as_secs_f64().max(1e-9),
        mib(mem.working_set),
        mib(mem.peak_working_set),
    ));
}

/// Zero-copy pass that keeps nothing; returns (records, stats, elapsed).
fn count_only(volume: &Volume, buffer_size: usize) -> (u64, EnumStats, Duration) {
    let started = Instant::now();
    let mut enumerator = volume.enumerate_with(EnumOptions {
        buffer_size,
        ..EnumOptions::default()
    });
    let mut records = 0_u64;
    while let Some(batch) = enumerator.next_batch().unwrap() {
        for rec in batch {
            rec.unwrap();
            records += 1;
        }
    }
    let elapsed = started.elapsed();
    let mut stats = enumerator.stats();
    stats.records = records; // the batch interface leaves per-record counting to the caller
    (records, stats, elapsed)
}

/// Enumerates `C:` three times in one test so the passes never overlap (parallel tests would
/// compete for the same disk and distort the timings):
///
/// 1. *cold-ish, owned*: the iterator interface, collecting every `RawRecord` as an indexer
///    would (the first read of the MFT in this process; the OS cache may already hold part of it);
/// 2. *warm, count-only, 1 MiB buffer*: zero-copy batches, nothing kept — the API's own cost;
/// 3. *warm, count-only, 64 KiB buffer*: sensitivity to the buffer size.
#[test]
#[ignore = "needs an elevated shell: opens the raw C: volume (read-only)"]
fn enumerates_system_volume() {
    let volume = Volume::open("C:").unwrap();
    let root = volume.root_frn().unwrap();
    assert_eq!(
        record::record_number(root),
        record::ROOT_RECORD_NUMBER,
        "root FRN {root:#x}"
    );
    report(&format!(
        "baseline: working_set_mib={:.1}",
        mib(process_memory().unwrap().working_set)
    ));

    // Pass 1: owned records.
    let started = Instant::now();
    let mut enumerator = volume.enumerate();
    let records: Vec<RawRecord> = enumerator.by_ref().map(Result::unwrap).collect();
    let elapsed = started.elapsed();
    let stats = enumerator.stats();
    report_pass("1-owned-1MiB", stats, elapsed);

    let dirs = records.iter().filter(|r| r.is_directory()).count();
    let name_bytes: usize = records.iter().map(|r| r.name.len()).sum();
    let non_utf8 = records.iter().filter(|r| r.name.as_str().is_none()).count();
    report(&format!(
        "records C: total={} dirs={dirs} name_wtf8_bytes={name_bytes} non_utf8_names={non_utf8}",
        records.len()
    ));

    // Name the records whose parent was not enumerated (system-volume metadata, no privacy
    // concern on a CI runner), so the session note can explain them.
    let known: std::collections::HashSet<u64> = records.iter().map(|r| r.frn).collect();
    for r in records
        .iter()
        .filter(|r| r.parent_frn != root && !known.contains(&r.parent_frn))
        .take(20)
    {
        report(&format!(
            "orphan C: name={:?} frn={:#x} parent={:#x} attrs={:#x}",
            r.name, r.frn, r.parent_frn, r.file_attributes
        ));
    }
    drop(known);

    let links: Vec<(u64, u64)> = records.iter().map(|r| (r.frn, r.parent_frn)).collect();
    drop(records);
    let chain_started = Instant::now();
    let chains = ParentChainReport::compute(&links, root);
    report(&format!(
        "chains C: root={root:#x} root_enumerated={} root_children={} resolved={} orphaned={} \
         cyclic={} ratio={:.6} chain_ms={}",
        chains.root_enumerated,
        chains.root_children,
        chains.resolved,
        chains.orphaned,
        chains.cyclic,
        chains.resolved_ratio(),
        chain_started.elapsed().as_millis()
    ));

    assert_eq!(stats.records, links.len() as u64);
    assert_eq!(stats.skipped, 0, "real kernel output must parse cleanly");
    assert!(
        stats.records > 100_000,
        "only {} records on C:",
        stats.records
    );
    assert!(
        chains.root_children > 0,
        "no record has the root {root:#x} as parent"
    );
    assert!(chains.resolved_ratio() >= 0.999, "{chains:?}");

    // Passes 2 and 3: zero-copy batches (also checks both interfaces agree).
    let (warm, stats, elapsed) = count_only(&volume, 1 << 20);
    report_pass("2-count-1MiB", stats, elapsed);
    let (small, stats, elapsed) = count_only(&volume, 64 << 10);
    report_pass("3-count-64KiB", stats, elapsed);
    // The volume is live (the runner writes logs), so allow a small drift between passes.
    for n in [warm, small] {
        assert!(
            n.abs_diff(links.len() as u64) * 1000 <= n,
            "pass counts {n} vs {}",
            links.len()
        );
    }
}
