use app_core::{AppConfig, SERVICE_NAME};
use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{Level, error, info};
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

pub fn run_service() -> anyhow::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| anyhow::anyhow!("Gagal memulai service dispatcher: {}", e))?;
    Ok(())
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service_inner() {
        error!("Service gagal: {}", e);
    }
    std::process::exit(0);
}

fn run_service_inner() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .try_init()
        .ok();
    info!("AppLocker Service menyala...");

    // Protect process DACL to disallow unauthorized termination from Task Manager
    if let Err(e) = infra::protect_current_process() {
        tracing::warn!("Gagal menerapkan proteksi DACL proses: {}", e);
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = Arc::new(std::sync::Mutex::new(Some(shutdown_tx)));

    let shutdown_tx_clone = shutdown_tx.clone();
    let status_handle_ref = Arc::new(std::sync::Mutex::new(None::<windows_service::service_control_handler::ServiceStatusHandle>));
    let status_handle_cb = status_handle_ref.clone();

    let status_handle = service_control_handler::register(
        SERVICE_NAME,
        move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    info!("Menerima sinyal stop/shutdown...");
                    if let Some(h) = status_handle_cb.lock().unwrap().as_ref() {
                        let _ = h.set_service_status(ServiceStatus {
                            service_type: ServiceType::OWN_PROCESS,
                            current_state: ServiceState::StopPending,
                            controls_accepted: ServiceControlAccept::empty(),
                            exit_code: ServiceExitCode::Win32(0),
                            checkpoint: 1,
                            wait_hint: Duration::from_secs(3),
                            process_id: None,
                        });
                    }
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

    *status_handle_ref.lock().unwrap() = Some(status_handle);

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
        let dir = crate::exe_dir();
        let self_exe = std::env::current_exe().expect("Gagal mendapatkan path executable");
        let interceptor_path = self_exe.to_string_lossy().to_string();

        let config = Arc::new(Mutex::new(AppConfig::load(&dir)));

        {
            let cfg = config.lock().await;
            info!("Aplikasi terkunci: {:?}", cfg.locked_apps);
            if let Err(e) = infra::sync_ifeo_with_config(&cfg, &interceptor_path) {
                error!("Gagal sinkronisasi IFEO: {}", e);
            }
        }

        let recently_unlocked: Arc<Mutex<std::collections::HashMap<String, tokio::time::Instant>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));

        let config_ipc = config.clone();
        let interceptor_ipc = interceptor_path.clone();
        let unlocked_ipc = recently_unlocked.clone();
        let ipc_handle = tokio::spawn(async move {
            crate::ipc_server_loop(config_ipc, interceptor_ipc, unlocked_ipc).await;
        });

        let _ = shutdown_rx.await;
        info!("Service menerima sinyal shutdown, menghentikan IPC loop...");
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
