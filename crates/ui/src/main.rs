#![windows_subsystem = "windows"]

use app_core::{AppConfig, IpcRequest, IpcResponse, PIPE_NAME, SERVICE_NAME, SERVICE_DISPLAY_NAME};
use slint::Model;
use std::env;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{Level, error, info, warn};

slint::include_modules!();

fn exe_dir() -> PathBuf {
    env::current_exe()
        .unwrap()
        .parent()
        .expect("Tidak dapat menemukan parent directory")
        .to_path_buf()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("--service") => run_service(),
        Some("--interceptor") => {
            let app_path = args.get(2).cloned().unwrap_or_default();
            let app_name = std::path::Path::new(&app_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown.exe")
                .to_lowercase();
            run_interceptor(&app_name, &app_path)
        }
        _ => run_management_panel().map_err(|e| anyhow::anyhow!(e)),
    }
}

use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

define_windows_service!(ffi_service_main, service_main);

fn run_service() -> anyhow::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| anyhow::anyhow!("Gagal memulai service dispatcher: {}", e))?;
    Ok(())
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service_inner() {
        error!("Service gagal: {}", e);
    }
}

fn run_service_inner() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .init();
    info!("AppLocker Service menyala...");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = Arc::new(std::sync::Mutex::new(Some(shutdown_tx)));

    let shutdown_tx_clone = shutdown_tx.clone();
    let status_handle = service_control_handler::register(
        SERVICE_NAME,
        move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    info!("Menerima sinyal stop/shutdown...");
                    if let Some(tx) = shutdown_tx_clone.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        },
    )
    .map_err(|e| anyhow::anyhow!("Gagal register service control handler: {}", e))?;

    status_handle
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .map_err(|e| anyhow::anyhow!("Gagal set service status: {}", e))?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let dir = exe_dir();
        let self_exe = env::current_exe().expect("Gagal mendapatkan path executable");
        let interceptor_path = self_exe.to_string_lossy().to_string();

        let config = Arc::new(Mutex::new(AppConfig::load(&dir)));

        {
            let cfg = config.lock().await;
            info!("Aplikasi terkunci: {:?}", cfg.locked_apps);
            if let Err(e) = app_core::sync_ifeo_with_config(&cfg, &interceptor_path) {
                error!("Gagal sinkronisasi IFEO: {}", e);
            }
        }

        let config_reload = config.clone();
        let dir_reload = dir.clone();
        let interceptor_reload = interceptor_path.clone();
        let reload_handle = tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(3)).await;
                let new_config = AppConfig::load(&dir_reload);
                let mut cfg = config_reload.lock().await;

                if cfg.locked_apps != new_config.locked_apps {
                    info!(
                        "Config ter-update! Aplikasi terkunci: {:?}",
                        new_config.locked_apps
                    );
                    *cfg = new_config;
                    if let Err(e) = app_core::sync_ifeo_with_config(&cfg, &interceptor_reload) {
                        error!("Gagal sinkronisasi IFEO setelah reload: {}", e);
                    }
                }
            }
        });

        let recently_unlocked: Arc<Mutex<std::collections::HashMap<String, tokio::time::Instant>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));

        let config_ipc = config.clone();
        let interceptor_ipc = interceptor_path.clone();
        let unlocked_ipc = recently_unlocked.clone();
        let ipc_handle = tokio::spawn(async move {
            run_ipc_server(config_ipc, interceptor_ipc, unlocked_ipc).await;
        });

        let _ = shutdown_rx.await;
        info!("Service menerima sinyal shutdown, membersihkan IFEO...");

        if let Err(e) = app_core::remove_all_ifeo(&interceptor_path) {
            error!("Gagal membersihkan IFEO: {}", e);
        }

        reload_handle.abort();
        ipc_handle.abort();
    });

    status_handle
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .ok();

    info!("AppLocker Service berhenti.");
    Ok(())
}

