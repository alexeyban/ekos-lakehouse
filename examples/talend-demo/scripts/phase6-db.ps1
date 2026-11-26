# Phase 6: throwaway PostgreSQL for the demo jobs, and table / file snapshots.
#   -Do setup   start PostgreSQL on 127.0.0.1:55432, create what the two PostgreSQL demo jobs expect
#   -Do report  row counts of every table in database mysid, and the demo output files
param([ValidateSet('setup','report')][string]$Do = 'setup', [string]$Tag = 'snapshot')
$ErrorActionPreference = 'Continue'
$B = 'C:\ekos-sandbox\pgsql\bin'; $Pg = 'C:\ekos-sandbox\pg.ps1'; $D = 'C:\ekos-sandbox\run\pgdata'
$out = 'C:\ekos-test\out\phase6'; New-Item -ItemType Directory -Force -Path $out | Out-Null
function Psql([string]$db, [string]$user, [string]$sql) {
    & "$B\psql.exe" -h 127.0.0.1 -p 55432 -U $user -d $db -v ON_ERROR_STOP=1 -X -tA -c $sql 2>&1 | ForEach-Object { "$_" }
}
if ($Do -eq 'setup') {
    $log = "$out\setup.txt"; @() | Set-Content $log
    if (-not (Test-Path "$D\PG_VERSION")) { & $Pg -Do init *>&1 | Add-Content $log }
    & "$B\pg_isready.exe" -h 127.0.0.1 -p 55432 -q
    if ($LASTEXITCODE -ne 0) { & $Pg -Do start *>&1 | Add-Content $log }
    Psql 'postgres' 'postgres' 'select version()' | Add-Content $log
    # The jobs say: host "myserver", port 5432, database "mysid", schema "myschema", user "login".
    # Only host and port are changed in Studio; the rest exists here as the jobs name it.
    Psql 'postgres' 'postgres' "DO `$`$BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='login') THEN CREATE ROLE login LOGIN; END IF; END`$`$" | Add-Content $log
    if (-not (Psql 'postgres' 'postgres' "SELECT 1 FROM pg_database WHERE datname='mysid'")) {
        Psql 'postgres' 'postgres' 'CREATE DATABASE mysid OWNER login' | Add-Content $log
    }
    Psql 'mysid' 'login' 'CREATE SCHEMA IF NOT EXISTS myschema' | Add-Content $log
    Psql 'mysid' 'postgres' 'ALTER ROLE login SET search_path = myschema, public' | Add-Content $log
    # Source for tPostgreSQLInput ("SELECT number,txt,date,flag FROM demotable"): 10 known rows.
    Psql 'mysid' 'login' 'DROP TABLE IF EXISTS myschema.demotable; CREATE TABLE myschema.demotable (number integer, txt varchar(50), date varchar(50), flag boolean); INSERT INTO myschema.demotable SELECT g, ''row'' || g, ''2026-11-24'', g % 2 = 0 FROM generate_series(1, 10) g' | Add-Content $log
    Get-Content $log
    Write-Host 'PostgreSQL ready on 127.0.0.1:55432 (database mysid, schema myschema, user login).' -ForegroundColor Green
}
$rep = "$out\report-$Tag.txt"
$lines = @("report $Tag $(Get-Date -Format s)", '-- tables in mysid (schema.table rows)')
$tables = Psql 'mysid' 'postgres' "SELECT table_schema || '.' || table_name FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog','information_schema') ORDER BY 1"
foreach ($t in $tables) { if ($t) { $lines += "$t " + (Psql 'mysid' 'postgres' "SELECT count(*) FROM $t") } }
$lines += '-- demo output files under C:\TalendDemoFiles'
if (Test-Path 'C:\TalendDemoFiles') {
    $lines += Get-ChildItem 'C:\TalendDemoFiles' -Recurse -File | ForEach-Object {
        $n = (Get-Content $_.FullName -ErrorAction SilentlyContinue | Measure-Object -Line).Lines
        "$($_.FullName) bytes=$($_.Length) lines=$n modified=$($_.LastWriteTime.ToString('s'))" }
} else { $lines += '(C:\TalendDemoFiles does not exist)' }
$lines | Set-Content $rep -Encoding UTF8
$lines | ForEach-Object { Write-Host $_ }
