@echo off
echo Memulai App Locker...

:: Jalankan daemon di background
start "" /B "%~dp0daemon.exe"
timeout /t 1 /nobreak >nul

:: Jalankan watchdog di background
start "" /B "%~dp0watchdog.exe"
timeout /t 1 /nobreak >nul

:: Jalankan UI
start "" "%~dp0ui.exe"

echo App Locker sudah berjalan.
