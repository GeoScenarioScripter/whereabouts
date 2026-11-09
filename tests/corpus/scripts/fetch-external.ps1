# Fetch the large real-world samples listed in external-files.txt into tests/corpus/external/
# (gitignored) and verify each file's sha256. Files already present with the right hash are kept.
# Usage: pwsh tests/corpus/scripts/fetch-external.ps1   (Windows PowerShell 5.1 works too)
$ErrorActionPreference = 'Stop'
# The progress bar makes Invoke-WebRequest orders of magnitude slower on Windows PowerShell 5.1.
$ProgressPreference = 'SilentlyContinue'

$destRoot = Join-Path $PSScriptRoot '..\external'
$list = Join-Path $PSScriptRoot 'external-files.txt'
$failed = $false

foreach ($line in Get-Content -LiteralPath $list) {
    $line = $line.Trim()
    if ($line -eq '' -or $line.StartsWith('#')) { continue }
    $sha, $rel, $url = $line -split '\s+', 3
    if ([System.IO.Path]::IsPathRooted($rel) -or $rel.Contains('..')) {
        Write-Error "refusing unsafe path: $rel" -ErrorAction Continue; $failed = $true; continue
    }
    $out = Join-Path $destRoot $rel
    if ((Test-Path -LiteralPath $out) -and ((Get-FileHash -Algorithm SHA256 -LiteralPath $out).Hash.ToLower() -eq $sha)) {
        Write-Output "ok (cached)  $rel"; continue
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $out) | Out-Null
    $tmp = "$out.part"
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $tmp
    $got = (Get-FileHash -Algorithm SHA256 -LiteralPath $tmp).Hash.ToLower()
    if ($got -ne $sha) {
        Remove-Item -LiteralPath $tmp -Force
        Write-Error "SHA256 MISMATCH ${rel}: expected $sha, got $got" -ErrorAction Continue
        $failed = $true; continue
    }
    Move-Item -Force -LiteralPath $tmp -Destination $out
    Write-Output "fetched      $rel"
}

if ($failed) { exit 1 }
