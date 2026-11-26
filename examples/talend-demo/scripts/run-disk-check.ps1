# Free space on C: and the largest folders this test and the lakehouse build added. Read-only.
$out = 'C:\ekos-test\out\disk-check.txt'
$d = Get-PSDrive C
$lines = @(("C: free {0:N1} GB of {1:N1} GB" -f ($d.Free / 1GB), (($d.Used + $d.Free) / 1GB)), '-- folder sizes (GB)')
$paths = 'C:\ekos-test\lh-target', 'C:\ekos-test\lakehouse', "$env:USERPROFILE\.cargo", "$env:USERPROFILE\.rustup",
    "$env:USERPROFILE\.ekos", 'C:\ekos-sandbox', 'C:\ekos-test\src', 'C:\ekos-test\out', $env:TEMP,
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio", 'C:\ProgramData\Package Cache', "$env:LOCALAPPDATA\Temp", 'C:\Windows\SoftwareDistribution\Download'
foreach ($p in $paths | Select-Object -Unique) {
    if (Test-Path $p) {
        $s = (Get-ChildItem $p -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
        $lines += ("{0,8:N2}  {1}" -f ($s / 1GB), $p)
    }
}
$lines += '-- files over 300 MB under C:\ekos-sandbox and the user folder'
$lines += Get-ChildItem 'C:\ekos-sandbox', $env:USERPROFILE -Recurse -File -Force -ErrorAction SilentlyContinue |
    Where-Object { $_.Length -gt 300MB } | Sort-Object Length -Descending | Select-Object -First 15 |
    ForEach-Object { "{0,8:N2}  {1}" -f ($_.Length / 1GB), $_.FullName }
$lines | Set-Content $out -Encoding UTF8
$lines
