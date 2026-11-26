@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0run-ekos-test.ps1"
echo.
echo Finished. Outputs are in C:\ekos-test\out
pause
