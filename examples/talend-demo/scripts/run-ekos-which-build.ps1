# Which ekos.exe does the reinstalled console use, and does its command catalogue include talend?
$ErrorActionPreference = 'Continue'
$out = 'C:\ekos-test\out\which-build.txt'
$installed = "$env:LOCALAPPDATA\Programs\ekos\bin\ekos.exe"
$built = Get-ChildItem 'C:\ekos' -Recurse -File -Filter 'ekos*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch 'desktop|setup' } | Sort-Object LastWriteTime -Descending |
    Select-Object -First 1 -ExpandProperty FullName
$lines = @()
foreach ($p in $installed, $built) {
    if (-not (Test-Path $p)) { $lines += "MISSING: $p"; continue }
    $f = Get-Item $p; $h = (Get-FileHash $p -Algorithm SHA256).Hash
    $lines += "== $p"
    $lines += "size=$($f.Length) modified=$($f.LastWriteTime.ToString('s')) sha256=$h"
    $lines += "version: " + ((& $p --version 2>&1) -join ' ')
    $lines += "talend --help exit: " + $(& $p talend --help *> $null; $LASTEXITCODE)
    $cat = (& $p commands 2>&1) -join "`n"
    $lines += "commands JSON mentions talend: " + [bool]($cat -match '"talend')
    $ext = (& $p extensions 2>&1) -join "`n"
    $lines += "extensions output mentions talend: " + [bool]($ext -match 'talend')
    $ext | Set-Content ("C:\ekos-test\out\extensions-" + ($(if ($p -eq $installed) {'installed'} else {'built'})) + ".json") -Encoding UTF8
}
$lines += "== running ekos processes"
$lines += (Get-Process | Where-Object { $_.Path -like '*ekos*' } | ForEach-Object { "$($_.Id) $($_.Path)" })
$lines | Set-Content $out -Encoding UTF8
$lines | ForEach-Object { Write-Host $_ }
