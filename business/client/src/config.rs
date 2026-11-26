//! 客户端配置：命令行 / 环境变量 / TOML 文件 / 内置默认值。
//!
//! 优先级（高 → 低）：
//!
//! ```text
//! 命令行 > 环境变量 > 配置文件 > 内置默认值
//! ```
//!
//! 配置文件默认读取工作目录下的 `client.toml`，也可以用 `--config <path>` 或
//! 环境变量 `ROBODOG_CLIENT_CONFIG` 指定。**显式指定**的文件不存在会报错，
//! 默认路径不存在则视为「全部使用默认值」，保持开箱可用。
//!
//! **设备编号不写配置**：默认在启动时从本体 SN 分区 `/dev/mmcblk0p5` 读出
//! （见 [`crate::device_sn`]），命令行 / 环境变量只用于本地调试时覆盖。
//!
//! 密钥（设备密钥 / 业务服务密码）支持两种写法，推荐环境变量：
//!
//! ```toml
//! [server]
//! secret_env = "ROBODOG_DEVICE_SECRET"   # 从环境变量读取，优先级高于 secret
//! [api]
//! password_env = "ROBODOG_API_PASSWORD"
//! ```

use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

// ─── 默认值 ────────────────────────────────────────────────────

/// 正式环境业务平台（外网）的 WebSocket 地址。
///
/// 现场机器人 / 客户端机默认连这台；内网临时联调用 `--server`、
/// `ROBODOG_SERVER_URL` 或 `/etc/robodog/client.toml` 覆盖。
pub const DEFAULT_SERVER_URL: &str = "ws://39.107.55.13:3100";

/// 现场机器狗本体地址（智航 Pro 网关，control/bulkdata 均为 10010）。
pub const DEFAULT_ROBOT_HOST: &str = "192.168.234.234";

pub const DEFAULT_MAP_ROOT: &str = "/ota/alg_data/map";
pub const DEFAULT_FIRMWARE: &str = "Hermes_M_v0.3.2";
pub const DEFAULT_HEARTBEAT_SECS: u64 = 10;
pub const DEFAULT_ROBOT_FILE_PORT: u16 = 12000;
pub const DEFAULT_BATTERY: i64 = 86;
pub const DEFAULT_POSITION: &str = "0.0, 0.0";
pub const DEFAULT_API_USERNAME: &str = "admin";
pub const DEFAULT_API_PASSWORD: &str = "admin123";
/// 地图定时增量同步间隔（秒）：只补平台没有的图，`0` = 关闭。
///
/// 遥控器/官方 App 建的图不会走 4001 自动回传，靠这个轮询把本体上「平台还没有」
/// 的地图补上去；中台点「同步地图」仍然走全量刷新。
pub const DEFAULT_MAP_SYNC_INTERVAL_SECS: u64 = 600;
/// 航点 AI 识别：板端 Agent 地址。
///
/// 默认 `127.0.0.1:8000` —— 背包 Agent 与设备客户端跑在同一块板子（现场 `.246`）上，
/// 只有本机能通；业务平台在公网够不到它，所以要由客户端本地代调（见 `ai_patrol`）。
pub const DEFAULT_AI_AGENT_URL: &str = "http://127.0.0.1:8000";

/// 显式关闭本体连接（纯模拟模式）的取值：`--robot sim` / `robot.host = "sim"`。
///
/// 内置默认值已指向现场本体，所以「不接本体」必须显式声明，
/// 不能再靠留空表达（留空 = 用默认本体地址）。
pub const SIMULATION_ALIASES: [&str; 5] = ["sim", "simulation", "none", "off", "无"];

/// 默认配置文件（相对工作目录）
pub const DEFAULT_CONFIG_FILE: &str = "client.toml";
/// 指定配置文件路径的环境变量
pub const CONFIG_PATH_ENV: &str = "ROBODOG_CLIENT_CONFIG";

// 环境变量名：与配置文件字段一一对应
pub const ENV_SERVER_URL: &str = "ROBODOG_SERVER_URL";
/// 平台证书（PEM）：`server.url = wss://…` 且平台用自签证书时必需
pub const ENV_SERVER_TLS_CA: &str = "ROBODOG_SERVER_TLS_CA";
/// 覆盖自动探测的设备编号（本地调试 / 多实例模拟用）
pub const ENV_DEVICE_ID: &str = "ROBODOG_DEVICE_ID";
/// 覆盖 SN 分区路径（默认 `/dev/mmcblk0p5`，测试与不同分区布局的机型用）
pub const ENV_SN_PATH: &str = "ROBODOG_SN_PATH";
pub const ENV_DEVICE_SECRET: &str = "ROBODOG_DEVICE_SECRET";
pub const ENV_ROBOT: &str = "ROBODOG_ROBOT";
pub const ENV_ROBOT_FILE_BASE: &str = "ROBODOG_ROBOT_FILE_BASE";
pub const ENV_ROBOT_FILE_PORT: &str = "ROBODOG_ROBOT_FILE_PORT";
pub const ENV_MAP_ROOT: &str = "ROBODOG_MAP_ROOT";
/// 5000 的 `map_type` / `goal_type`：`auto`（默认）/ `on`（强制路网 3）/ `off`（强制 2D 栅格 1）
pub const ENV_ROAD_NETWORK: &str = "ROBODOG_ROAD_NETWORK";
/// 巡检路径名前缀（写进本体 `nav_paths/<前缀>-<task_code>`）
pub const ENV_PATROL_PATH_PREFIX: &str = "ROBODOG_PATROL_PATH_PREFIX";
/// 巡检任务的返充电量阈值（`min_recharge_battery`）
pub const ENV_MIN_RECHARGE_BATTERY: &str = "ROBODOG_MIN_RECHARGE_BATTERY";
pub const ENV_HEARTBEAT: &str = "ROBODOG_HEARTBEAT";
pub const ENV_API_BASE: &str = "ROBODOG_API_BASE";
pub const ENV_API_USERNAME: &str = "ROBODOG_API_USERNAME";
pub const ENV_API_PASSWORD: &str = "ROBODOG_API_PASSWORD";
pub const ENV_PARK_ID: &str = "ROBODOG_PARK_ID";
/// 地图定时增量同步间隔（秒）
pub const ENV_MAP_SYNC_INTERVAL: &str = "ROBODOG_MAP_SYNC_INTERVAL";
/// 航点 AI 识别：板端 Agent 地址（客户端本地代调）
pub const ENV_AI_AGENT_URL: &str = "ROBODOG_AI_AGENT_URL";
/// 航点 AI 识别总开关（`0` / `false` 关闭）
pub const ENV_AI_ENABLED: &str = "ROBODOG_AI_ENABLED";
/// 航点 / 航段录像总开关（`0` / `false` 关闭）。录像也走板端 Agent，默认开。
pub const ENV_RECORD_ENABLED: &str = "ROBODOG_RECORD_ENABLED";
/// 航点 / 航段语音播报总开关（`0` / `false` 关闭）。播报也走板端 Agent，默认开。
pub const ENV_SPEAK_ENABLED: &str = "ROBODOG_SPEAK_ENABLED";
/// 航点拍照总开关（`0` / `false` 关闭）。拍照也走板端 Agent，默认开。
pub const ENV_PHOTO_ENABLED: &str = "ROBODOG_PHOTO_ENABLED";
/// 航点 AI 识别：板端存证根目录（`detect_jobs`），命中帧在这里，检出告警靠它发证据图
pub const ENV_AI_JOBS_DIR: &str = "ROBODOG_AI_JOBS_DIR";

