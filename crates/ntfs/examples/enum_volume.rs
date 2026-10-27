//! Enumerate an NTFS volume and print record count, elapsed time and peak working set.
//!
//! Run from an **elevated** terminal (read-only; nothing on the volume is modified):
//!
//! ```text
//! cargo run --release -p whereabouts-ntfs --example enum_volume -- C:
//! cargo run --release -p whereabouts-ntfs --example enum_volume -- D: --buffer-kib 64
//! cargo run --release -p whereabouts-ntfs --example enum_volume -- C: --count-only
//! cargo run --release -p whereabouts-ntfs --example enum_volume -- D: --mft
//! cargo run --release -p whereabouts-ntfs --example enum_volume -- D: --mft --count-only
//! ```
//!
//! Options:
//! - `--mft`: read `$MFT` directly in large sequential chunks (session 2.7) instead of calling
//!   `FSCTL_ENUM_USN_DATA`; prints the same figures so the two methods can be compared side by
//!   side, cold (right after a reboot) and warm. Read-only, like the default mode.
//! - `--chunk-mib N`: read size for `--mft` (default 4).
//! - `--buffer-kib N`: `FSCTL_ENUM_USN_DATA` output buffer size (default 1024).
//! - `--count-only`: use the zero-copy batch interface and keep nothing — measures the raw
//!   enumeration cost. By default every record is collected as an owned `RawRecord`, as an
//!   indexer would, and parent chains are checked against the root.

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    match windows_main::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("enum_volume: raw NTFS enumeration is only available on Windows");
    std::process::ExitCode::FAILURE
}

#[cfg(windows)]
mod windows_main {
    use std::error::Error;
    use std::ops::ControlFlow;
    use std::time::Instant;

    use whereabouts_ntfs::{
        EnumOptions, MftScanOptions, ParentChainReport, Volume, process_memory,
    };

    struct Args {
        volume: String,
        buffer_kib: usize,
        count_only: bool,
        mft: bool,
        chunk_mib: usize,
    }

