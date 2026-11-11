//! 配置解析集成测试。
//!
//! 放在 `tests/` 而不是 `src/config.rs` 里，因为这些用例验证的是「文件 IO + TOML
//! 解析 + 三层优先级」的端到端行为，属于集成测试；`src/` 里的内联单测只覆盖纯函数。
//!
//! 每个用例使用独立临时文件，避免并行执行时互相干扰。

use robodog_device_client::config::{
    CliArgs, Config, ConfigError, DEFAULT_API_PASSWORD, DEFAULT_DEVICE_ID, DEFAULT_MAP_ROOT,
    DEFAULT_SERVER_URL, EnvConfig, FileConfig,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 写入临时配置文件，返回路径
fn write_config(body: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "robodog-client-config-{}-{}.toml",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, body).expect("写入临时配置失败");
    path
}

fn load_file(path: &std::path::Path, env: EnvConfig, cli: CliArgs) -> Config {
    let cli = CliArgs {
        config: Some(path.to_path_buf()),
        ..cli
    };
    Config::load(cli, env).expect("配置应能解析")
}

#[test]
fn file_values_are_used_when_cli_and_env_absent() {
    let path = write_config(
        r#"
[server]
url = "ws://10.0.0.12:3100"
device_id = "RD-042"
heartbeat_secs = 15

[robot]
host = "192.168.168.100"
file_port = 12001
map_root = "/data/maps"

[api]
base = "http://10.0.0.12:3100/"
username = "ops"

[mapping]
kind = "indoor"
name = "A区"
upload = false
park_id = 7
ingest_name = "A区-入库"

[simulation]
battery = 55
signal = 2
position = "1.0, 2.0"
firmware = "Hermes_M_v9"

[log]
filter = "robodog_device_client=debug"
"#,
    );

    let config = load_file(&path, EnvConfig::default(), CliArgs::default());

    assert_eq!(config.server_url, "ws://10.0.0.12:3100");
    assert_eq!(config.device_id, "RD-042");
    assert_eq!(config.heartbeat_interval, 15);
    assert_eq!(config.robot.as_deref(), Some("192.168.168.100"));
    assert_eq!(config.robot_file_port, 12001);
    assert_eq!(config.map_root, "/data/maps");
    assert_eq!(config.api_base.as_deref(), Some("http://10.0.0.12:3100"));
    assert_eq!(config.api_username, "ops");
    assert_eq!(config.map_kind, "indoor");
    assert_eq!(config.map_name, "A区");
    assert!(!config.upload_map);
    assert_eq!(config.park_id, Some(7));
    assert_eq!(config.ingest_map_name.as_deref(), Some("A区-入库"));
    assert_eq!(config.battery, 55);
    assert_eq!(config.signal, 2);
    assert_eq!(config.position, "1.0, 2.0");
    assert_eq!(config.firmware, "Hermes_M_v9");
    assert_eq!(
        config.log_filter.as_deref(),
        Some("robodog_device_client=debug")
    );
}

#[test]
fn cli_overrides_env_and_file() {
    let path = write_config(
        r#"
[server]
url = "ws://file:3100"
device_id = "FROM-FILE"
heartbeat_secs = 30
"#,
    );
    let env = EnvConfig::from_pairs([
        ("ROBODOG_SERVER_URL", "ws://env:3100"),
        ("ROBODOG_DEVICE_ID", "FROM-ENV"),
    ]);
    let cli = CliArgs {
        server: Some("ws://cli:3100".to_string()),
        heartbeat: Some(5),
        ..CliArgs::default()
    };

    let config = load_file(&path, env, cli);

    assert_eq!(config.server_url, "ws://cli:3100");
    assert_eq!(config.device_id, "FROM-ENV");
    assert_eq!(config.heartbeat_interval, 5);
}

#[test]
fn env_overrides_file() {
    let path = write_config(
        r#"
[server]
url = "ws://file:3100"
[robot]
host = "10.0.0.1"
"#,
    );
    let env = EnvConfig::from_pairs([
        ("ROBODOG_SERVER_URL", "ws://env:3200"),
        ("ROBODOG_ROBOT", "10.0.0.2"),
        ("ROBODOG_HEARTBEAT", "7"),
    ]);

    let config = load_file(&path, env, CliArgs::default());

    assert_eq!(config.server_url, "ws://env:3200");
    assert_eq!(config.robot.as_deref(), Some("10.0.0.2"));
    assert_eq!(config.heartbeat_interval, 7);
}

#[test]
fn secret_env_reference_beats_plaintext() {
    let path = write_config(
        r#"
[server]
secret = "plaintext"
secret_env = "MY_DEVICE_SECRET"
[api]
password = "plain-password"
password_env = "MY_API_PASSWORD"
"#,
    );
    let env = EnvConfig::from_pairs([
        ("MY_DEVICE_SECRET", "from-env-secret"),
        ("MY_API_PASSWORD", "from-env-password"),
    ]);

    let config = load_file(&path, env, CliArgs::default());

    assert_eq!(config.device_secret.as_deref(), Some("from-env-secret"));
    assert_eq!(config.api_password(), "from-env-password");
}

