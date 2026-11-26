# One-time sign-in code for the ekos-lakehouse console on port 8001 (valid two minutes). Writes it to E:\ekos-work\out\lakehouse-run\signin.txt too.
$W = 'E:\ekos-work'; $H = "$W\ekos-home"; $LH = "$W\lakehouse\bin\ekos-lakehouse.exe"
$env:EKOS_CONSOLE_PORT = '8001'; $env:EKOS_CONSOLE_MODE = 'local'; $env:EKOS_CONSOLE_HOST = '127.0.0.1'
$f = "$W\out\lakehouse-run\signin.txt"
& $LH --home $H console open --no-browser *> $f
Get-Content $f