// ─── 错误类型 ──────────────────────────────────────────────────

#[derive(Debug)]
pub enum ConfigError {
    /// 命令行参数不合法（含未知参数）
    Args(String),
    /// 配置文件读取失败
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// 配置文件解析失败
    Toml {
        path: PathBuf,
        source: toml::de::Error,
    },
    /// 配置取值不合法
    Invalid(String),
    /// 设备编号缺失：SN 分区读不到，且没有显式覆盖
    MissingDeviceId,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Args(message) => write!(f, "参数错误: {message}"),
            Self::Io { path, source } => {
                write!(f, "读取配置文件 {} 失败: {source}", path.display())
            }
            Self::Toml { path, source } => {
                write!(f, "解析配置文件 {} 失败: {source}", path.display())
            }
            Self::Invalid(message) => write!(f, "配置不合法: {message}"),
            Self::MissingDeviceId => write!(
                f,
                "配置不合法: 未能自动读取设备编号（{} 不可读）：请确认以 root 运行、分区存在，\
                 或用 --device-id / {ENV_DEVICE_ID} 显式指定",
                crate::device_sn::DEFAULT_SN_PATH
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

// ─── 路网导航开关 ──────────────────────────────────────────────

/// `5000 params.map_type` / `goal_type` 的取值策略。
///
/// 协议 §2.5.1.4：`1` = 2D 栅格、`3` = Route 路网（`2` 是 3D 预留，暂不支持），
/// `goal_type` 必须与 `map_type` 一致，否则导航不会启动。
///
/// - [`Off`](RoadNetworkMode::Off)：强制 `1`（2D 栅格）。**默认值**：协议文档的导航任务
///   示例就是 `1`，且 2D 栅格不依赖本体上真的建好路网，现场最稳；
/// - [`Auto`](RoadNetworkMode::Auto)：服务端判「这张图有路网」（下发 `mapType=3`）
///   且本体地图 `map_route_exist=true` 时用 `3`，否则 `1`；
/// - [`On`](RoadNetworkMode::On)：强制 `3`；本体没有 `map.geojson` 时会被固件拒，
///   只用于验证路网能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoadNetworkMode {
    /// 强制 2D 栅格（默认）
    #[default]
    Off,
    Auto,
    On,
}

impl RoadNetworkMode {
    /// 协议里的 `map_type` / `goal_type` 数值（两者必须相同）
    pub fn map_type(self, route_is_road_network: bool, body_has_route: bool) -> u32 {
        match self {
            Self::On => 3,
            Self::Off => 1,
            Self::Auto if route_is_road_network && body_has_route => 3,
            Self::Auto => 1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

impl std::str::FromStr for RoadNetworkMode {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            // 空值 / default 走默认策略（off），不写显式值时行为一致
            "" | "default" => Ok(Self::default()),
            "on" | "1" | "true" | "route" | "路网" => Ok(Self::On),
            "off" | "0" | "false" | "grid" | "2d" | "栅格" => Ok(Self::Off),
            other => Err(format!("路网导航开关只接受 auto/on/off（收到 {other:?}）")),
        }
    }
}

// ─── 配置文件结构（client.toml） ───────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
// 顶层**不** deny_unknown_fields：现场 `/etc/robodog/client.toml` 里还留着已删除的
// `[video]` 段（2026-11-18 视频改由背包 relay 提供），升级后不能因为这一行起不来。
// 单个配置段仍然严格校验（拼错键名会报错）。
#[serde(default)]
pub struct FileConfig {
    pub server: ServerSection,
    pub api: ApiSection,
    pub robot: RobotSection,
    pub mapping: MappingSection,
    pub simulation: SimulationSection,
    pub ai: AiSection,
    pub log: LogSection,
}

/// 航点 AI 识别（板端 Agent）
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AiSection {
    /// 板端 Agent 基地址，默认 `http://127.0.0.1:8000`
    pub agent_url: Option<String>,
    /// 总开关，默认开启；没接背包的机器上会在到点时打一条 WARN 后跳过
    pub enabled: Option<bool>,
    /// 板端存证根目录（`detect_jobs`）：Agent 把命中帧写在
    /// `<jobs_dir>/<计划 id>/pt<航点下标>/`，检出告警的证据图从这里读。
    ///
    /// 留空 = 用板端默认位置（`/root/backpack_dashboard/backend/detect_jobs` 等），
    /// 且会从板端 `plan_hit.evidence` 自动学到真实路径。板子路径与其他部署不同时
    /// 才需要显式配（`[ai] jobs_dir` / `ROBODOG_AI_JOBS_DIR`）。
    pub jobs_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSection {
    pub url: Option<String>,
    /// 设备密钥明文；不推荐，优先用 `secret_env`
    pub secret: Option<String>,
    /// 保存设备密钥的环境变量名，优先级高于 `secret`
    pub secret_env: Option<String>,
    pub heartbeat_secs: Option<u64>,
    /// 平台证书（PEM 文件路径）：`server.url = wss://…` 且平台用自签证书时必须配。
    ///
    /// 现场只开 https 入口（公网 3008 关掉、只留 3443）时，设备 WS 与回传 HTTP 都要
    /// 信任这张证书；配了它就不需要（也不提供）「跳过校验」那种做法，见 `src/tls.rs`。
    pub tls_ca: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApiSection {
    /// 业务服务 REST 基地址；留空由 `server.url` 推导
    pub base: Option<String>,
    pub username: Option<String>,
    /// 业务服务登录密码明文；不推荐，优先用 `password_env`
    pub password: Option<String>,
    /// 保存登录密码的环境变量名，优先级高于 `password`
    pub password_env: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RobotSection {
    /// 智航本体地址；留空 = 模拟模式
    pub host: Option<String>,
    /// 本体地图文件服务端口（协议文档 §4，默认 12000）
    pub file_port: Option<u16>,
    /// 本体地图文件服务基地址；留空按 host + file_port 推导
    pub file_base: Option<String>,
    /// 本体地图根目录（4013 未返回时使用）
    pub map_root: Option<String>,
    /// 设备型号override：本体 `1201.type` 为空时用它（如 `ZSL-2-Ultra`）
    pub model: Option<String>,
    /// 5000 的 `map_type`/`goal_type`：`auto`（默认）/ `on`（强制路网 3）/ `off`（强制 2D 栅格 1）
    pub road_network: Option<String>,
    /// 巡检路径名前缀：写进本体 `nav_paths/<前缀>-<task_code>`，默认 `平台`
    pub patrol_path_prefix: Option<String>,
    /// 巡检任务返充电量阈值（`min_recharge_battery`），默认 25.0
    pub min_recharge_battery: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MappingSection {
    /// outdoor / indoor
    pub kind: Option<String>,
    /// 地图名；留空用 map-<时间戳>
    pub name: Option<String>,
    /// 是否在建图完成后回传中台
    pub upload: Option<bool>,
    /// 地图归属园区（业务库 parks.id）
    pub park_id: Option<i64>,
    /// 回传使用的地图名；留空用本体返回的目录名
    pub ingest_name: Option<String>,
    /// 只执行一次建图后退出
    pub once: Option<bool>,
    /// 地图定时增量同步间隔（秒），0 = 关闭
    pub sync_interval_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SimulationSection {
    pub battery: Option<i64>,
    pub position: Option<String>,
    pub firmware: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogSection {
    /// tracing 过滤器，等价于 RUST_LOG；RUST_LOG 优先
    pub filter: Option<String>,
}

// ─── 环境变量快照 ──────────────────────────────────────────────

/// 进程环境变量快照。
///
/// 显式快照而不是各处 `std::env::var`，让配置解析成为纯函数，测试可注入。
#[derive(Debug, Clone, Default)]
pub struct EnvConfig {
    vars: HashMap<String, String>,
}

impl EnvConfig {
    pub fn from_env() -> Self {
        Self {
            vars: std::env::vars()
                .filter(|(_, value)| !value.trim().is_empty())
                .collect(),
        }
    }

    /// 测试用：从键值对构造
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            vars: pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .filter(|(_, value)| !value.trim().is_empty())
                .collect(),
        }
    }

    /// 追加或覆盖一个键（测试用：注入 `ROBODOG_SN_PATH` 等）
    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        let value = value.into();
        if !value.trim().is_empty() {
            self.vars.insert(key.to_string(), value);
        }
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    fn get_string(&self, key: &str) -> Option<String> {
        self.get(key).map(|value| value.trim().to_string())
    }

    fn get_parsed<T>(&self, key: &str) -> Result<Option<T>, ConfigError>
    where
        T: std::str::FromStr,
        T::Err: fmt::Display,
    {
        match self.get(key) {
            Some(raw) => raw.trim().parse::<T>().map(Some).map_err(|error| {
                ConfigError::Invalid(format!("环境变量 {key}={raw:?} 不合法: {error}"))
            }),
            None => Ok(None),
        }
    }
}

// ─── 命令行 ────────────────────────────────────────────────────

/// 命令行参数（全部为 `Option`，`None` 表示未提供，交给下一层来源）
#[derive(Debug, Clone, Default)]
pub struct CliArgs {
    pub config: Option<PathBuf>,
    pub server: Option<String>,
    pub device_id: Option<String>,
    pub secret: Option<String>,
    pub robot: Option<String>,
    pub map_root: Option<String>,
    pub model: Option<String>,
    /// `--road-network auto|on|off`
    pub road_network: Option<RoadNetworkMode>,
    /// `--patrol-path-prefix`
    pub patrol_path_prefix: Option<String>,
    /// `--min-recharge-battery`
    pub min_recharge_battery: Option<f64>,
    pub heartbeat: Option<u64>,
    pub battery: Option<i64>,
    pub position: Option<String>,
    pub firmware: Option<String>,
    pub map_only: bool,
    pub map_name: Option<String>,
    pub map_kind: Option<String>,
    pub robot_file_base: Option<String>,
    pub robot_file_port: Option<u16>,
    /// `--no-upload-map` → `Some(false)`
    pub upload_map: Option<bool>,
    /// `--map-sync-interval <秒>`；`Some(0)` = 关闭定时同步
    pub map_sync_interval: Option<u64>,
    pub park_id: Option<i64>,
    pub ingest_map_name: Option<String>,
    pub api_base: Option<String>,
    pub ai_agent_url: Option<String>,
    /// `--no-ai` → `Some(false)`
    pub ai_enabled: Option<bool>,
    /// `--no-record` → `Some(false)`
    pub record_enabled: Option<bool>,
    /// `--no-speak` → `Some(false)`
    pub speak_enabled: Option<bool>,
    /// `--no-photo` → `Some(false)`
    pub photo_enabled: Option<bool>,
    pub api_username: Option<String>,
    pub api_password: Option<String>,
    /// 打印脱敏后的生效配置后退出
    pub print_config: bool,
}

impl CliArgs {
    /// 解析进程命令行；`-h/--help` 直接打印帮助并退出。
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut args = pico_args::Arguments::from_env();

        if args.contains(["-h", "--help"]) {
            print_help();
            std::process::exit(0);
        }

        let cli = Self {
            config: args
                .opt_value_from_str::<_, String>("--config")
                .map_err(|error| ConfigError::Args(format!("--config {error}")))?
                .map(PathBuf::from),
            server: opt(&mut args, "--server")?,
            device_id: opt(&mut args, "--device-id")?,
            secret: opt(&mut args, "--secret")?,
            robot: opt(&mut args, "--robot")?,
            map_root: opt(&mut args, "--map-root")?,
            model: opt(&mut args, "--model")?,
            road_network: opt(&mut args, "--road-network")?,
            patrol_path_prefix: opt(&mut args, "--patrol-path-prefix")?,
            min_recharge_battery: opt(&mut args, "--min-recharge-battery")?,
            heartbeat: opt(&mut args, "--heartbeat")?,
            battery: opt(&mut args, "--battery")?,
            position: opt(&mut args, "--position")?,
            firmware: opt(&mut args, "--firmware")?,
            map_only: args.contains("--map-only"),
            map_name: opt(&mut args, "--map-name")?,
            map_kind: opt(&mut args, "--map-kind")?,
            robot_file_base: opt(&mut args, "--robot-file-base")?,
            robot_file_port: opt(&mut args, "--robot-file-port")?,
            upload_map: args.contains("--no-upload-map").then_some(false),
            map_sync_interval: if args.contains("--no-map-sync") {
                Some(0)
            } else {
                opt(&mut args, "--map-sync-interval")?
            },
            park_id: opt(&mut args, "--park-id")?,
            ingest_map_name: opt(&mut args, "--ingest-map-name")?,
            api_base: opt(&mut args, "--api-base")?,
            ai_agent_url: opt(&mut args, "--ai-agent-url")?,
            ai_enabled: args.contains("--no-ai").then_some(false),
            record_enabled: args.contains("--no-record").then_some(false),
            speak_enabled: args.contains("--no-speak").then_some(false),
            photo_enabled: args.contains("--no-photo").then_some(false),
            api_username: opt(&mut args, "--api-username")?,
            api_password: opt(&mut args, "--api-password")?,
            print_config: args.contains("--print-config"),
        };

        let leftover = args.finish();
        if let Some(unknown) = leftover.first() {
            return Err(ConfigError::Args(format!(
                "未知参数 {}（--help 查看用法）",
                unknown.to_string_lossy()
            )));
        }

        Ok(cli)
    }
}

fn opt<T>(args: &mut pico_args::Arguments, key: &'static str) -> Result<Option<T>, ConfigError>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    args.opt_value_from_str(key)
        .map_err(|error| ConfigError::Args(format!("{key} {error}")))
}

pub fn print_help() {
    eprintln!(
        "\
机器人设备客户端（业务服务 ⇄ 智航 Pro 本体桥接）

用法: robodog-device-client [选项]

配置来源优先级: 命令行 > 环境变量 > 配置文件 > 默认值
配置文件默认读取 ./client.toml，可用 --config 或 ROBODOG_CLIENT_CONFIG 指定。

选项:
  --config <PATH>        配置文件路径 (默认: ./client.toml)
  --print-config         打印脱敏后的生效配置后退出
  --server <URL>         业务服务 WebSocket 地址 (默认: {DEFAULT_SERVER_URL})
  --device-id <ID>       设备编号（默认自动读取本体 SN 分区 /dev/mmcblk0p5）
  --secret <SECRET>      设备密钥 (可选，推荐 ROBODOG_DEVICE_SECRET)
  --robot <HOST>         智航本体地址，如 192.168.234.234 或 ws://ip:10010
                         (默认: {DEFAULT_ROBOT_HOST})；填 sim/none/off 关闭本体连接，
                         退化为纯模拟模式：只回传任务进度，不驱动真实机器人
                         填 mock = 进程内起一个假本体（联调用，能看到真实的
                         1201/1315/5009/5000 报文，详见 README）
  --map-root <PATH>      地图数据根目录 (默认: {DEFAULT_MAP_ROOT})
  任务形态固定为「巡检」= 5000 kind=4（与官方 App 一致）：先 4024 把路线写成本体
  nav_paths/<前缀>-<任务号>，请求只带 map_id+path_id，航点/到点动作/返程都由本体执行。
  旧的 5000 kind=2 导航模式已移除（`.1` 的 dog_task 与 `.234` 的 robots_dog_msgs
  版本不一致会崩，等厂商修复），详见 business/client/README.md。
  --patrol-path-prefix <P> 巡检路径名前缀 (默认: 平台)，落在本体 nav_paths/<前缀>-<任务号>
  --min-recharge-battery <N> 巡检返充电量阈值 min_recharge_battery (默认: 25)
  --road-network <MODE>  5000 的 map_type/goal_type: off|auto|on
                         off(默认)=强制 1(2D 栅格)；on=强制 3(路网)；
                         auto=服务端判这张图有路网（mapType=3）且本体地图有 map.geojson 时才用 3
                         （巡检 kind=4 时按巡检枚举换算：2D=0、路网=1）
  --model <型号>         设备型号：本体 1201 `type` 为空时用它（留空则不报）
  --heartbeat <秒>       心跳间隔 (默认: {DEFAULT_HEARTBEAT_SECS})
  --battery <0-100>      初始电量 (默认: {DEFAULT_BATTERY})
  --position <x,y>       初始地图坐标 (默认: {DEFAULT_POSITION})
  --firmware <版本>      固件版本 (默认: {DEFAULT_FIRMWARE})
  --map-only             只执行一次建图（需要 --robot）后退出
  --map-name <名称>      建图任务名称 (默认: map-<时间戳>)
  --map-kind <类型>      outdoor / indoor (默认: outdoor)

建图回传中台（A 方案）:
  --park-id <ID>         地图归属园区（业务库 parks.id），不填则回传时跳过
  --robot-file-base <URL> 本体文件服务地址，如 http://192.168.168.100:12000
  --robot-file-port <端口> 本体文件服务端口 (默认: {DEFAULT_ROBOT_FILE_PORT}，文档 §4)
  --ingest-map-name <名称> 入库地图名 (默认用本体返回的目录名)
  --no-upload-map        关闭建图完成后自动回传
  --map-sync-interval <秒> 地图定时增量同步间隔 (默认: {DEFAULT_MAP_SYNC_INTERVAL_SECS}；只补平台没有的图)
  --no-map-sync          关闭地图定时增量同步（中台「同步地图」仍可用）
  --ai-agent-url <URL>   板端 AI Agent 地址 (默认: {DEFAULT_AI_AGENT_URL})
                         背包 Agent 与客户端同机，航点 AI 识别由客户端本地调用
  --no-ai                关闭航点 AI 识别（到点不再本地开识别）
  --no-record            关闭航点 / 航段录像（到点不再调板端 Agent 录 mp4）
  --no-speak             关闭航点 / 航段语音播报（到点不再调板端 Agent 播音）
  --no-photo             关闭航点拍照（到点不再调板端 Agent 抓拍照片）
  --api-base <URL>       业务服务 REST 地址 (默认由 --server 推导)
  --api-username <账号>  业务服务登录账号 (默认: {DEFAULT_API_USERNAME})
  --api-password <密码>  业务服务登录密码 (默认: {DEFAULT_API_PASSWORD}，线上必须覆盖)
  -h, --help             显示帮助"
    );
}

// ─── 生效配置 ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Config {
    pub server_url: String,
    pub device_id: String,
    pub device_secret: Option<String>,
    /// 智航本体地址；为空则以模拟模式运行
    pub robot: Option<String>,
    /// 地图数据根目录（4013 未返回时使用）
    pub map_root: String,
    /// 设备型号：优先用本体 1201 `type`（运行时取），这里只是配置兵底
    pub model: Option<String>,
    /// 5000 的 `map_type`/`goal_type` 策略（见 [`RoadNetworkMode`]）
    pub road_network: RoadNetworkMode,
    /// 巡检路径名前缀（本体 `nav_paths/<前缀>-<task_code>`）
    pub patrol_path_prefix: String,
    /// 巡检任务返充电量阈值
    pub min_recharge_battery: f64,
    pub heartbeat_interval: u64,
    pub battery: i64,
    pub position: String,
    pub firmware: String,
    /// 只做一次建图后退出
    pub map_only: bool,
    pub map_name: String,
    /// 建图类型：indoor / outdoor
    pub map_kind: String,
    /// 本体地图文件服务基地址（A 方案：从本体 HTTP 拉取地图包）
    /// 为空时按 `robot` 推导为 `http://<host>:<file-port>`
    pub robot_file_base: Option<String>,
    /// 本体地图文件服务端口（文档 §4：HTTP 12000）
    pub robot_file_port: u16,
    /// 建图完成后是否自动回传中台地图库
    pub upload_map: bool,
    /// 地图定时增量同步间隔（秒），0 = 关闭
    pub map_sync_interval_secs: u64,
    /// 回传目标园区 ID（业务库 parks.id），显式指定时优先于服务端下发的绑定园区
    pub park_id: Option<i64>,
    /// 注册回执下发的绑定园区（控制台未绑定则为 None），建图回传时带上给中台
    pub bound_park_id: Arc<Mutex<Option<i64>>>,
    /// 回传使用的地图名称；为空则用本体返回的目录名
    pub ingest_map_name: Option<String>,
    /// 板端 Agent 基地址（客户端本地代调，服务端够不到板端）。
    ///
    /// 两个用途共用这一份配置：巡检到点的 AI 识别（`ai_patrol`），以及把背包页
    /// 要的模型目录 / 运行态 / AI 启停借给平台（`agent_relay`，见隧道）。
    pub ai_agent_url: String,
    /// 板端存证根目录（`detect_jobs`）：显式配置优先，留空由 `ai_patrol` 自己探/学
    pub ai_jobs_dir: Option<String>,
    /// 板端 Agent 代调总开关（`--no-ai` 关闭）：关掉后既不注册航点 AI 窗口，
    /// 也不向平台发 `agent_hello`（背包页就只剩现场入口能用）
    pub ai_enabled: bool,
    /// 航点 / 航段录像总开关（`--no-record` / `ROBODOG_RECORD_ENABLED=0`）：
    /// 关掉后不再调板端 `/api/record/*`，也不会回传 mp4。
    pub record_enabled: bool,
    /// 航点 / 航段语音播报总开关（`--no-speak` / `ROBODOG_SPEAK_ENABLED=0`）：
    /// 关掉后不再调板端 `/api/speak*`（现场「别在夜里喊」/ 静默试跑时用）。
    pub speak_enabled: bool,
    /// 航点拍照总开关（`--no-photo` / `ROBODOG_PHOTO_ENABLED=0`）：
    /// 关掉后不再调板端 `/api/photo*`，也不会回传照片。
    pub photo_enabled: bool,
    /// 业务服务 REST 基地址（默认由 server_url 推导）
    pub api_base: Option<String>,
    /// 业务服务登录账号
    pub api_username: String,
    api_password: String,
    /// 平台证书（PEM）路径：`wss://` + 自签证书时由 `tls::init` 读成信任根
    pub tls_ca: Option<PathBuf>,
    /// tracing 过滤器（RUST_LOG 优先）
    pub log_filter: Option<String>,
}

impl Config {
    /// 按「命令行 > 环境变量 > 配置文件 > 默认值」解析生效配置。
    pub fn load(cli: CliArgs, env: EnvConfig) -> Result<Self, ConfigError> {
        let explicit_path = cli
            .config
            .clone()
            .or_else(|| env.get_string(CONFIG_PATH_ENV).map(PathBuf::from));

        let file = match &explicit_path {
            Some(path) => load_file(path)?,
            None => load_optional_file(Path::new(DEFAULT_CONFIG_FILE))?,
        };

        // 设备编号：默认取本体 SN 分区（`hexdump -C /dev/mmcblk0p5` 第 3 行）
        let sn_path = env
            .get(ENV_SN_PATH)
            .unwrap_or(crate::device_sn::DEFAULT_SN_PATH);
        let detected = crate::device_sn::detect(Path::new(sn_path));
        let detected_sn = detected.as_ref().ok().map(String::as_str);

        Self::resolve(&file, &env, &cli, detected_sn).map_err(|error| match (error, &detected) {
            // 把「读不到 SN 的具体原因」带进错误信息，现场排障不用再跑命令
            (ConfigError::MissingDeviceId, Err(sn_error)) => ConfigError::Invalid(format!(
                "{sn_error}；可用 --device-id 或 {ENV_DEVICE_ID} 显式指定"
            )),
            (error, _) => error,
        })
    }

    /// 纯函数式合并：不读环境、不读文件（便于测试）。
    ///
    /// `detected_sn` 是 [`crate::device_sn::detect`] 的结果，由 [`Config::load`] 传入；
    /// `None` 表示没读到本体 SN，此时必须用 `--device-id` / `ROBODOG_DEVICE_ID` 覆盖。
    pub fn resolve(
        file: &FileConfig,
        env: &EnvConfig,
        cli: &CliArgs,
        detected_sn: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let server_url = normalize_server_url(
            pick_string([
                cli.server.as_deref(),
                env.get(ENV_SERVER_URL),
                file.server.url.as_deref(),
            ])
            .unwrap_or(DEFAULT_SERVER_URL),
        )?;

        // 设备编号：命令行 / 环境变量显式覆盖 > 本体 SN 分区自动探测。
        // 配置文件里不支持写死——一台机器狗一个 SN，抄错会注册出幽灵设备。
        let device_id = pick_string([cli.device_id.as_deref(), env.get(ENV_DEVICE_ID)])
            .or(detected_sn)
            .ok_or(ConfigError::MissingDeviceId)?
            .to_string();

        // 密钥：命令行 > 环境变量 > 文件里的 env 引用 > 文件明文
        let device_secret = pick_string([cli.secret.as_deref(), env.get(ENV_DEVICE_SECRET)])
            .map(str::to_string)
            .or_else(|| {
                file.server
                    .secret_env
                    .as_deref()
                    .and_then(|key| env.get_string(key))
            })
            .or_else(|| file.server.secret.clone());

        // 本体地址：显式 sim/none/off 关闭连接；未配置时用现场默认本体
        let robot = match pick_string([
            cli.robot.as_deref(),
            env.get(ENV_ROBOT),
            file.robot.host.as_deref(),
        ]) {
            Some(raw) if is_simulation_alias(raw) => None,
            Some(raw) => Some(raw.to_string()),
            None => Some(DEFAULT_ROBOT_HOST.to_string()),
        };

        let map_root = pick_string([
            cli.map_root.as_deref(),
            env.get(ENV_MAP_ROOT),
            file.robot.map_root.as_deref(),
        ])
        .unwrap_or(DEFAULT_MAP_ROOT)
        .to_string();

        let model =
            pick_string([cli.model.as_deref(), file.robot.model.as_deref()]).map(str::to_string);

        // 5000 的 map_type/goal_type：显式配置优先，缺省 auto（按航段与本体地图能力判定）
        let road_network = match (
            cli.road_network,
            env.get(ENV_ROAD_NETWORK),
            file.robot.road_network.as_deref(),
        ) {
            (Some(value), _, _) => value,
            (None, Some(raw), _) => raw.parse::<RoadNetworkMode>().map_err(|error| {
                ConfigError::Invalid(format!("环境变量 {ENV_ROAD_NETWORK} 不合法: {error}"))
            })?,
            (None, None, Some(raw)) => raw.parse::<RoadNetworkMode>().map_err(|error| {
                ConfigError::Invalid(format!("配置项 robot.road_network 不合法: {error}"))
            })?,
            (None, None, None) => RoadNetworkMode::default(),
        };

        let patrol_path_prefix = pick_string([
            cli.patrol_path_prefix.as_deref(),
            env.get(ENV_PATROL_PATH_PREFIX),
            file.robot.patrol_path_prefix.as_deref(),
        ])
        .unwrap_or("平台")
        .trim()
        .to_string();
        let min_recharge_battery = pick_number([
            cli.min_recharge_battery,
            env.get_parsed(ENV_MIN_RECHARGE_BATTERY)?,
            file.robot.min_recharge_battery,
        ])
        .unwrap_or(25.0)
        .clamp(0.0, 100.0);

        let heartbeat_interval = pick_number([
            cli.heartbeat,
            env.get_parsed(ENV_HEARTBEAT)?,
            file.server.heartbeat_secs,
        ])
        .unwrap_or(DEFAULT_HEARTBEAT_SECS)
        .max(1);

        // 平台证书：只有 `wss://` 才用得上，但这里不强制（模拟 / 公网 CA 证书的场景
        // 不需要）；文件读不到 / 不是 PEM 由 `tls::init` 在启动时报错。
        let tls_ca = pick_string([
            env.get(ENV_SERVER_TLS_CA),
            file.server.tls_ca.as_deref(),
        ])
        .map(|raw| PathBuf::from(raw.trim()))
        .filter(|path| !path.as_os_str().is_empty());

        let battery = pick_number([cli.battery, file.simulation.battery])
            .unwrap_or(DEFAULT_BATTERY)
            .clamp(0, 100);

        let position = pick_string([cli.position.as_deref(), file.simulation.position.as_deref()])
            .unwrap_or(DEFAULT_POSITION)
            .to_string();

        let firmware = pick_string([cli.firmware.as_deref(), file.simulation.firmware.as_deref()])
            .unwrap_or(DEFAULT_FIRMWARE)
            .to_string();

        let map_kind = pick_string([cli.map_kind.as_deref(), file.mapping.kind.as_deref()])
            .unwrap_or("outdoor")
            .to_lowercase();
        if map_kind != "indoor" && map_kind != "outdoor" {
            return Err(ConfigError::Invalid(format!(
                "mapping.kind / --map-kind 只支持 indoor / outdoor，收到 {map_kind:?}"
            )));
        }

        let map_only = cli.map_only || file.mapping.once.unwrap_or(false);

        let map_name = pick_string([cli.map_name.as_deref(), file.mapping.name.as_deref()])
            .map(str::to_string)
            .unwrap_or_else(|| format!("map-{}", chrono::Local::now().format("%Y%m%d_%H%M%S")));

        let robot_file_base = pick_string([
            cli.robot_file_base.as_deref(),
            env.get(ENV_ROBOT_FILE_BASE),
            file.robot.file_base.as_deref(),
        ])
        .map(|value| value.trim_end_matches('/').to_string());

        let robot_file_port = pick_number([
            cli.robot_file_port,
            env.get_parsed(ENV_ROBOT_FILE_PORT)?,
            file.robot.file_port,
        ])
        .unwrap_or(DEFAULT_ROBOT_FILE_PORT);

        let upload_map = cli.upload_map.or(file.mapping.upload).unwrap_or(true);

        // 定时增量同步：关闭自动回传时一并关掉，否则「关着回传却偷偷上传地图」很反直觉
        let map_sync_interval_secs = pick_number([
            cli.map_sync_interval,
            env.get_parsed(ENV_MAP_SYNC_INTERVAL)?,
            file.mapping.sync_interval_secs,
        ])
        .unwrap_or(DEFAULT_MAP_SYNC_INTERVAL_SECS);
        let map_sync_interval_secs = if upload_map {
            map_sync_interval_secs
        } else {
            0
        };

        let park_id = match pick_number([
            cli.park_id,
            env.get_parsed(ENV_PARK_ID)?,
            file.mapping.park_id,
        ]) {
            Some(id) if id <= 0 => {
                return Err(ConfigError::Invalid(format!(
                    "park_id 必须是正整数，收到 {id}"
                )));
            }
            other => other,
        };

        let ingest_map_name = pick_string([
            cli.ingest_map_name.as_deref(),
            file.mapping.ingest_name.as_deref(),
        ])
        .map(str::to_string);

        let ai_agent_url = pick_string([
            cli.ai_agent_url.as_deref(),
            env.get(ENV_AI_AGENT_URL),
            file.ai.agent_url.as_deref(),
        ])
        .unwrap_or(DEFAULT_AI_AGENT_URL)
        .trim_end_matches('/')
        .to_string();
        let ai_enabled = cli
            .ai_enabled
            .or_else(|| env.get(ENV_AI_ENABLED).map(parse_bool))
            .or(file.ai.enabled)
            .unwrap_or(true);
        // 录像与 AI 共用板端 Agent，但开关独立：现场可能只想关识别省 NPU，录像照录
        let record_enabled = cli
            .record_enabled
            .or_else(|| env.get(ENV_RECORD_ENABLED).map(parse_bool))
            .unwrap_or(true);
        // 播报同理：TTS 在云端，断网 / 夜间不想出声时单独关（不影响 AI 与录像）
        let speak_enabled = cli
            .speak_enabled
            .or_else(|| env.get(ENV_SPEAK_ENABLED).map(parse_bool))
            .unwrap_or(true);
        // 拍照：流没推 / 只想跑导航时单独关（不影响 AI、录像、播报）
        let photo_enabled = cli
            .photo_enabled
            .or_else(|| env.get(ENV_PHOTO_ENABLED).map(parse_bool))
            .unwrap_or(true);
        let env_jobs_dir = env.get_string(ENV_AI_JOBS_DIR);
        let ai_jobs_dir =
            pick_string([env_jobs_dir.as_deref(), file.ai.jobs_dir.as_deref()]).map(str::to_string);

        let api_base = pick_string([
            cli.api_base.as_deref(),
            env.get(ENV_API_BASE),
            file.api.base.as_deref(),
        ])
        .map(|value| value.trim_end_matches('/').to_string());

        let api_username = pick_string([
            cli.api_username.as_deref(),
            env.get(ENV_API_USERNAME),
            file.api.username.as_deref(),
        ])
        .unwrap_or(DEFAULT_API_USERNAME)
        .to_string();

        let api_password = pick_string([cli.api_password.as_deref(), env.get(ENV_API_PASSWORD)])
            .map(str::to_string)
            .or_else(|| {
                file.api
                    .password_env
                    .as_deref()
                    .and_then(|key| env.get_string(key))
            })
            .or_else(|| file.api.password.clone())
            .unwrap_or_else(|| DEFAULT_API_PASSWORD.to_string());

        Ok(Self {
            server_url,
            device_id,
            device_secret,
            robot,
            map_root,
            model,
            road_network,
            patrol_path_prefix,
            min_recharge_battery,
            heartbeat_interval,
            battery,
            position,
            firmware,
            map_only,
            map_name,
            map_kind,
            robot_file_base,
            robot_file_port,
            upload_map,
            map_sync_interval_secs,
            park_id,
            bound_park_id: Arc::new(Mutex::new(None)),
            ingest_map_name,
            ai_agent_url,
            ai_jobs_dir,
            ai_enabled,
            record_enabled,
            speak_enabled,
            photo_enabled,
            api_base,
            api_username,
            api_password,
            tls_ca,
            log_filter: file.log.filter.clone(),
        })
    }

    /// 业务服务 REST 基地址：显式配置优先，否则由 WS 地址推导
    pub fn resolve_api_base(&self) -> String {
        if let Some(base) = &self.api_base {
            return base.trim_end_matches('/').to_string();
        }
        // ws://… → http://…；wss://… → https://…
        let (scheme, rest) = if let Some(rest) = self.server_url.strip_prefix("wss://") {
            ("https", rest)
        } else {
            ("http", self.server_url.trim_start_matches("ws://"))
        };
        let host_port = rest.split('/').next().unwrap_or(rest);
        format!("{scheme}://{host_port}")
    }

    pub fn api_password(&self) -> &str {
        &self.api_password
    }

    /// 脱敏后的生效配置，用于 `--print-config` 与现场排障
    pub fn redacted_report(&self) -> String {
        let secret = self
            .device_secret
            .as_deref()
            .map(mask)
            .unwrap_or_else(|| "（未配置）".to_string());
        let tls_ca = self
            .tls_ca
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "（未配置：用默认根证书）".to_string());
        format!(
            "\
# 生效配置（敏感字段已脱敏）
server.url              = {server_url}
server.device_id        = {device_id}
server.secret           = {secret}
server.heartbeat_secs   = {heartbeat}
server.tls_ca           = {tls_ca}
robot.host              = {robot}
robot.file_base         = {file_base}
robot.file_port         = {file_port}
robot.map_root          = {map_root}
robot.model             = {model}
route.road_network      = {road_network}
task.patrol_path_prefix = {patrol_path_prefix}
task.min_recharge_batt  = {min_recharge_battery}
ai.agent_url            = {ai_agent_url}
ai.jobs_dir             = {ai_jobs_dir}
ai.enabled              = {ai_enabled}
record.enabled          = {record_enabled}
speak.enabled           = {speak_enabled}
photo.enabled           = {photo_enabled}
api.base                = {api_base}
api.username            = {api_username}
api.password            = {api_password}
mapping.kind            = {map_kind}
mapping.name            = {map_name}
mapping.upload          = {upload_map}
mapping.park_id         = {park_id}
mapping.ingest_name     = {ingest_name}
mapping.once            = {map_only}
mapping.sync_interval_s = {map_sync}
simulation.battery      = {battery}
simulation.position     = {position}
simulation.firmware     = {firmware}
log.filter              = {log_filter}",
            server_url = self.server_url,
            device_id = self.device_id,
            secret = secret,
            heartbeat = self.heartbeat_interval,
            tls_ca = tls_ca,
            robot = self.robot.as_deref().unwrap_or("（模拟模式）"),
            file_base = self
                .robot_file_base
                .as_deref()
                .unwrap_or("（按 host 推导）"),
            file_port = self.robot_file_port,
            map_root = self.map_root,
            model = self
                .model
                .as_deref()
                .unwrap_or("（未配置，用本体 1201 type）"),
            road_network = self.road_network.as_str(),
            patrol_path_prefix = self.patrol_path_prefix,
            min_recharge_battery = self.min_recharge_battery,
            ai_agent_url = self.ai_agent_url,
            ai_jobs_dir = self
                .ai_jobs_dir
                .as_deref()
                .unwrap_or("（未配置，自动探测板端 detect_jobs）"),
            ai_enabled = self.ai_enabled,
            record_enabled = self.record_enabled,
            speak_enabled = self.speak_enabled,
            photo_enabled = self.photo_enabled,
            api_base = self.api_base.as_deref().unwrap_or("（按 server.url 推导）"),
            api_username = self.api_username,
            api_password = mask(&self.api_password),
            map_kind = self.map_kind,
            map_name = self.map_name,
            upload_map = self.upload_map,
            park_id = self
                .park_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "（未配置）".to_string()),
            ingest_name = self
                .ingest_map_name
                .as_deref()
                .unwrap_or("（用本体目录名）"),
            map_only = self.map_only,
            map_sync = if self.map_sync_interval_secs == 0 {
                "（已关闭）".to_string()
            } else {
                format!("{}s", self.map_sync_interval_secs)
            },
            battery = self.battery,
            position = self.position,
            firmware = self.firmware,
            log_filter = self.log_filter.as_deref().unwrap_or("（默认）"),
        )
    }
}

// ─── 内部工具 ──────────────────────────────────────────────────

fn load_file(path: &Path) -> Result<FileConfig, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| ConfigError::Toml {
        path: path.to_path_buf(),
        source,
    })
}

