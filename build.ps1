# Hentikan eksekusi jika ada error
$ErrorActionPreference = "Stop"

Write-Host "Mulai proses kompilasi App Locker (Release Mode)..." -ForegroundColor Cyan

# 1. Jalankan Cargo Build untuk seluruh workspace
Write-Host "[1/2] Mengompilasi semua crate Rust..." -ForegroundColor Yellow
cargo build --release

# 2. Siapkan folder output siap pakai (dist)
$OutputDir = "dist"
if (!(Test-Path -Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir | Out-Null
    Write-Host "[2/2] Membuat folder '$OutputDir'..." -ForegroundColor Yellow
} else {
    Write-Host "[2/2] Folder '$OutputDir' sudah ada, membersihkan isi lama..." -ForegroundColor Yellow
    Remove-Item -Path "$OutputDir\*" -Recurse -Force
}

# Salin single executable
$SourcePath = "target\release\app_locker.exe"
if (Test-Path -Path $SourcePath) {
    Copy-Item -Path $SourcePath -Destination "$OutputDir\app_locker.exe" -Force
    Write-Host "  -> Berhasil menyalin app_locker.exe" -ForegroundColor Green
} else {
    Write-Host "  -> Peringatan: app_locker.exe tidak ditemukan di target/release/" -ForegroundColor Red
}

Write-Host ""
Write-Host "=================================================" -ForegroundColor Cyan
Write-Host "Build Selesai! Aplikasi siap dijalankan di dalam folder '$OutputDir'." -ForegroundColor Green
Write-Host "Cara menjalankan: Buka folder 'dist' dan jalankan 'app_locker.exe'." -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan