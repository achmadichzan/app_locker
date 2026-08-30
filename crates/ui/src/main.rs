#![windows_subsystem = "windows"]

use app_core::{AppConfig, PIPE_NAME};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{error, info};

mod cli;
mod interceptor;
mod ipc;
mod panel;
mod service;
pub mod tray;

slint::include_modules!();

pub fn exe_dir() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(app_core::get_default_config_dir)
}

pub async fn ipc_server_loop(
    config: Arc<Mutex<AppConfig>>,
    interceptor_path: String,
    recently_unlocked: Arc<Mutex<std::collections::HashMap<String, tokio::time::Instant>>>,
    failed_attempts: Arc<Mutex<std::collections::HashMap<String, (u32, tokio::time::Instant)>>>,
) {
    info!("IPC Server mendengarkan di {}", PIPE_NAME);

    loop {
        let server = match ipc::create_open_pipe() {
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
        let failed_clone = failed_attempts.clone();
        tokio::spawn(async move {
            let mut server = server;
            ipc::handle_ipc_client(&mut server, config_clone, interceptor_clone, unlocked_clone, failed_clone).await;
        });
    }
}

fn init_logging() {
    let log_path = exe_dir().join("app_locker.log");

    // ponytail: size-based log rotation at 5MB threshold
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 5 * 1024 * 1024 {
            let old_path = exe_dir().join("app_locker.log.old");
            let _ = std::fs::rename(&log_path, &old_path);
        }
    }

    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .try_init()
            .ok();
    } else {
        tracing_subscriber::fmt::try_init().ok();
    }
}

fn main() -> anyhow::Result<()> {
    init_logging();

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a.starts_with("--type=")) {
        tracing::debug!("App Locker dimulai dengan argumen: {:?}", args);
    } else {
        info!("App Locker dimulai dengan argumen: {:?}", args);
    }

    match cli::parse_cli_args(&args) {
        cli::AppMode::Service => service::run_service(),
        cli::AppMode::Interceptor {
            app_name,
            app_path,
            forward_args,
        } => interceptor::run_interceptor(&app_name, &app_path, forward_args),
        cli::AppMode::ManagementPanel => {
            panel::run_management_panel().map_err(|e| anyhow::anyhow!(e))
        }
    }
}
