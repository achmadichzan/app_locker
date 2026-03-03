# App Locker (Rust)

Sebuah aplikasi desktop Windows *native* yang dibangun dengan Rust yang memungkinkan pengguna untuk memantau dan mengunci aplikasi tertentu. Ketika aplikasi yang dikunci dijalankan, App Locker mencegatnya, segera menangguhkan eksekusinya menggunakan API Windows tingkat rendah, dan meminta pengguna untuk memasukkan kata sandi sebelum mengizinkan aplikasi tersebut untuk dilanjutkan.

## Fitur

- **Single Executable**: Seluruh aplikasi (daemon, watchdog, UI) dikemas dalam satu file `app_locker.exe` untuk distribusi yang mudah.
- **Start/Stop Protection**: Kontrol proteksi langsung dari UI dengan tombol toggle — tanpa perlu menjalankan script terpisah.
- **Pencegatan Instan**: Menggunakan API Windows *native* (`NtSuspendProcess`) untuk langsung menghentikan proses target sebelum mereka dapat menggambar jendela atau mengeksekusi logika.
- **UI Modern**: Dibangun dengan [Slint](https://slint.dev/) untuk menyediakan antarmuka grafis yang cepat, ringan, dan modern.
- **Komunikasi Antar Proses (IPC)**: Daemon dan UI berkomunikasi dengan aman melalui Windows Named Pipes (`\\.\pipe\applocker_pipe`).
- **Ketahanan**: Termasuk layanan *watchdog* untuk memastikan daemon pemantauan inti tetap aktif dan menjalankannya kembali jika terjadi kegagalan.
- **Auto Startup**: Opsi untuk menjalankan proteksi secara otomatis saat komputer menyala.

## Arsitektur

Proyek ini terstruktur sebagai Workspace Cargo. Semua komponen dikompilasi menjadi **satu executable** (`app_locker.exe`) yang berjalan dalam mode berbeda berdasarkan argumen CLI:

| Perintah | Mode | Fungsi |
|---|---|---|
| `app_locker.exe` | UI | Membuka Panel Manajemen |
| `app_locker.exe --daemon` | Daemon | Memantau dan mengunci proses |
| `app_locker.exe --watchdog` | Watchdog | Menjaga daemon tetap hidup |
| `app_locker.exe --prompt <pid> <nama>` | Prompt | Menampilkan popup autentikasi |

### Struktur Crate

1. **`app_core` (`crates/core`)**: Definisi bersama, model data, tipe permintaan/respons IPC, dan logika serialisasi.
2. **`infra` (`crates/infra`)**: Lapisan abstraksi di atas OS Windows. Memuat fungsi dari `ntdll.dll` untuk mengelola penangguhan, kelanjutan, dan penghentian proses.
3. **`app_locker` (`crates/ui`)**: Executable utama yang menggabungkan:
   - **Daemon**: Layanan latar belakang yang memantau proses dan mencegat aplikasi yang dikunci.
   - **Watchdog**: Supervisor yang memastikan daemon selalu aktif.
   - **Panel Manajemen**: Dasbor untuk menambah, menghapus, mengubah kunci, dan mengaktifkan/menonaktifkan proteksi.
   - **Lock Prompt**: Popup autentikasi saat aplikasi yang dikunci terdeteksi.

## Prasyarat

### Untuk Development
- Sistem Operasi: **Windows 10/11**
- Alat Bangun: **Rust Toolchain** (Cargo, rustc)

### Untuk Pengguna
- Sistem Operasi: **Windows 10/11**
- Cukup jalankan `app_locker.exe` — tidak perlu install Rust atau dependensi lainnya.

## Membangun dari Sumber

Build mode rilis:
```powershell
.\build.ps1
```

Build mode debug:
```powershell
.\debug.ps1
```

Hasil build tersedia di folder `dist/` (release) atau `debug_bin/` (debug).

## Menjalankan Aplikasi

Cukup jalankan satu file:
```powershell
.\dist\app_locker.exe
```

Dari Panel Manajemen, Anda dapat:
1. **Menambah/menghapus aplikasi** yang ingin dikunci (misal `chrome.exe`, `notepad.exe`)
2. **Mengaktifkan proteksi** dengan toggle 🛡️ Start/Stop Protection
3. **Mengaktifkan auto startup** agar proteksi berjalan otomatis saat komputer menyala
4. **Mengubah password** melalui menu Ubah Password

### Menguji Pencegatan

Dengan proteksi aktif, cukup buka aplikasi yang telah dikunci. Anda akan segera disajikan popup autentikasi sementara aplikasi tersebut membeku di latar belakang.

## Konfigurasi

Konfigurasi disimpan sebagai JSON di `config.json` di sebelah executable. File tersebut menyimpan daftar aplikasi yang dikunci dan kata sandi. Default awal: daftar kosong tanpa password.

## Lisensi

Proyek ini bersifat *open-source* dan (akan) tersedia di bawah Lisensi MIT.
