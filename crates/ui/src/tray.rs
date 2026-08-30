#[cfg(target_os = "windows")]
pub mod windows_tray {
    use slint::ComponentHandle;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tracing::{error, info};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        InvalidateRect, RedrawWindow, UpdateWindow, HRGN, RDW_ALLCHILDREN, RDW_ERASE,
        RDW_INVALIDATE, RDW_UPDATENOW,
    };
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
        DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, GetWindowLongPtrW, LoadIconW,
        PostQuitMessage, RegisterClassExW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow,
        TrackPopupMenu, TranslateMessage, GWLP_USERDATA, HMENU, IDI_APPLICATION, MF_SEPARATOR,
        MF_STRING, MSG, SW_RESTORE, SW_SHOW, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE,
        WM_COMMAND, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP, WM_USER,
        WNDCLASSEXW, WS_OVERLAPPED,
    };

    const WM_TRAYICON: u32 = WM_USER + 101;
    const ID_TRAY_OPEN: usize = 1001;
    const ID_TRAY_EXIT: usize = 1002;

    static IS_RUNNING: AtomicBool = AtomicBool::new(true);

    pub fn init_tray(ui_weak: slint::Weak<crate::ManagementPanel>) -> anyhow::Result<()> {
        std::thread::spawn(move || unsafe {
            let instance =
                windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
            let class_name = w!("AppLockerTrayClass");

            let wnd_class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(tray_wnd_proc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };

            let _ = RegisterClassExW(&wnd_class);

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("AppLockerTrayWindow"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            );

            if hwnd.0 == 0 {
                error!("Gagal membuat tray message window");
                return;
            }

            let ui_box = Box::new(ui_weak);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(ui_box) as isize);

            let hinstance = windows::Win32::Foundation::HINSTANCE(instance.0);
            let icon = LoadIconW(hinstance, PCWSTR(1 as *const u16))
                .or_else(|_| {
                    LoadIconW(
                        windows::Win32::Foundation::HINSTANCE::default(),
                        IDI_APPLICATION,
                    )
                })
                .unwrap_or_default();

            let mut tip_chars = [0u16; 128];
            let tip_text = "App Locker (Protected)";
            for (i, c) in tip_text.encode_utf16().enumerate().take(127) {
                tip_chars[i] = c;
            }

            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: WM_TRAYICON,
                hIcon: icon,
                szTip: tip_chars,
                ..Default::default()
            };

            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
            info!("System tray icon berhasil diinisialisasi");

            let mut msg = MSG::default();
            while IS_RUNNING.load(Ordering::SeqCst)
                && GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool()
            {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            let _ = DestroyWindow(hwnd);
        });

        Ok(())
    }

    unsafe extern "system" fn tray_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TRAYICON => {
                let event = lparam.0 as u32;
                match event {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                        unsafe { show_ui(hwnd) };
                    }
                    WM_RBUTTONUP => {
                        unsafe { show_context_menu(hwnd) };
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as usize;
                if id == ID_TRAY_OPEN {
                    unsafe { show_ui(hwnd) };
                } else if id == ID_TRAY_EXIT {
                    IS_RUNNING.store(false, Ordering::SeqCst);
                    let _ = slint::invoke_from_event_loop(|| {
                        let _ = slint::quit_event_loop();
                    });
                    unsafe { PostQuitMessage(0) };
                    std::process::exit(0);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) }
                    as *mut slint::Weak<crate::ManagementPanel>;
                if !ptr.is_null() {
                    let _ = unsafe { Box::from_raw(ptr) };
                    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
                }
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    unsafe fn show_ui(hwnd: HWND) {
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) }
            as *mut slint::Weak<crate::ManagementPanel>;
        if !ptr.is_null() {
            let weak = unsafe { &*ptr };
            let weak_clone = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak_clone.upgrade() {
                    ui.window().set_minimized(false);
                    ui.window().request_redraw();
                }
            });

            let title = w!("App Locker");
            let app_hwnd = unsafe { FindWindowW(PCWSTR::null(), title) };
            if app_hwnd.0 != 0 {
                let _ = unsafe { ShowWindow(app_hwnd, SW_RESTORE) };
                let _ = unsafe { ShowWindow(app_hwnd, SW_SHOW) };
                let _ = unsafe { SetForegroundWindow(app_hwnd) };
                let _ = unsafe { InvalidateRect(app_hwnd, None, BOOL(1)) };
                let _ = unsafe {
                    RedrawWindow(
                        app_hwnd,
                        None,
                        HRGN::default(),
                        RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN | RDW_ERASE,
                    )
                };
                let _ = unsafe { UpdateWindow(app_hwnd) };
            }
        }
    }

    unsafe fn show_context_menu(hwnd: HWND) {
        let hmenu = unsafe { CreatePopupMenu() }.unwrap_or_default();
        if hmenu.0 != 0 {
            let _ = unsafe { AppendMenuW(hmenu, MF_STRING, ID_TRAY_OPEN, w!("Buka App Locker")) };
            let _ = unsafe { AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null()) };
            let _ = unsafe { AppendMenuW(hmenu, MF_STRING, ID_TRAY_EXIT, w!("Keluar")) };

            let mut pt = POINT::default();
            let _ = unsafe { GetCursorPos(&mut pt) };

            unsafe {
                SetForegroundWindow(hwnd);
                let _ = TrackPopupMenu(
                    hmenu,
                    TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                    pt.x,
                    pt.y,
                    0,
                    hwnd,
                    None,
                );
                let _ = DestroyMenu(hmenu);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub mod windows_tray {
    pub fn init_tray(_ui_weak: slint::Weak<crate::ManagementPanel>) -> anyhow::Result<()> {
        Ok(())
    }
}
