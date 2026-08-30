use app_core::{
    AppConfig, IpcRequest, IpcResponse, SERVICE_DISPLAY_NAME, SERVICE_NAME,
};
use slint::{ComponentHandle, Model};
use std::env;

pub fn run_management_panel() -> Result<(), slint::PlatformError> {
    let ui = crate::ManagementPanel::new()?;
    center_window(ui.window(), 480.0, 520.0);

    let config = AppConfig::load(&crate::exe_dir());
    refresh_app_list(&ui, &config);

    let is_active = is_service_running();
    ui.set_protection_active(is_active);

    let ui_weak = ui.as_weak();
    ui.on_toggle_app(move |index| {
        let ui = ui_weak.unwrap();
        let mut apps = model_to_vec(&ui.get_apps());
        if let Some(item) = apps.get_mut(index as usize) {
            item.locked = !item.locked;
        }
        let model = std::rc::Rc::new(slint::VecModel::from(apps));
        ui.set_apps(model.into());
    });

    let ui_weak = ui.as_weak();
    ui.on_add_app(move |name| {
        let ui = ui_weak.unwrap();
        let name_str = name.to_string().trim().to_lowercase();
        if name_str.is_empty() {
            return;
        }

        let mut apps = model_to_vec(&ui.get_apps());

        if apps
            .iter()
            .any(|a| a.name.to_string().to_lowercase() == name_str)
        {
            ui.set_status_msg("Aplikasi sudah ada di daftar.".into());
            return;
        }

        apps.push(crate::AppItem {
            name: name_str.into(),
            locked: true,
        });
        let model = std::rc::Rc::new(slint::VecModel::from(apps));
        ui.set_apps(model.into());
        ui.set_status_msg("".into());
    });

    let ui_weak = ui.as_weak();
    ui.on_remove_app(move |index| {
        let ui = ui_weak.unwrap();
        let mut apps = model_to_vec(&ui.get_apps());
        if (index as usize) < apps.len() {
            apps.remove(index as usize);
        }
        let model = std::rc::Rc::new(slint::VecModel::from(apps));
        ui.set_apps(model.into());
    });

    let ui_weak = ui.as_weak();
    ui.on_browse_file(move || {
        let picked = rfd::FileDialog::new()
            .add_filter("Executable", &["exe"])
            .pick_file();
        if let Some(file_name) = picked.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str())
            && let Some(ui) = ui_weak.upgrade()
        {
            ui.set_new_app_name(file_name.into());
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_save_config(move || {
        let Some(ui) = ui_weak.upgrade() else { return; };
        let apps = model_to_vec(&ui.get_apps());
        let locked_apps: Vec<String> = apps
            .iter()
            .filter(|a| a.locked)
            .map(|a| a.name.to_string())
            .collect();

        ui.set_status_msg("Menyimpan...".into());
        let ui_handle = ui_weak.clone();

        std::thread::spawn(move || {
            let mut config = AppConfig::load(&crate::exe_dir());
            config.locked_apps = locked_apps;

            let result_msg = if let Err(e) = config.save(&crate::exe_dir()) {
                format!("Gagal menyimpan config: {}", e)
            } else {
                let self_exe = std::env::current_exe().unwrap_or_default();
                let interceptor_path = self_exe.to_string_lossy().to_string();
                let _ = infra::sync_ifeo_with_config(&config, &interceptor_path);
                let _ = crate::ipc::send_ipc_request(IpcRequest::ReloadConfig);
                "Konfigurasi berhasil disimpan!".to_string()
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_status_msg(result_msg.into());
                }
            });
        });
    });

    let ui_weak = ui.as_weak();
    ui.on_change_password(move |old_pw, new_pw, confirm_pw| {
        let Some(ui) = ui_weak.upgrade() else { return; };

        if old_pw.is_empty() || new_pw.is_empty() || confirm_pw.is_empty() {
            ui.set_password_status_msg("Semua field harus diisi.".into());
            return;
        }

        if new_pw != confirm_pw {
            ui.set_password_status_msg("Password baru tidak cocok.".into());
            return;
        }

        if new_pw == old_pw {
            ui.set_password_status_msg("Password baru harus berbeda dari password lama.".into());
            return;
        }

        ui.set_password_status_msg("Memproses...".into());
        let ui_handle = ui_weak.clone();
        let old_pw = old_pw.to_string();
        let new_pw = new_pw.to_string();

        std::thread::spawn(move || {
            let request = IpcRequest::ChangePassword {
                old_password: old_pw.clone(),
                new_password: new_pw.clone(),
            };

            let (success, msg) = match crate::ipc::send_ipc_request(request) {
                Ok(IpcResponse::PasswordChanged) => (true, "Password berhasil diubah!".to_string()),
                Ok(IpcResponse::WrongPassword) => (false, "Password lama salah!".to_string()),
                Ok(IpcResponse::Error(e)) => (false, format!("Gagal: {}", e)),
                _ => {
                    let mut config = AppConfig::load(&crate::exe_dir());
                    if !config.verify_password(&old_pw) {
                        (false, "Password lama salah!".to_string())
                    } else if let Err(e) = config.set_password(&new_pw) {
                        (false, format!("Gagal: {}", e))
                    } else if let Err(e) = config.save(&crate::exe_dir()) {
                        (false, format!("Gagal menyimpan: {}", e))
                    } else {
                        (true, "Password berhasil diubah!".to_string())
                    }
                }
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_password_status_msg(msg.into());
                    if success {
                        ui.set_old_password("".into());
                        ui.set_new_password("".into());
                        ui.set_confirm_password("".into());
                    }
                }
            });
        });
    });

    let ui_weak = ui.as_weak();
    ui.on_toggle_protection(move || {
        let Some(ui) = ui_weak.upgrade() else { return; };
        let currently_active = ui.get_protection_active();
        ui.set_protection_status_msg("Sedang memproses perubahan status service...".into());
        let ui_handle = ui_weak.clone();

        std::thread::spawn(move || {
            let (new_state, msg) = if currently_active {
                match stop_service() {
                    Ok(_) => (false, "Service dihentikan.".to_string()),
                    Err(e) => (true, format!("Gagal stop: {}", e)),
                }
            } else {
                match start_service() {
                    Ok(_) => (true, "Service dimulai!".to_string()),
                    Err(e) => (false, format!("Gagal start: {}", e)),
                }
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_protection_active(new_state);
                    ui.set_protection_status_msg(msg.into());
                }
            });
        });
    });

    let ui_weak = ui.as_weak();
    ui.on_check_protection_status(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_protection_active(is_service_running());
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_install_service(move || {
        let Some(ui) = ui_weak.upgrade() else { return; };
        ui.set_protection_status_msg("Sedang memproses install & start service...".into());
        let ui_handle = ui_weak.clone();

        std::thread::spawn(move || {
            let (installed, active, msg) = match install_service() {
                Ok(_) => match start_service() {
                    Ok(_) => (true, true, "Service berhasil diinstall & aktif!".to_string()),
                    Err(e) => (true, false, format!("Service terinstall, tapi gagal start: {}", e)),
                },
                Err(e) => (false, false, format!("Gagal install service: {}", e)),
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_service_installed(installed);
                    ui.set_protection_active(active);
                    ui.set_protection_status_msg(msg.into());
                }
            });
        });
    });

    let ui_weak = ui.as_weak();
    ui.on_uninstall_service(move || {
        let Some(ui) = ui_weak.upgrade() else { return; };
        ui.set_protection_status_msg("Sedang memproses uninstall service...".into());
        let ui_handle = ui_weak.clone();

        std::thread::spawn(move || {
            let (installed, msg) = match uninstall_service() {
                Ok(_) => {
                    let self_exe = std::env::current_exe().unwrap_or_default();
                    let interceptor_path = self_exe.to_string_lossy().to_string();
                    let _ = infra::remove_all_ifeo(&interceptor_path);
                    (false, "Service berhasil di-uninstall.".to_string())
                }
                Err(e) => (true, format!("Gagal uninstall: {}", e)),
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_service_installed(installed);
                    ui.set_protection_active(false);
                    ui.set_protection_status_msg(msg.into());
                }
            });
        });
    });

    ui.set_service_installed(is_service_installed());

    ui.run()
}

