//! 配置解析集成测试。
//!
//! 放在 `tests/` 而不是 `src/config.rs` 里，因为这些用例验证的是「文件 IO + TOML
//! 解析 + 三层优先级」的端到端行为，属于集成测试；`src/` 里的内联单测只覆盖纯函数。
//!
//! 每个用例使用独立临时文件，避免并行执行时互相干扰。

use robodog_device_client::config::{
    CliArgs, Config, ConfigError, DEFAULT_API_PASSWORD, DEFAULT_MAP_ROOT, DEFAULT_ROBOT_HOST,
    DEFAULT_SERVER_URL, ENV_SN_PATH, EnvConfig, FileConfig,
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

/// 假 SN 分区里的设备编号（等价本体 `/dev/mmcblk0p5` 第 3 行）
const SN_FROM_PARTITION: &str = "0202222TEST0001";

/// 造一个「SN 分区」临时文件：SN 写在偏移 0x20，其余为 0
fn write_sn_partition(sn: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "robodog-client-sn-{}-{}.bin",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    // 现场布局：前 512 字节全 0，SN 从 0x200 开始
    let mut bytes = vec![0u8; 0x300];
    let body = format!("{sn:<16}");
    bytes[0x200..0x210].copy_from_slice(body.as_bytes());
    std::fs::write(&path, bytes).expect("写入假 SN 分区失败");
    path
}

/// 默认注入假 SN 分区：测试环境没有 `/dev/mmcblk0p5`
fn env_with_sn(env: EnvConfig) -> EnvConfig {
    let path = write_sn_partition(SN_FROM_PARTITION);
    env.with(ENV_SN_PATH, path.display().to_string())
}

fn load_file(path: &std::path::Path, env: EnvConfig, cli: CliArgs) -> Config {
    let cli = CliArgs {
        config: Some(path.to_path_buf()),
        ..cli
    };
    Config::load(cli, env_with_sn(env)).expect("配置应能解析")
}

#[test]
fn file_values_are_used_when_cli_and_env_absent() {
    let path = write_config(
        r#"
[server]
url = "ws://10.0.0.12:3100"
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
position = "1.0, 2.0"
firmware = "Hermes_M_v9"

[log]
filter = "robodog_device_client=debug"
"#,
    );

    let config = load_file(&path, EnvConfig::default(), CliArgs::default());

    assert_eq!(config.server_url, "ws://10.0.0.12:3100");
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
    // 未指定本体时用现场默认本体
    assert_eq!(config.robot.as_deref(), Some(DEFAULT_ROBOT_HOST));
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
        env_with_sn(EnvConfig::default()),
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
        env_with_sn(EnvConfig::default()),
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
        env_with_sn(EnvConfig::default()),
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
        env_with_sn(EnvConfig::default()),
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
        env_with_sn(EnvConfig::default()),
    )
    .expect_err("park_id 必须为正整数");

    assert!(matches!(err, ConfigError::Invalid(_)), "实际 {err:?}");
}

#[test]
fn defaults_apply_when_file_is_empty() {
    let path = write_config("");

    let config = load_file(&path, EnvConfig::default(), CliArgs::default());

    assert_eq!(config.server_url, DEFAULT_SERVER_URL);
    assert_eq!(
        config.device_id, SN_FROM_PARTITION,
        "设备编号应来自本体 SN 分区，而不是配置/常量"
    );
    assert_eq!(config.map_root, DEFAULT_MAP_ROOT);
    assert_eq!(config.api_password(), DEFAULT_API_PASSWORD);
    assert_eq!(
        config.robot.as_deref(),
        Some(DEFAULT_ROBOT_HOST),
        "未配置本体时应指向现场本体，而不是模拟模式"
    );
    assert!(config.upload_map);
    assert!(!config.map_only);
}

#[test]
fn simulation_alias_disables_robot() {
    // 内置默认值已指向现场本体，所以「不接本体」必须显式声明
    for alias in ["sim", "none", "off", "NONE"] {
        let config = Config::resolve(
            &FileConfig::default(),
            &EnvConfig::default(),
            &CliArgs {
                robot: Some(alias.to_string()),
                ..CliArgs::default()
            },
            Some(SN_FROM_PARTITION),
        )
        .expect("配置应能解析");
        assert!(config.robot.is_none(), "--robot {alias} 应关闭本体连接");
    }

    let path = write_config(
        r#"
[robot]
host = "sim"
"#,
    );
    let config = load_file(&path, EnvConfig::default(), CliArgs::default());
    assert!(config.robot.is_none(), "robot.host = sim 应关闭本体连接");
}

#[test]
fn heartbeat_and_simulation_are_clamped() {
    let path = write_config(
        r#"
[server]
heartbeat_secs = 0
[simulation]
battery = 120
"#,
    );

    let config = load_file(&path, EnvConfig::default(), CliArgs::default());

    assert_eq!(config.heartbeat_interval, 1);
    assert_eq!(config.battery, 100);
}

#[test]
fn api_base_is_derived_from_server_url() {
    let file = FileConfig::default();
    let cli = CliArgs {
        server: Some("wss://ops.example.com:8443".to_string()),
        ..CliArgs::default()
    };

    let config = Config::resolve(&file, &EnvConfig::default(), &cli, Some(SN_FROM_PARTITION))
        .expect("解析失败");

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

#[test]
fn device_id_comes_from_sn_partition_and_can_be_overridden() {
    let path = write_config("");
    let sn_path = write_sn_partition("0202222ABCDE001");
    let env = EnvConfig::default().with(ENV_SN_PATH, sn_path.display().to_string());

    // 不配置时：设备编号来自本体 SN 分区
    let config = Config::load(
        CliArgs {
            config: Some(path.clone()),
            ..CliArgs::default()
        },
        env.clone(),
    )
    .expect("配置应能解析");
    assert_eq!(config.device_id, "0202222ABCDE001");

    // 命令行覆盖优先（本地调试 / 模拟多设备）
    let config = Config::load(
        CliArgs {
            config: Some(path),
            device_id: Some("RD-DEBUG-01".to_string()),
            ..CliArgs::default()
        },
        env,
    )
    .expect("配置应能解析");
    assert_eq!(config.device_id, "RD-DEBUG-01");
}

#[test]
fn missing_sn_without_override_is_rejected() {
    let path = write_config("");
    let missing = std::env::temp_dir().join("robodog-client-sn-no-such-partition.bin");

    let err = Config::load(
        CliArgs {
            config: Some(path),
            ..CliArgs::default()
        },
        EnvConfig::default().with(ENV_SN_PATH, missing.display().to_string()),
    )
    .expect_err("读不到 SN 且未显式指定设备编号时必须报错");

    assert!(matches!(err, ConfigError::Invalid(_)), "实际 {err:?}");
    // 报错要带上「为什么读不到」和「怎么覆盖」，现场不用再翻文档
    let text = err.to_string();
    assert!(text.contains("读取设备 SN 失败"), "实际 {text}");
    assert!(text.contains("--device-id"), "实际 {text}");
}

#[test]
fn device_id_in_config_file_is_rejected() {
    let path = write_config(
        r#"
[server]
device_id = "RD-042"
"#,
    );

    let err = Config::load(
        CliArgs {
            config: Some(path),
            ..CliArgs::default()
        },
        env_with_sn(EnvConfig::default()),
    )
    .expect_err("设备编号不应再写在配置文件里");

    assert!(matches!(err, ConfigError::Toml { .. }), "实际 {err:?}");
}