#[test]
fn direct_env_secret_beats_file_reference() {
    let path = write_config(
        r#"
[server]
secret_env = "MY_DEVICE_SECRET"
"#,
    );
    let env = EnvConfig::from_pairs([
        ("ROBODOG_DEVICE_SECRET", "direct-env"),
        ("MY_DEVICE_SECRET", "referenced-env"),
    ]);

    let config = load_file(&path, env, CliArgs::default());

    assert_eq!(config.device_secret.as_deref(), Some("direct-env"));
}

#[test]
fn missing_explicit_config_is_an_error() {
    let missing = std::env::temp_dir().join("robodog-client-config-does-not-exist.toml");
    let err = Config::load(
        CliArgs {
            config: Some(missing.clone()),
            ..CliArgs::default()
        },
        EnvConfig::default(),
    )
    .expect_err("显式指定的配置文件缺失必须报错");

    match err {
        ConfigError::Io { path, .. } => assert_eq!(path, missing),
        other => panic!("期望 Io 错误，实际 {other:?}"),
    }
}

#[test]
fn unknown_config_key_is_rejected() {
    let path = write_config(
        r#"
[server]
url = "ws://127.0.0.1:3100"
typo_key = "boom"
"#,
    );

    let err = Config::load(
        CliArgs {
            config: Some(path),
            ..CliArgs::default()
        },
        EnvConfig::default(),
    )
    .expect_err("未知配置项必须报错，避免拼写错误被静默忽略");

    assert!(matches!(err, ConfigError::Toml { .. }), "实际 {err:?}");
}

#[test]
fn invalid_server_url_is_rejected() {
    let path = write_config(
        r#"
[server]
url = "http://127.0.0.1:3100"
"#,
    );

    let err = Config::load(
        CliArgs {
            config: Some(path),
            ..CliArgs::default()
        },
        EnvConfig::default(),
    )
    .expect_err("非 ws:// 地址必须报错");

    assert!(matches!(err, ConfigError::Invalid(_)), "实际 {err:?}");
}

#[test]
fn invalid_map_kind_is_rejected() {
    let path = write_config(
        r#"
[mapping]
kind = "aerial"
"#,
    );

    let err = Config::load(
        CliArgs {
            config: Some(path),
            ..CliArgs::default()
        },
        EnvConfig::default(),
    )
    .expect_err("非法建图类型必须报错");

    assert!(matches!(err, ConfigError::Invalid(_)), "实际 {err:?}");
}

#[test]
fn non_positive_park_id_is_rejected() {
    let path = write_config(
        r#"
[mapping]
park_id = 0
"#,
    );

    let err = Config::load(
        CliArgs {
            config: Some(path),
            ..CliArgs::default()
        },
        EnvConfig::default(),
    )
    .expect_err("park_id 必须为正整数");

    assert!(matches!(err, ConfigError::Invalid(_)), "实际 {err:?}");
}

#[test]
fn defaults_apply_when_file_is_empty() {
    let path = write_config("");

    let config = load_file(&path, EnvConfig::default(), CliArgs::default());

    assert_eq!(config.server_url, DEFAULT_SERVER_URL);
    assert_eq!(config.device_id, DEFAULT_DEVICE_ID);
    assert_eq!(config.map_root, DEFAULT_MAP_ROOT);
    assert_eq!(config.api_password(), DEFAULT_API_PASSWORD);
    assert!(config.robot.is_none(), "未配置本体时应为模拟模式");
    assert!(config.upload_map);
    assert!(!config.map_only);
}

#[test]
fn heartbeat_and_simulation_are_clamped() {
    let path = write_config(
        r#"
[server]
heartbeat_secs = 0
[simulation]
battery = 120
signal = 9
"#,
    );

    let config = load_file(&path, EnvConfig::default(), CliArgs::default());

    assert_eq!(config.heartbeat_interval, 1);
    assert_eq!(config.battery, 100);
    assert_eq!(config.signal, 5);
}

#[test]
fn api_base_is_derived_from_server_url() {
    let file = FileConfig::default();
    let cli = CliArgs {
        server: Some("wss://ops.example.com:8443".to_string()),
        ..CliArgs::default()
    };

    let config = Config::resolve(&file, &EnvConfig::default(), &cli).expect("解析失败");

    assert_eq!(config.resolve_api_base(), "https://ops.example.com:8443");
}

#[test]
fn redacted_report_hides_secrets() {
    let path = write_config(
        r#"
[server]
secret = "super-secret-value"
[api]
password = "super-password-value"
"#,
    );

    let config = load_file(&path, EnvConfig::default(), CliArgs::default());
    let report = config.redacted_report();

    assert!(!report.contains("super-secret-value"));
    assert!(!report.contains("super-password-value"));
    assert!(report.contains("***"));
}
