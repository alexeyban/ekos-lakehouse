# Follow-up to run-ekos-test.ps1: help texts, assess variants, sharper secrets scan.
$ErrorActionPreference = 'Continue'
$out = 'C:\ekos-test\out'; $log = "$out\log2"; $src = 'C:\ekos-test\src\di-demos'
New-Item -ItemType Directory -Force -Path $log | Out-Null
Remove-Item "$log\*" -Force -ErrorAction SilentlyContinue
$ekos = Get-ChildItem 'C:\ekos' -Recurse -File -Filter 'ekos*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch 'desktop|setup' } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
if (-not $ekos) { Write-Host 'No ekos*.exe under C:\ekos' -ForegroundColor Red; exit 1 }
$summary = "$out\run-summary2.txt"
$fi = Get-Item $ekos
"EKOS follow-up $(Get-Date -Format s)  ekos=$ekos  size=$($fi.Length)  modified=$($fi.LastWriteTime.ToString('s'))" | Set-Content $summary -Encoding UTF8
$script:n = 0
function Run([string]$name, [string[]]$a) {
    $script:n++
    $file = '{0}\{1:d2}-{2}.txt' -f $log, $script:n, $name
    $cmd = 'ekos ' + ($a -join ' ')
    Write-Host ">> $cmd"
    try { $o = & $ekos @a 2>&1 | ForEach-Object { "$_" }; $code = $LASTEXITCODE } catch { $o = "$_"; $code = -1 }
    @("COMMAND: $cmd", "EXIT: $code", '---') + $o | Set-Content $file -Encoding UTF8
    ('{0:d2} exit={1,-4} {2}' -f $script:n, $code, $cmd) | Add-Content $summary -Encoding UTF8
}
$tp = 'LOCAL_PROJECT_DATA_INTEGRATION_DEMOS'
Run 'assess-help'   @('talend', 'assess', '--help')
Run 'explain-help'  @('talend', 'explain', '--help')
Run 'project-add-help' @('project', 'add', '--help')
Run 'doctor'        @('--project', 'didemo', 'doctor')
Run 'assess-positional' @('--project', 'didemo', 'talend', 'assess', $tp, '--out', "$out\assess.md")
Run 'assess-flag-project' @('--project', 'didemo', 'talend', 'assess', '--talend-project', $tp, '--out', "$out\assess.md")
Run 'assess-flag-name' @('--project', 'didemo', 'talend', 'assess', '--name', $tp, '--out', "$out\assess.md")
foreach ($job in 'tMysqlOutput', 'scd_step2', 'generic_elt_step1', 'repositoryContext', 'tMap_1') {
    Run "explain-json-$job" @('--project', 'didemo', 'talend', 'explain', '--job', $job, '--json')
}

# Secrets scan: which kind of value was found, and where. Values are never written out.
$needles = @{}
Get-ChildItem $src -Recurse -Filter *.item | ForEach-Object {
    $t = [IO.File]::ReadAllText($_.FullName); $item = $_.Name
    $ms = @([regex]::Matches($t, '(?i)\bpassword="([^"]+)"')) + @([regex]::Matches($t, '(?i)name="(?:PASS|PASSWORD)"[^>]*\bvalue="([^"]+)"'))
    foreach ($m in $ms) {
        $v = $m.Groups[1].Value.Replace('&quot;', '').Trim('"')
        if ($v.Length -ge 4 -and $v -notmatch '^context\.' -and -not $needles.ContainsKey($v)) { $needles[$v] = $item }
    }
}
$zstd = (Get-Command zstd -ErrorAction SilentlyContinue).Source
$latin = [Text.Encoding]::GetEncoding(28591)
$rep = "$out\secrets-scan2.txt"
"needles: $($needles.Count) (encrypted: $(@($needles.Keys | Where-Object { $_ -like 'enc:*' }).Count))  zstd tool: $([bool]$zstd)" | Set-Content $rep -Encoding UTF8
foreach ($f in (Get-ChildItem "$env:USERPROFILE\.ekos" -Recurse -File -Force -ErrorAction SilentlyContinue)) {
    if ($f.Extension -in '.zst', '.zstd') {
        if (-not $zstd) { "NOT READ (zstd): $($f.FullName)" | Add-Content $rep; continue }
        $tmp = [IO.Path]::GetTempFileName(); & $zstd -d -q -f -o $tmp $f.FullName 2>$null
        $text = $latin.GetString([IO.File]::ReadAllBytes($tmp)); Remove-Item $tmp -Force
    } else { try { $text = $latin.GetString([IO.File]::ReadAllBytes($f.FullName)) } catch { continue } }
    foreach ($k in $needles.Keys) {
        $i = $text.IndexOf($k)
        if ($i -ge 0) {
            $kind = if ($k -like 'enc:*') { 'ENCRYPTED value' } else { "PLAINTEXT value of length $($k.Length)" }
            $before = $text.Substring([Math]::Max(0, $i - 60), [Math]::Min(60, $i)) -replace '\s+', ' '
            "HIT: $kind from $($needles[$k]) in $($f.FullName) | text before it: $before" | Add-Content $rep
        }
    }
}
Get-Content $summary; Get-Content $rep
Write-Host ''
Write-Host 'Console sign-in (one-time code, valid two minutes):'
& $ekos --project didemo console open
