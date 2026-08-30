use anyhow::Result;
use argon2::{
    password_hash::{
        rand_core::OsRng,
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
    },
    Argon2,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Nama Windows Named Pipe untuk komunikasi IPC.
pub const PIPE_NAME: &str = r"\\.\pipe\applocker_pipe";

/// Nama file penyimpanan konfigurasi aplikasi.
pub const CONFIG_FILENAME: &str = "config.json";

/// Nama identifikasi internal Windows Service.
pub const SERVICE_NAME: &str = "AppLockerService";

/// Nama tampilan Windows Service di Windows Service Manager.
pub const SERVICE_DISPLAY_NAME: &str = "App Locker Protection Service";

/// Mengembalikan direktori konfigurasi default (`%ProgramData%\AppLocker` atau direktori eksekutabel).
pub fn get_default_config_dir() -> PathBuf {
    if let Ok(program_data) = std::env::var("ProgramData") {
        PathBuf::from(program_data).join("AppLocker")
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Struktur data konfigurasi aplikasi dan daftar target terkunci.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    /// Daftar nama file executable yang dikunci (misal: `notepad.exe`).
    pub locked_apps: Vec<String>,
    /// Hash kata sandi Argon2id (atau teks polos untuk backward compatibility).
    pub password: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        let default_pw = "admin";
        let hashed = Self::hash_password(default_pw).unwrap_or_else(|_| default_pw.to_string());
        Self {
            locked_apps: vec![],
            password: hashed,
        }
    }
}

impl AppConfig {
    /// Menghasilkan hash Argon2id dengan salt acak dari kata sandi teks polos.
    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|e| format!("Hashing failed: {}", e))
    }

    /// Memverifikasi kata sandi masukan terhadap hash Argon2 atau teks polos lama.
    pub fn verify_password(&self, input_password: &str) -> bool {
        if let Ok(parsed_hash) = PasswordHash::new(&self.password) {
            Argon2::default()
                .verify_password(input_password.as_bytes(), &parsed_hash)
                .is_ok()
        } else {
            self.password == input_password
        }
    }

    /// Memperbarui kata sandi dengan hash baru.
    pub fn set_password(&mut self, new_password: &str) -> Result<(), String> {
        self.password = Self::hash_password(new_password)?;
        Ok(())
    }

    /// Membersihkan spasi, entri kosong, dan duplikasi nama aplikasi.
    pub fn sanitize_locked_apps(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.locked_apps.retain_mut(|name| {
            *name = name.trim().to_lowercase();
            if name.is_empty() || seen.contains(name) {
                false
            } else {
                seen.insert(name.clone());
                true
            }
        });
    }

    /// Membaca konfigurasi dari direktori target dengan migrasi otomatis hash kata sandi lama.
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(CONFIG_FILENAME);
        if let Ok(data) = std::fs::read_to_string(&path) {
            let mut config: Self = serde_json::from_str(&data).unwrap_or_default();
            config.sanitize_locked_apps();
            if PasswordHash::new(&config.password).is_err()
                && let Ok(hashed) = Self::hash_password(&config.password)
            {
                config.password = hashed;
                config.save(config_dir).ok();
            }
            config
        } else {
            let mut config = Self::default();
            config.save(config_dir).ok();
            config
        }
    }

    /// Menyimpan konfigurasi secara atomic ke file `config.json`.
    pub fn save(&mut self, config_dir: &Path) -> Result<()> {
        self.sanitize_locked_apps();
        std::fs::create_dir_all(config_dir)?;
        let path = config_dir.join(CONFIG_FILENAME);
        let tmp_path = config_dir.join(format!("{}.tmp", CONFIG_FILENAME));
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp_path, &json)?;
        if let Err(e) = std::fs::rename(&tmp_path, &path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e.into());
        }
        Ok(())
    }
}

/// Permintaan IPC yang dikirimkan oleh Interceptor atau Panel ke Service.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum IpcRequest {
    /// Permintaan membuka aplikasi dengan verifikasi kata sandi.
    LaunchApp {
        app_name: String,
        app_path: String,
        password: String,
    },
    /// Permintaan pembatalan peluncuran aplikasi.
    CancelLaunch {
        app_name: String,
    },
    /// Permintaan pengubahan kata sandi master.
    ChangePassword {
        old_password: String,
        new_password: String,
    },
    /// Permintaan sinkronisasi ulang konfigurasi dari disk.
    ReloadConfig,
}

impl std::fmt::Debug for IpcRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LaunchApp { app_name, app_path, .. } => f
                .debug_struct("LaunchApp")
                .field("app_name", app_name)
                .field("app_path", app_path)
                .field("password", &"[REDACTED]")
                .finish(),
            Self::CancelLaunch { app_name } => f
                .debug_struct("CancelLaunch")
                .field("app_name", app_name)
                .finish(),
            Self::ChangePassword { .. } => f
                .debug_struct("ChangePassword")
                .field("old_password", &"[REDACTED]")
                .field("new_password", &"[REDACTED]")
                .finish(),
            Self::ReloadConfig => write!(f, "ReloadConfig"),
        }
    }
}

/// Respon balasan IPC dari Service ke client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum IpcResponse {
    /// Operasi berhasil disetujui / dieksekusi.
    Success,
    /// Kata sandi yang dimasukkan salah.
    WrongPassword,
    /// Kata sandi berhasil diubah.
    PasswordChanged,
    /// Konfigurasi berhasil disinkronkan ulang.
    ConfigReloaded,
    /// Terjadi kesalahan sistem.
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing_and_verification() {
        let mut config = AppConfig::default();
        assert!(config.verify_password("admin"));
        assert!(!config.verify_password("wrong_password"));

        config.set_password("new_secret_123").unwrap();
        assert!(config.verify_password("new_secret_123"));
        assert!(!config.verify_password("admin"));
    }

    #[test]
    fn test_legacy_plaintext_password_verification() {
        let config = AppConfig {
            locked_apps: vec![],
            password: "legacy_plain_password".to_string(),
        };
        assert!(config.verify_password("legacy_plain_password"));
        assert!(!config.verify_password("other_password"));
    }
}