/// 默认路径：不存在不算错误（全部走默认值）
fn load_optional_file(path: &Path) -> Result<FileConfig, ConfigError> {
    if path.is_file() {
        load_file(path)
    } else {
        Ok(FileConfig::default())
    }
}

/// 取第一个非空字符串
fn pick_string<const N: usize>(candidates: [Option<&str>; N]) -> Option<&str> {
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
}

/// 取第一个有值的数字
fn pick_number<T: Copy, const N: usize>(candidates: [Option<T>; N]) -> Option<T> {
    candidates.into_iter().flatten().next()
}

/// 解析布尔开关：`1/0` / `true/false` / `on/off` / `yes/no`
fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes" | "y"
    )
}

/// 是否显式声明「不接本体」
fn is_simulation_alias(raw: &str) -> bool {
    SIMULATION_ALIASES
        .iter()
        .any(|alias| raw.eq_ignore_ascii_case(alias))
}

fn normalize_server_url(raw: &str) -> Result<String, ConfigError> {
    let trimmed = raw.trim().trim_end_matches('/');
    if !(trimmed.starts_with("ws://") || trimmed.starts_with("wss://")) {
        return Err(ConfigError::Invalid(format!(
            "server.url 必须以 ws:// 或 wss:// 开头，收到 {trimmed:?}"
        )));
    }
    Ok(trimmed.to_string())
}

fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 2 {
        return "**".to_string();
    }
    let head: String = chars.iter().take(1).collect();
    format!("{head}***（{} 字符）", chars.len())
}
