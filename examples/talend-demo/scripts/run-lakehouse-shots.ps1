# Full-resolution screenshots 30, 32, 33 of the ekos-lakehouse console (port 8001) with headless Edge.
# Signs in by itself: issues a one-time code, opens the sign-in link in a throwaway Edge profile on E:, then captures.
$ErrorActionPreference = 'Continue'
$W = 'E:\ekos-work'; $H = "$W\ekos-home"; $LH = "$W\lakehouse\bin\ekos-lakehouse.exe"
$shots = "$W\screenshots"; $log = "$W\out\lakehouse-run\shots.txt"
New-Item -ItemType Directory -Force -Path $shots | Out-Null
$edge = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $edge) { Write-Host 'Microsoft Edge not found' -ForegroundColor Red; exit 1 }
$prof = "$W\tmp\edge-shots-$(Get-Date -Format yyyyMMddHHmmss)"
New-Item -ItemType Directory -Force -Path $prof | Out-Null
$env:EKOS_CONSOLE_PORT = '8001'; $env:EKOS_CONSOLE_MODE = 'local'; $env:EKOS_CONSOLE_HOST = '127.0.0.1'
$text = (& $LH --home $H console open --no-browser 2>&1 | ForEach-Object { "$_" }) -join "`n"
$code = [regex]::Match($text, '[0-9a-f]{48}').Value
if (-not $code) { "no sign-in code in: $text" | Set-Content $log; Write-Host 'could not get a sign-in code' -ForegroundColor Red; exit 1 }
$common = @('--headless=new', "--user-data-dir=$prof", '--no-first-run', '--disable-gpu', '--hide-scrollbars')
& $edge @common '--dump-dom' "http://127.0.0.1:8001/api/auth/local?code=$code" 2>$null | Out-Null
$pages = @(
    @{ File = '30-lakehouse-console-talend-projects.png'; Url = 'http://127.0.0.1:8001/w/didemodocs/x/lakehouse/talend' },
    @{ File = '32-lakehouse-console-documentation.png';   Url = 'http://127.0.0.1:8001/w/didemodocs/x/lakehouse/docs' },
    @{ File = '33-lakehouse-console-lakehouse-page.png';  Url = 'http://127.0.0.1:8001/w/didemodocs/x/lakehouse/lakehouse' }
)
"shots $(Get-Date -Format s)" | Set-Content $log -Encoding UTF8
foreach ($p in $pages) {
    $f = Join-Path $shots $p.File
    & $edge @common '--window-size=1600,1000' '--force-device-scale-factor=1.5' '--virtual-time-budget=20000' "--screenshot=$f" $p.Url 2>$null | Out-Null
    $ok = Test-Path $f
    "$($p.File)  exists=$ok  bytes=$(if ($ok) { (Get-Item $f).Length })" | Add-Content $log
}
Remove-Item $prof -Recurse -Force -ErrorAction SilentlyContinue
Get-Content $log
