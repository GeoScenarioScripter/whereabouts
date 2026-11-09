# Run manually from an elevated PowerShell after each owner-controlled reboot.
[CmdletBinding()]
param([Parameter(Mandatory)][ValidateSet('usn','mft')][string]$Method,
      [Parameter(Mandatory)][ValidateRange(1,3)][int]$Repeat)
$ErrorActionPreference='Stop'
$repo=(Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Set-Location $repo
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (!$admin) { throw 'Open an elevated PowerShell first.' }
$boot=(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime().ToString('o')
$ledger=Join-Path $repo 'target/v4-boot-ledger.txt'
if ((Test-Path $ledger) -and ((Get-Content $ledger) -contains $boot)) { throw 'This boot already has a measurement. Reboot manually before the next cold-first method.' }
$env:CARGO_HOME='D:/code/whereabouts/.tools/cargo'
$env:RUSTUP_HOME='D:/code/whereabouts/.tools/rustup'
$env:PATH="$env:CARGO_HOME/bin;$env:PATH"
$env:GIT_CONFIG_COUNT='1'
$env:GIT_CONFIG_KEY_0='safe.directory'
$env:GIT_CONFIG_VALUE_0=$repo
# Reserve the boot before measurement; a failed attempt must not be relabelled cold.
$boot | Add-Content $ledger
& ./scripts/bench/bench-vs-everything.ps1 -Volume D: -Enumeration $Method -CacheState first-after-reboot -EnumerationOnly -SkipBuild -Output "target/v4-boot-$Method-$Repeat"

