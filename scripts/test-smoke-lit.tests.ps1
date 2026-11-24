# Run with powershell.exe. Checks the lit-frame line without starting xcl.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'smoke_lit.ps1')

function Assert-Eq($name, $got, $want) {
    if ($got -ne $want) { throw "$name : got '$got' wanted '$want'" }
}

$dir = Join-Path ([IO.Path]::GetTempPath()) ('xcl-smoke-lit-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $dir | Out-Null
try {
    $report = Join-Path $dir 'report.json'
    function Write-Report($pixels, $lit) {
        $litJson = if ($null -eq $lit) { 'null' } else { "$lit" }
        @"
{"d3d12":{"last_frame_dump":{"frame":1,"width":8,"height":8,"format":0,"sample_step":4,"sampled_pixels":$pixels,"non_dark_samples":$litJson}}}
"@ | Set-Content -LiteralPath $report -Encoding utf8
    }
    Write-Report 1000 1000
    Assert-Eq 'all lit' (Get-XclSmokeLit $report) '100 %'
    Write-Report 1000 0
    Assert-Eq 'black' (Get-XclSmokeLit $report) '0 %'
    Write-Report 1000 4
    Assert-Eq 'fraction' (Get-XclSmokeLit $report) '0.4 %'
    Write-Report 1000 $null
    Assert-Eq 'undecoded' (Get-XclSmokeLit $report) 'n/a'
    Write-Report 0 0
    Assert-Eq 'no samples' (Get-XclSmokeLit $report) 'n/a'
    '{"d3d12":{"last_frame_dump":null}}' | Set-Content -LiteralPath $report -Encoding utf8
    Assert-Eq 'no dump' (Get-XclSmokeLit $report) 'n/a'
    Assert-Eq 'missing file' (Get-XclSmokeLit (Join-Path $dir 'nope.json')) 'n/a'
    $invalid = Join-Path $dir 'invalid.exe'
    [IO.File]::WriteAllText($invalid, 'not an executable')
    $savedFrameDump = $env:XCL_FRAME_DUMP
    try {
        $env:XCL_FRAME_DUMP = '37'
        $failed = $false
        try {
            & (Join-Path $PSScriptRoot 'smoke.ps1') -Game $dir -Xcl $invalid -Seconds 1 | Out-Null
        } catch {
            $failed = $true
        }
        if (-not $failed) { throw 'Invalid executable did not fail to start.' }
        Assert-Eq 'caller frame dump after a failed start' $env:XCL_FRAME_DUMP '37'
    } finally {
        if ($null -eq $savedFrameDump) {
            Remove-Item Env:XCL_FRAME_DUMP -ErrorAction SilentlyContinue
        } else {
            $env:XCL_FRAME_DUMP = $savedFrameDump
        }
    }
} finally {
    Remove-Item -LiteralPath $dir -Recurse -Force
}
Write-Output 'smoke lit tests passed'
