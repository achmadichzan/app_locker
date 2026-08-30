fn main() {
    slint_build::compile("app_locker.slint").unwrap();

    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "App Locker");
        res.set("FileDescription", "App Locker");
        let _ = res.compile();
    }
}