async fn run_ipc_server(
    config: Arc<Mutex<AppConfig>>,
    interceptor_path: String,
    recently_unlocked: Arc<Mutex<std::collections::HashMap<String, tokio::time::Instant>>>,
) {
    info!("IPC Server mendengarkan di {}", PIPE_NAME);

    loop {
        let mut server = match create_open_pipe() {
            Ok(s) => s,
            Err(e) => {
                error!("Gagal membuat Named Pipe: {}", e);
                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        if server.connect().await.is_err() {
            continue;
        }

        info!("Client terhubung ke pipe!");

        let config_clone = config.clone();
        let interceptor_clone = interceptor_path.clone();
        let unlocked_clone = recently_unlocked.clone();
        tokio::spawn(async move {
            handle_ipc_client(&mut server, config_clone, interceptor_clone, unlocked_clone).await;
        });
    }
}

fn create_open_pipe() -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use std::ffi::c_void;

    #[repr(C)]
    struct SECURITY_DESCRIPTOR {
        revision: u8,
        sbz1: u8,
        control: u16,
        owner: *mut c_void,
        group: *mut c_void,
        sacl: *mut c_void,
        dacl: *mut c_void,
    }

    #[repr(C)]
    struct SECURITY_ATTRIBUTES {
        n_length: u32,
        lp_security_descriptor: *mut c_void,
        b_inherit_handle: i32,
    }

    unsafe extern "system" {
        fn InitializeSecurityDescriptor(sd: *mut SECURITY_DESCRIPTOR, rev: u32) -> i32;
        fn SetSecurityDescriptorDacl(
            sd: *mut SECURITY_DESCRIPTOR,
            present: i32,
            dacl: *mut c_void,
            defaulted: i32,
        ) -> i32;
    }

    unsafe {
        let mut sd: SECURITY_DESCRIPTOR = std::mem::zeroed();
        InitializeSecurityDescriptor(&mut sd, 1);
        SetSecurityDescriptorDacl(&mut sd, 1, std::ptr::null_mut(), 0);

        let mut sa = SECURITY_ATTRIBUTES {
            n_length: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lp_security_descriptor: &mut sd as *mut _ as *mut c_void,
            b_inherit_handle: 0,
        };

        ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .create_with_security_attributes_raw(
                PIPE_NAME,
                &mut sa as *mut _ as *mut c_void,
            )
    }
}

async fn handle_ipc_client(
    server: &mut tokio::net::windows::named_pipe::NamedPipeServer,
    config: Arc<Mutex<AppConfig>>,
    interceptor_path: String,
    recently_unlocked: Arc<Mutex<std::collections::HashMap<String, tokio::time::Instant>>>,
) {
    let mut buffer = vec![0u8; 4096];
    let bytes_read = match server.read(&mut buffer).await {
        Ok(0) => return,
        Ok(n) => n,
        Err(_) => return,
    };

    let request = match serde_json::from_slice::<IpcRequest>(&buffer[..bytes_read]) {
        Ok(req) => req,
        Err(e) => {
            warn!("Gagal parse IPC request: {}", e);
            return;
        }
    };

    let response = match request {
        IpcRequest::LaunchApp { app_name, app_path: _, password } => {
            let cfg = config.lock().await;

            let auto_approved = {
                let unlocked = recently_unlocked.lock().await;
                if let Some(unlock_time) = unlocked.get(&app_name) {
                    unlock_time.elapsed() < Duration::from_secs(30)
                } else {
                    false
                }
            };

            if auto_approved || password == cfg.password {
                if auto_approved {
                    info!("Auto-approve untuk {} (child process dalam 30 detik)", app_name);
                } else {
                    info!("Password benar untuk {}. Menghapus IFEO sementara...", app_name);
                }

                {
                    let mut unlocked = recently_unlocked.lock().await;
                    unlocked.insert(app_name.clone(), tokio::time::Instant::now());
                }
                if let Err(e) = app_core::remove_ifeo(&app_name) {
                    error!("Gagal menghapus IFEO sementara untuk {}: {}", app_name, e);
                    IpcResponse::Error(format!("Gagal menghapus IFEO: {}", e))
                } else {
                    let app_name_clone = app_name.clone();
                    let interceptor_clone = interceptor_path.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        if let Err(e) = app_core::set_ifeo(&app_name_clone, &interceptor_clone) {
                            error!("Gagal memasang kembali IFEO untuk {}: {}", app_name_clone, e);
                        } else {
                            info!("IFEO dipasang kembali untuk {}", app_name_clone);
                        }
                    });

                    IpcResponse::Success
                }
            } else {
                warn!("Password salah untuk {}", app_name);
                IpcResponse::WrongPassword
            }
        }
        IpcRequest::CancelLaunch { app_name } => {
            info!("Peluncuran dibatalkan untuk {}", app_name);
            IpcResponse::Success
        }
        IpcRequest::ChangePassword {
            old_password,
            new_password,
        } => {
            let mut cfg = config.lock().await;
            if old_password != cfg.password {
                warn!("Gagal ubah password: password lama salah");
                IpcResponse::WrongPassword
            } else {
                cfg.password = new_password;
                match cfg.save(&exe_dir()) {
                    Ok(_) => {
                        info!("Password berhasil diubah");
                        IpcResponse::PasswordChanged
                    }
                    Err(e) => {
                        error!("Gagal menyimpan password baru: {}", e);
                        IpcResponse::Error(format!("Gagal menyimpan: {}", e))
                    }
                }
            }
        }
    };

    if let Ok(response_bytes) = serde_json::to_vec(&response) {
        let _ = server.write_all(&response_bytes).await;
    }
}

