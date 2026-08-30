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

slint::include_modules!();

pub fn exe_dir() -> PathBuf {
    env::current_exe()
        .unwrap()
        .parent()
        .expect("Tidak dapat menemukan parent directory")
        .to_path_buf()
}

pub async fn ipc_server_loop(
    config: Arc<Mutex<AppConfig>>,
    interceptor_path: String,
    recently_unlocked: Arc<Mutex<std::collections::HashMap<String, tokio::time::Instant>>>,
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
        tokio::spawn(async move {
            let mut server = server;
            ipc::handle_ipc_client(&mut server, config_clone, interceptor_clone, unlocked_clone).await;
        });
    }
}

fn init_logging() {
    let log_path = exe_dir().join("app_locker.log");
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
    info!("App Locker dimulai dengan argumen: {:?}", env::args().collect::<Vec<_>>());

    let args: Vec<String> = env::args().collect();
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
