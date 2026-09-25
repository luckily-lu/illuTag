@echo off
rem COLD-CACHE startup capture. Reboot first, then double-click this file.
rem Collects 5 samples and prints P50/P90 at the end.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0collect-startup.ps1" -Label cold -Runs 5
echo.
pause
