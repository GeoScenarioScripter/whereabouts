//! Volume-level tests (Windows only).
//!
//! The `#[ignore]` tests need an elevated shell (raw volume handle) and read the system volume
//! `C:` — read-only: the volume is opened with `GENERIC_READ`, no journal is created and nothing
//! is written. CI runs them on the elevated Windows runner with
//! `cargo test -p whereabouts-ntfs -- --ignored`.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::io::Write as _;
use std::time::Instant;

use whereabouts_ntfs::{Error, ParentChainReport, Volume, process_memory, record, volume_info};

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

    let mem_before = process_memory().unwrap();
    let started = Instant::now();
    let mut enumerator = volume.enumerate();
    let mut links = Vec::new();
    let mut dirs = 0_u64;
    let mut name_bytes = 0_u64;
    let mut non_utf8 = 0_u64;
    for rec in enumerator.by_ref() {
        let rec = rec.unwrap();
        dirs += u64::from(rec.is_directory());
        name_bytes += rec.name.len() as u64;
        non_utf8 += u64::from(rec.name.as_str().is_none());
        links.push((rec.frn, rec.parent_frn));
    }
    let elapsed = started.elapsed();
    let stats = enumerator.stats();
    let mem_after = process_memory().unwrap();

    let chain_started = Instant::now();
    let chains = ParentChainReport::compute(&links, root);
    let chain_elapsed = chain_started.elapsed();

    report(&format!(
        "enumerate C: records={} dirs={dirs} skipped={} batches={} kernel_bytes={} \
         name_bytes={name_bytes} non_utf8_names={non_utf8} elapsed_ms={} records_per_s={:.0}",
        stats.records,
        stats.skipped,
        stats.batches,
        stats.bytes,
        elapsed.as_millis(),
        stats.records as f64 / elapsed.as_secs_f64().max(1e-9),
    ));
    report(&format!(
        "memory C: working_set_before={} working_set_after={} peak_working_set={} (test binary, \
         unoptimised, holds a Vec of (frn,parent) links)",
        mem_before.working_set, mem_after.working_set, mem_after.peak_working_set
    ));
    report(&format!(
        "chains C: root={root:#x} root_enumerated={} root_children={} resolved={} orphaned={} \
         cyclic={} ratio={:.5} chain_ms={}",
        chains.root_enumerated,
        chains.root_children,
        chains.resolved,
        chains.orphaned,
        chains.cyclic,
        chains.resolved_ratio(),
        chain_elapsed.as_millis()
    ));

    assert_eq!(stats.records, links.len() as u64);
    assert_eq!(stats.skipped, 0, "real kernel output must parse cleanly");
    assert!(
        stats.records > 100_000,
        "only {} records on C:",
        stats.records
    );
    assert!(
        chains.root_enumerated || chains.root_children > 0,
        "root FRN {root:#x} neither enumerated nor referenced as a parent"
    );
    assert!(chains.root_children > 0, "no record has the root as parent");
    assert!(chains.resolved_ratio() >= 0.999, "{chains:?}");
}

#[test]
#[ignore = "needs an elevated shell: opens the raw C: volume (read-only)"]
fn batch_and_iterator_interfaces_agree() {
    let volume = Volume::open("C:").unwrap();
    // A small buffer forces many kernel round-trips and exercises batch boundaries.
    let opts = whereabouts_ntfs::EnumOptions {
        buffer_size: 64 << 10,
        ..Default::default()
    };
    let mut by_batch = 0_u64;
    let mut enumerator = volume.enumerate_with(opts);
    while let Some(batch) = enumerator.next_batch().unwrap() {
        for rec in batch {
            rec.unwrap();
            by_batch += 1;
        }
    }
    let mut by_iter = 0_u64;
    for rec in volume.enumerate_with(opts) {
        rec.unwrap();
        by_iter += 1;
    }
    report(&format!(
        "interfaces C: batch_records={by_batch} iterator_records={by_iter} batches={}",
        enumerator.stats().batches
    ));
    // The volume is live (the runner writes logs), so allow a small drift between the two passes.
    let drift = by_batch.abs_diff(by_iter);
    assert!(drift * 1000 <= by_batch, "batch={by_batch} iter={by_iter}");
}