fn run_interceptor(app_name: &str, app_path: &str) -> anyhow::Result<()> {
    let ui = AppPrompt::new().map_err(|e| anyhow::anyhow!(e))?;
    ui.set_target_name(app_name.into());
    center_window(ui.window(), 380.0, 260.0);

    let ui_weak = ui.as_weak();
    let app_name_cancel = app_name.to_string();
    ui.on_cancel_requested(move || {
        let _ = send_ipc_request(IpcRequest::CancelLaunch {
            app_name: app_name_cancel.clone(),
        });
        if let Some(ui) = ui_weak.upgrade() {
            let _ = ui.hide();
        }
    });

    let ui_weak = ui.as_weak();
    let app_name_unlock = app_name.to_string();
    let app_path_unlock = app_path.to_string();
    ui.on_unlock_requested(move |password| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };

        let request = IpcRequest::LaunchApp {
            app_name: app_name_unlock.clone(),
            app_path: app_path_unlock.clone(),
            password: password.to_string(),
        };

        match send_ipc_request(request) {
            Ok(IpcResponse::Success) => {
                let _ = std::process::Command::new(&app_path_unlock)
                    .spawn();
                let _ = ui.hide();
            }
            Ok(IpcResponse::WrongPassword) => {
                ui.set_error_msg("Password salah! Coba lagi.".into());
            }
            Ok(other) => {
                ui.set_error_msg(format!("Error: {:?}", other).into());
            }
            Err(e) => {
                ui.set_error_msg(format!("Gagal: {}", e).into());
            }
        }
    });

    ui.run().map_err(|e| anyhow::anyhow!(e))
}

