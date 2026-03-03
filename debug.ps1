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
$SourcePath = "target\debug\app_locker.exe"
if (Test-Path -Path $SourcePath) {
    Copy-Item -Path $SourcePath -Destination "$OutputDir\app_locker.exe" -Force
}

Write-Host "🚀 Menjalankan Management Panel..." -ForegroundColor Green
Write-Host "Catatan: Untuk install service, jalankan sebagai Administrator." -ForegroundColor DarkGray
Write-Host "-------------------------------------------------"

# 4. Pindah ke folder debug_bin dan jalankan Management Panel
Set-Location -Path $OutputDir
.\app_locker.exe