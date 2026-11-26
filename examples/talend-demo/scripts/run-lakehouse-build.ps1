# Build EKOS-Lakehouse (github.com/alexeyban/ekos-lakehouse) on this Windows VM.
# Installs, if missing: Git, Rustup (stable MSVC toolchain), CMake, NASM and the Visual Studio 2022 Build Tools
# with the C++ workload (winget; the Build Tools ask for administrator rights). Then builds ekos-lakehouse.exe.
# Source, build output, Cargo cache, temp files and logs all go to E:\ekos-work (C: is full).
$ErrorActionPreference = 'Continue'
$W = 'E:\ekos-work'
$out = "$W\out\lakehouse-build"; New-Item -ItemType Directory -Force -Path $out | Out-Null
$log = "$out\build.txt"; "lakehouse build $(Get-Date -Format s)" | Set-Content $log -Encoding UTF8
function Say($m) { Write-Host $m -ForegroundColor Cyan; $m | Add-Content $log }
function RefreshPath {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
    foreach ($p in "$env:USERPROFILE\.cargo\bin", 'C:\Program Files\NASM', "$env:LOCALAPPDATA\bin\NASM", "$env:LOCALAPPDATA\Programs\NASM", 'C:\Program Files\CMake\bin', 'C:\Program Files\Git\cmd') {
        if ((Test-Path $p) -and ($env:Path -notlike "*$p*")) { $env:Path = "$p;$env:Path" } }
}
if (-not (Test-Path 'E:\')) { Write-Host 'Drive E: not found' -ForegroundColor Red; exit 1 }
if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { Say 'winget not found: install "App Installer" from the Microsoft Store, then run this again.'; exit 1 }
$pkgs = @(
    @{ Id = 'Git.Git';           Test = { Get-Command git -ErrorAction SilentlyContinue } },
    @{ Id = 'Rustlang.Rustup';   Test = { Test-Path "$env:USERPROFILE\.cargo\bin\rustup.exe" } },
    @{ Id = 'Kitware.CMake';     Test = { Get-Command cmake -ErrorAction SilentlyContinue } },
    @{ Id = 'NASM.NASM';         Test = { Get-Command nasm -ErrorAction SilentlyContinue } },
    @{ Id = 'Microsoft.VisualStudio.2022.BuildTools'; Test = {
        $vw = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        (Test-Path $vw) -and (& $vw -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath) };
       Extra = @('--override', '--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended') }
)
RefreshPath
foreach ($p in $pkgs) {
    if (& $p.Test) { Say "present: $($p.Id)"; continue }
    Say "installing: $($p.Id)"
    $a = @('install', '--id', $p.Id, '-e', '--accept-package-agreements', '--accept-source-agreements', '--disable-interactivity') + $p.Extra
    & winget @a 2>&1 | Out-File "$out\winget-$($p.Id).txt" -Encoding UTF8
    Say "   winget exit $LASTEXITCODE"
    RefreshPath
}
foreach ($p in $pkgs) { if (-not (& $p.Test)) { Say "STILL MISSING: $($p.Id) - see $out\winget-$($p.Id).txt"; $missing = $true } }
if ($missing) { exit 1 }
& rustup toolchain install stable --profile minimal 2>&1 | Out-File "$out\rustup.txt" -Encoding UTF8
& rustup default stable 2>&1 | Add-Content "$out\rustup.txt"
Say ("rustc: " + ((& rustc --version) -join ' ') + "; cargo: " + ((& cargo --version) -join ' '))
$src = "$W\lakehouse\src"
if (Test-Path "$src\.git") { & git -C $src pull --ff-only 2>&1 | Add-Content $log }
else { New-Item -ItemType Directory -Force -Path (Split-Path $src) | Out-Null; & git clone --depth 1 https://github.com/alexeyban/ekos-lakehouse $src 2>&1 | Add-Content $log }
Say ("ekos-lakehouse commit: " + ((& git -C $src rev-parse HEAD) -join ''))
$env:CARGO_TARGET_DIR = "$W\lh-target"
# Crate downloads and temp files on E: too; rustup's toolchain stays where it is.
$env:CARGO_HOME = "$W\cargo-home"; $env:TEMP = "$W\tmp"; $env:TMP = $env:TEMP
New-Item -ItemType Directory -Force -Path $env:CARGO_HOME, $env:TEMP | Out-Null
$cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe"
$env:CARGO_NET_GIT_FETCH_WITH_CLI = 'true'
Say "cargo build --release --bin ekos-lakehouse (long: often 30-60 min on first build)"
$sw = [Diagnostics.Stopwatch]::StartNew()
Push-Location $src
& $cargo build --release --bin ekos-lakehouse 2>&1 | ForEach-Object { "$_" } | Tee-Object -FilePath "$out\cargo-build.txt"
$code = $LASTEXITCODE
Pop-Location
Say "cargo exit $code after $([int]$sw.Elapsed.TotalMinutes) min"
$exe = "$env:CARGO_TARGET_DIR\release\ekos-lakehouse.exe"
if ($code -eq 0 -and (Test-Path $exe)) {
    New-Item -ItemType Directory -Force -Path "$W\lakehouse\bin" | Out-Null
    Copy-Item $exe "$W\lakehouse\bin\ekos-lakehouse.exe" -Force
    $b = "$W\lakehouse\bin\ekos-lakehouse.exe"
    Say ("built: $b  " + ((& $b --version) -join ' '))
    & $b lakehouse --help 2>&1 | Set-Content "$out\help-lakehouse.txt" -Encoding UTF8
    & $b lakehouse talend --help 2>&1 | Set-Content "$out\help-lakehouse-talend.txt" -Encoding UTF8
    & $b extensions 2>&1 | Set-Content "$out\extensions.txt" -Encoding UTF8
    Say 'done.'
} else { Say "build failed: see $out\cargo-build.txt (last lines below)"; Get-Content "$out\cargo-build.txt" -Tail 30 }