pub fn install_service() -> anyhow::Result<()> {
    let self_exe = env::current_exe()?;
    let bin_path = format!("\"{}\" --service", self_exe.to_string_lossy());
    infra::install_service(SERVICE_NAME, SERVICE_DISPLAY_NAME, &bin_path)
}

pub fn uninstall_service() -> anyhow::Result<()> {
    let self_exe = env::current_exe()?;
    let interceptor_path = self_exe.to_string_lossy().to_string();
    infra::remove_all_ifeo(&interceptor_path).ok();
    infra::uninstall_service(SERVICE_NAME)
}

pub fn start_service() -> anyhow::Result<()> {
    infra::start_service(SERVICE_NAME)
}

pub fn stop_service() -> anyhow::Result<()> {
    infra::stop_service(SERVICE_NAME)
}

pub fn is_service_running() -> bool {
    infra::is_service_running(SERVICE_NAME)
}

pub fn is_service_installed() -> bool {
    infra::is_service_installed(SERVICE_NAME)
}

#[cfg(target_os = "windows")]
pub fn center_window(window: &slint::Window, logical_width: f32, logical_height: f32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) } as f32;
    let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) } as f32;

    let scale_factor = window.scale_factor();
    let physical_width = logical_width * scale_factor;
    let physical_height = logical_height * scale_factor;

    let x = (screen_width - physical_width) / 2.0;
    let y = (screen_height - physical_height) / 2.0;

    window.set_position(slint::PhysicalPosition::new(x as i32, y as i32));
}

#[cfg(not(target_os = "windows"))]
pub fn center_window(_window: &slint::Window, _logical_width: f32, _logical_height: f32) {}

fn model_to_vec(model: &slint::ModelRc<crate::AppItem>) -> Vec<crate::AppItem> {
    (0..model.row_count())
        .filter_map(|i| model.row_data(i))
        .collect()
}

fn refresh_app_list(ui: &crate::ManagementPanel, config: &AppConfig) {
    let items: Vec<crate::AppItem> = config
        .locked_apps
        .iter()
        .map(|name| crate::AppItem {
            name: name.clone().into(),
            locked: true,
        })
        .collect();

    let model = std::rc::Rc::new(slint::VecModel::from(items));
    ui.set_apps(model.into());
}
