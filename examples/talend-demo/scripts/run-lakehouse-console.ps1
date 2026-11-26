# EKOS-Lakehouse on the Talend demo: the Talend commands that need no Trino, and a console served by ekos-lakehouse.exe
# (its Talend, Lakehouse, Documentation ... tabs) on port 8001, home E:\ekos-work\ekos-home. The console on 8000 is left as it is.
$ErrorActionPreference = 'Continue'
$W = 'E:\ekos-work'; $H = "$W\ekos-home"; $LH = "$W\lakehouse\bin\ekos-lakehouse.exe"; $P = 'didemodocs'
$out = "$W\out\lakehouse-run"; New-Item -ItemType Directory -Force -Path $out | Out-Null
$log = "$out\run.txt"; "lakehouse run $(Get-Date -Format s)" | Set-Content $log -Encoding UTF8
if (-not (Test-Path $LH)) { Write-Host "missing $LH" -ForegroundColor Red; exit 1 }
# The console runtime (embedded Python + web app) is looked for beside the binary: <bin>\..\console.
$console = "$W\lakehouse\console"
if (-not (Test-Path "$console\app\main.py")) {
    robocopy "$env:LOCALAPPDATA\Programs\EKOS\console" $console /E /NFL /NDL /NP /R:1 /W:1 | Out-File "$out\robocopy-console.txt" -Encoding UTF8
    "copied console runtime: robocopy exit $LASTEXITCODE" | Add-Content $log
}
function Run([string]$name, [string[]]$a) {
    Write-Host ">> ekos-lakehouse $($a -join ' ')"
    $o = & $LH --home $H @a 2>&1 | ForEach-Object { "$_" } | Where-Object { $_ -notmatch 'RemoteException' }
    @("COMMAND: ekos-lakehouse --home $H $($a -join ' ')", "EXIT: $LASTEXITCODE", '---') + $o | Set-Content "$out\$name.txt" -Encoding UTF8
    "{0,-28} exit={1}" -f $name, $LASTEXITCODE | Add-Content $log
    Write-Host "   exit=$LASTEXITCODE"
}
Run '01-extensions'      @('extensions')
Run '02-talend-scan'     @('--project', $P, 'lakehouse', 'talend', 'scan')
Run '03-talend-report'   @('--project', $P, 'lakehouse', 'talend', 'report')
Run '04-lakehouse-status' @('--project', $P, 'lakehouse', 'status')
Run '05-lakehouse-doctor' @('--project', $P, 'lakehouse', 'doctor')
Run '06-translate-help'  @('lakehouse', 'talend', 'translate', '--help')
Run '07-docs-help'       @('lakehouse', 'docs', '--help')
Run '08-spec'            @('--project', $P, 'lakehouse', 'spec')
# Console on 8001 in local mode (this machine only).
$env:EKOS_CONSOLE_PORT = '8001'; $env:EKOS_CONSOLE_MODE = 'local'; $env:EKOS_CONSOLE_HOST = '127.0.0.1'
Run '09-console-start'   @('console', 'start')
Run '10-console-status'  @('console', 'status')
Get-Content $log
Write-Host ''
Write-Host 'Console on http://127.0.0.1:8001 - one-time sign-in code (two minutes):' -ForegroundColor Yellow
& $LH --home $H console open --no-browser 2>&1 | Where-Object { "$_" -notmatch 'RemoteException' } | Tee-Object -FilePath "$out\console-open.txt"
