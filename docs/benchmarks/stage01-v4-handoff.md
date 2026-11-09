# Stage01 owner V4 handoff to session 5.1

Software verification in 4.3 is V2. V4 remains a stage-exit requirement. On 2026-11-09 the
orchestrator found no supplied owner logs, no elevated shell, no running Everything/es.exe,
and no existing owner benchmark catalog. The workstation last booted on November 4, so no
current scan can be called first-after-reboot. Nothing below was run against an owner volume
by session 4.3. Session 5.1 collects these results and presents Q-16/Q-18 for owner decision.

## Preparation (ordinary terminal, before reboot)

Use the final merged `develop` commit and Rust toolchain. Do not benchmark while compiling,
regenerating the corpus, copying large files or running other session workers.

```powershell
# Run at the repository root; builds only, no volume scan or measurement.
pwsh -NoProfile -File scripts/bench/bench-vs-everything.ps1 -PrepareOnly -Output target/v4-preparation
```

Obtain ES from the [official voidtools download page](https://www.voidtools.com/downloads/),
start Everything normally, enable the same local NTFS volume, and wait until its indexing
finishes. Record both Everything application version and ES version, included volumes and
exclusions, SSD model, power mode, background activity, and the final Whereabouts commit.
The script saves CPU/RAM/OS, boot timestamp, executable hashes and ES file version.

## Enumeration (elevated terminal, separate boots)

Reboot before the first USN measurement. Do not build, open the GIS tree or enumerate it first.
The script uses the prebuilt executable, does two read-only count-only scans and returns.
The first-run cache label is the operator's statement, not a cache-flush claim.

```powershell
pwsh -NoProfile -File scripts/bench/bench-vs-everything.ps1 -Volume D: -Enumeration usn -CacheState first-after-reboot -EnumerationOnly -SkipBuild -Output target/v4-usn-boot1
# Reboot again, then run raw MFT first on the otherwise idle volume:
pwsh -NoProfile -File scripts/bench/bench-vs-everything.ps1 -Volume D: -Enumeration mft -CacheState first-after-reboot -EnumerationOnly -SkipBuild -Output target/v4-mft-boot2
```

Repeat each first-scan method across three separate boots; retain all runs. Each directory
contains its first scan and immediate warm repeat. A `warm` label explicitly performs an
untimed warmup first. The default `uncontrolled` label never claims warm or cold cache.
Compare record counts and review raw-MFT metadata-record caveats from session 2.7. Resolve any
unexpected count difference before selecting a default. The script never reboots, provisions
a journal, changes volume settings or flushes caches.

## Full index and query comparison

In an elevated terminal, index the owner's real volume into a dedicated output directory.
Use an output path that is not itself a GIS dataset directory; preserve an existing catalog
rather than overwriting it. Indexing creates its catalog/snapshot/retry files, not GIS sidecars.

```powershell
$evidence = Join-Path $env:LOCALAPPDATA 'Whereabouts/stage01-v4'
New-Item -ItemType Directory -Path $evidence -ErrorAction Stop
& target/release/whereabouts.exe index --volume D: --db "$evidence/catalog.sqlite" 2>&1 | Tee-Object "$evidence/index.log"
if ($LASTEXITCODE -ne 0) { throw 'Indexing failed; retain the log and diagnose before benchmarking.' }
# ES path is an example: substitute the installed executable's absolute path.
pwsh -NoProfile -File scripts/bench/bench-vs-everything.ps1 -Volume D: -Database "$evidence/catalog.sqlite" -Everything C:/Tools/es.exe -SkipBuild -SkipComponents -Output target/v4-query
```

Query comparisons need no new enumeration. After one discarded warmup per tool/term, the
script alternates process order and records 50 samples per tool/term. ES receives explicit
case-insensitive/name matching and a `-path` volume filter. Both output limits are 100.
CLI timing includes process startup, Whereabouts snapshot/catalog load or Everything IPC,
query and output processing. These result sets differ: datasets versus files/folders. A resident
Everything index is not equivalent to a fresh Whereabouts CLI process; retain this caveat.
Missing ES or missing catalog produces explicit not-measured evidence, never a passing score.

Submit the output directories, index log, machine/storage notes and reboot conditions to 5.1.
Run component benchmarks separately after these measurements, using the default script without
`SkipComponents`; their synthetic data is not a substitute for the owner-volume results.
Q-16/Q-18 remain Open until the owner reviews the evidence. No hardware or raw-volume privileges
are needed to rerun the committed oracle/GDAL drift tests.
