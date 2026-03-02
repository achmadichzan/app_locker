@echo off
echo Menghentikan App Locker...

taskkill /IM daemon.exe /F >nul 2>&1
taskkill /IM watchdog.exe /F >nul 2>&1
taskkill /IM ui.exe /F >nul 2>&1

echo Semua proses App Locker telah dihentikan.
pause
