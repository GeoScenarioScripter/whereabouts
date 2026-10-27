//! Raw `$MFT` reader against the real system volume (Windows only, stage01 session 2.7).
//!
//! Needs an elevated shell (raw volume handle), so the test is `#[ignore]`d; CI runs it on the
//! elevated Windows runner (`cargo test -p whereabouts-ntfs -- --ignored`). It is strictly
//! read-only: the volume is opened with `GENERIC_READ`, and the only operations are
//! `FSCTL_GET_NTFS_VOLUME_DATA`, `FSCTL_QUERY_USN_JOURNAL`, `FSCTL_ENUM_USN_DATA`,
//! `FSCTL_READ_USN_JOURNAL` and plain `ReadFile` of the volume's own `$MFT` clusters. AGENTS.md
//! allows that kind of read-only access to the system volume.
//!
//! The test enumerates `C:` with both methods back to back, records cold-ish and warm timings,
//! and compares the record sets (FRN with sequence number, parent FRN, name, directory bit).
//! Every difference is classified; only unexplained ones fail the test.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use whereabouts_core::FileRecord;
use whereabouts_ntfs::record::record_number;
use whereabouts_ntfs::{
    EnumerationMethod, MftScanOptions, MftScanStats, NtfsVolumeSource, RawRecord, SourceOptions,
    Volume, process_memory,
};
use whereabouts_source::{Changes, Source, SourceEvent};

