use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const PIPE_NAME: &str = r"\\.\pipe\applocker_pipe";
pub const CONFIG_FILENAME: &str = "config.json";
pub const SERVICE_NAME: &str = "AppLockerService";
pub const SERVICE_DISPLAY_NAME: &str = "App Locker Protection Service";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub locked_apps: Vec<String>,
    pub password: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            locked_apps: vec![],
            password: "admin".to_string(),
        }
    }
}

impl AppConfig {
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(CONFIG_FILENAME);
        if let Ok(data) = std::fs::read_to_string(&path) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            let config = Self::default();
            config.save(config_dir).ok();
            config
        }
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        let path = config_dir.join(CONFIG_FILENAME);
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum IpcRequest {
    LaunchApp { app_name: String, app_path: String, password: String },
    CancelLaunch { app_name: String },
    ChangePassword {
        old_password: String,
        new_password: String,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum IpcResponse {
    Success,
    WrongPassword,
    PasswordChanged,
    Error(String),
}

const IFEO_BASE_KEY: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";

#[cfg(target_os = "windows")]
pub fn set_ifeo(app_name: &str, interceptor_path: &str) -> Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let subkey = format!("{}\\{}", IFEO_BASE_KEY, app_name);
    let (key, _) = hklm.create_subkey(&subkey)?;

    let debugger_value = format!("\"{}\" --interceptor", interceptor_path);
    key.set_value("Debugger", &debugger_value)?;

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn remove_ifeo(app_name: &str) -> Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let base_key = hklm.open_subkey_with_flags(IFEO_BASE_KEY, KEY_WRITE)?;
    base_key.delete_subkey_all(app_name).ok();

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn sync_ifeo_with_config(config: &AppConfig, interceptor_path: &str) -> Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    for app in &config.locked_apps {
        if let Err(e) = set_ifeo(app, interceptor_path) {
            tracing::warn!("Gagal memasang IFEO untuk {}: {}", app, e);
        } else {
            tracing::info!("IFEO terpasang untuk: {}", app);
        }
    }

    if let Ok(base_key) = hklm.open_subkey_with_flags(IFEO_BASE_KEY, KEY_READ) {
        for subkey_name in base_key.enum_keys().flatten() {
            if let Ok(subkey) = base_key.open_subkey(&subkey_name) {
                if let Ok(debugger_val) = subkey.get_value::<String, _>("Debugger") {
                    if debugger_val.contains("--interceptor") && debugger_val.contains(interceptor_path) {
                        let is_still_locked = config
                            .locked_apps
                            .iter()
                            .any(|a| a.to_lowercase() == subkey_name.to_lowercase());

                        if !is_still_locked {
                            tracing::info!("Menghapus IFEO tidak terpakai: {}", subkey_name);
                            remove_ifeo(&subkey_name).ok();
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn remove_all_ifeo(interceptor_path: &str) -> Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(base_key) = hklm.open_subkey_with_flags(IFEO_BASE_KEY, KEY_READ) {
        let subkeys: Vec<String> = base_key.enum_keys().flatten().collect();
        for subkey_name in subkeys {
            if let Ok(subkey) = base_key.open_subkey(&subkey_name) {
                if let Ok(debugger_val) = subkey.get_value::<String, _>("Debugger") {
                    if debugger_val.contains("--interceptor") && debugger_val.contains(interceptor_path) {
                        tracing::info!("Membersihkan IFEO: {}", subkey_name);
                        remove_ifeo(&subkey_name).ok();
                    }
                }
            }
        }
    }

    Ok(())
}
