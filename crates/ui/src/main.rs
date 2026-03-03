#![windows_subsystem = "windows"]

use app_core::{AppConfig, IpcRequest, IpcResponse, PIPE_NAME, ProcessMonitor};
use infra::WindowsProcessManager;
use slint::Model;
use std::collections::HashSet;
use std::env;
use std::fs::OpenOptions;
use std::io::{Read, Write};
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
        Some("--daemon") => run_daemon(),
        Some("--watchdog") => run_watchdog(),
        Some("--prompt") => {
            // --prompt <pid> <name>
            run_lock_prompt(&args[2..]).map_err(|e| anyhow::anyhow!(e))
        }
        _ => run_management_panel().map_err(|e| anyhow::anyhow!(e)),
    }
}

fn run_daemon() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();
    info!("Daemon App Locker menyala...");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let dir = exe_dir();
        let monitor = Arc::new(WindowsProcessManager::new());
        let suspended_pids = Arc::new(Mutex::new(HashSet::<u32>::new()));
        let unlocked_apps = Arc::new(Mutex::new(HashSet::<String>::new()));
        let prompting_apps = Arc::new(Mutex::new(HashSet::<String>::new()));

        let config = Arc::new(Mutex::new(AppConfig::load(&dir)));
        {
            let cfg = config.lock().await;
            info!("Aplikasi terkunci: {:?}", cfg.locked_apps);
        }

        let monitor_ipc = monitor.clone();
        let pids_ipc = suspended_pids.clone();
        let unlocked_ipc = unlocked_apps.clone();
        let prompting_ipc = prompting_apps.clone();
        let config_ipc = config.clone();
        tokio::spawn(async move {
            run_ipc_server(monitor_ipc, pids_ipc, unlocked_ipc, prompting_ipc, config_ipc).await;
        });

        let config_reload = config.clone();
        let dir_reload = dir.clone();
        let unlocked_reload = unlocked_apps.clone();
        let monitor_reload = monitor.clone();

        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(3)).await;
                let new_config = AppConfig::load(&dir_reload);
                let mut cfg = config_reload.lock().await;

                if cfg.locked_apps != new_config.locked_apps {
                    let  baru_dikunci: Vec<String> = new_config
                        .locked_apps
                        .iter()
                        .filter(|app| !cfg.locked_apps.contains(app))
                        .map(|s| s.to_lowercase())
                        .collect();

                    if !baru_dikunci.is_empty() {
                        let proc_list = monitor_reload.get_running_processes();
                        let active_names: HashSet<String> = proc_list
                            .into_iter()
                            .map(|p| p.name.to_lowercase())
                            .collect();

                        let mut unlocked = unlocked_reload.lock().await;
                        for nama in baru_dikunci {
                            if active_names.contains(&nama) {
                                unlocked.insert(nama);
                            }
                        }
                    }

                    info!("Config ter-update! Aplikasi terkunci: {:?}", new_config.locked_apps);
                    *cfg = new_config;
                }
            }
        });

        let self_exe = std::env::current_exe().expect("Gagal mendapatkan path executable");

        loop {
            let processes = monitor.get_running_processes();
            let mut pids_lock = suspended_pids.lock().await;
            let mut prompting_lock = prompting_apps.lock().await;

            let active_apps: HashSet<String> =
                processes.iter().map(|p| p.name.to_lowercase()).collect();
            let active_pids: HashSet<u32> = processes.iter().map(|p| p.pid).collect();

            unlocked_apps
                .lock()
                .await
                .retain(|app| active_apps.contains(app));
            prompting_lock.retain(|app| active_apps.contains(app));
            pids_lock.retain(|pid| active_pids.contains(pid));

            let target_apps: Vec<String> = config.lock().await.locked_apps.clone();

            for process in processes {
                let proc_name = process.name.to_lowercase();
                let pid = process.pid;

                if target_apps.iter().any(|t| t.to_lowercase() == proc_name)
                    && !pids_lock.contains(&pid)
                    && !unlocked_apps.lock().await.contains(&proc_name)
                {
                    info!("Mencegat aplikasi: {} (PID: {})", process.name, pid);

                    match monitor.suspend_process(pid) {
                        Ok(_) => {
                            pids_lock.insert(pid);

                            if !prompting_lock.contains(&proc_name) {
                                prompting_lock.insert(proc_name.clone());

                                match std::process::Command::new(&self_exe)
                                    .arg("--prompt")
                                    .arg(pid.to_string())
                                    .arg(&proc_name)
                                    .spawn()
                                {
                                    Ok(_) => {
                                        info!("UI prompt diluncurkan untuk {}", proc_name);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Gagal meluncurkan UI: {}. Melepas PID {} kembali.",
                                            e, pid
                                        );
                                        let _ = monitor.resume_process(pid);
                                        pids_lock.remove(&pid);
                                        prompting_lock.remove(&proc_name);
                                    }
                                }
                            } else {
                                info!(
                                    "Aplikasi {} (PID: {}) ditangguhkan (menunggu prompt selesai)",
                                    proc_name, pid
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                "Gagal menangguhkan {} (PID: {}): {}. Kemungkinan butuh akses Administrator.",
                                proc_name, pid, e
                            );
                            pids_lock.insert(pid);
                        }
                    }
                }
            }

            drop(pids_lock);
            drop(prompting_lock);
            sleep(Duration::from_millis(150)).await;
        }
    })
}

