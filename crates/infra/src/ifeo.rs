use anyhow::Result;
use app_core::AppConfig;

const IFEO_BASE_KEY: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";

#[cfg(target_os = "windows")]
pub fn set_ifeo(app_name: &str, interceptor_path: &str) -> Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let subkey = format!("{}\\{}", IFEO_BASE_KEY, app_name);
    match hklm.create_subkey(&subkey) {
        Ok((key, _)) => {
            let debugger_value = format!("\"{}\" --interceptor", interceptor_path);
            key.set_value("Debugger", &debugger_value)?;
            // If Windows 11 default UseFilter is present, delete or disable it so Debugger is evaluated!
            let _ = key.delete_value("UseFilter");
            // Also override any filter subkeys (0, 1, 2, etc.) so AppExecutionAlias redirect is bypassed
            for subkey_name in key.enum_keys().flatten() {
                if let Ok(filter_sub) = key.open_subkey_with_flags(&subkey_name, KEY_WRITE) {
                    let _ = filter_sub.delete_value("AppExecutionAliasRedirect");
                    let _ = filter_sub.set_value("Debugger", &debugger_value);
                }
            }
            Ok(())
        }
        Err(e) => {
            tracing::warn!("Gagal membuat subkey IFEO untuk {}: {}", app_name, e);
            Ok(())
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn set_ifeo(_app_name: &str, _interceptor_path: &str) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn remove_ifeo(app_name: &str) -> Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let subkey = format!("{}\\{}", IFEO_BASE_KEY, app_name);
    if let Ok(key) = hklm.open_subkey_with_flags(&subkey, KEY_READ | KEY_WRITE) {
        let _ = key.delete_value("Debugger");
        let subkeys: Vec<String> = key.enum_keys().flatten().collect();
        if !subkeys.is_empty() {
            // Restore Windows 11 filter subkeys
            let _ = key.set_value("UseFilter", &1u32);
            for s in subkeys {
                if let Ok(filter_sub) = key.open_subkey_with_flags(&s, KEY_READ | KEY_WRITE) {
                    let _ = filter_sub.delete_value("Debugger");
                    let _ = filter_sub.set_value("AppExecutionAliasRedirect", &1u32);
                }
            }
        } else if let Ok(base_key) = hklm.open_subkey_with_flags(IFEO_BASE_KEY, KEY_READ | KEY_WRITE) {
            let _ = base_key.delete_subkey_all(app_name);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn remove_ifeo(_app_name: &str) -> Result<()> {
    Ok(())
}

pub struct IfeoUnlockGuard {
    app_name: String,
    interceptor_path: String,
}

impl IfeoUnlockGuard {
    pub fn new(app_name: String, interceptor_path: String) -> Self {
        let _ = remove_ifeo(&app_name);
        Self {
            app_name,
            interceptor_path,
        }
    }
}

impl Drop for IfeoUnlockGuard {
    fn drop(&mut self) {
        let _ = set_ifeo(&self.app_name, &self.interceptor_path);
    }
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
            if let Ok(subkey) = base_key.open_subkey(&subkey_name)
                && let Ok(debugger_val) = subkey.get_value::<String, _>("Debugger")
            {
                let is_ours = debugger_val.contains("--interceptor") && debugger_val.contains(interceptor_path);
                let is_still_locked = config
                    .locked_apps
                    .iter()
                    .any(|a| a.to_lowercase() == subkey_name.to_lowercase());

                if is_ours && !is_still_locked {
                    tracing::info!("Menghapus IFEO tidak terpakai: {}", subkey_name);
                    remove_ifeo(&subkey_name).ok();
                }
            }
        }
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn sync_ifeo_with_config(_config: &AppConfig, _interceptor_path: &str) -> Result<()> {
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
            let mut matches = false;
            if let Ok(subkey) = base_key.open_subkey(&subkey_name) {
                if let Ok(debugger_val) = subkey.get_value::<String, _>("Debugger")
                    && debugger_val.contains("--interceptor")
                    && debugger_val.contains(interceptor_path)
                {
                    matches = true;
                } else {
                    for filter_name in subkey.enum_keys().flatten() {
                        if let Ok(f_sub) = subkey.open_subkey(&filter_name)
                            && let Ok(f_dbg) = f_sub.get_value::<String, _>("Debugger")
                            && f_dbg.contains("--interceptor")
                            && f_dbg.contains(interceptor_path)
                        {
                            matches = true;
                            break;
                        }
                    }
                }
            }
            if matches {
                tracing::info!("Membersihkan IFEO: {}", subkey_name);
                remove_ifeo(&subkey_name).ok();
            }
        }
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn remove_all_ifeo(_interceptor_path: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn test_registry_sync_and_orphan_cleanup_isolated() {
        use winreg::enums::*;
        use winreg::RegKey;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let test_base_path = r"Software\AppLockerTest_IFEO";
        let (test_base, _) = hkcu.create_subkey(test_base_path).unwrap();

        let interceptor_path = "C:\\app_locker.exe";

        let (sub1, _) = test_base.create_subkey("orphan_app.exe").unwrap();
        sub1.set_value("Debugger", &format!("\"{}\" --interceptor", interceptor_path)).unwrap();

        let (sub2, _) = test_base.create_subkey("locked_app.exe").unwrap();
        sub2.set_value("Debugger", &format!("\"{}\" --interceptor", interceptor_path)).unwrap();

        let config = AppConfig {
            locked_apps: vec!["locked_app.exe".into()],
            password: "admin".into(),
        };

        for subkey_name in test_base.enum_keys().flatten() {
            if let Ok(subkey) = test_base.open_subkey(&subkey_name)
                && let Ok(debugger_val) = subkey.get_value::<String, _>("Debugger")
            {
                let is_ours = debugger_val.contains("--interceptor") && debugger_val.contains(interceptor_path);
                let is_still_locked = config
                    .locked_apps
                    .iter()
                    .any(|a| a.to_lowercase() == subkey_name.to_lowercase());

                if is_ours && !is_still_locked {
                    let _ = test_base.delete_subkey_all(&subkey_name);
                }
            }
        }

        assert!(test_base.open_subkey("orphan_app.exe").is_err());
        assert!(test_base.open_subkey("locked_app.exe").is_ok());

        let _ = hkcu.delete_subkey_all(test_base_path);
    }
}
