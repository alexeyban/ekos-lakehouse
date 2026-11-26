# EKOS x Talend test, phases 3-5 (command side). Run via run-ekos-test.cmd.
# Writes every command, its output and exit code under C:\ekos-test\out\log.
# Sets no API key, passes no --allow-metered, pushes nothing.
$ErrorActionPreference = 'Continue'
$root = 'C:\ekos-test'
$src  = "$root\src\di-demos"
$zip  = "$root\src\di-demos.zip"
$out  = "$root\out"
$log  = "$out\log"
$ws   = 'C:\ekos-sandbox\TOS_BD-20211109_1610-V8.0.1\workspace\LOCAL_PROJECT_DATA_INTEGRATION_DEMOS'
if (Test-Path $log) { Remove-Item "$log\*", "$out\secrets-scan.txt" -Force -ErrorAction SilentlyContinue }
New-Item -ItemType Directory -Force -Path "$root\src", $out, $log | Out-Null

$ekos = $null
if (Test-Path 'C:\ekos') {
    $ekos = Get-ChildItem 'C:\ekos' -Recurse -File -Filter 'ekos*.exe' -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -notmatch 'desktop|setup' } |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $ekos) {
    Write-Host 'No ekos*.exe found under C:\ekos - stopping so the old installed build is not tested by mistake.' -ForegroundColor Red
    Get-ChildItem 'C:\ekos' -Recurse -ErrorAction SilentlyContinue | Select-Object -First 60 FullName, Length |
        Out-File "$out\ekos-folder-listing.txt" -Encoding UTF8
    exit 1
}
$summary = "$out\run-summary.txt"
"EKOS test run $(Get-Date -Format s)  ekos=$ekos" | Set-Content $summary -Encoding UTF8

$script:n = 0
function Run([string]$name, [string[]]$a) {
    $script:n++
    $file = '{0}\{1:d2}-{2}.txt' -f $log, $script:n, $name
    $cmd = 'ekos ' + ($a -join ' ')
    Write-Host ">> $cmd"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    try { $o = & $ekos @a 2>&1 | ForEach-Object { "$_" }; $code = $LASTEXITCODE }
    catch { $o = "$_"; $code = -1 }
    $sw.Stop()
    @("COMMAND: $cmd", "EXIT: $code", "SECONDS: $([int]$sw.Elapsed.TotalSeconds)", '---') + $o |
        Set-Content $file -Encoding UTF8
    ('{0:d2} exit={1,-4} {2,4}s  {3}' -f $script:n, $code, [int]$sw.Elapsed.TotalSeconds, $cmd) |
        Add-Content $summary -Encoding UTF8
}

# Phase 1 check
Run 'version' @('--version')
Run 'help' @('--help')
Run 'talend-help' @('talend', '--help')

# Phase 2: copy the project folder out of the Studio workspace
robocopy $ws $src /E /NFL /NDL /NP /R:1 /W:1 | Out-File "$log\00-robocopy.txt" -Encoding UTF8
"00 robocopy exit=$LASTEXITCODE (below 8 = ok)" | Add-Content $summary -Encoding UTF8

# Phase 3: compile, folder source then zip source
$projects = @(@{ Name = 'didemo'; Source = $src })
if (Test-Path $zip) { $projects += @{ Name = 'didemozip'; Source = $zip } }
else { "SKIPPED didemozip: $zip not found" | Add-Content $summary -Encoding UTF8 }
foreach ($p in $projects) {
    $name = $p.Name
    Run "$name-project-add" @('project', 'add', $name, '--source', $p.Source)
    foreach ($stage in 'build', 'recover', 'resolve', 'compile', 'commit') {
        Run "$name-$stage" @('--project', $name, $stage)
    }
    foreach ($kind in 'TalendJob', 'TalendComponent', 'TalendContext') {
        Run "$name-ekl-$kind" @('--project', $name, 'ekl', "FIND Object WHERE kind = '$kind'")
    }
}
Run 'project-list' @('project', 'list')

# Phase 4: assess, explain, docs
Run 'assess-md'   @('--project', 'didemo', 'talend', 'assess', '--out', "$out\assess.md")
Run 'assess-json' @('--project', 'didemo', 'talend', 'assess', '--json')
foreach ($job in 'tMap_1', 'tMap_4', 'repositoryContext', 'jobParent', 'tJavaRow') {
    Run "explain-$job" @('--project', 'didemo', 'talend', 'explain', '--job', $job)
}
Run 'explain-component' @('--project', 'didemo', 'talend', 'explain', '--component', 'tMap_1:tMap_1')
Run 'docs-estate' @('--project', 'didemo', 'docs', 'generate', '--layout', 'estate', '--output', "$out\docs")

# Phase 5: secrets scan of EKOS state. Values are never printed, only counts.
$needles = @()
Get-ChildItem $src -Recurse -Filter *.item | ForEach-Object {
    $t = [IO.File]::ReadAllText($_.FullName)
    foreach ($m in [regex]::Matches($t, '(?i)\bpassword="([^"]+)"')) { $needles += $m.Groups[1].Value }
    foreach ($m in [regex]::Matches($t, '(?i)name="(?:PASS|PASSWORD)"[^>]*\bvalue="([^"]+)"')) { $needles += $m.Groups[1].Value }
}
$needles = $needles | ForEach-Object { $_.Replace('&quot;', '').Trim('"') } |
    Where-Object { $_.Length -ge 4 -and $_ -notmatch '^context\.' } | Sort-Object -Unique
$control = 'tMap_Reload_At_Each_Row_1'   # a job name that must be in the state
$zstd = (Get-Command zstd -ErrorAction SilentlyContinue).Source
$hits = 0; $controlHits = 0; $files = 0; $zstFiles = 0; $zstRead = 0
$latin = [Text.Encoding]::GetEncoding(28591)
$states = @("$env:USERPROFILE\.ekos") | Where-Object { Test-Path $_ }
foreach ($f in (Get-ChildItem $states -Recurse -File -Force -ErrorAction SilentlyContinue)) {
    $files++
    try {
        if ($f.Extension -in '.zst', '.zstd') {
            $zstFiles++
            if (-not $zstd) { continue }
            $tmp = [IO.Path]::GetTempFileName()
            & $zstd -d -q -f -o $tmp $f.FullName 2>$null
            $text = $latin.GetString([IO.File]::ReadAllBytes($tmp)); Remove-Item $tmp -Force
            $zstRead++
        } else { $text = $latin.GetString([IO.File]::ReadAllBytes($f.FullName)) }
    } catch { continue }
    if ($text.Contains($control)) { $controlHits++ }
    foreach ($nd in $needles) { if ($text.Contains($nd)) { $hits++; "password value found in: $($f.FullName)" | Add-Content "$out\secrets-scan.txt" } }
}
@("state dirs: $($states -join ', ')", "files scanned: $files", "zstd files: $zstFiles (decompressed: $zstRead; zstd tool: $([bool]$zstd))",
  "distinct password values taken from the Talend items: $($needles.Count)",
  "files containing a password value: $hits", "files containing the control string '$control': $controlHits") |
    Add-Content "$out\secrets-scan.txt" -Encoding UTF8

Write-Host ''
Get-Content $summary
Write-Host ''
Write-Host 'Console sign-in (one-time code, valid two minutes):'
& $ekos --project didemo console open