fn report(line: &str) {
    let _ = writeln!(std::io::stderr(), "[mft-measure] {line}");
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

/// A raw scan that keeps its records, plus the FRNs of the hard-linked files it found.
fn scan_all(
    volume: &Volume,
    options: &MftScanOptions,
) -> (Vec<RawRecord>, Duration, String, MftScanStats) {
    let started = Instant::now();
    let scanner = volume.open_mft().unwrap();
    let opened = started.elapsed();
    let mut records = Vec::new();
    let stats = scanner
        .scan(options, &mut |r| {
            records.push(r);
            ControlFlow::Continue(())
        })
        .unwrap();
    let elapsed = started.elapsed();
    let line = format!(
        "slots={} examined={} yielded={} unused={} not_file={} extension={} torn={} malformed={} \
         no_name={} hard_link_files={} extension_reads={} extension_errors={} \
         chunks_skipped={} reads={} read_mib={:.1} open_ms={} read_ms={} total_ms={}",
        scanner.record_count(),
        stats.examined,
        stats.yielded,
        stats.unused,
        stats.not_file,
        stats.extension,
        stats.torn,
        stats.malformed,
        stats.no_name,
        stats.hard_link_files,
        stats.extension_reads,
        stats.extension_errors,
        stats.chunks_skipped,
        stats.reads,
        mib(stats.bytes),
        ms(opened),
        ms(stats.read_time),
        ms(elapsed),
    );

    (records, elapsed, line, stats)
}

fn count_mft(volume: &Volume) -> (u64, Duration) {
    let started = Instant::now();
    let scanner = volume.open_mft().unwrap();
    let mut n = 0_u64;
    scanner
        .scan(&MftScanOptions::default(), &mut |_| {
            n += 1;
            ControlFlow::Continue(())
        })
        .unwrap();
    (n, started.elapsed())
}

fn count_usn(source: &mut NtfsVolumeSource) -> (u64, Duration) {
    let started = Instant::now();
    let mut n = 0_u64;
    source
        .enumerate(&mut |_: FileRecord| {
            n += 1;
            ControlFlow::Continue(())
        })
        .unwrap();
    (n, started.elapsed())
}

#[test]
#[ignore = "needs an elevated shell: reads the raw C: volume (read-only)"]
fn raw_mft_matches_usn_enumeration_on_system_volume() {
    let volume = Volume::open("C:").unwrap();
    let geom = volume.geometry().unwrap();
    report(&format!(
        "geometry C: sector={} cluster={} record={} total_clusters={} mft_start_lcn={} \
         mft_valid_mib={:.1}",
        geom.bytes_per_sector,
        geom.bytes_per_cluster,
        geom.bytes_per_record,
        geom.total_clusters,
        geom.mft_start_lcn,
        mib(geom.mft_valid_data_length),
    ));
    report(&format!(
        "baseline: working_set_mib={:.1}",
        mib(process_memory().unwrap().working_set)
    ));

    // The passes run one after the other so they never compete for the disk.
    //
    // 1. USN API, first read in this process (cold-ish: the OS cache may hold part of the MFT).
    //    Going through `Source::enumerate` also gives the checkpoint that bounds the churn
    //    window below.
    let mut usn_source = NtfsVolumeSource::open("C:").unwrap();
    let mut usn_records: Vec<FileRecord> = Vec::new();
    let started = Instant::now();
    let checkpoint = usn_source.enumerate(&mut usn_records).unwrap();
    let usn_cold = started.elapsed();
    let usn_stats = usn_source.last_enumerate_stats();
    report(&format!(
        "pass 1 usn-cold: records={} skipped={} batches={} kernel_ms={} total_ms={}",
        usn_records.len(),
        usn_stats.skipped,
        usn_stats.batches,
        ms(usn_stats.kernel_time),
        ms(usn_cold),
    ));

    // 2. Raw $MFT, first raw read in this process.
    let options = MftScanOptions {
        list_hard_links: true,
        ..MftScanOptions::default()
    };
    let (mft_records, mft_cold, stats_line, mft_stats) = scan_all(&volume, &options);
    let hard_links: HashSet<u64> = mft_stats.hard_link_frns.iter().copied().collect();
    report(&format!("pass 2 mft-cold: {stats_line}"));
    report(&format!(
        "pass 2 mft-cold: records={} total_ms={} working_set_mib={:.1}",
        mft_records.len(),
        ms(mft_cold),
        mib(process_memory().unwrap().working_set)
    ));

    // 3. + 4. Warm, count only, in the opposite order (so neither method always runs second).
    let (warm_mft_n, mft_warm) = count_mft(&volume);
    let (warm_usn_n, usn_warm) = count_usn(&mut usn_source);
    let (warm_mft2_n, mft_warm2) = count_mft(&volume);
    report(&format!(
        "pass 3 mft-warm: records={warm_mft_n} total_ms={} (again: {} ms, records={warm_mft2_n})",
        ms(mft_warm),
        ms(mft_warm2),
    ));
    report(&format!(
        "pass 4 usn-warm: records={warm_usn_n} total_ms={}",
        ms(usn_warm)
    ));
    report(&format!(
        "summary: usn_cold_ms={} mft_cold_ms={} usn_warm_ms={} mft_warm_ms={} \
         (debug build; CI numbers are for relative comparison)",
        ms(usn_cold),
        ms(mft_cold),
        ms(usn_warm),
        ms(mft_warm),
    ));

    // 5. The Source wiring: the same enumeration through `NtfsVolumeSource` with the raw option.
    let mut raw_source = NtfsVolumeSource::open_with(
        "C:",
        SourceOptions {
            enumeration: EnumerationMethod::RawMft,
            ..SourceOptions::default()
        },
    )
    .unwrap();
    let mut raw_source_records: Vec<FileRecord> = Vec::new();
    let started = Instant::now();
    raw_source.enumerate(&mut raw_source_records).unwrap();
    report(&format!(
        "pass 5 source-raw-mft: records={} total_ms={} skipped={}",
        raw_source_records.len(),
        ms(started.elapsed()),
        raw_source.last_enumerate_stats().skipped
    ));

    // The volume is live: collect what changed since pass 1 started, so differences caused by
    // the passes' own timing window are told apart from real disagreements.
    let mut churn: HashSet<u64> = HashSet::new();
    let mut churn_complete = true;
    let mut cp = checkpoint;
    for _ in 0..10_000 {
        match usn_source.changes(&cp).unwrap() {
            Changes::Batch(batch) => {
                if batch.events.is_empty() {
                    break;
                }
                // Only events that change the (parent, name) set excuse a difference; a plain
                // `Modified` cannot explain a disagreement about names.
                for e in &batch.events {
                    if !matches!(e, SourceEvent::Modified(_)) {
                        churn.insert(record_number(e.record().frn.0));
                    }
                }
                cp = batch.checkpoint;
            }
            Changes::FullRescanRequired(reason) => {
                report(&format!("churn: journal unusable ({reason:?})"));
                churn_complete = false;
                break;
            }
        }
    }
    report(&format!(
        "churn: {} files changed during the run (complete: {churn_complete})",
        churn.len()
    ));

    // ---- compare pass 1 (USN) with pass 2 (raw $MFT) -------------------------------------
    let usn_index: HashMap<u64, usize> = usn_records
        .iter()
        .enumerate()
        .map(|(i, r)| (r.frn.0, i))
        .collect();
    let mft_index: HashMap<u64, usize> = mft_records
        .iter()
        .enumerate()
        .map(|(i, r)| (r.frn, i))
        .collect();
    assert_eq!(usn_index.len(), usn_records.len(), "duplicate FRNs in USN");
    assert_eq!(mft_index.len(), mft_records.len(), "duplicate FRNs in MFT");

    let mut categories: Vec<(&str, Vec<String>, usize)> = vec![
        ("both_equal", vec![], 0),
        ("differ:hard_link", vec![], 0),
        ("differ:churn", vec![], 0),
        ("differ:attributes_only", vec![], 0),
        ("differ:UNEXPLAINED", vec![], 0),
        ("only_usn:churn", vec![], 0),
        ("only_usn:UNEXPLAINED", vec![], 0),
        ("only_mft:reserved_record", vec![], 0),
        ("only_mft:churn", vec![], 0),
        ("only_mft:UNEXPLAINED", vec![], 0),
    ];
    let mut note = |name: &str, line: String| {
        let entry = categories.iter_mut().find(|c| c.0 == name).unwrap();
        entry.2 += 1;
        if !line.is_empty() && entry.1.len() < 12 {
            entry.1.push(line);
        }
    };
    let describe = |parent: u64, name: &[u8], dir: bool| {
        format!(
            "parent={parent:#x} name={:?} dir={dir}",
            String::from_utf8_lossy(name)
        )
    };
    for u in &usn_records {
        let frn = u.frn.0;
        if record_number(frn) == 5 {
            continue; // the Source synthesises the root (empty name)
        }
        let Some(&mi) = mft_index.get(&frn) else {
            let line = format!(
                "{frn:#x} {}",
                describe(u.parent.0, u.name.as_wtf8(), u.is_dir())
            );
            if churn.contains(&record_number(frn)) {
                note("only_usn:churn", line);
            } else {
                note("only_usn:UNEXPLAINED", line);
            }
            continue;
        };
        let m = &mft_records[mi];
        let same_core = m.parent_frn == u.parent.0
            && m.name.as_wtf8() == u.name.as_wtf8()
            && m.is_directory() == u.is_dir();
        if same_core {
            if m.file_attributes == u.attrs.0 {
                note("both_equal", String::new());
            } else {
                note(
                    "differ:attributes_only",
                    format!(
                        "{frn:#x} usn_attrs={:#x} mft_attrs={:#x}",
                        u.attrs.0, m.file_attributes
                    ),
                );
            }
            continue;
        }
        let line = format!(
            "{frn:#x} usn[{}] mft[{}]",
            describe(u.parent.0, u.name.as_wtf8(), u.is_dir()),
            describe(m.parent_frn, m.name.as_wtf8(), m.is_directory())
        );
        if hard_links.contains(&frn) {
            note("differ:hard_link", line);
        } else if churn.contains(&record_number(frn)) {
            note("differ:churn", line);
        } else {
            note("differ:UNEXPLAINED", line);
        }
    }
    for m in &mft_records {
        if record_number(m.frn) == 5 || usn_index.contains_key(&m.frn) {
            continue;
        }
        let line = format!(
            "{:#x} {}",
            m.frn,
            describe(m.parent_frn, m.name.as_wtf8(), m.is_directory())
        );
        let metadata =
            m.name.as_wtf8().starts_with(b"$") && matches!(record_number(m.parent_frn), 11 | 0x1b);
        if record_number(m.frn) < 16 || metadata {
            note("only_mft:reserved_record", line);
        } else if churn.contains(&record_number(m.frn)) {
            note("only_mft:churn", line);
        } else {
            note("only_mft:UNEXPLAINED", line);
        }
    }
    let mut unexplained = 0;
    for (name, samples, count) in &categories {
        report(&format!("compare {name}: {count}"));
        for s in samples {
            report(&format!("  e.g. {s}"));
        }
        if name.contains("UNEXPLAINED") || name.contains("attributes_only") {
            unexplained += count;
        }
    }
    report(&format!(
        "compare totals: usn={} mft={} unexplained={unexplained}",
        usn_records.len(),
        mft_records.len()
    ));

    // The Source wiring must yield the direct scan's records (reserved ones and the children of
    // `$Extend` left out): compare the FRN *sets*; only churn excuses a difference.
    let expected: HashSet<u64> = mft_records
        .iter()
        .filter(|r| {
            let rn = record_number(r.frn);
            rn == 5 || (rn >= 16 && record_number(r.parent_frn) != 11)
        })
        .map(|r| r.frn)
        .collect();
    let wired: HashSet<u64> = raw_source_records.iter().map(|r| r.frn.0).collect();
    assert_eq!(wired.len(), raw_source_records.len(), "duplicate FRNs");
    let wiring_diff: Vec<u64> = expected.symmetric_difference(&wired).copied().collect();
    let wiring_unexplained: Vec<&u64> = wiring_diff
        .iter()
        .filter(|f| !churn.contains(&record_number(**f)))
        .collect();
    report(&format!(
        "source wiring: source_raw={} direct_non_reserved={} differing={} unexplained={}",
        wired.len(),
        expected.len(),
        wiring_diff.len(),
        wiring_unexplained.len()
    ));

    // ---- assertions -----------------------------------------------------------------------
    assert!(
        usn_records.len() > 100_000,
        "only {} records",
        usn_records.len()
    );
    // The raw scan must account for every in-use record, also after the torn-record retry.
    assert_eq!(mft_stats.lost(), 0, "raw scan lost records: {mft_stats:?}");
    assert_eq!(mft_stats.extension_errors, 0);
    assert_eq!(raw_source.last_enumerate_stats().skipped, 0);
    let by = |name: &str| categories.iter().find(|c| c.0 == name).unwrap().2;
    // Nothing the USN API reports may be missing from the raw scan, churn aside.
    assert_eq!(by("only_usn:UNEXPLAINED"), 0);
    assert_eq!(by("only_mft:UNEXPLAINED"), 0);
    assert_eq!(by("differ:attributes_only"), 0);
    // The name rule agreed with the USN API for every hard-linked file seen so far; a
    // disagreement means the rule is wrong, not that the volume changed.
    assert_eq!(by("differ:hard_link"), 0);
    // A churn-excused difference needs a create/delete/rename in the journal since pass 1, and
    // there cannot be more of them than such events.
    assert!(churn_complete, "journal unusable: churn cannot be verified");
    let excused = by("differ:churn") + by("only_usn:churn") + by("only_mft:churn");
    assert!(
        excused <= churn.len(),
        "{excused} excused vs {} events",
        churn.len()
    );
    // The only tolerated name/parent disagreement: a hard link added or removed while the passes
    // ran (the journal source reports nothing for that, 2.2). A small fixed bound.
    assert!(
        by("differ:UNEXPLAINED") <= 5,
        "{}",
        by("differ:UNEXPLAINED")
    );
    assert!(wiring_unexplained.is_empty(), "{wiring_unexplained:x?}");
    assert!(warm_mft_n.abs_diff(mft_records.len() as u64) <= churn.len() as u64 + 5);
    let _ = (warm_usn_n, unexplained);
}
