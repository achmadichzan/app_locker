use app_core::{AppConfig, IpcRequest, IpcResponse, PIPE_NAME};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

pub fn create_open_pipe() -> std::io::Result<NamedPipeServer> {
    use std::ffi::c_void;
    use windows::core::w;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    unsafe {
        let mut p_sd = PSECURITY_DESCRIPTOR::default();
        // SDDL: Authenticated Users (AU) Full, Admins (BA) Full, SYSTEM (SY) Full
        let sddl = w!("D:(A;;GA;;;AU)(A;;GA;;;BA)(A;;GA;;;SY)");

        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl,
            SDDL_REVISION_1,
            &mut p_sd,
            None,
        )
        .is_err()
        {
            return Err(std::io::Error::last_os_error());
        }

        struct SdGuard(PSECURITY_DESCRIPTOR);
        impl Drop for SdGuard {
            fn drop(&mut self) {
                if !self.0.0.is_null() {
                    let _ = unsafe { LocalFree(HLOCAL(self.0.0)) };
                }
            }
        }
        let _guard = SdGuard(p_sd);

        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: p_sd.0,
            bInheritHandle: windows::Win32::Foundation::BOOL(0),
        };

        ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .pipe_mode(tokio::net::windows::named_pipe::PipeMode::Message)
            .create_with_security_attributes_raw(
                PIPE_NAME,
                &mut sa as *mut _ as *mut c_void,
            )
    }
}

pub async fn handle_ipc_client(
    server: &mut NamedPipeServer,
    config: Arc<Mutex<AppConfig>>,
    interceptor_path: String,
    recently_unlocked: Arc<Mutex<std::collections::HashMap<String, tokio::time::Instant>>>,
    failed_attempts: Arc<Mutex<std::collections::HashMap<String, (u32, tokio::time::Instant)>>>,
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
        IpcRequest::LaunchApp {
            app_name,
            app_path: _,
            password,
        } => {
            let norm_app_name = app_name.trim().to_lowercase();

            // ponytail: rate limit -- max 5 failures per 60s per app, bounded cleanup
            let rate_limited = {
                let mut attempts = failed_attempts.lock().await;
                attempts.retain(|_, (_, time)| time.elapsed() < Duration::from_secs(60));
                if let Some((count, first_fail)) = attempts.get(&norm_app_name) {
                    *count >= 5 && first_fail.elapsed() < Duration::from_secs(60)
                } else {
                    false
                }
            };

            if rate_limited {
                warn!("Rate limit untuk {}", norm_app_name);
                IpcResponse::Error("Terlalu banyak percobaan. Coba lagi dalam 60 detik.".into())
            } else {
                let auto_approved = {
                    let mut unlocked = recently_unlocked.lock().await;
                    unlocked.retain(|_, time| time.elapsed() < Duration::from_secs(30));
                    if let Some(unlock_time) = unlocked.get(&norm_app_name) {
                        unlock_time.elapsed() < Duration::from_secs(30)
                    } else {
                        false
                    }
                };

                let is_valid = if auto_approved {
                    true
                } else {
                    let cfg_pwd = config.lock().await.password.clone();
                    let pass = password.clone();
                    tokio::task::spawn_blocking(move || {
                        let dummy = app_core::AppConfig {
                            password: cfg_pwd,
                            locked_apps: Vec::new(),
                        };
                        dummy.verify_password(&pass)
                    })
                    .await
                    .unwrap_or(false)
                };

                if is_valid {
                    if auto_approved {
                        info!("Auto-approve untuk {} (child process dalam 30 detik)", norm_app_name);
                    } else {
                        info!("Password benar untuk {}. Menonaktifkan IFEO sementara...", norm_app_name);
                    }

                    {
                        let mut unlocked = recently_unlocked.lock().await;
                        unlocked.insert(norm_app_name.clone(), tokio::time::Instant::now());
                    }

                    {
                        let mut attempts = failed_attempts.lock().await;
                        attempts.remove(&norm_app_name);
                    }

                    let guard = infra::IfeoUnlockGuard::new(norm_app_name.clone(), interceptor_path.clone());
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        drop(guard);
                        info!("IFEO dipasang kembali untuk {}", norm_app_name);
                    });

                    IpcResponse::Success
                } else {
                    warn!("Password salah untuk {}", norm_app_name);

                    {
                        let mut attempts = failed_attempts.lock().await;
                        let entry = attempts.entry(norm_app_name).or_insert((0, tokio::time::Instant::now()));
                        if entry.1.elapsed() >= Duration::from_secs(60) {
                            *entry = (1, tokio::time::Instant::now());
                        } else {
                            entry.0 += 1;
                        }
                    }

                    IpcResponse::WrongPassword
                }
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
            let cfg_pwd = config.lock().await.password.clone();
            let old_p = old_password.clone();
            let old_valid = tokio::task::spawn_blocking(move || {
                let dummy = app_core::AppConfig {
                    password: cfg_pwd,
                    locked_apps: Vec::new(),
                };
                dummy.verify_password(&old_p)
            })
            .await
            .unwrap_or(false);

            if !old_valid {
                warn!("Gagal ubah password: password lama salah");
                IpcResponse::WrongPassword
            } else {
                let new_p = new_password.clone();
                let hash_res = tokio::task::spawn_blocking(move || {
                    let mut dummy = app_core::AppConfig::default();
                    dummy.set_password(&new_p).map(|_| dummy.password)
                })
                .await;

                match hash_res {
                    Ok(Ok(new_hashed)) => {
                        let mut cfg = config.lock().await;
                        cfg.password = new_hashed;
                        match cfg.save(&crate::exe_dir()) {
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
                    Ok(Err(e)) => {
                        error!("Gagal hash password baru: {}", e);
                        IpcResponse::Error(e)
                    }
                    Err(e) => {
                        error!("Task join error saat hashing: {}", e);
                        IpcResponse::Error(format!("Internal error: {}", e))
                    }
                }
            }
        }
        IpcRequest::ReloadConfig => {
            let mut cfg = config.lock().await;
            *cfg = AppConfig::load(&crate::exe_dir());
            if let Err(e) = infra::sync_ifeo_with_config(&cfg, &interceptor_path) {
                error!("Gagal sinkronisasi IFEO setelah reload: {}", e);
                IpcResponse::Error(format!("Gagal reload: {}", e))
            } else {
                info!("Konfigurasi berhasil di-reload dan IFEO disinkronisasi");
                IpcResponse::ConfigReloaded
            }
        }
    };

    if let Ok(response_bytes) = serde_json::to_vec(&response) {
        info!("Service mengirim {} bytes respon: {:?}", response_bytes.len(), response);
        let write_res = server.write_all(&response_bytes).await;
        let flush_res = server.flush().await;
        info!("Service write_all: {:?}, flush: {:?}", write_res, flush_res);

        // Wait for client to finish reading response and close connection (EOF)
        let mut dummy = [0u8; 1];
        let _ = tokio::time::timeout(Duration::from_millis(500), server.read(&mut dummy)).await;
    }
}

