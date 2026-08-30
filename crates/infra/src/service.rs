use anyhow::{Result, anyhow};

#[cfg(target_os = "windows")]
use windows::core::HSTRING;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{BOOL, CloseHandle};
#[cfg(target_os = "windows")]
use windows::Win32::System::Services::{
    ChangeServiceConfig2W, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, QueryServiceStatus, QueryServiceStatusEx, SC_ACTION,
    SC_ACTION_RESTART, SC_MANAGER_CONNECT, SC_MANAGER_CREATE_SERVICE, SC_STATUS_PROCESS_INFO,
    SERVICE_ALL_ACCESS, SERVICE_AUTO_START, SERVICE_CONFIG_FAILURE_ACTIONS,
    SERVICE_CONFIG_FAILURE_ACTIONS_FLAG, SERVICE_CONTROL_STOP, SERVICE_ERROR_NORMAL,
    SERVICE_FAILURE_ACTIONS_FLAG, SERVICE_FAILURE_ACTIONSW, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
    SERVICE_START, SERVICE_STATUS, SERVICE_STATUS_PROCESS, SERVICE_STOP, SERVICE_STOPPED,
    SERVICE_WIN32_OWN_PROCESS, StartServiceW,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

#[cfg(target_os = "windows")]
pub fn is_service_installed(service_name: &str) -> bool {
    if service_name.trim().is_empty() {
        return false;
    }
    unsafe {
        let scm = match OpenSCManagerW(None, None, SC_MANAGER_CONNECT) {
            Ok(h) => h,
            Err(_) => return false,
        };

        let s_name = HSTRING::from(service_name);
        let service = OpenServiceW(scm, &s_name, SERVICE_QUERY_STATUS);
        let installed = service.is_ok();

        if let Ok(s) = service {
            let _ = CloseServiceHandle(s);
        }
        let _ = CloseServiceHandle(scm);

        installed
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_service_installed(_service_name: &str) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub fn is_service_running(service_name: &str) -> bool {
    if service_name.trim().is_empty() {
        return false;
    }
    unsafe {
        let scm = match OpenSCManagerW(None, None, SC_MANAGER_CONNECT) {
            Ok(h) => h,
            Err(_) => return false,
        };

        let s_name = HSTRING::from(service_name);
        let service = match OpenServiceW(scm, &s_name, SERVICE_QUERY_STATUS) {
            Ok(s) => s,
            Err(_) => {
                let _ = CloseServiceHandle(scm);
                return false;
            }
        };

        let mut status = SERVICE_STATUS::default();
        let running = if QueryServiceStatus(service, &mut status).is_ok() {
            status.dwCurrentState == SERVICE_RUNNING
        } else {
            false
        };

        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(scm);

        running
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_service_running(_service_name: &str) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub fn install_service(service_name: &str, display_name: &str, bin_path: &str) -> Result<()> {
    if service_name.trim().is_empty() {
        return Err(anyhow!("Nama service tidak boleh kosong"));
    }
    unsafe {
        let scm = OpenSCManagerW(
            None,
            None,
            SC_MANAGER_CREATE_SERVICE | SC_MANAGER_CONNECT,
        )
        .map_err(|e| anyhow!("Gagal membuka Service Manager: {}", e))?;

        let s_name = HSTRING::from(service_name);
        let d_name = HSTRING::from(display_name);
        let b_path = HSTRING::from(bin_path);

        let mut last_err = anyhow!("Gagal membuat service");
        let mut created = false;

        // If service was recently marked for deletion, wait and retry for SCM to purge it
        for _ in 0..15 {
            let service = CreateServiceW(
                scm,
                &s_name,
                &d_name,
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                &b_path,
                None,
                None,
                None,
                None,
                None,
            );

            match service {
                Ok(s) => {
                    // Configure service auto-restart failure actions (SCM recovery)
                    let mut actions = [
                        SC_ACTION {
                            Type: SC_ACTION_RESTART,
                            Delay: 0,
                        },
                        SC_ACTION {
                            Type: SC_ACTION_RESTART,
                            Delay: 0,
                        },
                        SC_ACTION {
                            Type: SC_ACTION_RESTART,
                            Delay: 0,
                        },
                    ];

                    let mut failure_actions = SERVICE_FAILURE_ACTIONSW {
                        dwResetPeriod: 86400,
                        lpRebootMsg: windows::core::PWSTR::null(),
                        lpCommand: windows::core::PWSTR::null(),
                        cActions: actions.len() as u32,
                        lpsaActions: actions.as_mut_ptr(),
                    };

                    let _ = ChangeServiceConfig2W(
                        s,
                        SERVICE_CONFIG_FAILURE_ACTIONS,
                        Some(&mut failure_actions as *mut _ as *const _),
                    );

                    let mut flag = SERVICE_FAILURE_ACTIONS_FLAG {
                        fFailureActionsOnNonCrashFailures: BOOL(1),
                    };

                    let _ = ChangeServiceConfig2W(
                        s,
                        SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
                        Some(&mut flag as *mut _ as *const _),
                    );

                    // Allow Everyone (WD) to query and start the service if it was stopped
                    let sddl = HSTRING::from("D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)(A;;CCLCSWLOCRRC;;;WD)");
                    let mut sd = windows::Win32::Security::PSECURITY_DESCRIPTOR::default();
                    if windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW(
                        windows::core::PCWSTR(sddl.as_ptr()),
                        windows::Win32::Security::Authorization::SDDL_REVISION_1,
                        &mut sd,
                        None,
                    ).is_ok() {
                        let _ = windows::Win32::System::Services::SetServiceObjectSecurity(
                            s,
                            windows::Win32::Security::DACL_SECURITY_INFORMATION,
                            sd,
                        );
                    }

                    let _ = CloseServiceHandle(s);
                    created = true;
                    break;
                }
                Err(e) => {
                    let code = e.code().0 as u32;
                    if code == 0x80070430 {
                        // ERROR_SERVICE_MARKED_FOR_DELETE (1072) - SCM is purging old instance
                        last_err = anyhow!("Service sedang dalam antrean hapus. Harap tutup jendela Services (services.msc) lalu klik install lagi.");
                        std::thread::sleep(std::time::Duration::from_millis(250));
                        continue;
                    } else {
                        last_err = anyhow!("Gagal membuat service: {}", e);
                        break;
                    }
                }
            }
        }

        let _ = CloseServiceHandle(scm);
        if created {
            Ok(())
        } else {
            Err(last_err)
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn install_service(_service_name: &str, _display_name: &str, _bin_path: &str) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn uninstall_service(service_name: &str) -> Result<()> {
    if service_name.trim().is_empty() {
        return Err(anyhow!("Nama service tidak boleh kosong"));
    }

    // Step 1: Ensure service is fully stopped and process terminated
    let _ = stop_service(service_name);

    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_CONNECT)
            .map_err(|e| anyhow!("Gagal membuka Service Manager: {}", e))?;

        let s_name = HSTRING::from(service_name);
        let service = match OpenServiceW(scm, &s_name, SERVICE_ALL_ACCESS) {
            Ok(s) => s,
            Err(e) => {
                let _ = CloseServiceHandle(scm);
                let code = e.code().0 as u32;
                if code == 0x80070430 || code == 0x80070424 {
                    return Ok(());
                }
                return Err(anyhow!("Gagal membuka service untuk dihapus: {}", e));
            }
        };

        let delete_res = DeleteService(service);
        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(scm);

        match delete_res {
            Ok(_) => Ok(()),
            Err(e) => {
                let code = e.code().0 as u32;
                if code == 0x80070430 || code == 0x80070424 {
                    Ok(())
                } else {
                    Err(anyhow!("Gagal menghapus service: {}", e))
                }
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn uninstall_service(_service_name: &str) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn start_service(service_name: &str) -> Result<()> {
    if service_name.trim().is_empty() {
        return Err(anyhow!("Nama service tidak boleh kosong"));
    }
    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_CONNECT)
            .map_err(|e| anyhow!("Gagal membuka Service Manager: {}", e))?;

        let s_name = HSTRING::from(service_name);
        let service = OpenServiceW(scm, &s_name, SERVICE_START)
            .map_err(|e| anyhow!("Gagal membuka service untuk dijalankan: {}", e))?;

        let start_res = StartServiceW(service, None);
        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(scm);

        match start_res {
            Ok(_) => Ok(()),
            Err(e) => {
                let code = e.code().0 as u32;
                if code == 0x80070420 {
                    // ERROR_SERVICE_ALREADY_RUNNING (1056)
                    Ok(())
                } else {
                    Err(anyhow!("Gagal memulai service: {}", e))
                }
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn start_service(_service_name: &str) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn stop_service(service_name: &str) -> Result<()> {
    if service_name.trim().is_empty() {
        return Err(anyhow!("Nama service tidak boleh kosong"));
    }
    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_CONNECT)
            .map_err(|e| anyhow!("Gagal membuka Service Manager: {}", e))?;

        let s_name = HSTRING::from(service_name);
        let service = match OpenServiceW(scm, &s_name, SERVICE_STOP | SERVICE_QUERY_STATUS) {
            Ok(s) => s,
            Err(e) => {
                let _ = CloseServiceHandle(scm);
                let code = e.code().0 as u32;
                // If service doesn't exist or already marked for deletion, consider stopped
                if code == 0x80070424 || code == 0x80070430 {
                    return Ok(());
                }
                return Err(anyhow!("Gagal membuka service untuk dihentikan: {}", e));
            }
        };

        let mut status = SERVICE_STATUS::default();
        let _ = ControlService(service, SERVICE_CONTROL_STOP, &mut status);

        let mut proc_status = SERVICE_STATUS_PROCESS::default();
        let mut bytes_needed = 0u32;
        let proc_slice = std::slice::from_raw_parts_mut(
            &mut proc_status as *mut _ as *mut u8,
            std::mem::size_of::<SERVICE_STATUS_PROCESS>(),
        );

        let _ = QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            Some(proc_slice),
            &mut bytes_needed,
        );

        // Terminate the service process immediately if PID is valid to prevent lingering handles
        if proc_status.dwProcessId > 0
            && let Ok(process) = OpenProcess(PROCESS_TERMINATE, false, proc_status.dwProcessId)
        {
            let _ = TerminateProcess(process, 0);
            let _ = CloseHandle(process);
        }

        // Wait up to 500ms for SCM state to update to SERVICE_STOPPED
        for _ in 0..5 {
            let proc_slice = std::slice::from_raw_parts_mut(
                &mut proc_status as *mut _ as *mut u8,
                std::mem::size_of::<SERVICE_STATUS_PROCESS>(),
            );
            if QueryServiceStatusEx(
                service,
                SC_STATUS_PROCESS_INFO,
                Some(proc_slice),
                &mut bytes_needed,
            )
            .is_ok()
                && proc_status.dwCurrentState == SERVICE_STOPPED
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(scm);

        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub fn stop_service(_service_name: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn test_is_service_installed_nonexistent() {
        assert!(!is_service_installed("NonExistentService12345XYZ"));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_is_service_running_nonexistent() {
        assert!(!is_service_running("NonExistentService12345XYZ"));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_service_api_invalid_inputs() {
        assert!(!is_service_installed(""));
        assert!(!is_service_running(""));
        assert!(install_service("", "", "").is_err());
        assert!(uninstall_service("").is_err());
        assert!(start_service("").is_err());
        assert!(stop_service("").is_err());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_service_stop_and_uninstall_nonexistent() {
        // Idempotent stop and uninstall for non-existent service should return Ok(())
        let res_stop = stop_service("DefinitelyNonExistentService12345");
        assert!(res_stop.is_ok());

        let res_uninstall = uninstall_service("DefinitelyNonExistentService12345");
        assert!(res_uninstall.is_ok());
    }
}
