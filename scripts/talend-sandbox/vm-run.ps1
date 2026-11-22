# EKOS RFC 0268 phase 3: the Talend side of a differential run, inside the sandbox VM.
#
# Runs from C:\ekos-sandbox\run, where run.sh unpacked the payload: plan.json, jobs\*.jar, lib\*.jar,
# the project's files (SQL scripts, inputs) under project\, and pg.ps1. Writes C:\ekos-sandbox\run\out:
# a dump of every table after each job, each job's output, and manifest.json. Third-party code runs
# only here: the VM has no route out, and run.sh reverts it to its clean snapshot afterwards.
$ErrorActionPreference = 'Stop'
$Run = 'C:\ekos-sandbox\run'
$Out = "$Run\out"
$Pg = 'C:\ekos-sandbox\pgsql\bin'
$Java = (Get-ChildItem 'C:\ekos-sandbox\jre\*\bin\java.exe' | Select-Object -First 1).FullName
$Plan = Get-Content "$Run\plan.json" -Raw | ConvertFrom-Json
$Port = 5432
New-Item -ItemType Directory -Force "$Out\logs", "$Out\dumps" | Out-Null
if (Test-Path "$Run\origins.json") { Copy-Item "$Run\origins.json" "$Out\origins.json" }
$env:PGCLIENTENCODING = 'UTF8'

$manifest = [ordered]@{
  project = $Plan.project
  started = (Get-Date).ToString('o')
  run_day = (Get-Date -Format 'yyyyMMdd')
  java = $Java
  sandbox_settings = [ordered]@{
    context_rows = $Plan.context_rows
    jvm_locale = $Plan.jvm_locale
    inputs = @()
    # Jars the project did not ship ready to run (packaged from its classes, or from the Studio).
    jars = 'origins.json'
  }
  sql = @()
  jobs = @()
}

# A process with its output in files, never a pipe (a server would inherit it and hold SSH open).
function Invoke-Logged($exe, [string[]]$argv, $name, [int]$timeoutSec = 900) {
  # Start-Process joins arguments with spaces and quotes none: no argument may hold a space (SQL goes
  # through a file).
  foreach ($a in $argv) { if ($a -match '\s') { throw "argument with a space: $a" } }
  $p = Start-Process -FilePath $exe -ArgumentList $argv -NoNewWindow -PassThru `
    -RedirectStandardOutput "$Out\logs\$name.out" -RedirectStandardError "$Out\logs\$name.err"
  # Read the handle now: without it, ExitCode is empty once the process has exited.
  $null = $p.Handle
  $done = $p.WaitForExit($timeoutSec * 1000)
  if (-not $done) { Stop-Process -Id $p.Id -Force; return @{ exit = $null; timed_out = $true } }
  $p.WaitForExit()
  return @{ exit = $p.ExitCode; timed_out = $false }
}

function Psql([string[]]$argv, $name) {
  Invoke-Logged "$Pg\psql.exe" (@('-h', '127.0.0.1', '-p', "$Port", '-U', 'postgres', '-X') + $argv) $name 300
}

# One SQL command, through a file (Start-Process cannot pass it as one argument).
function Sql($db, $sql, $name) {
  $f = "$Out\logs\$name.sql"
  Set-Content -Encoding UTF8 $f $sql
  Psql @('-v', 'ON_ERROR_STOP=1', '-d', $db, '-f', $f) $name
}

function Dump-Tables($step) {
  $dir = "$Out\dumps\$step"
  New-Item -ItemType Directory -Force $dir | Out-Null
  $schemas = ($Plan.schemas | ForEach-Object { "'$_'" }) -join ','
  $list = & "$Pg\psql.exe" -h 127.0.0.1 -p $Port -U postgres -X -d $Plan.database -tA -F '|' `
    -c "select table_schema, table_name from information_schema.tables where table_schema in ($schemas) and table_type = 'BASE TABLE' order by 1, 2"
  foreach ($line in $list) {
    if (-not $line) { continue }
    $s, $t = $line.Split('|')
    $file = "$dir\$s.$t.csv" -replace '\\', '/'
    Sql $Plan.database "\copy (select * from `"$s`".`"$t`") to '$file' with (format csv, header)" "dump-$step-$s.$t" | Out-Null
  }
}