fn run_management_panel() -> Result<(), slint::PlatformError> {
    let ui = ManagementPanel::new()?;
    center_window(ui.window(), 480.0, 520.0);

    let config = AppConfig::load(&exe_dir());
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
    ui.on_browse_file(move || {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Executable", &["exe"])
            .pick_file()
        {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_new_app_name(file_name.into());
                }
            }
        }
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

        let mut config = AppConfig::load(&exe_dir());
        config.locked_apps = locked_apps;

        match config.save(&exe_dir()) {
            Ok(_) => ui.set_status_msg("✅ Konfigurasi berhasil disimpan!".into()),
            Err(e) => ui.set_status_msg(format!("❌ Gagal menyimpan: {}", e).into()),
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_change_password(move |old_pw, new_pw, confirm_pw| {
        let ui = ui_weak.unwrap();

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

        let request = IpcRequest::ChangePassword {
            old_password: old_pw.to_string(),
            new_password: new_pw.to_string(),
        };

        match send_ipc_request(request) {
            Ok(IpcResponse::PasswordChanged) => {
                ui.set_password_status_msg("✅ Password berhasil diubah!".into());
                ui.set_old_password("".into());
                ui.set_new_password("".into());
                ui.set_confirm_password("".into());
            }
            Ok(IpcResponse::WrongPassword) => {
                ui.set_password_status_msg("Password lama salah!".into());
            }
            Ok(IpcResponse::Error(e)) => {
                ui.set_password_status_msg(format!("Gagal: {}", e).into());
            }
            Err(e) => {
                ui.set_password_status_msg(format!("Gagal: {}", e).into());
            }
            _ => {
                ui.set_password_status_msg("Gagal terhubung ke service.".into());
            }
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_toggle_protection(move || {
        let ui = ui_weak.unwrap();
        let currently_active = ui.get_protection_active();

        if currently_active {
            match stop_service() {
                Ok(_) => {
                    ui.set_protection_active(false);
                    ui.set_protection_status_msg("✅ Service dihentikan.".into());
                }
                Err(e) => {
                    ui.set_protection_status_msg(format!("❌ Gagal: {}", e).into());
                }
            }
        } else {
            match start_service() {
                Ok(_) => {
                    ui.set_protection_active(true);
                    ui.set_protection_status_msg("✅ Service dimulai!".into());
                }
                Err(e) => {
                    ui.set_protection_status_msg(format!("❌ Gagal: {}", e).into());
                }
            }
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_check_protection_status(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_protection_active(is_service_running());
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_install_service(move || {
        let ui = ui_weak.unwrap();
        match install_service() {
            Ok(_) => {
                ui.set_service_installed(true);
                ui.set_protection_status_msg("✅ Service berhasil diinstall!".into());
                // Auto-start setelah install
                if let Ok(_) = start_service() {
                    ui.set_protection_active(true);
                }
            }
            Err(e) => {
                ui.set_protection_status_msg(format!("❌ Gagal install: {}", e).into());
            }
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_uninstall_service(move || {
        let ui = ui_weak.unwrap();
        let _ = stop_service();
        match uninstall_service() {
            Ok(_) => {
                ui.set_service_installed(false);
                ui.set_protection_active(false);
                ui.set_protection_status_msg("✅ Service berhasil di-uninstall.".into());
            }
            Err(e) => {
                ui.set_protection_status_msg(format!("❌ Gagal uninstall: {}", e).into());
            }
        }
    });

    ui.set_service_installed(is_service_installed());

    ui.run()
}

fn install_service() -> anyhow::Result<()> {
    let self_exe = env::current_exe()?;
    let bin_path = format!("\"{}\" --service", self_exe.to_string_lossy());

    let output = std::process::Command::new("sc.exe")
        .arg("create")
        .arg(SERVICE_NAME)
        .arg("binPath=")
        .arg(&bin_path)
        .arg("DisplayName=")
        .arg(SERVICE_DISPLAY_NAME)
        .arg("start=")
        .arg("auto")
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| anyhow::anyhow!("Gagal menjalankan sc.exe create: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow::anyhow!(
            "sc.exe create gagal: {} {}",
            stdout.trim(),
            stderr.trim()
        ));
    }

    let _ = std::process::Command::new("sc.exe")
        .arg("failure")
        .arg(SERVICE_NAME)
        .arg("reset=")
        .arg("86400")
        .arg("actions=")
        .arg("restart/5000/restart/10000/restart/30000")
        .creation_flags(0x08000000)
        .output();

    info!("Service berhasil diinstall");
    Ok(())
}

fn uninstall_service() -> anyhow::Result<()> {
    let self_exe = env::current_exe()?;
    let interceptor_path = self_exe.to_string_lossy().to_string();
    app_core::remove_all_ifeo(&interceptor_path).ok();

    let output = std::process::Command::new("sc.exe")
        .args(["delete", SERVICE_NAME])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| anyhow::anyhow!("Gagal menjalankan sc.exe delete: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow::anyhow!(
            "sc.exe delete gagal: {} {}",
            stdout.trim(),
            stderr.trim()
        ));
    }

    info!("Service berhasil di-uninstall");
    Ok(())
}

fn start_service() -> anyhow::Result<()> {
    let output = std::process::Command::new("sc.exe")
        .args(["start", SERVICE_NAME])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| anyhow::anyhow!("Gagal menjalankan sc.exe start: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow::anyhow!(
            "sc.exe start gagal: {} {}",
            stdout.trim(),
            stderr.trim()
        ));
    }

    Ok(())
}

fn stop_service() -> anyhow::Result<()> {
    let output = std::process::Command::new("sc.exe")
        .args(["stop", SERVICE_NAME])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| anyhow::anyhow!("Gagal menjalankan sc.exe stop: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow::anyhow!(
            "sc.exe stop gagal: {} {}",
            stdout.trim(),
            stderr.trim()
        ));
    }

    Ok(())
}

fn is_service_running() -> bool {
    let output = std::process::Command::new("sc.exe")
        .args(["query", SERVICE_NAME])
        .creation_flags(0x08000000)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.contains("RUNNING")
        }
        Err(_) => false,
    }
}

fn is_service_installed() -> bool {
    let output = std::process::Command::new("sc.exe")
        .args(["query", SERVICE_NAME])
        .creation_flags(0x08000000)
        .output();

    match output {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
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

fn send_ipc_request(request: IpcRequest) -> Result<IpcResponse, String> {
    let payload = serde_json::to_vec(&request)
        .map_err(|e| format!("Serialize error: {}", e))?;

    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_NAME)
        .map_err(|e| format!("Pipe open error: {}", e))?;

    pipe.write_all(&payload)
        .map_err(|e| format!("Pipe write error: {}", e))?;

    let mut buffer = vec![0u8; 4096];
    let bytes_read = pipe.read(&mut buffer)
        .map_err(|e| format!("Pipe read error: {}", e))?;

    serde_json::from_slice(&buffer[..bytes_read])
        .map_err(|e| format!("Deserialize error: {}", e))
}
