#[derive(Debug, PartialEq, Eq)]
pub enum AppMode {
    Service,
    Interceptor {
        app_name: String,
        app_path: String,
        forward_args: Vec<String>,
    },
    ManagementPanel,
}

pub fn parse_cli_args(args: &[String]) -> AppMode {
    match args.get(1).map(|s| s.as_str()) {
        Some("--service") => AppMode::Service,
        Some("--interceptor") => {
            let app_path = args.get(2).cloned().unwrap_or_default();
            let app_name = std::path::Path::new(&app_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown.exe")
                .to_lowercase();
            let forward_args = if args.len() > 3 {
                args[3..].to_vec()
            } else {
                Vec::new()
            };
            AppMode::Interceptor {
                app_name,
                app_path,
                forward_args,
            }
        }
        _ => AppMode::ManagementPanel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing_service() {
        let args = vec!["app_locker.exe".into(), "--service".into()];
        assert_eq!(parse_cli_args(&args), AppMode::Service);
    }

    #[test]
    fn test_cli_parsing_management_panel() {
        let args = vec!["app_locker.exe".into()];
        assert_eq!(parse_cli_args(&args), AppMode::ManagementPanel);

        let args_unknown = vec!["app_locker.exe".into(), "--other".into()];
        assert_eq!(parse_cli_args(&args_unknown), AppMode::ManagementPanel);
    }

    #[test]
    fn test_cli_parsing_interceptor() {
        let args = vec![
            "app_locker.exe".into(),
            "--interceptor".into(),
            "C:\\Program Files\\App\\target.exe".into(),
            "--flag".into(),
            "document.txt".into(),
        ];
        assert_eq!(
            parse_cli_args(&args),
            AppMode::Interceptor {
                app_name: "target.exe".into(),
                app_path: "C:\\Program Files\\App\\target.exe".into(),
                forward_args: vec!["--flag".into(), "document.txt".into()],
            }
        );
    }

    #[test]
    fn test_cli_parsing_interceptor_no_extra_args() {
        let args = vec![
            "app_locker.exe".into(),
            "--interceptor".into(),
            "notepad.exe".into(),
        ];
        assert_eq!(
            parse_cli_args(&args),
            AppMode::Interceptor {
                app_name: "notepad.exe".into(),
                app_path: "notepad.exe".into(),
                forward_args: vec![],
            }
        );
    }

    #[test]
    fn test_cli_parsing_complex_paths_and_quotes() {
        let args = vec![
            "app_locker.exe".into(),
            "--interceptor".into(),
            "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe".into(),
            "--profile-directory=\"Profile 1\"".into(),
            "--flag=value with spaces".into(),
        ];
        assert_eq!(
            parse_cli_args(&args),
            AppMode::Interceptor {
                app_name: "chrome.exe".into(),
                app_path: "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe".into(),
                forward_args: vec![
                    "--profile-directory=\"Profile 1\"".into(),
                    "--flag=value with spaces".into()
                ],
            }
        );
    }

    #[test]
    fn test_cli_parsing_chromium_child_process_args() {
        let args = vec![
            "app_locker.exe".into(),
            "--interceptor".into(),
            "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe".into(),
            "--type=utility".into(),
            "--utility-sub-type=network.mojom.NetworkService".into(),
            "--mojo-platform-channel-handle=2204".into(),
        ];
        let parsed = parse_cli_args(&args);
        match parsed {
            AppMode::Interceptor {
                app_name,
                app_path,
                forward_args,
            } => {
                assert_eq!(app_name, "chrome.exe");
                assert_eq!(app_path, "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe");
                assert_eq!(forward_args.len(), 3);
                assert!(forward_args.iter().any(|arg| arg.starts_with("--type=")));
            }
            _ => panic!("Expected AppMode::Interceptor"),
        }
    }
}
