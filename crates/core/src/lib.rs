use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub trait ProcessMonitor: Send + Sync {
    fn get_running_processes(&self) -> Vec<ProcessInfo>;
    fn suspend_process(&self, pid: u32) -> Result<()>;
    fn resume_process(&self, pid: u32) -> Result<()>;
    fn kill_process(&self, pid: u32) -> Result<()>;

    fn close_process_gracefully(&self, pid: u32) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
}

pub const PIPE_NAME: &str = r"\\.\pipe\applocker_pipe";

pub const CONFIG_FILENAME: &str = "config.json";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub locked_apps: Vec<String>,
    pub password: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            locked_apps: vec!["notepad.exe".to_string()],
            password: "rust2026".to_string(),
        }
    }
}

impl AppConfig {
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(CONFIG_FILENAME);
        if let Ok(data) = std::fs::read_to_string(&path) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            let config = Self::default();
            config.save(config_dir).ok();
            config
        }
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        let path = config_dir.join(CONFIG_FILENAME);
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum IpcRequest {
    UnlockApp { app_name: String, password: String },
    CancelUnlock { app_name: String },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum IpcResponse {
    Success,
    WrongPassword,
    Error(String),
}