# The throwaway cluster, on the port the jobs' context names.
& "$Run\pg.ps1" -Do init -D "$Run\pgdata" -Port $Port
& "$Run\pg.ps1" -Do start -D "$Run\pgdata" -Port $Port
try {
  $r = Sql 'postgres' "create database `"$($Plan.database)`"" 'createdb'
  if ($r.exit -ne 0) { throw "create database failed: $(Get-Content "$Out\logs\createdb.err" -Raw)" }
  foreach ($f in $Plan.sql) {
    $name = 'sql-' + [IO.Path]::GetFileNameWithoutExtension($f)
    $r = Psql @('-d', $Plan.database, '-f', "$Run\project\$f") $name
    $sqlErr = @(Get-Content "$Out\logs\$name.err" -ErrorAction SilentlyContinue | Where-Object { $_ -match 'ERROR' } | ForEach-Object { "$_" })
    $manifest.sql += [ordered]@{ file = $f; exit = $r.exit; errors = $sqlErr }
  }
  # Sandbox settings: context rows the runner changes.
  foreach ($k in $Plan.context_rows.set.PSObject.Properties) {
    $sql = "update $($Plan.context_rows.table) set `"value`" = '$($k.Value)' where `"key`" = '$($k.Name)'"
    $r = Sql $Plan.database $sql "context-$($k.Name)"
    if ($r.exit -ne 0) { throw "context row $($k.Name) failed" }
  }
  # Inputs where the jobs look for them.
  foreach ($i in $Plan.inputs) {
    $to = $i.to -replace '\{run_day\}', $manifest.run_day
    New-Item -ItemType Directory -Force $to | Out-Null
    $from = "$Run\project\$($i.from)"
    if (Test-Path $from -PathType Container) { Copy-Item "$from\*" $to -Recurse -Force } else { Copy-Item $from $to -Force }
    $manifest.sandbox_settings.inputs += [ordered]@{ from = $i.from; to = $to; why = $i.why }
  }
  Dump-Tables 'r1_00_setup'
  $runs = @($Plan.runs)
  if ($runs.Count -eq 0) { $runs = @([pscustomobject]@{ name = 'r1' }) }
  $manifest.runs = @()
  $k = 0
  # Not `$run`: PowerShell's names ignore case, and `$Run` is the run directory.
  foreach ($pass in $runs) {
  $k++
  if ($k -gt 1) {
    # A later run: the input files replaced by its own (run.sh made them from the plan's edits).
    foreach ($i in $Plan.inputs) {
      $to = $i.to -replace '\{run_day\}', $manifest.run_day
      $from = "$Run\project-r$k\$($i.from)"
      if (Test-Path $from -PathType Container) {
        Get-ChildItem $to -File | Remove-Item -Force
        Copy-Item "$from\*" $to -Recurse -Force
      } else {
        Copy-Item $from $to -Force
      }
    }
  }
  $manifest.runs += [ordered]@{ name = "r$k"; edits = $pass.edits }
  $n = 0
  foreach ($job in $Plan.jobs) {
    $n++
    $step = 'r{0}_{1:D2}_{2}' -f $k, $n, $job
    $jar = Get-ChildItem "$Run\jobs\$($job.ToLower())_*.jar" | Select-Object -First 1
    if (-not $jar) {
      $manifest.jobs += [ordered]@{ run = "r$k"; job = $job; ran = $false; reason = 'no built jar' }
      continue
    }
    $class = "$($Plan.package).$($jar.BaseName).$job"
    $t0 = Get-Date
    $jvm = @('-Xms256m', '-Xmx1024m', '-Dfile.encoding=UTF-8')
    # The JVM's locale, when the plan states it: week numbers and month and day names follow it.
    if ($Plan.jvm_locale) {
      $lang, $country = $Plan.jvm_locale.Split('_')
      $jvm += @("-Duser.language=$lang", "-Duser.country=$country")
    }
    $r = Invoke-Logged $Java ($jvm + @('-cp', "$($jar.FullName);$Run\lib\*", $class, '--context=Default')) $step
    $secs = [math]::Round(((Get-Date) - $t0).TotalSeconds, 1)
    $err = @(Get-Content "$Out\logs\$step.err" -ErrorAction SilentlyContinue | ForEach-Object { "$_" })
    $manifest.jobs += [ordered]@{
      run = "r$k"; job = $job; ran = $true; class = $class; exit = $r.exit; timed_out = $r.timed_out; seconds = $secs
      exception = (@($err | Where-Object { $_ -match 'Exception|Error' }) | Select-Object -First 3)
    }
    Dump-Tables $step
  }
  }
}
finally {
  & "$Run\pg.ps1" -Do stop -D "$Run\pgdata" -Port $Port
  $manifest.finished = (Get-Date).ToString('o')
  $manifest | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 "$Out\manifest.json"
}