async fn run_ipc_server(
    monitor: Arc<WindowsProcessManager>,
    suspended_pids: Arc<Mutex<HashSet<u32>>>,
    unlocked_apps: Arc<Mutex<HashSet<String>>>,
    prompting_apps: Arc<Mutex<HashSet<String>>>,
    config: Arc<Mutex<AppConfig>>,
) {
    info!("IPC Server mendengarkan di {}", PIPE_NAME);

    loop {
        let mut server = match ServerOptions::new().create(PIPE_NAME) {
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

        info!("UI Client terhubung ke pipe!");

        let monitor_clone = monitor.clone();
        let pids_clone = suspended_pids.clone();
        let unlocked_clone = unlocked_apps.clone();
        let prompting_clone = prompting_apps.clone();
        let config_clone = config.clone();
        tokio::spawn(async move {
            handle_ipc_client(
                &mut server,
                monitor_clone,
                pids_clone,
                unlocked_clone,
                prompting_clone,
                config_clone,
            )
            .await;
        });
    }
}

async fn handle_ipc_client(
    server: &mut tokio::net::windows::named_pipe::NamedPipeServer,
    monitor: Arc<WindowsProcessManager>,
    suspended_pids: Arc<Mutex<HashSet<u32>>>,
    unlocked_apps: Arc<Mutex<HashSet<String>>>,
    prompting_apps: Arc<Mutex<HashSet<String>>>,
    config: Arc<Mutex<AppConfig>>,
) {
    let mut buffer = vec![0u8; 1024];
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
        IpcRequest::UnlockApp { app_name, password } => {
            let cfg = config.lock().await;

            if password == cfg.password {
                unlocked_apps.lock().await.insert(app_name.clone());
                prompting_apps.lock().await.remove(&app_name);

                let processes = monitor.get_running_processes();
                for p in processes {
                    if p.name.to_lowercase() == app_name.to_lowercase() {
                        let mut lock = suspended_pids.lock().await;
                        if lock.contains(&p.pid) {
                            let _ = monitor.resume_process(p.pid);
                            lock.remove(&p.pid);
                        }
                    }
                }

                info!("Aplikasi dilepaskan untuk {}", app_name);
                IpcResponse::Success
            } else {
                warn!("Password salah untuk {}", app_name);
                IpcResponse::WrongPassword
            }
        }
        IpcRequest::CancelUnlock { app_name } => {
            let processes = monitor.get_running_processes();
            let mut pids_to_kill = Vec::new();

            for p in processes {
                if p.name.to_lowercase() == app_name.to_lowercase() {
                    let lock = suspended_pids.lock().await;
                    if lock.contains(&p.pid) {
                        pids_to_kill.push(p.pid);
                    }
                }
            }

            for pid in pids_to_kill {
                let _ = monitor.kill_process(pid);
            }

            prompting_apps.lock().await.remove(&app_name);
            info!("Aplikasi dimatikan untuk {}", app_name);
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

fn run_watchdog() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();
    info!("Watchdog App Locker menyala...");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let self_exe = std::env::current_exe().expect("Gagal mendapatkan path executable");

        const RESTART_DELAY: Duration = Duration::from_secs(2);

        loop {
            info!("Memulai daemon: {:?} --daemon", self_exe);

            let status = tokio::process::Command::new(&self_exe)
                .arg("--daemon")
                .status()
                .await;

            match status {
                Ok(exit_status) => {
                    warn!(
                        "Daemon berhenti dengan kode: {:?}. Restart dalam {} detik...",
                        exit_status.code(),
                        RESTART_DELAY.as_secs()
                    );
                }
                Err(e) => {
                    error!(
                        "Gagal menjalankan daemon: {}. Retry dalam {} detik...",
                        e,
                        RESTART_DELAY.as_secs()
                    );
                }
            }

            sleep(RESTART_DELAY).await;
        }
    })
}

