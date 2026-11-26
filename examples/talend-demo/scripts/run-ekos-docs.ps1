# Documentation for the Talend demo with its text shown. EKOS shows text only from a named, licensed source, and an
# existing project cannot be given source names, so this creates project "didemodocs" with one source "talend-demo"
# (same files as didemo), licenses it Apache-2.0 (Talend Open Studio and its demos), compiles it and writes the docs
# under E:\ekos-work\out\docs-v2, with its EKOS home on E:. Project didemo is left as it is.
$ErrorActionPreference = 'Continue'
$W = 'E:\ekos-work'; $H = "$W\ekos-home"
$out = "$W\out\docs-v2"; New-Item -ItemType Directory -Force -Path $out | Out-Null
$log = "$out\run.txt"; "docs run $(Get-Date -Format s)" | Set-Content $log -Encoding UTF8
$ekos = Get-ChildItem 'C:\ekos' -Recurse -File -Filter 'ekos*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch 'desktop|setup' } | Sort-Object LastWriteTime -Descending |
    Select-Object -First 1 -ExpandProperty FullName
if (-not $ekos) { Write-Host 'No ekos*.exe under C:\ekos' -ForegroundColor Red; exit 1 }
function Run([string]$name, [string[]]$a) {
    Write-Host ">> ekos $($a -join ' ')"
    $o = & $ekos --home $H @a 2>&1 | ForEach-Object { "$_" } | Where-Object { $_ -notmatch 'RemoteException' }
    @("== ekos $($a -join ' ')  exit=$LASTEXITCODE") + ($o | Select-Object -Last 40) | Add-Content $log
    Write-Host "   exit=$LASTEXITCODE"
}
$p = 'didemodocs'
$toml = (Get-ChildItem "$H\data" -Recurse -Filter ekos.toml -ErrorAction SilentlyContinue | Where-Object { $_.Directory.Name -eq $p } | Select-Object -First 1).FullName
function FindToml { (Get-ChildItem "$H\data" -Recurse -Filter ekos.toml -ErrorAction SilentlyContinue | Where-Object { $_.Directory.Name -eq $p } | Select-Object -First 1).FullName }
if (-not $toml) { Run 'project-add' @('project', 'add', $p, '--source', 'talend-demo=C:\ekos-test\src\di-demos') }
$toml = FindToml
if (-not $toml) { Write-Host "project $p was not created - see $log" -ForegroundColor Red; Get-Content $log; exit 1 }
$text = [IO.File]::ReadAllText($toml)
if ($text -notmatch '(?m)^\[docs-estate\]') {
    $text = $text.TrimEnd() + "`r`n`r`n# Talend Open Studio and its bundled demo projects are Apache-2.0 (set for the docs)`r`n[docs-estate]`r`nlicences = { `"talend-demo`" = `"Apache-2.0`" }`r`n"
    [IO.File]::WriteAllText($toml, $text, (New-Object Text.UTF8Encoding $false))
}
Copy-Item $toml "$out\ekos.toml" -Force
foreach ($stage in 'build', 'recover', 'resolve', 'compile', 'commit') { Run $stage @('--project', $p, $stage) }
foreach ($d in 'estate', 'solution-architect', 'curated') { Remove-Item "$out\$d" -Recurse -Force -ErrorAction SilentlyContinue }
Run 'estate'  @('--project', $p, 'docs', 'generate', '--layout', 'estate', '--output', "$out\estate")
Run 'sa'      @('--project', $p, 'docs', 'generate', '--layout', 'solution-architect', '--output', "$out\solution-architect")
Run 'curated' @('--project', $p, 'docs', 'generate', '--layout', 'curated', '--output', "$out\curated")
$env:EKOS_PROJECT = $p
Run 'assess'  @('talend', 'assess', '--out', "$out\assess.md")
foreach ($j in 'tMap_1', 'scd_step2', 'tFileOutputXML', 'repositoryContext') {
    & $ekos --home $H talend explain --job $j 2>&1 | Where-Object { "$_" -notmatch 'RemoteException' } | Set-Content "$out\explain-$j.txt" -Encoding UTF8
}
Remove-Item Env:\EKOS_PROJECT
Remove-Item "$W\out\docs-v2.zip" -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$out\*" -DestinationPath "$W\out\docs-v2.zip" -Force
Get-Content $log | Select-String '^==|written|Error|left out'
