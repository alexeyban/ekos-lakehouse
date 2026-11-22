# The sandbox's throwaway PostgreSQL (EKOS RFC 0268 phase 3). Never a pipe from a server: it would
# inherit the handle and keep the SSH session open.
param([ValidateSet('init','start','stop','smoke')][string]$Do, [string]$D = 'C:\ekos-sandbox\run\pgdata', [int]$Port = 55432)
$ErrorActionPreference = 'Stop'
$B = 'C:\ekos-sandbox\pgsql\bin'
$Log = 'C:\ekos-sandbox\run\pg'
New-Item -ItemType Directory -Force (Split-Path $Log) | Out-Null
function Run($exe, $argv) {
  $p = Start-Process -FilePath "$B\$exe" -ArgumentList $argv -NoNewWindow -Wait -PassThru `
    -RedirectStandardOutput "$Log-$exe.out" -RedirectStandardError "$Log-$exe.err"
  if ($p.ExitCode -ne 0) { Get-Content "$Log-$exe.out", "$Log-$exe.err"; throw "$exe exited $($p.ExitCode)" }
}
function Ready { & "$B\pg_isready.exe" -h 127.0.0.1 -p $Port -q; return ($LASTEXITCODE -eq 0) }
switch ($Do) {
  'init'  { if (Test-Path $D) { throw "$D exists" }; Run 'initdb.exe' @('-D', $D, '-U', 'postgres', '-A', 'trust', '-E', 'UTF8', '--no-locale') }
  'start' {
    Start-Process -FilePath "$B\pg_ctl.exe" -ArgumentList @('-D', $D, '-o', "`"-h 127.0.0.1 -p $Port`"", '-l', "$Log-server.log", 'start') `
      -NoNewWindow -RedirectStandardOutput "$Log-start.out" -RedirectStandardError "$Log-start.err" | Out-Null
    for ($i = 0; $i -lt 60 -and -not (Ready); $i++) { Start-Sleep 1 }
    if (-not (Ready)) { Get-Content "$Log-server.log" -ErrorAction SilentlyContinue | Select-Object -Last 5; throw 'not ready' }
    'ready'
  }
  'stop'  {
    if (Test-Path "$D\PG_VERSION") { Run 'pg_ctl.exe' @('-D', $D, '-w', 'stop') }
    for ($i = 0; $i -lt 30 -and @(Get-Process postgres -ErrorAction SilentlyContinue).Count; $i++) { Start-Sleep 1 }
    'postgres processes: ' + @(Get-Process postgres -ErrorAction SilentlyContinue).Count
  }
  'smoke' {
    & $PSCommandPath -Do init -D $D -Port $Port
    & $PSCommandPath -Do start -D $D -Port $Port
    try { & "$B\psql.exe" -h 127.0.0.1 -p $Port -U postgres -tAc 'select version()' }
    finally {
      & $PSCommandPath -Do stop -D $D -Port $Port
      if (@(Get-Process postgres -ErrorAction SilentlyContinue).Count -eq 0) { Remove-Item -Recurse -Force $D }
    }
  }
}
