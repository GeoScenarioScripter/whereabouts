$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'statistics.ps1')
foreach ($case in @(@{n=20; expected='10,19,20'}, @{n=50; expected='25,48,50'})) {
    $values = [double[]](1..$case.n)
    $actual = @(0.5,0.95,0.99 | ForEach-Object { Get-NearestRank $values $_ }) -join ','
    if ($actual -ne $case.expected) { throw "N=$($case.n): $actual != $($case.expected)" }
}
Write-Output 'Nearest-rank boundary checks passed (N=20, N=50).'