    fn parse_args() -> Result<Args, Box<dyn Error>> {
        let mut args = Args {
            volume: "C:".to_owned(),
            buffer_kib: 1024,
            count_only: false,
            mft: false,
            chunk_mib: 4,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--buffer-kib" => {
                    args.buffer_kib = it.next().ok_or("--buffer-kib needs a value")?.parse()?;
                }
                "--count-only" => args.count_only = true,
                "--mft" => args.mft = true,
                "--chunk-mib" => {
                    args.chunk_mib = it.next().ok_or("--chunk-mib needs a value")?.parse()?;
                }
                "-h" | "--help" => {
                    println!(
                        "usage: enum_volume [VOLUME (default C:)] [--mft [--chunk-mib N]] \
                         [--buffer-kib N] [--count-only]"
                    );
                    std::process::exit(0);
                }
                other if !other.starts_with('-') => args.volume = other.to_owned(),
                other => return Err(format!("unknown option {other}").into()),
            }
        }
        Ok(args)
    }

    fn mib(bytes: u64) -> f64 {
        bytes as f64 / (1024.0 * 1024.0)
    }

    /// The `--mft` mode: the same figures as the USN mode, from a raw `$MFT` read.
    fn run_mft(volume: &Volume, args: &Args, root: u64) -> Result<(), Box<dyn Error>> {
        let geom = volume.geometry()?;
        println!(
            "geometry      sector={} cluster={} record={} mft_start_lcn={} mft_valid={:.1} MiB",
            geom.bytes_per_sector,
            geom.bytes_per_cluster,
            geom.bytes_per_record,
            geom.mft_start_lcn,
            mib(geom.mft_valid_data_length)
        );
        let options = MftScanOptions {
            chunk_bytes: args.chunk_mib << 20,
            ..MftScanOptions::default()
        };
        let before = process_memory().ok_or("GetProcessMemoryInfo failed")?;
        let started = Instant::now();
        let scanner = volume.open_mft()?;
        let opened = started.elapsed();
        let mut records = Vec::new();
        let (mut count, mut dirs, mut name_bytes) = (0_u64, 0_u64, 0_u64);
        let stats = scanner.scan(&options, &mut |rec| {
            count += 1;
            dirs += u64::from(rec.is_directory());
            name_bytes += rec.name.len() as u64;
            if !args.count_only {
                records.push(rec);
            }
            ControlFlow::Continue(())
        })?;
        let elapsed = started.elapsed();
        let after = process_memory().ok_or("GetProcessMemoryInfo failed")?;
        println!(
            "mode          raw $MFT, {} MiB chunks ({})",
            args.chunk_mib,
            if args.count_only {
                "count only"
            } else {
                "collect owned records"
            }
        );
        println!(
            "records       {count} ({dirs} directories, {} files); {} record slots in $MFT, {} unused",
            count - dirs,
            scanner.record_count(),
            stats.unused
        );
        println!(
            "lost          {} (torn {} after {} retries, malformed {}, not-FILE in use {}, \
             extension errors {}; expected 0); {} records without a name (legitimate)",
            stats.lost(),
            stats.torn,
            stats.torn_retried,
            stats.malformed,
            stats.not_file_in_use,
            stats.extension_errors,
            stats.no_name
        );
        println!(
            "details       {} extension records, {} hard-linked files, {} extension reads \
             ({} errors), {} free chunks skipped",
            stats.extension,
            stats.hard_link_files,
            stats.extension_reads,
            stats.extension_errors,
            stats.chunks_skipped
        );
        println!(
            "reads         {} returning {:.1} MiB",
            stats.reads,
            mib(stats.bytes)
        );
        println!("name length   {name_bytes} WTF-8 bytes total");
        println!(
            "elapsed       {:.3} s ({:.0} records/s); open {:.3} s, reading {:.3} s, in user code {:.3} s",
            elapsed.as_secs_f64(),
            count as f64 / elapsed.as_secs_f64().max(1e-9),
            opened.as_secs_f64(),
            stats.read_time.as_secs_f64(),
            elapsed
                .saturating_sub(stats.read_time + opened)
                .as_secs_f64()
        );
        println!(
            "working set   before {:.1} MiB, after {:.1} MiB, peak {:.1} MiB",
            mib(before.working_set),
            mib(after.working_set),
            mib(after.peak_working_set)
        );
        if !args.count_only {
            let links: Vec<(u64, u64)> = records.iter().map(|r| (r.frn, r.parent_frn)).collect();
            let chains = ParentChainReport::compute(&links, root);
            println!(
                "parent chains {:.4}% resolve to root ({} orphaned, {} cyclic, root present: {})",
                chains.resolved_ratio() * 100.0,
                chains.orphaned,
                chains.cyclic,
                chains.root_enumerated
            );
        }
        // A raw scan that lost records must not look like a complete listing.
        if stats.lost() > 0 {
            return Err(format!(
                "raw $MFT scan lost {} records (torn {}, malformed {}, not-FILE in use {}, \
                 extension errors {})",
                stats.lost(),
                stats.torn,
                stats.malformed,
                stats.not_file_in_use,
                stats.extension_errors
            )
            .into());
        }
        Ok(())
    }

    pub(crate) fn run() -> Result<(), Box<dyn Error>> {
        let args = parse_args()?;
        let volume = Volume::open(&args.volume)?;
        let info = volume.info();
        println!(
            "volume        {}: label={:?} fs={} serial={:08X}",
            info.letter, info.label, info.file_system, info.serial_number
        );
        match volume.query_journal()? {
            Some(j) => println!(
                "journal       id={:#x} first_usn={} next_usn={} max_size={} MiB",
                j.journal_id,
                j.first_usn,
                j.next_usn,
                j.maximum_size >> 20
            ),
            None => println!("journal       not active (not created; enumeration still works)"),
        }
        let root = volume.root_frn()?;
        println!("root frn      {root:#018x}");
        if args.mft {
            return run_mft(&volume, &args, root);
        }

        let options = EnumOptions {
            buffer_size: args.buffer_kib << 10,
            ..EnumOptions::default()
        };
        let before = process_memory().ok_or("GetProcessMemoryInfo failed")?;
        let started = Instant::now();
        let mut enumerator = volume.enumerate_with(options);
        let mut records = Vec::new();
        let (mut count, mut dirs, mut name_bytes) = (0_u64, 0_u64, 0_u64);
        let mut batch_skipped = 0_u64;
        if args.count_only {
            while let Some(batch) = enumerator.next_batch()? {
                for rec in batch {
                    // Same policy as the owned iterator: skip and count record-local errors,
                    // stop on structural ones — so both modes report comparable numbers.
                    let rec = match rec {
                        Ok(rec) => rec,
                        Err(e) if e.is_record_local() => {
                            batch_skipped += 1;
                            continue;
                        }
                        Err(e) => return Err(e.into()),
                    };
                    count += 1;
                    dirs += u64::from(rec.is_directory());
                    name_bytes += rec.name_utf16le().len() as u64 / 2;
                }
            }
        } else {
            for rec in enumerator.by_ref() {
                let rec = rec?;
                count += 1;
                dirs += u64::from(rec.is_directory());
                name_bytes += rec.name.len() as u64;
                records.push(rec);
            }
        }
        let elapsed = started.elapsed();
        let after = process_memory().ok_or("GetProcessMemoryInfo failed")?;
        let stats = enumerator.stats();

        println!(
            "mode          {} (buffer {} KiB)",
            if args.count_only {
                "count-only (zero-copy batches)"
            } else {
                "collect owned records"
            },
            options.buffer_bytes() >> 10
        );
        println!(
            "records       {count} ({dirs} directories, {} files)",
            count - dirs
        );
        println!(
            "skipped       {} (records with parse errors; expected 0)",
            stats.skipped + batch_skipped
        );
        println!(
            "kernel calls  {} returning {:.1} MiB",
            stats.batches,
            mib(stats.bytes)
        );
        println!(
            "name length   {name_bytes} {} total",
            if args.count_only {
                "UTF-16 units"
            } else {
                "WTF-8 bytes"
            }
        );
        println!(
            "elapsed       {:.3} s ({:.0} records/s); in kernel {:.3} s, in user code {:.3} s",
            elapsed.as_secs_f64(),
            count as f64 / elapsed.as_secs_f64().max(1e-9),
            stats.kernel_time.as_secs_f64(),
            elapsed.saturating_sub(stats.kernel_time).as_secs_f64()
        );
        println!(
            "working set   before {:.1} MiB, after {:.1} MiB, peak {:.1} MiB",
            mib(before.working_set),
            mib(after.working_set),
            mib(after.peak_working_set)
        );

        if !args.count_only {
            let links: Vec<(u64, u64)> = records.iter().map(|r| (r.frn, r.parent_frn)).collect();
            let chain_started = Instant::now();
            let chains = ParentChainReport::compute(&links, root);
            println!(
                "parent chains {:.4}% resolve to root ({} orphaned, {} cyclic, root enumerated: {}) in {:.3} s",
                chains.resolved_ratio() * 100.0,
                chains.orphaned,
                chains.cyclic,
                chains.root_enumerated,
                chain_started.elapsed().as_secs_f64()
            );
            let peak = process_memory().map_or(0, |m| m.peak_working_set);
            println!("peak (final)  {:.1} MiB", mib(peak));
        }
        Ok(())
    }
}
