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

fn load_ntdll_fn(name: &str) -> Result<NtSuspendResumeProcess> {
    unsafe {
        let ntdll =
            LoadLibraryA(s!("ntdll.dll")).map_err(|e| anyhow!("Gagal load ntdll.dll: {}", e))?;

        let c_name = std::ffi::CString::new(name).map_err(|_| anyhow!("Nama fungsi invalid"))?;

        let proc = GetProcAddress(ntdll, windows::core::PCSTR(c_name.as_ptr() as *const u8))
            .ok_or_else(|| anyhow!("Fungsi {} tidak ditemukan di ntdll.dll", name))?;

        Ok(std::mem::transmute(proc))
    }
}

pub struct WindowsProcessManager {
    nt_suspend: NtSuspendResumeProcess,
    nt_resume: NtSuspendResumeProcess,
}

unsafe impl Send for WindowsProcessManager {}
unsafe impl Sync for WindowsProcessManager {}

impl WindowsProcessManager {
    pub fn new() -> Self {
        Self {
            nt_suspend: load_ntdll_fn("NtSuspendProcess").expect("Gagal load NtSuspendProcess"),
            nt_resume: load_ntdll_fn("NtResumeProcess").expect("Gagal load NtResumeProcess"),
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
