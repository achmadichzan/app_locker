use app_core::{AppConfig, IpcRequest, IpcResponse, CONFIG_FILENAME};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn test_config_load_and_save_roundtrip() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("app_locker_test_{}", nonce));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let mut config = AppConfig {
        locked_apps: vec!["chrome.exe".into(), "notepad.exe".into()],
        password: "admin".into(),
    };
    config.set_password("mypassword").unwrap();
    config.save(&temp_dir).unwrap();

    let loaded = AppConfig::load(&temp_dir);
    assert_eq!(loaded.locked_apps, vec!["chrome.exe", "notepad.exe"]);
    assert!(loaded.verify_password("mypassword"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_config_auto_migration_from_plaintext() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("app_locker_migrate_test_{}", nonce));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let legacy_json = r#"{"locked_apps":["calc.exe"],"password":"plain_text_secret"}"#;
    std::fs::write(temp_dir.join(CONFIG_FILENAME), legacy_json).unwrap();

    let loaded = AppConfig::load(&temp_dir);
    assert_eq!(loaded.locked_apps, vec!["calc.exe"]);
    assert!(loaded.verify_password("plain_text_secret"));
    assert!(!loaded.verify_password("wrong"));
    assert!(loaded.password.starts_with("$argon2"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_ipc_request_serialization_roundtrip() {
    let requests = vec![
        IpcRequest::LaunchApp {
            app_name: "test.exe".into(),
            app_path: "C:\\path\\test.exe".into(),
            password: "secret".into(),
        },
        IpcRequest::CancelLaunch {
            app_name: "test.exe".into(),
        },
        IpcRequest::ChangePassword {
            old_password: "old".into(),
            new_password: "new".into(),
        },
        IpcRequest::ReloadConfig,
    ];

    for req in requests {
        let serialized = serde_json::to_string(&req).unwrap();
        let deserialized: IpcRequest = serde_json::from_str(&serialized).unwrap();
        assert_eq!(req, deserialized);
    }
}

#[test]
fn test_ipc_response_serialization_roundtrip() {
    let responses = vec![
        IpcResponse::Success,
        IpcResponse::WrongPassword,
        IpcResponse::PasswordChanged,
        IpcResponse::ConfigReloaded,
        IpcResponse::Error("sample error".into()),
    ];

    for res in responses {
        let serialized = serde_json::to_string(&res).unwrap();
        let deserialized: IpcResponse = serde_json::from_str(&serialized).unwrap();
        assert_eq!(res, deserialized);
    }
}

#[test]
fn test_config_corrupted_json_recovery() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("app_locker_corrupt_test_{}", nonce));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let invalid_json = "{ locked_apps: [invalid json... ";
    std::fs::write(temp_dir.join(CONFIG_FILENAME), invalid_json).unwrap();

    let loaded = AppConfig::load(&temp_dir);
    assert_eq!(loaded.locked_apps, Vec::<String>::new());
    assert!(loaded.verify_password("admin"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_password_edge_cases_unicode_and_long() {
    let mut config = AppConfig::default();

    // Unicode and symbols
    let unicode_pass = "P@ssw0rd!_🔒_Indonésia-2026";
    assert!(config.set_password(unicode_pass).is_ok());
    assert!(config.verify_password(unicode_pass));
    assert!(!config.verify_password("P@ssw0rd!_🔒_Indonesia-2026"));

    // Long password (1024 characters)
    let long_pass = "A".repeat(1024);
    assert!(config.set_password(&long_pass).is_ok());
    assert!(config.verify_password(&long_pass));
    assert!(!config.verify_password(&"A".repeat(1023)));
}

#[test]
fn test_locked_apps_sanitization_and_deduplication() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("app_locker_sanitize_test_{}", nonce));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let mut config = AppConfig {
        locked_apps: vec![
            "  notepad.exe  ".into(),
            "NOTEPAD.EXE".into(),
            "".into(),
            "   ".into(),
            "Chrome.EXE".into(),
            "chrome.exe".into(),
            "calc.exe".into(),
        ],
        password: "admin".into(),
    };

    config.save(&temp_dir).unwrap();

    let loaded = AppConfig::load(&temp_dir);
    assert_eq!(
        loaded.locked_apps,
        vec!["notepad.exe", "chrome.exe", "calc.exe"]
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_config_save_in_nonexistent_directory() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let nested_dir = std::env::temp_dir()
        .join(format!("app_locker_nested_{}", nonce))
        .join("deep")
        .join("sub")
        .join("dir");

    // Do NOT create nested_dir beforehand
    let mut config = AppConfig {
        locked_apps: vec!["app.exe".into()],
        password: "admin".into(),
    };

    assert!(config.save(&nested_dir).is_ok());
    let loaded = AppConfig::load(&nested_dir);
    assert_eq!(loaded.locked_apps, vec!["app.exe"]);

    let root_temp = std::env::temp_dir().join(format!("app_locker_nested_{}", nonce));
    let _ = std::fs::remove_dir_all(&root_temp);
}

#[test]
fn test_get_default_config_dir() {
    let dir = app_core::get_default_config_dir();
    assert!(!dir.as_os_str().is_empty());
}

#[test]
fn test_concurrent_atomic_save_race() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("app_locker_race_{}", nonce));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let temp_dir_arc = std::sync::Arc::new(temp_dir.clone());
    let mut handles = Vec::new();

    for i in 0..10 {
        let dir = temp_dir_arc.clone();
        handles.push(std::thread::spawn(move || {
            let mut config = AppConfig {
                locked_apps: vec![format!("app_{}.exe", i)],
                password: "admin".into(),
            };
            let _ = config.save(&dir);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Must be valid readable JSON without corruption
    let loaded = AppConfig::load(&temp_dir);
    assert_eq!(loaded.locked_apps.len(), 1);
    assert!(loaded.verify_password("admin"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_multithreaded_password_verification() {
    let mut config = AppConfig::default();
    config.set_password("multi_secret_2026").unwrap();
    let config = std::sync::Arc::new(config);

    let mut handles = Vec::new();
    for i in 0..20 {
        let cfg = config.clone();
        handles.push(std::thread::spawn(move || {
            if i % 2 == 0 {
                assert!(cfg.verify_password("multi_secret_2026"));
            } else {
                assert!(!cfg.verify_password("wrong_password"));
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
}
