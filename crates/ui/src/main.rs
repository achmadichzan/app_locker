#![windows_subsystem = "windows"]

use app_core::{AppConfig, IpcRequest, IpcResponse, PIPE_NAME};
use slint::Model;
use std::env;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;

slint::include_modules!();

fn config_dir() -> PathBuf {
    env::current_exe()
        .unwrap()
        .parent()
        .expect("Tidak dapat menemukan parent directory")
        .to_path_buf()
}

#[cfg(target_os = "windows")]
fn center_window(window: &slint::Window, logical_width: f32, logical_height: f32) {
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
fn center_window(_window: &slint::Window, _logical_width: f32, _logical_height: f32) {}

fn model_to_vec(model: &slint::ModelRc<AppItem>) -> Vec<AppItem> {
    (0..model.row_count())
        .filter_map(|i| model.row_data(i))
        .collect()
}

fn main() -> Result<(), slint::PlatformError> {
    let args: Vec<String> = env::args().collect();

    if args.len() >= 3 {
        run_lock_prompt(&args)
    } else {
        run_management_panel()
    }
}

fn run_management_panel() -> Result<(), slint::PlatformError> {
    let ui = ManagementPanel::new()?;
    center_window(ui.window(), 480.0, 520.0);

    let config = AppConfig::load(&config_dir());
    refresh_app_list(&ui, &config);

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

        apps.push(AppItem {
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
    ui.on_save_config(move || {
        let ui = ui_weak.unwrap();
        let apps = model_to_vec(&ui.get_apps());

        let locked_apps: Vec<String> = apps
            .iter()
            .filter(|a| a.locked)
            .map(|a| a.name.to_string())
            .collect();

        let mut config = AppConfig::load(&config_dir());
        config.locked_apps = locked_apps;

        match config.save(&config_dir()) {
            Ok(_) => ui.set_status_msg("✅ Konfigurasi berhasil disimpan!".into()),
            Err(e) => ui.set_status_msg(format!("❌ Gagal menyimpan: {}", e).into()),
        }
    });

    ui.run()
}

fn refresh_app_list(ui: &ManagementPanel, config: &AppConfig) {
    let items: Vec<AppItem> = config
        .locked_apps
        .iter()
        .map(|name| AppItem {
            name: name.clone().into(),
            locked: true,
        })
        .collect();

    let model = std::rc::Rc::new(slint::VecModel::from(items));
    ui.set_apps(model.into());
}

fn run_lock_prompt(args: &[String]) -> Result<(), slint::PlatformError> {
    let _target_pid: u32 = args[1].parse().unwrap_or(0);
    let target_name = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "Aplikasi".to_string());

    let ui = AppPrompt::new()?;
    ui.set_target_name(target_name.clone().into());
    center_window(ui.window(), 380.0, 260.0);

    let ui_weak = ui.as_weak();
    let target_name_cancel = target_name.clone();
    ui.on_cancel_requested(move || {
        send_ipc_request(IpcRequest::CancelUnlock {
            app_name: target_name_cancel.clone(),
        });
        if let Some(ui) = ui_weak.upgrade() {
            let _ = ui.hide();
        }
    });

    let ui_weak = ui.as_weak();
    let target_name_unlock = target_name.clone();
    ui.on_unlock_requested(move |password| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };

        let request = IpcRequest::UnlockApp {
            app_name: target_name_unlock.clone(),
            password: password.to_string(),
        };

        match send_ipc_request(request) {
            Some(IpcResponse::Success) => {
                let _ = ui.hide();
            }
            Some(IpcResponse::WrongPassword) => {
                ui.set_error_msg("Password salah! Coba lagi.".into());
            }
            _ => {
                ui.set_error_msg("Gagal terhubung ke sistem keamanan.".into());
            }
        }
    });

    ui.run()
}

fn send_ipc_request(request: IpcRequest) -> Option<IpcResponse> {
    let payload = serde_json::to_vec(&request).ok()?;

    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_NAME)
        .ok()?;

    pipe.write_all(&payload).ok()?;

    let mut buffer = vec![0u8; 512];
    let bytes_read = pipe.read(&mut buffer).ok()?;
    serde_json::from_slice(&buffer[..bytes_read]).ok()
}
