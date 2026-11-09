# Run from the repo root. All writes are benchmark output or isolated temporary catalogs.
[CmdletBinding()]
param(
    [ValidatePattern('^[A-Za-z]:$')][string]$Volume = 'D:',
    [string]$Database,
    [string]$Everything = 'es.exe',
    [string]$Output = "benchmark-results-$(Get-Date -Format yyyyMMdd-HHmmss)",
    [ValidateSet('none','usn','mft')][string]$Enumeration = 'none',
    [ValidateSet('warm','first-after-reboot')][string]$CacheState = 'warm',
    [ValidateRange(20,1000)][int]$Runs = 50,
    [ValidateRange(1000,10000000)][int]$Entries = 1000000,
    [ValidateRange(100,1000000)][int]$CatalogDatasets = 100000,
    [string[]]$Terms = @('a','ro','map','road','river','parcel','imagery','elevation')
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Push-Location $repo
try {
    $out = [IO.Path]::GetFullPath($Output)
    if (Test-Path -LiteralPath $out) { throw "Output directory already exists: $out" }
    New-Item -ItemType Directory -Path $out | Out-Null
    $metadata = [ordered]@{ date_utc=(Get-Date).ToUniversalTime().ToString('o'); commit=(& git rev-parse HEAD); os=[Environment]::OSVersion.ToString(); cpu=(Get-CimInstance Win32_Processor | Select-Object -ExpandProperty Name); ram_bytes=(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory; volume=$Volume; enumeration=$Enumeration; cache_label=$CacheState; entries=$Entries; runs=$Runs; terms=$Terms; rust=(& rustc --version); everything=$Everything }
    $metadata | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'environment.json')
    function Build([string[]]$BuildArgs) { & cargo @BuildArgs; if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' } }
    function Logged([string]$Executable, [string[]]$Arguments, [string]$Log) {
        & $Executable @Arguments 2>&1 | Tee-Object -FilePath (Join-Path $out $Log)
        if ($LASTEXITCODE -ne 0) { throw "$Executable failed; see $Log" }
    }
    # For cold enumeration, build before reboot separately; never run another scan first.
    if ($Enumeration -ne 'none') {
        $enumExe = Join-Path $repo 'target/release/examples/enum_volume.exe'
        if (!(Test-Path $enumExe)) { throw 'Build enum_volume before reboot: cargo build --release -p whereabouts-ntfs --example enum_volume' }
        $enumArgs = @($Volume, '--count-only')
        if ($Enumeration -eq 'mft') { $enumArgs += '--mft' }
        Logged $enumExe $enumArgs "enum-$Enumeration-$CacheState.txt"
        Logged $enumExe $enumArgs "enum-$Enumeration-warm-repeat.txt"
    }
    Build @('build','--release','-p','whereabouts-nameindex','--example','measure')
    Build @('build','--release','-p','whereabouts-catalog','--example','bulk_insert')
    Build @('build','--release','-p','whereabouts-oracle','--example','pipeline_costs')
    Logged 'target/release/examples/measure.exe' @("$Entries") 'nameindex.txt'
    Logged 'target/release/examples/bulk_insert.exe' @("$CatalogDatasets",'1000') 'catalog.txt'
    Logged 'target/release/examples/pipeline_costs.exe' @("$Entries") 'pipeline.txt'
    $rows = [Collections.Generic.List[object]]::new()
    $es = Get-Command $Everything -ErrorAction SilentlyContinue
    if (!$es) { 'Everything es.exe unavailable; comparison NOT measured.' | Set-Content (Join-Path $out 'everything-missing.txt') }
    if ($Database) {
        Build @('build','--release','-p','whereabouts-cli')
        $Database = (Resolve-Path -LiteralPath $Database).Path
    }
    function Sample([string]$Tool, [string]$Exe, [string[]]$CommandArgs, [string]$Term) {
        # Process launch + snapshot/catalog load + query + rendering, not an in-process scan.
        & $Exe @CommandArgs > $null
        if ($LASTEXITCODE -ne 0) { throw "$Tool warmup failed" }
        for ($i=0; $i -lt $Runs; $i++) {
            $watch=[Diagnostics.Stopwatch]::StartNew()
            & $Exe @CommandArgs > $null
            $watch.Stop()
            if ($LASTEXITCODE -ne 0) { throw "$Tool query failed" }
            $rows.Add([pscustomobject]@{tool=$Tool; term=$Term; term_length=$Term.Length; sample=$i; milliseconds=$watch.Elapsed.TotalMilliseconds})
        }
    }
    foreach ($term in $Terms) {
        if ($Database) { Sample 'whereabouts-cli' 'target/release/whereabouts.exe' @('query',$term,'--db',$Database,'--limit','100','--json') $term }
        if ($es) { Sample 'everything-es' $es.Source @('-n','100',"$Volume\",$term) $term }
    }
    $rows | Export-Csv -NoTypeInformation (Join-Path $out 'query-samples.csv')
    $summary = foreach ($group in ($rows | Group-Object tool,term)) {
        $sorted=@($group.Group.milliseconds | Sort-Object)
        $row=$group.Group[0]
        [pscustomobject]@{tool=$row.tool;term=$row.term;length=$row.term_length;count=$sorted.Count;p50_ms=$sorted[[Math]::Ceiling(($sorted.Count-1)*.5)];p95_ms=$sorted[[Math]::Ceiling(($sorted.Count-1)*.95)];p99_ms=$sorted[[Math]::Ceiling(($sorted.Count-1)*.99)]}
    }
    $summary | Export-Csv -NoTypeInformation (Join-Path $out 'query-summary.csv')
    Write-Host "Evidence saved to $out. Record storage model, Everything version/index scope and reboot conditions with these results."
} finally { Pop-Location }

