# App Locker (Rust)

Aplikasi desktop Windows native yang dibangun dengan Rust untuk mengamankan dan mengunci aplikasi tertentu menggunakan proteksi Image File Execution Options (IFEO) dan Windows Service.

## Fitur

- Single Executable: Seluruh komponen (Service, Interceptor, Management Panel) dikompilasi ke dalam satu file binary `app_locker.exe`.
- Keamanan Password: Menggunakan algoritma hashing Argon2id untuk melindungi kredensial otentikasi.
- Windows Service: Berjalan sebagai Windows Service di latar belakang untuk manajemen proteksi dan sinkronisasi registry IFEO secara otomatis.
- Event-Driven IPC: Komunikasi antara Panel Manajemen, Interceptor, dan Service melalui Windows Named Pipes (`\\.\pipe\applocker_pipe`) dengan Access Control List (DACL) yang aman.
- UI Modern: Dibangun dengan Slint untuk antarmuka grafis ringan dan responsif.
- Dukungan Argumen CLI: Meneruskan seluruh parameter CLI dan file association aplikasi target setelah proses unlock berhasil.

## Arsitektur

Proyek terstruktur sebagai Cargo Workspace dengan pemisahan Clean Architecture:

| Perintah | Mode | Fungsi |
|---|---|---|
| `app_locker.exe` | UI | Membuka Panel Manajemen |
| `app_locker.exe --service` | Service | Windows Service latar belakang |
| `app_locker.exe --interceptor <path> [args...]` | Interceptor | Popup autentikasi saat aplikasi terkunci dipanggil |

### Struktur Crate

1. `app_core` (`crates/core`): Domain model murni (`AppConfig`), logika hashing password Argon2, dan definisi protokol IPC (`IpcRequest`, `IpcResponse`). Bebas dari dependensi platform spesifik.
2. `infra` (`crates/infra`): Abstraksi platform Windows untuk manajemen Image File Execution Options (`ifeo`) dan kontrol proses tingkat rendah (`process`).
3. `app_locker` (`crates/ui`): Binary eksekutabel utama yang terbagi dalam modul modular:
   - `cli`: Parsing argumen dan pemilihan mode eksekusi.
   - `service`: Dispatcher dan lifecycle Windows Service.
   - `ipc`: Server dan client Windows Named Pipe dengan DACL aman.
   - `interceptor`: Dialog autentikasi aplikasi terkunci.
   - `panel`: Dashboard konfigurasi dan instalasi service.

## Prasyarat

- Sistem Operasi: Windows 10 / Windows 11
- Toolchain: Rust 1.80+ (Edition 2024 / 2021)

## Kompilasi dan Pengujian

Menjalankan seluruh unit test dan integration test:
```powershell
cargo test --workspace
```

Membangun mode rilis:
```powershell
.\build.ps1
```

Membangun mode debug:
```powershell
.\debug.ps1
```

Hasil build tersedia di folder `dist/` (release) atau `debug_bin/` (debug).

## Penggunaan

1. Jalankan `app_locker.exe` sebagai Administrator.
2. Tambahkan aplikasi yang ingin dikunci (misal `notepad.exe`, `chrome.exe`).
3. Klik tombol Install Service dan Start Service.
4. Saat aplikasi target dijalankan, popup autentikasi akan muncul sebelum aplikasi dapat diluncurkan.

## Lisensi

Proyek ini bersifat *open-source* dan (akan) tersedia di bawah Lisensi MIT.
