# Step 3: assess through $EKOS_PROJECT (its own --project means the Talend project), then console sign-in.
$ErrorActionPreference = 'Continue'
$out = 'C:\ekos-test\out'; $log = "$out\log3"
New-Item -ItemType Directory -Force -Path $log | Out-Null
$ekos = Get-ChildItem 'C:\ekos' -Recurse -File -Filter 'ekos*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch 'desktop|setup' } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
if (-not $ekos) { Write-Host 'No ekos*.exe under C:\ekos' -ForegroundColor Red; exit 1 }
$env:EKOS_PROJECT = 'didemo'
function Run([string]$name, [string[]]$a) {
    Write-Host ">> ekos $($a -join ' ')"
    try { $o = & $ekos @a 2>&1 | ForEach-Object { "$_" }; $code = $LASTEXITCODE } catch { $o = "$_"; $code = -1 }
    @("COMMAND (EKOS_PROJECT=didemo): ekos $($a -join ' ')", "EXIT: $code", '---') + $o | Set-Content "$log\$name.txt" -Encoding UTF8
    Write-Host "   exit=$code"
}
Run '01-assess-md'   @('talend', 'assess', '--out', "$out\assess.md")
Run '02-assess-json' @('talend', 'assess', '--json', '--out', "$out\assess.json")
Run '03-assess-named' @('talend', 'assess', '--project', 'LOCAL_PROJECT_DATA_INTEGRATION_DEMOS', '--out', "$out\assess-named.md")
Run '04-console-status' @('console', 'status')
Remove-Item Env:\EKOS_PROJECT
Write-Host ''
Write-Host 'Console sign-in: a signed-in page opens, or a one-time code is printed (valid two minutes).' -ForegroundColor Yellow
& $ekos console open
