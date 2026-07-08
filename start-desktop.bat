@echo off
setlocal
cd /d "%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\start-desktop.ps1"
if errorlevel 1 (
  echo.
  echo start-desktop failed. Press any key to close.
  pause >nul
)
