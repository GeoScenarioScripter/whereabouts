# Shared nearest-rank estimator: rank = ceil(N * p), with one-based ranks.
function Get-NearestRank {
    param([Parameter(Mandatory)][double[]]$SortedValues, [Parameter(Mandatory)][double]$Probability)
    if ($SortedValues.Count -eq 0 -or $Probability -le 0 -or $Probability -gt 1) {
        throw 'Nearest rank requires nonempty samples and 0 < probability <= 1.'
    }
    $SortedValues[[Math]::Ceiling($SortedValues.Count * $Probability) - 1]
}
