@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0run-lakehouse-build.ps1"
echo.
pause
