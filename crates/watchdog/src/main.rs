use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{Level, error, info, warn};

const RESTART_DELAY: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();
    info!("Watchdog App Locker menyala...");

    let daemon_path = std::env::current_exe()?
        .parent()
        .expect("Tidak dapat menemukan parent directory")
        .join("daemon.exe");

    if !daemon_path.exists() {
        error!("daemon.exe tidak ditemukan di: {:?}", daemon_path);
        return Err(anyhow::anyhow!("daemon.exe tidak ditemukan"));
    }

    loop {
        info!("Memulai daemon: {:?}", daemon_path);

        let status = Command::new(&daemon_path).status().await;

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
}
