use app_core::{IpcRequest, IpcResponse};
use slint::ComponentHandle;
use tracing::{error, info, warn};

pub fn run_interceptor(
    app_name: &str,
    app_path: &str,
    forward_args: Vec<String>,
) -> anyhow::Result<()> {
    let is_child_process = forward_args.iter().any(|arg| arg.starts_with("--type="));
    if is_child_process {
        tracing::debug!(
            "Auto-approving child process launch for {} ({:?})",
            app_name,
            forward_args
        );
        let mut cmd = std::process::Command::new(app_path);
        if !forward_args.is_empty() {
            cmd.args(&forward_args);
        }
        let _ = cmd.spawn();
        return Ok(());
    }

    let ui = crate::AppPrompt::new().map_err(|e| anyhow::anyhow!(e))?;
    ui.set_target_name(app_name.into());
    crate::panel::center_window(ui.window(), 380.0, 260.0);

    let app_name_cancel = app_name.to_string();
    ui.on_cancel_requested(move || {
        let _ = crate::ipc::send_ipc_request(IpcRequest::CancelLaunch {
            app_name: app_name_cancel.clone(),
        });
        let _ = slint::quit_event_loop();
    });

    let ui_weak = ui.as_weak();
    let app_name_unlock = app_name.to_string();
    let app_path_unlock = app_path.to_string();
    let forward_args_unlock = forward_args;
    ui.on_unlock_requested(move |password| {
        let ui_handle = ui_weak.clone();
        let app_name = app_name_unlock.clone();
        let app_path = app_path_unlock.clone();
        let forward_args = forward_args_unlock.clone();
        let password_str = password.to_string();

        info!("Tombol Buka Kunci ditekan untuk {}", app_name);

        std::thread::spawn(move || {
            let request = IpcRequest::LaunchApp {
                app_name: app_name.clone(),
                app_path: app_path.clone(),
                password: password_str.clone(),
            };

            // 1. If service is not running, try starting it
            if !infra::is_service_running(app_core::SERVICE_NAME) {
                info!("Service terdeteksi tidak berjalan, mencoba start_service...");
                let _ = infra::start_service(app_core::SERVICE_NAME);
                for _ in 0..15 {
                    if infra::is_service_running(app_core::SERVICE_NAME) {
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }

            info!("Mengirim request IPC LaunchApp ke service...");
            let mut response = crate::ipc::send_ipc_request(request.clone());
            info!("Hasil respon IPC pertama: {:?}", response);

            if response.is_err() && !infra::is_service_running(app_core::SERVICE_NAME) {
                warn!("Service offline setelah IPC pertama, mencoba start ulang...");
                let _ = infra::start_service(app_core::SERVICE_NAME);
                std::thread::sleep(std::time::Duration::from_millis(300));
                response = crate::ipc::send_ipc_request(request);
                info!("Hasil respon IPC kedua: {:?}", response);
            }

            match response {
                Ok(IpcResponse::Success) => {
                    info!("IPC Success diterima, meluncurkan {}...", app_path);
                    let mut cmd = std::process::Command::new(&app_path);
                    if !forward_args.is_empty() {
                        cmd.args(&forward_args);
                    }
                    match cmd.spawn() {
                        Ok(child) => {
                            info!("Aplikasi berhasil diluncurkan dengan PID: {}", child.id());
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = ui_handle.upgrade() {
                                    let _ = ui.hide();
                                }
                                let _ = slint::quit_event_loop();
                            });
                        }
                        Err(e) => {
                            error!("Gagal meluncurkan aplikasi: {}", e);
                            let msg = format!("Gagal meluncurkan aplikasi: {}", e);
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = ui_handle.upgrade() {
                                    ui.set_is_loading(false);
                                    ui.set_error_msg(msg.into());
                                }
                            });
                        }
                    }
                }
                Ok(IpcResponse::WrongPassword) => {
                    warn!("Password salah diterima dari service");
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_handle.upgrade() {
                            ui.set_is_loading(false);
                            ui.set_error_msg("Password salah! Coba lagi.".into());
                        }
                    });
                }
                Ok(other) => {
                    warn!("Respon tak terduga diterima: {:?}", other);
                    let msg = format!("Terjadi kesalahan: {:?}", other);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_handle.upgrade() {
                            ui.set_is_loading(false);
                            ui.set_error_msg(msg.into());
                        }
                    });
                }
                Err(e) => {
                    error!("Gagal total komunikasi IPC dengan service: {}", e);
                    let msg = format!("Service tidak merespons ({}). Buka Management Panel.", e);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_handle.upgrade() {
                            ui.set_is_loading(false);
                            ui.set_error_msg(msg.into());
                        }
                    });
                }
            }
        });
    });

    ui.run().map_err(|e| anyhow::anyhow!(e))
}
