@echo off
title Rollstamp build
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\build-portable.ps1" %*
if errorlevel 1 (
  echo.
  echo Build failed - see the red message above.
  pause
  exit /b 1
)
echo.
echo Done. The portable program is in dist\Rollstamp
timeout /t 10 >nul
