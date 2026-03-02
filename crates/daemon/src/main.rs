use app_core::{AppConfig, IpcRequest, IpcResponse, PIPE_NAME, ProcessMonitor};
use infra::WindowsProcessManager;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{Level, error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();
    info!("Daemon App Locker menyala...");

    let exe_dir = std::env::current_exe()?
        .parent()
        .expect("Tidak dapat menemukan parent directory")
        .to_path_buf();

    let monitor = Arc::new(WindowsProcessManager::new());
    let suspended_pids = Arc::new(Mutex::new(HashSet::<u32>::new()));
    let unlocked_apps = Arc::new(Mutex::new(HashSet::<String>::new()));
    let prompting_apps = Arc::new(Mutex::new(HashSet::<String>::new()));

    let config = Arc::new(Mutex::new(AppConfig::load(&exe_dir)));
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
        run_ipc_server(
            monitor_ipc,
            pids_ipc,
            unlocked_ipc,
            prompting_ipc,
            config_ipc,
        )
        .await;
    });

    let config_reload = config.clone();
    let exe_dir_reload = exe_dir.clone();
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(3)).await;
            let new_config = AppConfig::load(&exe_dir_reload);
            let mut cfg = config_reload.lock().await;
            if cfg.locked_apps != new_config.locked_apps {
                info!(
                    "Config ter-update! Aplikasi terkunci: {:?}",
                    new_config.locked_apps
                );
                *cfg = new_config;
            }
        }
    });

    let ui_path = exe_dir.join("ui.exe");

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

                            match std::process::Command::new(&ui_path)
                                .arg(pid.to_string())
                                .arg(&proc_name)
                                .spawn()
                            {
                                Ok(_) => {
                                    info!("UI prompt diluncurkan untuk {}", proc_name);
                                }
                                Err(e) => {
                                    error!("Gagal meluncurkan UI: {}. Melepas PID {} kembali.", e, pid);
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
                        error!("Gagal menangguhkan {} (PID: {}): {}. Kemungkinan butuh akses Administrator.", proc_name, pid, e);
                        pids_lock.insert(pid);
                    }
                }
            }
        }

        drop(pids_lock);
        drop(prompting_lock);
        sleep(Duration::from_millis(150)).await;
    }
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

            let processes = monitor.get_running_processes();
            let mut pids_to_resume = Vec::new();

            for p in processes {
                if p.name.to_lowercase() == app_name.to_lowercase() {
                    let mut lock = suspended_pids.lock().await;
                    if lock.contains(&p.pid) {
                        pids_to_resume.push(p.pid);
                        lock.remove(&p.pid);
                    }
                }
            }

            if password == cfg.password {
                for pid in pids_to_resume {
                    let _ = monitor.resume_process(pid);
                }

                unlocked_apps.lock().await.insert(app_name.clone());
                prompting_apps.lock().await.remove(&app_name);
                info!("Aplikasi dilepaskan untuk {}", app_name);
                IpcResponse::Success
            } else {
                warn!("Password salah untuk {}", app_name);

                let mut lock = suspended_pids.lock().await;
                for pid in pids_to_resume {
                    lock.insert(pid);
                }
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
    };

    if let Ok(response_bytes) = serde_json::to_vec(&response) {
        let _ = server.write_all(&response_bytes).await;
    }
}
