use anyhow::{Result, anyhow};

use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE, TerminateProcess,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, PostMessageA, WM_CLOSE,
};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows::core::s;

type NtSuspendResumeProcess = unsafe extern "system" fn(HANDLE) -> i32;

pub struct WindowsProcessManager {
    nt_suspend: NtSuspendResumeProcess,
    nt_resume: NtSuspendResumeProcess,
}

// SAFETY: Function pointers loaded from ntdll.dll are process-global, immutable, and stateless.
unsafe impl Send for WindowsProcessManager {}
unsafe impl Sync for WindowsProcessManager {}

impl WindowsProcessManager {
    pub fn new() -> Self {
        unsafe {
            let ntdll = LoadLibraryA(s!("ntdll.dll")).expect("Gagal load ntdll.dll");
            let get_fn = |name: &str| -> NtSuspendResumeProcess {
                let c_name = std::ffi::CString::new(name).unwrap();
                let proc = GetProcAddress(ntdll, windows::core::PCSTR(c_name.as_ptr() as *const u8))
                    .unwrap_or_else(|| panic!("Fungsi {} tidak ditemukan di ntdll.dll", name));
                std::mem::transmute::<unsafe extern "system" fn() -> isize, NtSuspendResumeProcess>(proc)
            };
            Self {
                nt_suspend: get_fn("NtSuspendProcess"),
                nt_resume: get_fn("NtResumeProcess"),
            }
        }
    }

    pub fn kill_process(&self, pid: u32) -> Result<()> {
        let _ = self.resume_process(pid);

        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, false, pid)
                .map_err(|e| anyhow!("Gagal membuka proses {}: {}", pid, e))?;

            let result = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);

            result.map_err(|e| anyhow!("Gagal mematikan proses: {}", e))
        }
    }

    pub fn suspend_process(&self, pid: u32) -> Result<()> {
        unsafe {
            let handle = OpenProcess(PROCESS_SUSPEND_RESUME, false, pid)
                .map_err(|e| anyhow!("Gagal membuka proses {}: {}", pid, e))?;

            let status = (self.nt_suspend)(handle);
            let _ = CloseHandle(handle);

            if status == 0 {
                Ok(())
            } else {
                Err(anyhow!(
                    "NtSuspendProcess gagal dengan NTSTATUS: 0x{:08X}",
                    status
                ))
            }
        }
    }

    pub fn resume_process(&self, pid: u32) -> Result<()> {
        unsafe {
            let handle = OpenProcess(PROCESS_SUSPEND_RESUME, false, pid)
                .map_err(|e| anyhow!("Gagal membuka proses {}: {}", pid, e))?;

            let status = (self.nt_resume)(handle);
            let _ = CloseHandle(handle);

            if status == 0 {
                Ok(())
            } else {
                Err(anyhow!(
                    "NtResumeProcess gagal dengan NTSTATUS: 0x{:08X}",
                    status
                ))
            }
        }
    }

    pub fn close_process_gracefully(&self, pid: u32) -> Result<()> {
        self.resume_process(pid)?;

        unsafe {
            let target_pid = pid;
            let _ = EnumWindows(Some(enum_windows_callback), LPARAM(target_pid as isize));
        }

        Ok(())
    }
}

impl Default for WindowsProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let target_pid = lparam.0 as u32;
    let mut window_pid: u32 = 0;

    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
    }

    if window_pid == target_pid {
        unsafe {
            let _ = PostMessageA(hwnd, WM_CLOSE, None, None);
        }
    }

    BOOL(1)
}

