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

/// 现场业务服务（192.168.41.246）的 WebSocket 地址。
pub const DEFAULT_SERVER_URL: &str = "ws://192.168.41.246:3100";

/// 现场机器狗本体地址（智航 Pro 网关，control/bulkdata 均为 10010）。
pub const DEFAULT_ROBOT_HOST: &str = "192.168.234.234";

pub const DEFAULT_MAP_ROOT: &str = "/ota/alg_data/map";
pub const DEFAULT_FIRMWARE: &str = "Hermes_M_v0.3.2";
pub const DEFAULT_HEARTBEAT_SECS: u64 = 10;
pub const DEFAULT_ROBOT_FILE_PORT: u16 = 12000;
pub const DEFAULT_BATTERY: i64 = 86;
pub const DEFAULT_SIGNAL: i64 = 4;
pub const DEFAULT_POSITION: &str = "0.0, 0.0";
pub const DEFAULT_API_USERNAME: &str = "admin";
pub const DEFAULT_API_PASSWORD: &str = "admin123";

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
/// 覆盖自动探测的设备编号（本地调试 / 多实例模拟用）
pub const ENV_DEVICE_ID: &str = "ROBODOG_DEVICE_ID";
/// 覆盖 SN 分区路径（默认 `/dev/mmcblk0p5`，测试与不同分区布局的机型用）
pub const ENV_SN_PATH: &str = "ROBODOG_SN_PATH";
pub const ENV_DEVICE_SECRET: &str = "ROBODOG_DEVICE_SECRET";
pub const ENV_ROBOT: &str = "ROBODOG_ROBOT";
pub const ENV_ROBOT_FILE_BASE: &str = "ROBODOG_ROBOT_FILE_BASE";
pub const ENV_ROBOT_FILE_PORT: &str = "ROBODOG_ROBOT_FILE_PORT";
pub const ENV_MAP_ROOT: &str = "ROBODOG_MAP_ROOT";
pub const ENV_HEARTBEAT: &str = "ROBODOG_HEARTBEAT";
pub const ENV_API_BASE: &str = "ROBODOG_API_BASE";
pub const ENV_API_USERNAME: &str = "ROBODOG_API_USERNAME";
pub const ENV_API_PASSWORD: &str = "ROBODOG_API_PASSWORD";
pub const ENV_PARK_ID: &str = "ROBODOG_PARK_ID";

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

// ─── 配置文件结构（client.toml） ───────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    pub server: ServerSection,
    pub api: ApiSection,
    pub robot: RobotSection,
    pub mapping: MappingSection,
    pub simulation: SimulationSection,
    pub log: LogSection,
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
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SimulationSection {
    pub battery: Option<i64>,
    pub signal: Option<i64>,
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
    pub heartbeat: Option<u64>,
    pub battery: Option<i64>,
    pub signal: Option<i64>,
    pub position: Option<String>,
    pub firmware: Option<String>,
    pub map_only: bool,
    pub map_name: Option<String>,
    pub map_kind: Option<String>,
    pub robot_file_base: Option<String>,
    pub robot_file_port: Option<u16>,
    /// `--no-upload-map` → `Some(false)`
    pub upload_map: Option<bool>,
    pub park_id: Option<i64>,
    pub ingest_map_name: Option<String>,
    pub api_base: Option<String>,
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
            heartbeat: opt(&mut args, "--heartbeat")?,
            battery: opt(&mut args, "--battery")?,
            signal: opt(&mut args, "--signal")?,
            position: opt(&mut args, "--position")?,
            firmware: opt(&mut args, "--firmware")?,
            map_only: args.contains("--map-only"),
            map_name: opt(&mut args, "--map-name")?,
            map_kind: opt(&mut args, "--map-kind")?,
            robot_file_base: opt(&mut args, "--robot-file-base")?,
            robot_file_port: opt(&mut args, "--robot-file-port")?,
            upload_map: args.contains("--no-upload-map").then_some(false),
            park_id: opt(&mut args, "--park-id")?,
            ingest_map_name: opt(&mut args, "--ingest-map-name")?,
            api_base: opt(&mut args, "--api-base")?,
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
  --map-root <PATH>      地图数据根目录 (默认: {DEFAULT_MAP_ROOT})
  --model <型号>         设备型号：本体 1201 `type` 为空时用它（留空则不报）
  --heartbeat <秒>       心跳间隔 (默认: {DEFAULT_HEARTBEAT_SECS})
  --battery <0-100>      初始电量 (默认: {DEFAULT_BATTERY})
  --signal <0-5>         初始信号强度 (默认: {DEFAULT_SIGNAL})
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
    pub heartbeat_interval: u64,
    pub battery: i64,
    pub signal: i64,
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
    /// 回传目标园区 ID（业务库 parks.id），显式指定时优先于服务端下发的绑定园区
    pub park_id: Option<i64>,
    /// 注册回执下发的绑定园区（控制台未绑定则为 None），建图回传时带上给中台
    pub bound_park_id: Arc<Mutex<Option<i64>>>,
    /// 回传使用的地图名称；为空则用本体返回的目录名
    pub ingest_map_name: Option<String>,
    /// 业务服务 REST 基地址（默认由 server_url 推导）
    pub api_base: Option<String>,
    /// 业务服务登录账号
    pub api_username: String,
    api_password: String,
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

        let heartbeat_interval = pick_number([
            cli.heartbeat,
            env.get_parsed(ENV_HEARTBEAT)?,
            file.server.heartbeat_secs,
        ])
        .unwrap_or(DEFAULT_HEARTBEAT_SECS)
        .max(1);

        let battery = pick_number([cli.battery, file.simulation.battery])
            .unwrap_or(DEFAULT_BATTERY)
            .clamp(0, 100);

        let signal = pick_number([cli.signal, file.simulation.signal])
            .unwrap_or(DEFAULT_SIGNAL)
            .clamp(0, 5);

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
            heartbeat_interval,
            battery,
            signal,
            position,
            firmware,
            map_only,
            map_name,
            map_kind,
            robot_file_base,
            robot_file_port,
            upload_map,
            park_id,
            bound_park_id: Arc::new(Mutex::new(None)),
            ingest_map_name,
            api_base,
            api_username,
            api_password,
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
        format!(
            "\
# 生效配置（敏感字段已脱敏）
server.url              = {server_url}
server.device_id        = {device_id}
server.secret           = {secret}
server.heartbeat_secs   = {heartbeat}
robot.host              = {robot}
robot.file_base         = {file_base}
robot.file_port         = {file_port}
robot.map_root          = {map_root}
robot.model             = {model}
api.base                = {api_base}
api.username            = {api_username}
api.password            = {api_password}
mapping.kind            = {map_kind}
mapping.name            = {map_name}
mapping.upload          = {upload_map}
mapping.park_id         = {park_id}
mapping.ingest_name     = {ingest_name}
mapping.once            = {map_only}
simulation.battery      = {battery}
simulation.signal       = {signal}
simulation.position     = {position}
simulation.firmware     = {firmware}
log.filter              = {log_filter}",
            server_url = self.server_url,
            device_id = self.device_id,
            secret = secret,
            heartbeat = self.heartbeat_interval,
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
            battery = self.battery,
            signal = self.signal,
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