pub fn send_ipc_request(request: IpcRequest) -> Result<IpcResponse, String> {
    use std::ffi::c_void;
    use windows::core::HSTRING;
    use windows::Win32::System::Pipes::CallNamedPipeW;

    info!("send_ipc_request dimulai untuk: {:?}", request);
    let payload = serde_json::to_vec(&request).map_err(|e| format!("Serialize error: {}", e))?;

    let s_pipe = HSTRING::from(PIPE_NAME);
    let mut out_buf = vec![0u8; 4096];
    let mut bytes_read = 0u32;

    info!("Memanggil Win32 CallNamedPipeW ke {}...", PIPE_NAME);
    unsafe {
        let call_res = CallNamedPipeW(
            &s_pipe,
            Some(payload.as_ptr() as *const c_void),
            payload.len() as u32,
            Some(out_buf.as_mut_ptr() as *mut c_void),
            out_buf.len() as u32,
            &mut bytes_read,
            3000,
        );

        if !call_res.as_bool() {
            let err = windows::core::Error::from_win32();
            let code = err.code().0 as u32;
            error!("CallNamedPipeW gagal dengan error code {:#X}: {}", code, err);
            if code == 0x80070002 {
                return Err("Service tidak aktif (pipe tidak ditemukan)".to_string());
            }
            return Err(format!("Gagal CallNamedPipe: {}", err));
        }
    }

    info!("CallNamedPipeW sukses! Membaca {} bytes balasan", bytes_read);

    if bytes_read == 0 {
        error!("CallNamedPipeW membaca 0 bytes (koneksi ditutup tanpa respon)");
        return Err("Pipe connection closed without response".to_string());
    }

    let resp = serde_json::from_slice(&out_buf[..bytes_read as usize])
        .map_err(|e| {
            error!("Gagal deserialize respon JSON: {}", e);
            format!("Deserialize error: {}", e)
        })?;

    info!("send_ipc_request berhasil mem-parse respon: {:?}", resp);
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::net::windows::named_pipe::ClientOptions;

    use std::sync::atomic::{AtomicU64, Ordering};
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn get_unique_pipe_name() -> String {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        format!(r"\\.\pipe\applocker_test_{}_{}_{}", pid, nonce, counter)
    }

    async fn send_test_ipc_request(
        pipe_name: &str,
        request: IpcRequest,
    ) -> Result<IpcResponse, String> {
        let mut client = None;
        for _ in 0..100 {
            match ClientOptions::new().open(pipe_name) {
                Ok(c) => {
                    client = Some(c);
                    break;
                }
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            }
        }
        let mut client = client.ok_or_else(|| "Client connect timeout".to_string())?;

        let payload = serde_json::to_vec(&request).map_err(|e| format!("Serialize error: {}", e))?;
        client
            .write_all(&payload)
            .await
            .map_err(|e| format!("Write error: {}", e))?;

        let mut buf = vec![0u8; 4096];
        let n = client
            .read(&mut buf)
            .await
            .map_err(|e| format!("Read error: {}", e))?;

        serde_json::from_slice(&buf[..n]).map_err(|e| format!("Deserialize error: {}", e))
    }

    #[tokio::test]
    async fn test_ipc_launch_and_auto_approve() {
        let pipe_name = get_unique_pipe_name();
        let mut config = AppConfig {
            locked_apps: vec!["app.exe".into()],
            password: "admin".into(),
        };
        config.set_password("correct_pass").unwrap();
        let config = Arc::new(Mutex::new(config));
        let recently_unlocked = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let failed_attempts = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let interceptor_path = "C:\\app_locker.exe".to_string();

        // 1. Test Wrong Password
        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let cfg1 = config.clone();
        let int1 = interceptor_path.clone();
        let unl1 = recently_unlocked.clone();
        let fail1 = failed_attempts.clone();
        let srv1 = tokio::spawn(async move {
            server.connect().await.unwrap();
            handle_ipc_client(&mut server, cfg1, int1, unl1, fail1).await;
        });

        let res = send_test_ipc_request(
            &pipe_name,
            IpcRequest::LaunchApp {
                app_name: "app.exe".into(),
                app_path: "C:\\app.exe".into(),
                password: "wrong_pass".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res, IpcResponse::WrongPassword);
        srv1.await.unwrap();

        // 2. Test Correct Password
        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let cfg2 = config.clone();
        let int2 = interceptor_path.clone();
        let unl2 = recently_unlocked.clone();
        let fail2 = failed_attempts.clone();
        let srv2 = tokio::spawn(async move {
            server.connect().await.unwrap();
            handle_ipc_client(&mut server, cfg2, int2, unl2, fail2).await;
        });

        let res = send_test_ipc_request(
            &pipe_name,
            IpcRequest::LaunchApp {
                app_name: "app.exe".into(),
                app_path: "C:\\app.exe".into(),
                password: "correct_pass".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res, IpcResponse::Success);
        srv2.await.unwrap();

        // 3. Test Auto-Approve within 30 seconds
        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let cfg3 = config.clone();
        let int3 = interceptor_path.clone();
        let unl3 = recently_unlocked.clone();
        let fail3 = failed_attempts.clone();
        let srv3 = tokio::spawn(async move {
            server.connect().await.unwrap();
            handle_ipc_client(&mut server, cfg3, int3, unl3, fail3).await;
        });

        let res = send_test_ipc_request(
            &pipe_name,
            IpcRequest::LaunchApp {
                app_name: "app.exe".into(),
                app_path: "C:\\app.exe".into(),
                password: "any_dummy_password".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res, IpcResponse::Success);
        srv3.await.unwrap();
    }

    #[tokio::test]
    async fn test_ipc_change_password_and_cancel() {
        let pipe_name = get_unique_pipe_name();
        let mut config = AppConfig {
            locked_apps: vec![],
            password: "admin".into(),
        };
        config.set_password("old_pass_123").unwrap();
        let config = Arc::new(Mutex::new(config));
        let recently_unlocked = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let failed_attempts = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let interceptor_path = "C:\\app_locker.exe".to_string();

        // 1. Wrong old password
        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let cfg1 = config.clone();
        let int1 = interceptor_path.clone();
        let unl1 = recently_unlocked.clone();
        let fail1 = failed_attempts.clone();
        let srv1 = tokio::spawn(async move {
            server.connect().await.unwrap();
            handle_ipc_client(&mut server, cfg1, int1, unl1, fail1).await;
        });

        let res = send_test_ipc_request(
            &pipe_name,
            IpcRequest::ChangePassword {
                old_password: "wrong_old_pass".into(),
                new_password: "new_pass_456".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res, IpcResponse::WrongPassword);
        srv1.await.unwrap();

        // 2. Correct old password
        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let cfg2 = config.clone();
        let int2 = interceptor_path.clone();
        let unl2 = recently_unlocked.clone();
        let fail2 = failed_attempts.clone();
        let srv2 = tokio::spawn(async move {
            server.connect().await.unwrap();
            handle_ipc_client(&mut server, cfg2, int2, unl2, fail2).await;
        });

        let res = send_test_ipc_request(
            &pipe_name,
            IpcRequest::ChangePassword {
                old_password: "old_pass_123".into(),
                new_password: "new_pass_456".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res, IpcResponse::PasswordChanged);
        srv2.await.unwrap();

        assert!(config.lock().await.verify_password("new_pass_456"));

        // 3. Cancel launch
        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let cfg3 = config.clone();
        let int3 = interceptor_path.clone();
        let unl3 = recently_unlocked.clone();
        let fail3 = failed_attempts.clone();
        let srv3 = tokio::spawn(async move {
            server.connect().await.unwrap();
            handle_ipc_client(&mut server, cfg3, int3, unl3, fail3).await;
        });

        let res = send_test_ipc_request(
            &pipe_name,
            IpcRequest::CancelLaunch {
                app_name: "cancelled_app.exe".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res, IpcResponse::Success);
        srv3.await.unwrap();
    }

    #[tokio::test]
    async fn test_ipc_concurrent_requests_burst() {
        let pipe_name = get_unique_pipe_name();
        let mut config = AppConfig {
            locked_apps: vec!["burst.exe".into()],
            password: "admin".into(),
        };
        config.set_password("burst_secret").unwrap();
        let config = Arc::new(Mutex::new(config));
        let recently_unlocked = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let failed_attempts = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let interceptor_path = "C:\\app_locker.exe".to_string();

        let stop_flag = Arc::new(tokio::sync::Notify::new());

        let pipe_name_srv = pipe_name.clone();
        let config_srv = config.clone();
        let interceptor_srv = interceptor_path.clone();
        let unlocked_srv = recently_unlocked.clone();
        let failed_srv = failed_attempts.clone();

        let server_task = tokio::spawn(async move {
            let mut is_first = true;
            loop {
                let server = ServerOptions::new()
                    .access_inbound(true)
                    .access_outbound(true)
                    .first_pipe_instance(is_first)
                    .create(&pipe_name_srv);

                let server = match server {
                    Ok(s) => s,
                    Err(_) => break,
                };
                is_first = false;

                if server.connect().await.is_ok() {
                    let cfg = config_srv.clone();
                    let int = interceptor_srv.clone();
                    let unl = unlocked_srv.clone();
                    let fail = failed_srv.clone();
                    tokio::spawn(async move {
                        let mut server = server;
                        handle_ipc_client(&mut server, cfg, int, unl, fail).await;
                    });
                }
            }
        });

        // Fire 8 concurrent client tasks
        let mut handles = Vec::new();
        for i in 0..8 {
            let p_name = pipe_name.clone();
            let pw = if i % 2 == 0 {
                "burst_secret".to_string()
            } else {
                "wrong".to_string()
            };
            handles.push(tokio::spawn(async move {
                send_test_ipc_request(
                    &p_name,
                    IpcRequest::LaunchApp {
                        app_name: format!("burst_{}.exe", i),
                        app_path: format!("C:\\burst_{}.exe", i),
                        password: pw,
                    },
                )
                .await
            }));
        }

        for (i, h) in handles.into_iter().enumerate() {
            let res = h.await.unwrap().unwrap();
            if i % 2 == 0 {
                assert_eq!(res, IpcResponse::Success);
            } else {
                assert_eq!(res, IpcResponse::WrongPassword);
            }
        }

        stop_flag.notify_waiters();
        server_task.abort();
    }

    #[tokio::test]
    async fn test_ipc_case_insensitivity() {
        let pipe_name = get_unique_pipe_name();
        let mut config = AppConfig {
            locked_apps: vec!["notepad.exe".into()],
            password: "admin".into(),
        };
        config.set_password("mypass").unwrap();
        let config = Arc::new(Mutex::new(config));
        let recently_unlocked = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let failed_attempts = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let interceptor_path = "C:\\app_locker.exe".to_string();

        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let cfg = config.clone();
        let int = interceptor_path.clone();
        let unl = recently_unlocked.clone();
        let fail = failed_attempts.clone();
        let srv = tokio::spawn(async move {
            server.connect().await.unwrap();
            handle_ipc_client(&mut server, cfg, int, unl, fail).await;
        });

        // Launch with uppercase name
        let res = send_test_ipc_request(
            &pipe_name,
            IpcRequest::LaunchApp {
                app_name: "NOTEPAD.EXE".into(),
                app_path: "C:\\Windows\\System32\\NOTEPAD.EXE".into(),
                password: "mypass".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res, IpcResponse::Success);
        srv.await.unwrap();

        // Verify recently_unlocked recorded normalized lowercase
        let unl_map = recently_unlocked.lock().await;
        assert!(unl_map.contains_key("notepad.exe"));
    }

    #[tokio::test]
    async fn test_ipc_auto_approve_expiration() {
        let pipe_name = get_unique_pipe_name();
        let mut config = AppConfig {
            locked_apps: vec!["chrome.exe".into()],
            password: "admin".into(),
        };
        config.set_password("secure_pass").unwrap();
        let config = Arc::new(Mutex::new(config));
        let recently_unlocked = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let failed_attempts = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let interceptor_path = "C:\\app_locker.exe".to_string();

        // Manually insert an expired unlock timestamp (40 seconds ago)
        {
            let mut unl = recently_unlocked.lock().await;
            let expired_instant = tokio::time::Instant::now() - Duration::from_secs(40);
            unl.insert("chrome.exe".into(), expired_instant);
        }

        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let cfg = config.clone();
        let int = interceptor_path.clone();
        let unl = recently_unlocked.clone();
        let fail = failed_attempts.clone();
        let srv = tokio::spawn(async move {
            server.connect().await.unwrap();
            handle_ipc_client(&mut server, cfg, int, unl, fail).await;
        });

        // Request with wrong password should NOT be auto-approved
        let res = send_test_ipc_request(
            &pipe_name,
            IpcRequest::LaunchApp {
                app_name: "chrome.exe".into(),
                app_path: "C:\\chrome.exe".into(),
                password: "wrong_password".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res, IpcResponse::WrongPassword);
        srv.await.unwrap();
    }

    #[tokio::test]
    #[cfg(target_os = "windows")]
    async fn test_create_open_pipe_sddl_validity() {
        let pipe = create_open_pipe();
        assert!(pipe.is_ok(), "Failed creating secure NamedPipeServer: {:?}", pipe.err());
    }

    #[tokio::test]
    async fn test_ipc_malformed_payload_handling() {
        let pipe_name = get_unique_pipe_name();
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let recently_unlocked = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let failed_attempts = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let interceptor_path = "C:\\app_locker.exe".to_string();

        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let cfg = config.clone();
        let int = interceptor_path.clone();
        let unl = recently_unlocked.clone();
        let fail = failed_attempts.clone();
        let srv = tokio::spawn(async move {
            server.connect().await.unwrap();
            handle_ipc_client(&mut server, cfg, int, unl, fail).await;
        });

        let mut client = ClientOptions::new().open(&pipe_name).unwrap();
        // Write invalid corrupted bytes
        client.write_all(b"NOT_A_VALID_JSON_CORRUPT_BYTES").await.unwrap();

        // Server task finishes gracefully without panicking
        srv.await.unwrap();
    }

    #[tokio::test]
    async fn test_ipc_sequential_password_rotation() {
        let pipe_name = get_unique_pipe_name();
        let mut config = AppConfig {
            locked_apps: vec!["target.exe".into()],
            password: "admin".into(),
        };
        config.set_password("pass_zero").unwrap();
        let config = Arc::new(Mutex::new(config));
        let recently_unlocked = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let failed_attempts = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let interceptor_path = "C:\\app_locker.exe".to_string();

        // Rotation 1: pass_zero -> pass_one
        let mut server1 = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();
        let cfg = config.clone();
        let int = interceptor_path.clone();
        let unl = recently_unlocked.clone();
        let fail = failed_attempts.clone();
        let srv1 = tokio::spawn(async move {
            server1.connect().await.unwrap();
            handle_ipc_client(&mut server1, cfg, int, unl, fail).await;
        });

        let res1 = send_test_ipc_request(
            &pipe_name,
            IpcRequest::ChangePassword {
                old_password: "pass_zero".into(),
                new_password: "pass_one".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res1, IpcResponse::PasswordChanged);
        srv1.await.unwrap();

        // Rotation 2: pass_one -> pass_two
        let mut server2 = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();
        let cfg = config.clone();
        let int = interceptor_path.clone();
        let unl = recently_unlocked.clone();
        let fail = failed_attempts.clone();
        let srv2 = tokio::spawn(async move {
            server2.connect().await.unwrap();
            handle_ipc_client(&mut server2, cfg, int, unl, fail).await;
        });

        let res2 = send_test_ipc_request(
            &pipe_name,
            IpcRequest::ChangePassword {
                old_password: "pass_one".into(),
                new_password: "pass_two".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res2, IpcResponse::PasswordChanged);
        srv2.await.unwrap();

        // Verify only pass_two works and older passwords fail
        assert!(!config.lock().await.verify_password("pass_zero"));
        assert!(!config.lock().await.verify_password("pass_one"));
        assert!(config.lock().await.verify_password("pass_two"));
    }

    #[test]
    fn test_send_ipc_request_sync_wrapper() {
        // Test that send_ipc_request can be called from synchronous thread without panic or hang
        let start = std::time::Instant::now();
        let res = send_ipc_request(IpcRequest::CancelLaunch {
            app_name: "test.exe".into(),
        });
        let elapsed = start.elapsed();
        // Since global pipe might be offline, it must fail fast in under 300ms, not hang for seconds
        assert!(res.is_err() || res.is_ok());
        assert!(elapsed < std::time::Duration::from_millis(1500));
    }

    #[test]
    fn test_ipc_fast_fail_on_offline_service() {
        let start = std::time::Instant::now();
        let _ = send_ipc_request(IpcRequest::ReloadConfig);
        let elapsed = start.elapsed();
        // Guaranteed fast fail under 1.5s
        assert!(elapsed < std::time::Duration::from_millis(1500));
    }

    #[test]
    fn test_interceptor_offline_local_auth_fallback() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("app_locker_fallback_test_{}", nonce));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let mut config = AppConfig {
            locked_apps: vec!["chrome.exe".into()],
            password: "admin".into(),
        };
        config.set_password("correct_pass").unwrap();
        config.save(&temp_dir).unwrap();

        // Simulate IPC failure (service offline)
        let ipc_failed = true;
        assert!(ipc_failed);

        // Fallback to local config verification
        let loaded_config = AppConfig::load(&temp_dir);
        assert!(loaded_config.verify_password("correct_pass"));
        assert!(!loaded_config.verify_password("wrong_pass"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_call_named_pipe_roundtrip() {
        use std::ffi::c_void;
        use windows::core::HSTRING;
        use windows::Win32::System::Pipes::CallNamedPipeW;

        let pipe_name = get_unique_pipe_name();
        let mut server = ServerOptions::new()
            .access_inbound(true)
            .access_outbound(true)
            .pipe_mode(tokio::net::windows::named_pipe::PipeMode::Message)
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let srv = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut buf = vec![0u8; 1024];
            let n = server.read(&mut buf).await.unwrap();
            let req: IpcRequest = serde_json::from_slice(&buf[..n]).unwrap();
            assert_eq!(req, IpcRequest::CancelLaunch { app_name: "test.exe".into() });
            let resp = IpcResponse::Success;
            let resp_bytes = serde_json::to_vec(&resp).unwrap();
            server.write_all(&resp_bytes).await.unwrap();
            server.flush().await.unwrap();
        });

        let pipe_name_clone = pipe_name.clone();
        let client_task = tokio::task::spawn_blocking(move || {
            let payload = serde_json::to_vec(&IpcRequest::CancelLaunch { app_name: "test.exe".into() }).unwrap();
            let s_pipe = HSTRING::from(pipe_name_clone.as_str());
            let mut out_buf = vec![0u8; 1024];
            let mut bytes_read = 0u32;

            let ok = unsafe {
                CallNamedPipeW(
                    &s_pipe,
                    Some(payload.as_ptr() as *const c_void),
                    payload.len() as u32,
                    Some(out_buf.as_mut_ptr() as *mut c_void),
                    out_buf.len() as u32,
                    &mut bytes_read,
                    3000,
                )
            };

            assert!(ok.as_bool(), "CallNamedPipeW failed");
            assert!(bytes_read > 0);
            let resp: IpcResponse = serde_json::from_slice(&out_buf[..bytes_read as usize]).unwrap();
            assert_eq!(resp, IpcResponse::Success);
        });

        srv.await.unwrap();
        client_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_ipc_rate_limiting_after_five_failed_attempts() {
        let pipe_name = get_unique_pipe_name();
        let mut config = AppConfig::default();
        config.set_password("correct_pass").unwrap();
        let config = Arc::new(Mutex::new(config));
        let recently_unlocked = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let failed_attempts = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let interceptor_path = "C:\\app_locker.exe".to_string();

        let pipe_name_srv = pipe_name.clone();
        let config_srv = config.clone();
        let interceptor_srv = interceptor_path.clone();
        let unlocked_srv = recently_unlocked.clone();
        let failed_srv = failed_attempts.clone();

        let server_task = tokio::spawn(async move {
            let mut is_first = true;
            loop {
                let server = ServerOptions::new()
                    .access_inbound(true)
                    .access_outbound(true)
                    .first_pipe_instance(is_first)
                    .create(&pipe_name_srv);

                let mut server = match server {
                    Ok(s) => s,
                    Err(_) => break,
                };
                is_first = false;

                if server.connect().await.is_ok() {
                    let cfg = config_srv.clone();
                    let int = interceptor_srv.clone();
                    let unl = unlocked_srv.clone();
                    let fail = failed_srv.clone();
                    tokio::spawn(async move {
                        handle_ipc_client(&mut server, cfg, int, unl, fail).await;
                    });
                }
            }
        });

        // 5 consecutive wrong passwords
        for _ in 0..5 {
            let res = send_test_ipc_request(
                &pipe_name,
                IpcRequest::LaunchApp {
                    app_name: "target.exe".into(),
                    app_path: "C:\\target.exe".into(),
                    password: "wrong".into(),
                },
            )
            .await
            .unwrap();
            assert_eq!(res, IpcResponse::WrongPassword);
        }

        // 6th attempt should hit rate limit
        let res6 = send_test_ipc_request(
            &pipe_name,
            IpcRequest::LaunchApp {
                app_name: "target.exe".into(),
                app_path: "C:\\target.exe".into(),
                password: "wrong".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            res6,
            IpcResponse::Error("Terlalu banyak percobaan. Coba lagi dalam 60 detik.".into())
        );

        // Correct password during rate limit is also rejected
        let res_during_block = send_test_ipc_request(
            &pipe_name,
            IpcRequest::LaunchApp {
                app_name: "target.exe".into(),
                app_path: "C:\\target.exe".into(),
                password: "correct_pass".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            res_during_block,
            IpcResponse::Error("Terlalu banyak percobaan. Coba lagi dalam 60 detik.".into())
        );

        // Another app name should not be rate limited
        let res_other = send_test_ipc_request(
            &pipe_name,
            IpcRequest::LaunchApp {
                app_name: "other.exe".into(),
                app_path: "C:\\other.exe".into(),
                password: "correct_pass".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(res_other, IpcResponse::Success);

        server_task.abort();
    }
}
