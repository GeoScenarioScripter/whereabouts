//! Enumerate an NTFS volume and print record count, elapsed time and peak working set.
//!
//! Run from an **elevated** terminal (read-only; nothing on the volume is modified):
//!
//! ```text
//! cargo run --release -p whereabouts-ntfs --example enum_volume -- C:
//! cargo run --release -p whereabouts-ntfs --example enum_volume -- D: --buffer-kib 64
//! cargo run --release -p whereabouts-ntfs --example enum_volume -- C: --count-only
//! ```
//!
//! Options:
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
    use std::time::Instant;

    use whereabouts_ntfs::{EnumOptions, ParentChainReport, Volume, process_memory};

    struct Args {
        volume: String,
        buffer_kib: usize,
        count_only: bool,
    }

    fn parse_args() -> Result<Args, Box<dyn Error>> {
        let mut args = Args {
            volume: "C:".to_owned(),
            buffer_kib: 1024,
            count_only: false,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--buffer-kib" => {
                    args.buffer_kib = it.next().ok_or("--buffer-kib needs a value")?.parse()?;
                }
                "--count-only" => args.count_only = true,
                "-h" | "--help" => {
                    println!(
                        "usage: enum_volume [VOLUME (default C:)] [--buffer-kib N] [--count-only]"
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

        let options = EnumOptions {
            buffer_size: args.buffer_kib << 10,
            ..EnumOptions::default()
        };
        let before = process_memory().ok_or("GetProcessMemoryInfo failed")?;
        let started = Instant::now();
        let mut enumerator = volume.enumerate_with(options);
        let mut records = Vec::new();
        let (mut count, mut dirs, mut name_bytes) = (0_u64, 0_u64, 0_u64);
        if args.count_only {
            while let Some(batch) = enumerator.next_batch()? {
                for rec in batch {
                    let rec = rec?;
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
            stats.skipped
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
            "elapsed       {:.3} s ({:.0} records/s)",
            elapsed.as_secs_f64(),
            count as f64 / elapsed.as_secs_f64().max(1e-9)
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
