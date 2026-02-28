# App Locker (Rust)

Sebuah aplikasi desktop Windows *native* yang dibangun dengan Rust yang memungkinkan pengguna untuk memantau dan mengunci aplikasi tertentu. Ketika aplikasi yang dikunci dijalankan, App Locker mencegatnya, segera menangguhkan eksekusinya menggunakan API Windows tingkat rendah, dan meminta pengguna untuk memasukkan kata sandi sebelum mengizinkan aplikasi tersebut untuk dilanjutkan.

## Fitur

- **Pencegatan Instan**: Menggunakan API Windows *native* (`NtSuspendProcess`) untuk langsung menghentikan proses target sebelum mereka dapat menggambar jendela atau mengeksekusi logika.
- **Pengelompokan Multi-Proses**: Menangani aplikasi multi-proses modern secara cerdas (seperti Google Chrome atau aplikasi Electron) dengan menangguhkan semua proses anak dan mengelompokkannya di bawah satu *prompt* autentikasi tunggal.
- **UI Modern**: Dibangun dengan cermat menggunakan [Slint](https://slint.dev/) untuk menyediakan antarmuka grafis yang cepat, ringan, dan modern.
- **Komunikasi Antar Proses (IPC)**: Daemon dan UI berkomunikasi dengan aman melalui Windows Named Pipes (`\\.\pipe\applocker_pipe`).
- **Ketahanan**: Termasuk layanan `watchdog` untuk memastikan daemon pemantauan inti tetap aktif dan mengulangnya kembali jika terjadi kegagalan.

## Arsitektur

Proyek ini terstruktur sebagai Workspace Cargo yang berisi beberapa *crate* khusus:

1. **`app_core` (`crates/core`)**: Definisi bersama, model data, tipe permintaan/respons IPC, dan logika serialisasi.
2. **`infra` (`crates/infra`)**: Lapisan abstraksi di atas OS Windows. Secara malas memuat fungsi dari `ntdll.dll` untuk mengelola penangguhan, kelanjutan, dan penghentian proses secara *graceful* melalui `WM_CLOSE`.
3. **`daemon` (`crates/daemon`)**: Layanan latar belakang yang memantau proses sistem yang berjalan. Jika mendeteksi aplikasi yang dibatasi diluncurkan, layanan ini menangguhkan proses dan memunculkan instans *prompt* `ui`. Menangani server Named Pipe IPC.
4. **`ui` (`crates/ui`)**: *Frontend* grafis berbasis Slint. Beroperasi dalam dua mode:
   - **Panel Manajemen**: Dasbor untuk menambah, menghapus, dan mengubah kunci pada berbagai aplikasi (misal, `chrome.exe`, `notepad.exe`).
   - **Lock Prompt**: Muncul saat aplikasi yang dicegat membutuhkan autentikasi. Menangguhkan atau mematikan aplikasi yang terkunci berdasarkan masukan pengguna.
5. **`watchdog` (`crates/watchdog`)**: Proses supervisor sederhana yang berjalan terus-menerus, memantau `daemon.exe` dan memulainya kembali secara otomatis jika *crash* atau berhenti tanpa terduga.

## Prasyarat

- Sistem Operasi: **Windows 10/11** (bergantung pada API `win32` dan `ntdll.dll`)
- Alat Bangun: **Rust Toolchain** (Cargo, rustc)

## Membangun dari Sumber

Untuk mengompilasi seluruh *workspace* dalam mode rilis:

```powershell
.\build.ps1
```

*Binary* yang dikompilasi akan tersedia di `dist`.

Anda juga dapat menggunakan skrip *build* yang disediakan:
```powershell
.\debug.ps1
```

## Menjalankan Aplikasi

1. **Mulai daemon latar belakang**:
   ```powershell
   .\dist\daemon.exe
   ```
   *(Untuk penggunaan produksi, Anda biasanya akan menjalankan `.\dist\watchdog.exe` yang pada gilirannya menjaga `daemon.exe` tetap hidup).*

2. **Buka Panel Manajemen**:
   Untuk mengelola aplikasi mana yang dikunci atau mengubah konfigurasi, jalankan:
   ```powershell
   .\dist\ui.exe
   ```
   Dari sini, Anda dapat menambahkan nama proses seperti `notepad.exe` atau `chrome.exe`.

3. **Uji Pencegatan**:
   Dengan daemon berjalan, cukup luncurkan aplikasi terbatas yang dikonfigurasi (misal, buka Notepad). Anda akan segera disajikan dengan *prompt* kata sandi Slint sementara aplikasi sebenarnya tetap membeku di latar belakang.

## Konfigurasi

Konfigurasi disimpan sebagai JSON di `config.json` di sebelah *executable*. File tersebut menyimpan daftar aplikasi yang dikunci dan kata sandi master global. Kata sandi awal *default* saat pertama kali dihidupkan adalah `rust2026`.

## Lisensi

Proyek ini bersifat *open-source* dan (akan) tersedia di bawah Lisensi MIT.