#[cfg(target_os = "windows")]
pub fn launch_bypassing_ifeo(app_path: &str, args: &[String]) -> Result<u32> {
    use windows::Win32::Foundation::{CloseHandle, BOOL, DBG_CONTINUE};
    use windows::Win32::System::Diagnostics::Debug::{
        ContinueDebugEvent, DebugActiveProcessStop, DebugSetProcessKillOnExit,
        WaitForDebugEvent, DEBUG_EVENT,
    };
    use windows::Win32::System::Threading::{
        CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW, DEBUG_ONLY_THIS_PROCESS,
    };

    let mut full_cmd = format!("\"{}\"", app_path);
    for arg in args {
        full_cmd.push(' ');
        if arg.contains(' ') {
            full_cmd.push('"');
            full_cmd.push_str(arg);
            full_cmd.push('"');
        } else {
            full_cmd.push_str(arg);
        }
    }

    let mut cmd_vec: Vec<u16> = full_cmd.encode_utf16().chain(std::iter::once(0)).collect();

    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();

    unsafe {
        CreateProcessW(
            None,
            windows::core::PWSTR(cmd_vec.as_mut_ptr()),
            None,
            None,
            // Chromium multiprocess architecture passes IPC/Mojo handles via command-line arguments
            // (e.g. --mojo-platform-channel-handle, --metrics-shmem-handle) which require handle inheritance.
            true,
            DEBUG_ONLY_THIS_PROCESS,
            None,
            None,
            &si,
            &mut pi,
        )
        .map_err(|e| anyhow!("Gagal CreateProcessW: {}", e))?;

        // 1. Prevent Windows kernel from terminating the target process when debugger detaches/exits
        let _ = DebugSetProcessKillOnExit(BOOL(0));

        // 2. Consume the initial process creation debug event and resume threads
        let mut dbg_event = DEBUG_EVENT::default();
        if WaitForDebugEvent(&mut dbg_event, 2000).is_ok() {
            let _ = ContinueDebugEvent(dbg_event.dwProcessId, dbg_event.dwThreadId, DBG_CONTINUE);
        }

        // 3. Detach cleanly from the target process
        let _ = DebugActiveProcessStop(pi.dwProcessId);

        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);

        Ok(pi.dwProcessId)
    }
}

#[cfg(not(target_os = "windows"))]
pub fn launch_bypassing_ifeo(_app_path: &str, _args: &[String]) -> Result<u32> {
    Ok(0)
}

#[cfg(target_os = "windows")]
pub fn protect_current_process() -> Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        SetKernelObjectSecurity, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    // SDDL denying PROCESS_TERMINATE (0x1) and PROCESS_SUSPEND_RESUME (0x800) to World/Everyone (WD)
    // while granting SYSTEM (SY) full access and Administrators (BA) read/query access
    let sddl = HSTRING::from("D:P(D;;0x0801;;;WD)(A;;0x1201FF;;;SY)(A;;0x120155;;;BA)");
    let mut sd = PSECURITY_DESCRIPTOR::default();

    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
        .map_err(|e| anyhow!("Gagal membuat security descriptor proteksi proses: {}", e))?;

        SetKernelObjectSecurity(
            GetCurrentProcess(),
            DACL_SECURITY_INFORMATION,
            sd,
        )
        .map_err(|e| anyhow!("Gagal menerapkan DACL proteksi proses: {}", e))?;

        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub fn protect_current_process() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn test_windows_process_manager_init() {
        let pm = WindowsProcessManager::new();
        assert!(!std::ptr::eq(pm.nt_suspend as *const (), std::ptr::null()));
        assert!(!std::ptr::eq(pm.nt_resume as *const (), std::ptr::null()));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_windows_process_lifecycle() {
        let pm = WindowsProcessManager::new();
        let mut child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 10"])
            .spawn()
            .expect("Gagal spawn dummy process");

        let pid = child.id();

        let suspend_res = pm.suspend_process(pid);
        assert!(suspend_res.is_ok(), "Suspend failed: {:?}", suspend_res);

        let resume_res = pm.resume_process(pid);
        assert!(resume_res.is_ok(), "Resume failed: {:?}", resume_res);

        let kill_res = pm.kill_process(pid);
        assert!(kill_res.is_ok(), "Kill failed: {:?}", kill_res);

        let _ = child.wait();
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_windows_process_manager_invalid_pid() {
        let pm = WindowsProcessManager::new();
        let invalid_pid = 999_999_999;

        // All operations on non-existent PID should fail gracefully
        assert!(pm.suspend_process(invalid_pid).is_err());
        assert!(pm.resume_process(invalid_pid).is_err());
        assert!(pm.kill_process(invalid_pid).is_err());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_protect_current_process() {
        let res = protect_current_process();
        assert!(res.is_ok(), "protect_current_process failed: {:?}", res);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_launch_bypassing_ifeo() {
        let res = launch_bypassing_ifeo("cmd.exe", &[
            "/C".to_string(),
            "echo test".to_string(),
        ]);
        assert!(res.is_ok(), "launch_bypassing_ifeo failed: {:?}", res);
        let pid = res.unwrap();
        assert!(pid > 0);
    }
}
