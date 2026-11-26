# Restart the web console from the build in C:\ekos (the CLI's `console` subcommand), then sign in.
$ErrorActionPreference = 'Continue'
$log = 'C:\ekos-test\out\log4'; New-Item -ItemType Directory -Force -Path $log | Out-Null
$ekos = Get-ChildItem 'C:\ekos' -Recurse -File -Filter 'ekos*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch 'desktop|setup' } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
if (-not $ekos) { Write-Host 'No ekos*.exe under C:\ekos' -ForegroundColor Red; exit 1 }
function Run([string]$name, [string[]]$a) {
    Write-Host ">> ekos $($a -join ' ')"
    try { $o = & $ekos @a 2>&1 | ForEach-Object { "$_" }; $code = $LASTEXITCODE } catch { $o = "$_"; $code = -1 }
    @("COMMAND: $ekos $($a -join ' ')", "EXIT: $code", '---') + $o | Set-Content "$log\$name.txt" -Encoding UTF8
    $o | Select-Object -First 8 | ForEach-Object { Write-Host "   $_" }
    Write-Host "   exit=$code"
}
Run '01-console-help'   @('console', '--help')
Run '02-status-before'  @('console', 'status')
Run '03-stop'           @('console', 'stop')
Start-Sleep -Seconds 5
Run '04-status-stopped' @('console', 'status')
Run '05-start'          @('console', 'start')
Start-Sleep -Seconds 10
Run '06-status-after'   @('console', 'status')
Get-Process | Where-Object { $_.Path -like '*ekos*' } | Select-Object Id, ProcessName, Path |
    Format-Table -AutoSize | Out-String -Width 300 | Set-Content "$log\07-processes.txt" -Encoding UTF8
Get-Content "$log\07-processes.txt"
Write-Host ''
Write-Host 'Console sign-in: a signed-in page opens, or a one-time code is printed (valid two minutes).' -ForegroundColor Yellow
& $ekos console open
