# Hentikan eksekusi jika ada error
$ErrorActionPreference = "Stop"

Write-Host "Mulai proses kompilasi App Locker (Release Mode)..." -ForegroundColor Cyan

# 1. Jalankan Cargo Build untuk seluruh workspace
Write-Host "[1/3] Mengompilasi semua crate Rust..." -ForegroundColor Yellow
cargo build --release

# 2. Siapkan folder output siap pakai (dist)
$OutputDir = "dist"
if (!(Test-Path -Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir | Out-Null
    Write-Host "[2/3] Membuat folder '$OutputDir'..." -ForegroundColor Yellow
} else {
    Write-Host "[2/3] Folder '$OutputDir' sudah ada, membersihkan isi lama..." -ForegroundColor Yellow
    Remove-Item -Path "$OutputDir\*" -Recurse -Force
}

# 3. Pindahkan file executable (.exe) ke folder dist
Write-Host "[3/3] Menyalin file executable ke folder '$OutputDir'..." -ForegroundColor Yellow

# Daftar file yang akan disalin (sesuaikan dengan nama crate Anda)
$Executables = @("daemon.exe", "ui.exe", "watchdog.exe")

foreach ($Exe in $Executables) {
    $SourcePath = "target\release\$Exe"
    $DestPath = "$OutputDir\$Exe"
    
    if (Test-Path -Path $SourcePath) {
        Copy-Item -Path $SourcePath -Destination $DestPath -Force
        Write-Host "  -> Berhasil menyalin $Exe" -ForegroundColor Green
    } else {
        Write-Host "  -> Peringatan: $Exe tidak ditemukan di target/release/" -ForegroundColor Red
    }
}

# Salin script start dan stop
Copy-Item -Path "start.bat" -Destination "$OutputDir\start.bat" -Force
Copy-Item -Path "stop.bat" -Destination "$OutputDir\stop.bat" -Force
Write-Host "  -> Berhasil menyalin start.bat dan stop.bat" -ForegroundColor Green

Write-Host ""
Write-Host "=================================================" -ForegroundColor Cyan
Write-Host "Build Selesai! Aplikasi siap dijalankan di dalam folder '$OutputDir'." -ForegroundColor Green
Write-Host "Cara menjalankan: Buka folder 'dist' dan jalankan 'start.bat'." -ForegroundColor Cyan
Write-Host "=================================================" -ForegroundColor Cyan