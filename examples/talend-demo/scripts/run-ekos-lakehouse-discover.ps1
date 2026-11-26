# Discover what `ekos lakehouse` and `ekos docs` offer in the build from main. Read-only: only --help and status commands.
$ErrorActionPreference = 'Continue'
$out = 'C:\ekos-test\out\lakehouse'; New-Item -ItemType Directory -Force -Path $out | Out-Null
$ekos = Get-ChildItem 'C:\ekos' -Recurse -File -Filter 'ekos*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch 'desktop|setup' } | Sort-Object LastWriteTime -Descending |
    Select-Object -First 1 -ExpandProperty FullName
if (-not $ekos) { Write-Host 'No ekos*.exe under C:\ekos' -ForegroundColor Red; exit 1 }
Copy-Item (Join-Path (Split-Path $ekos) 'README.md') "$out\README-ekos.md" -Force -ErrorAction SilentlyContinue
$seen = @{}
function HelpTree([string[]]$path, [int]$depth) {
    $key = $path -join ' '
    if ($seen[$key] -or $depth -gt 4) { return }; $seen[$key] = 1
    $o = & $ekos @($path + '--help') 2>&1 | ForEach-Object { "$_" }
    @("COMMAND: ekos $key --help", '---') + $o | Set-Content ("$out\help-" + ($key -replace '\s+','_') + '.txt') -Encoding UTF8
    $in = $false
    foreach ($l in $o) {
        if ($l -match '^Commands:') { $in = $true; continue }
        if ($in -and $l -match '^\S') { $in = $false }
        if ($in -and $l -match '^\s{2}([a-z][a-z0-9-]*)\s') { if ($matches[1] -ne 'help') { HelpTree ($path + $matches[1]) ($depth + 1) } }
    }
}
HelpTree @('lakehouse') 0
HelpTree @('docs') 0
HelpTree @('talend') 0
$env:EKOS_PROJECT = 'didemo'
& $ekos lakehouse status 2>&1 | Set-Content "$out\status.txt" -Encoding UTF8
Remove-Item Env:\EKOS_PROJECT
$tools = foreach ($t in 'docker', 'trino', 'dbt', 'java', 'python', 'duckdb', 'zstd') {
    $c = Get-Command $t -ErrorAction SilentlyContinue; "{0,-8} {1}" -f $t, $(if ($c) { $c.Source } else { '-' }) }
$tools | Set-Content "$out\tools.txt" -Encoding UTF8
Get-ChildItem $out | Select-Object Name, Length | Format-Table -AutoSize
$tools
