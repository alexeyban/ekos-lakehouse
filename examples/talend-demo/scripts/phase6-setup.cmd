@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0phase6-db.ps1" -Do setup -Tag before
echo.
pause
