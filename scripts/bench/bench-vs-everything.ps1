# Run from the repo root. All writes are benchmark output or isolated temporary catalogs.
[CmdletBinding()]
param(
    [ValidatePattern('^[A-Za-z]:$')][string]$Volume = 'D:',
    [string]$Database,
    [string]$Everything = 'es.exe',
    [string]$Output = "benchmark-results-$(Get-Date -Format yyyyMMdd-HHmmss)",
    [ValidateSet('none','usn','mft')][string]$Enumeration = 'none',
    [ValidateSet('uncontrolled','warm','first-after-reboot')][string]$CacheState = 'uncontrolled',
    [switch]$EnumerationOnly,
    [switch]$SkipComponents,
    [switch]$SkipBuild,
    [switch]$PrepareOnly,
    [ValidateRange(20,1000)][int]$Runs = 50,
    [ValidateRange(1000,10000000)][int]$Entries = 1000000,
    [ValidateRange(100,1000000)][int]$CatalogDatasets = 100000,
    [string[]]$Terms = @('a','ro','map','road','river','parcel','imagery','elevation')
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'statistics.ps1')
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Push-Location $repo
try {
    $out = [IO.Path]::GetFullPath($Output)
    if (Test-Path -LiteralPath $out) { throw "Output directory already exists: $out" }
    New-Item -ItemType Directory -Path $out | Out-Null
    $metadata = [ordered]@{ date_utc=(Get-Date).ToUniversalTime().ToString('o'); boot_utc=(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime().ToString('o'); commit=(& git rev-parse HEAD); dirty=([bool](& git status --porcelain)); os=[Environment]::OSVersion.ToString(); cpu=(Get-CimInstance Win32_Processor | Select-Object -ExpandProperty Name); ram_bytes=(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory; volume=$Volume; enumeration=$Enumeration; cache_label=$CacheState; entries=$Entries; runs=$Runs; terms=$Terms; rust=(& rustc --version); everything=$Everything; quantile='nearest-rank ceil(N*p)-1'; binary_sha256=@{} }
    $metadata | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'environment.json')
    function Build([string[]]$BuildArgs) { if ($SkipBuild) { return }; & cargo @BuildArgs; if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' } }
    function Record-Binary([string]$Executable) {
        $metadata.binary_sha256[$Executable] = (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash
        $metadata | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'environment.json')
    }
    function Logged([string]$Executable, [string[]]$Arguments, [string]$Log) {
        Record-Binary $Executable
        & $Executable @Arguments 2>&1 | Tee-Object -FilePath (Join-Path $out $Log)
        if ($LASTEXITCODE -ne 0) { throw "$Executable failed; see $Log" }
    }
    if ($PrepareOnly) {
        if ($SkipBuild) { throw 'PrepareOnly and SkipBuild cannot be combined.' }
        Build @('build','--release','-p','whereabouts-ntfs','--example','enum_volume')
        Build @('build','--release','-p','whereabouts-cli')
        Build @('build','--release','-p','whereabouts-nameindex','--example','measure')
        Build @('build','--release','-p','whereabouts-catalog','--example','bulk_insert')
        Build @('build','--release','-p','whereabouts-oracle','--example','pipeline_costs')
        return
    }
    if ($EnumerationOnly -and $Enumeration -eq 'none') { throw 'EnumerationOnly requires usn or mft.' }
    # For cold enumeration, build before reboot separately; never run another scan first.
    if ($Enumeration -ne 'none') {
        $enumExe = Join-Path $repo 'target/release/examples/enum_volume.exe'
        if (!(Test-Path $enumExe)) { throw 'Build enum_volume before reboot: cargo build --release -p whereabouts-ntfs --example enum_volume' }
        $enumArgs = @($Volume, '--count-only')
        if ($Enumeration -eq 'mft') { $enumArgs += '--mft' }
        if ($CacheState -eq 'warm') { Logged $enumExe $enumArgs "enum-$Enumeration-warmup.txt" }
        Logged $enumExe $enumArgs "enum-$Enumeration-$CacheState.txt"
        Logged $enumExe $enumArgs "enum-$Enumeration-warm-repeat.txt"
    }
    if ($EnumerationOnly) { return }
    if (!$SkipComponents) {
    Build @('build','--release','-p','whereabouts-nameindex','--example','measure')
    Build @('build','--release','-p','whereabouts-catalog','--example','bulk_insert')
    Build @('build','--release','-p','whereabouts-oracle','--example','pipeline_costs')
    Logged 'target/release/examples/measure.exe' @("$Entries") 'nameindex.txt'
    Logged 'target/release/examples/bulk_insert.exe' @("$CatalogDatasets",'1000') 'catalog.txt'
    Logged 'target/release/examples/pipeline_costs.exe' @("$Entries") 'pipeline.txt'
    }
    $rows = [Collections.Generic.List[object]]::new()
    $es = Get-Command $Everything -ErrorAction SilentlyContinue
    if (!$es) { 'Everything es.exe unavailable; comparison NOT measured.' | Set-Content (Join-Path $out 'everything-missing.txt') }
    if ($es) {
        Record-Binary $es.Source
        $metadata.everything_file_version = [Diagnostics.FileVersionInfo]::GetVersionInfo($es.Source).FileVersion
    }
    if ($Database) {
        Build @('build','--release','-p','whereabouts-cli')
        $Database = (Resolve-Path -LiteralPath $Database).Path
        Record-Binary 'target/release/whereabouts.exe'
    }
    if (!$Database) { 'No owner catalog supplied; Whereabouts CLI comparison NOT measured.' | Set-Content (Join-Path $out 'catalog-missing.txt') }
    $metadata | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'environment.json')
    function Sample([string]$Tool, [string]$Exe, [string[]]$CommandArgs, [string]$Term, [int]$SampleId) {
        # Process launch + snapshot/catalog load + query + rendering, not an in-process scan.
            $watch=[Diagnostics.Stopwatch]::StartNew()
            & $Exe @CommandArgs > $null
            $watch.Stop()
            if ($LASTEXITCODE -ne 0) { throw "$Tool query failed" }
            if ($SampleId -ge 0) { $rows.Add([pscustomobject]@{tool=$Tool; term=$Term; term_length=$Term.Length; sample=$SampleId; milliseconds=$watch.Elapsed.TotalMilliseconds}) }
    }
    foreach ($term in $Terms) {
        $jobs = @()
        if ($Database) { $jobs += @{tool='whereabouts-cli'; exe='target/release/whereabouts.exe'; arguments=@('query',$term,'--db',$Database,'--limit','100','--json')} }
        if ($es) { $jobs += @{tool='everything-es'; exe=$es.Source; arguments=@('-n','100','-no-case','-no-whole-word','-no-match-path','-path',"$Volume\",$term)} }
        foreach ($job in $jobs) { Sample $job.tool $job.exe $job.arguments $term -1 }
        for ($i=0; $i -lt $Runs; $i++) {
            # Alternate process order to reduce systematic drift between the two tools.
            $ordered = @($jobs)
            if ($i % 2) { [array]::Reverse($ordered) }
            foreach ($job in $ordered) { Sample $job.tool $job.exe $job.arguments $term $i }
        }
    }
    $rows | Export-Csv -NoTypeInformation (Join-Path $out 'query-samples.csv')
    $summary = foreach ($group in ($rows | Group-Object tool,term)) {
        $sorted=@($group.Group.milliseconds | Sort-Object)
        $row=$group.Group[0]
        [pscustomobject]@{tool=$row.tool;term=$row.term;length=$row.term_length;count=$sorted.Count;p50_ms=(Get-NearestRank $sorted .5);p95_ms=(Get-NearestRank $sorted .95);p99_ms=(Get-NearestRank $sorted .99)}
    }
    $summary | Export-Csv -NoTypeInformation (Join-Path $out 'query-summary.csv')
    Write-Host "Evidence saved to $out. Record storage model, Everything version/index scope and reboot conditions with these results."
} finally { Pop-Location }

