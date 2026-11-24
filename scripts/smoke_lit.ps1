# Share of the last sampled frame that is not black, from report.json's
# d3d12.last_frame_dump. `n/a` when no frame was sampled or the format was not decoded.
function Get-XclSmokeLit([string]$Report) {
    if (-not (Test-Path -LiteralPath $Report)) { return 'n/a' }
    try {
        $dump = (Get-Content -LiteralPath $Report -Raw -Encoding utf8 | ConvertFrom-Json).d3d12.last_frame_dump
    } catch {
        return 'n/a'
    }
    if (-not $dump -or $null -eq $dump.sampled_pixels -or $null -eq $dump.non_dark_samples) { return 'n/a' }
    $pixels = [int64]$dump.sampled_pixels
    if ($pixels -le 0) { return 'n/a' }
    $pct = 100.0 * [int64]$dump.non_dark_samples / $pixels
    # A comma decimal would not parse in pr_check.py, so the line is always dotted.
    return ([string]::Format([Globalization.CultureInfo]::InvariantCulture, '{0:0.#} %', [double]$pct))
}
