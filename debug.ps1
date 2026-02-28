$ErrorActionPreference = "Stop"

Write-Host "🛠️ Memulai Build App Locker (Mode Debug)..." -ForegroundColor Cyan

# 1. Cargo build tanpa flag --release
cargo build

# 2. Siapkan folder khusus debug
$OutputDir = "debug_bin"
if (!(Test-Path -Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir | Out-Null
}

# 3. Salin executable dari target\debug ke folder debug_bin
Write-Host "Menyiapkan file di folder '$OutputDir'..." -ForegroundColor Yellow
$Executables = @("daemon.exe", "ui.exe", "watchdog.exe")
foreach ($Exe in $Executables) {
    $SourcePath = "target\debug\$Exe"
    if (Test-Path -Path $SourcePath) {
        Copy-Item -Path $SourcePath -Destination "$OutputDir\$Exe" -Force
    }
}

Write-Host "🚀 Menjalankan Daemon di terminal..." -ForegroundColor Green
Write-Host "Tekan Ctrl+C untuk menghentikan Daemon." -ForegroundColor DarkGray
Write-Host "-------------------------------------------------"

# 4. Pindah ke folder debug_bin dan jalankan daemon
Set-Location -Path $OutputDir
.\daemon.exe