fn run_management_panel() -> Result<(), slint::PlatformError> {
    let ui = ManagementPanel::new()?;
    center_window(ui.window(), 480.0, 520.0);

    let config = AppConfig::load(&exe_dir());
    refresh_app_list(&ui, &config);

    let is_active = is_daemon_running();
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
    ui.on_fetch_running_apps(move || {
        let mut sys = sysinfo::System::new_all();
        sys.refresh_processes();

        let mut app_names: Vec<String> = sys
            .processes()
            .values()
            .map(|p| p.name().to_string())
            .filter(|name: &String| name.to_lowercase().ends_with(".exe"))
            .collect();

        app_names.sort();
        app_names.dedup();

        if let Some(ui) = ui_weak.upgrade() {
            let model = std::rc::Rc::new(slint::VecModel::from(
                app_names
                    .into_iter()
                    .map(slint::SharedString::from)
                    .collect::<Vec<_>>(),
            ));
            ui.set_running_apps(model.into());
            ui.set_show_running_apps(!ui.get_show_running_apps());
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_select_running_app(move |app_name| {
        if let Some(ui) = ui_weak.upgrade() {
            ui.set_new_app_name(app_name);
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
            Some(IpcResponse::PasswordChanged) => {
                ui.set_password_status_msg("✅ Password berhasil diubah!".into());
                ui.set_old_password("".into());
                ui.set_new_password("".into());
                ui.set_confirm_password("".into());
            }
            Some(IpcResponse::WrongPassword) => {
                ui.set_password_status_msg("Password lama salah!".into());
            }
            Some(IpcResponse::Error(e)) => {
                ui.set_password_status_msg(format!("Gagal: {}", e).into());
            }
            _ => {
                ui.set_password_status_msg("Gagal terhubung ke daemon.".into());
            }
        }
    });

    ui.set_startup_enabled(app_core::is_startup_enabled());

    let ui_weak = ui.as_weak();
    ui.on_toggle_startup(move || {
        let ui = ui_weak.unwrap();
        let new_state = !ui.get_startup_enabled();

        match app_core::set_startup_enabled(new_state) {
            Ok(_) => {
                ui.set_startup_enabled(new_state);
                if new_state {
                    ui.set_startup_status_msg("✅ Startup diaktifkan!".into());
                } else {
                    ui.set_startup_status_msg("✅ Startup dinonaktifkan.".into());
                }
            }
            Err(e) => {
                ui.set_startup_status_msg(format!("❌ Gagal: {}", e).into());
            }
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_toggle_protection(move || {
        let ui = ui_weak.unwrap();
        let currently_active = ui.get_protection_active();

        if currently_active {
            stop_protection();
            ui.set_protection_active(false);
            ui.set_protection_status_msg("✅ Proteksi dinonaktifkan.".into());
        } else {
            match start_protection() {
                Ok(_) => {
                    ui.set_protection_active(true);
                    ui.set_protection_status_msg("✅ Proteksi diaktifkan!".into());
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
            ui.set_protection_active(is_daemon_running());
        }
    });

    ui.run()
}

fn start_protection() -> anyhow::Result<()> {
    let self_exe = std::env::current_exe()?;

    std::process::Command::new(&self_exe)
        .arg("--watchdog")
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| anyhow::anyhow!("Gagal memulai watchdog: {}", e))?;

    Ok(())
}

fn stop_protection() {
    let current_pid = std::process::id();
    let mut sys = sysinfo::System::new_all();
    sys.refresh_processes();

    let self_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase().to_string()))
        .unwrap_or_default();

    for (pid, process) in sys.processes() {
        if pid.as_u32() != current_pid && process.name().to_lowercase() == self_name {
            process.kill();
        }
    }
}

fn is_daemon_running() -> bool {
    let current_pid = std::process::id();
    let mut sys = sysinfo::System::new_all();
    sys.refresh_processes();

    let self_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase().to_string()))
        .unwrap_or_default();

    for (pid, process) in sys.processes() {
        if pid.as_u32() != current_pid && process.name().to_lowercase() == self_name {
            return true;
        }
    }

    false
}

fn run_lock_prompt(args: &[String]) -> Result<(), slint::PlatformError> {
    let _target_pid: u32 = args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let target_name = args
        .get(1)
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

use std::os::windows::process::CommandExt;
