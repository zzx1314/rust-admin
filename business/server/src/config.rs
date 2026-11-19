//! 业务平台配置：**环境变量 > `server.toml` > 内置默认值**。
//!
//! 取向与设备端 `business/client` 一致（那边还多一层 CLI），只是服务端没有
//! 命令行参数：
//!
//! - 配置文件路径：`ROBODOG_SERVER_CONFIG`；未设置时尝试当前目录的 `server.toml`。
//!   默认文件不存在视为「只用环境变量」，静默跳过；**显式指定的文件不存在直接报错**，
//!   避免部署时把地址写错却被悄悄忽略。
//! - 环境变量永远优先，方便 systemd / 容器只覆盖个别项。
//! - 文件里的相对路径按**配置文件所在目录**解析，部署时不依赖当前工作目录。
//! - 未知字段报错：拼错的键静默失效比启动失败更难排查。
//! - 密钥（`jwt_secret` / `zentai.password` / `service_token`）仍建议只走环境变量，
//!   文件里放地址类配置。

use serde::Deserialize;
use std::env;
use std::path::{Path, PathBuf};

/// 默认配置文件（相对当前工作目录）。
pub const DEFAULT_CONFIG_FILE: &str = "server.toml";
/// 指定配置文件路径的环境变量。
pub const CONFIG_FILE_ENV: &str = "ROBODOG_SERVER_CONFIG";
/// 企业微信群机器人 webhook 的**部署级兜底**入口。
///
/// 常规做法是在平台设置页里填（`settings.wecom_webhook`，保存即生效、只回掩码）；
/// 这里配了但页面没配时才用它 —— 适合「锁定地址、不许页面上改」的部署。
pub const WECOM_WEBHOOK_ENV: &str = "ROBODOG_WECOM_WEBHOOK";

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub database_path: PathBuf,
    pub jwt_secret: String,
    pub jwt_ttl_hours: i64,
    pub web_dist: PathBuf,
    /// 根目录 backend 的 SQLite 文件，用于导入 dog_map 中已上传的 ROS 地图。
    pub backend_db_path: PathBuf,
    /// 中台（根 backend）服务地址，用于把机器人建图自动推入 dog_map。
    /// 为空时禁用自动回传，`/api/map-versions/ingest` 会返回 503。
    pub zentai_base_url: Option<String>,
    /// 中台登录账号（需具备地图上传权限）
    pub zentai_username: Option<String>,
    pub zentai_password: Option<String>,
    /// 自动上传的地图包本地暂存目录
    pub ingest_dir: PathBuf,
    /// 中台调用业务平台内部接口的服务令牌；为空时内部触发接口拒绝请求。
    pub service_token: Option<String>,
    /// 反向控制信道的中台地址（`ws://` / `wss://`）。
    ///
    /// 业务平台部署在内网、只能出站访问互联网时必须配置：中台无法主动连入，
    /// 改由本进程主动拨入中台的长连接接收指令。为空时保持「中台直连 HTTP」模式。
    pub zentai_ws_url: Option<String>,
    /// 设备心跳超时秒数：超过这个时长没心跳就把设备置为「离线」（看门狗）。
    ///
    /// 客户端默认 10s 一次心跳，取 9 倍留出网络重连窗口；WS 静默断开（拉闸、掉线）
    /// 收不到 Close 帧，没有看门狗会一直显示「在线」。
    pub device_timeout_secs: i64,
    /// 任务执行超时秒数：`执行中` 的任务超过这个时长没有进度上报，看门狗判定为「异常」。
    ///
    /// 客户端在跑任务时每 5s 上报一次进度；客户端重启 / 断连 / 平台只发了
    /// `task_control` 没发 `task_dispatch` 时，平台侧收不到任何后续上报，
    /// 任务会永久停在「执行中」——必须有一条时间兜底。
    pub task_timeout_secs: i64,
    /// 任务进度停滞秒数：`执行中` 的任务进度数值超过这个时长没有变化，判定为「异常」。
    ///
    /// 只靠 `task_timeout_secs` 不够：客户端没有执行上下文时仍会周期性上报
    /// 「执行中 / 进度不变」，`updated_at` 一直是新鲜的，看门狗永远不触发。
    /// 取 10 分钟——正常航段/作业很少连续这么久没有一点进度推进。
    pub task_stuck_secs: i64,
    /// 数据采集背包板端 Agent 地址（`http://<板IP>:8000`）。
    ///
    /// 为空表示本部署未接入背包：`/api/backpack/*` 统一返回 503 并说明配置方法，
    /// 前端页面给出「未接入」占位，而不是转圈等超时。
    pub backpack_base_url: Option<String>,
    /// 航点 AI 识别的调用方（`auto` / `client` / `server`）。
    ///
    /// 正式环境的业务平台在公网、背包 Agent 只在现场内网，服务端直连必然超时；
    /// 设备客户端跑在同一块板子上（`127.0.0.1:8000`），由它代调才通。
    /// `auto`（默认）按「有没有配 `backpack_base_url`」自动选：
    /// - 配了（服务端与板端同网段）→ `server`，沿用原来的直连逻辑；
    /// - 没配（正式环境）→ `client`，平台下发 `ai_start`，客户端本地启停 Agent 并回传检出。
    pub point_ai: String,
    /// 识别窗口时长上限（秒）。默认 300；窗口本身在「离开航点」时立即结束。
    pub ai_window_max_secs: u64,
    /// 浏览器直连 mediamtx 的 WebRTC 信令地址（`http://<板IP>:8889`）。
    ///
    /// WHEP 协商与媒体**都由浏览器直连板端**，不经业务平台中继：
    /// 浏览器本来就要在背包所在局域网才能看画面，多一跳只会增加延时。
    /// 未配置时由 `backpack_base_url` / `backpack_browser_url` 推导（同主机、端口 8889）。
    pub backpack_webrtc_url: Option<String>,
    /// **平台侧** mediamtx 的信令地址（`http://127.0.0.1:8889`，默认只绑回环）。
    ///
    /// 有它才有一条「平台自己出 WebRTC」的通路：板端 relay 用 RTSP 把流转推到平台
    /// mediamtx（`deploy/external-hz/mediamtx/`），浏览器经 `/api/backpack/whep/<stream>`
    /// 同源反代拉 WHEP。和上面的 `backpack_webrtc_url`（浏览器直连板端）是两条完全
    /// 不同的路：前者受「操作员电脑必须在内网」限制，后者只要求板子能出公网。
    /// 为空时 `/api/backpack/whep/*` 返回 503（本部署没开 WebRTC 出口，走 FLV 中继）。
    pub mediamtx_url: Option<String>,
    /// **浏览器**可访问的 Agent 地址（`http://<板IP>:8000`），用于「直连模式」。
    ///
    /// 平台在公网、背包在现场内网时（本项目的正式环境），业务服务连不上 Agent，
    /// 但操作员的浏览器与背包同网段。此时由后端只下发这个地址，页面自己去调
    /// REST/SSE（Agent 已开 CORS），视频仍然是浏览器 ⇄ mediamtx 直连。
    /// 配置了本项即进入直连模式，`backpack_base_url` 不再参与。
    pub backpack_browser_url: Option<String>,
    /// 隧道指定的设备码（`ROBODOG_BACKPACK_TUNNEL_DEVICE`）。
    ///
    /// 平台与背包/浏览器都不同网段时，`/api/backpack/*` 走「设备客户端代调」：
    /// 板端客户端连得上本机 Agent，且有一条到平台的常驻 WS（见 `agent_tunnel.rs`）。
    /// 多背包部署必须显式指定；只有一台时留空即可（取最近上报的那台）。
    pub backpack_tunnel_device: Option<String>,
    /// 控制台对外访问地址（如 `http://39.107.55.13:3100`），用于拼告警消息里的链接。
    ///
    /// 为空时企业微信消息不带链接：猜一个地址比不给更容易误导人。
    pub public_url: Option<String>,
    /// 企业微信群机器人 webhook 是否启用（配置级总开关）。
    ///
    /// 与库里的 `alert_push` / `wecom_alert_push` 是**与**关系：这边是部署级
    /// （有没有这个通道），那边是运营级（这个班次推不推）。
    pub wecom_enabled: bool,
    /// 企业微信群机器人 webhook 地址（**部署级兜底**）。
    ///
    /// **地址本身就是凭据**（无加签），常规做法是在平台设置页里填
    /// （`settings.wecom_webhook`，`value_type='secret'`，保存即生效）；
    /// 这里的值只在平台配置为空时才生效。仍可从环境变量
    /// `ROBODOG_WECOM_WEBHOOK` 读（文件里出现时启动会 warn），方便「锁定地址、
    /// 不许页面上改」的部署。
    pub wecom_webhook: Option<String>,
    /// 推送级别下限：`严重` / `警告`（默认） / `提示`。
    pub wecom_min_level: String,
    /// 严重告警是否额外 @所有人。
    ///
    /// 注意：企微群机器人 markdown 不支持提及，实现上是**另发一条 text**
    /// 带 `mentioned_list: ["@all"]`，所以开这个开关会占双倍限流额度。
    pub wecom_mention_all: bool,
}

/// 配置文件读取 / 解析失败。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("配置文件 {path} 不存在（由 {env} 指定）")]
    Missing { path: String, env: String },
    #[error("配置文件 {path} 读取失败: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("配置文件 {path} 解析失败: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
}

/// `server.toml` 的结构。全部可选：缺省项回落到内置默认值。
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServerFile {
    port: Option<u16>,
    database: Option<String>,
    jwt_secret: Option<String>,
    jwt_ttl_hours: Option<i64>,
    web_dist: Option<String>,
    backend_db: Option<String>,
    ingest_dir: Option<String>,
    device_timeout_secs: Option<i64>,
    task_timeout_secs: Option<i64>,
    task_stuck_secs: Option<i64>,
    public_url: Option<String>,
    wecom: WecomFile,
    zentai: ZentaiFile,
    backpack: BackpackFile,
}

/// 企业微信群机器人推送配置。
///
/// webhook 地址等价于「谁拿到谁就能往群里发消息」，优先用环境变量
/// `ROBODOG_WECOM_WEBHOOK`；写在本文件里意味着密钥进 git，启动时会打一条警告。
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WecomFile {
    enabled: Option<bool>,
    webhook: Option<String>,
    min_level: Option<String>,
    mention_all: Option<bool>,
}

/// 数据采集背包（板端 Agent）接入配置。
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BackpackFile {
    /// 平台 → 背包 Agent（REST/SSE 代理目标）
    base_url: Option<String>,
    /// 航点 AI 识别的调用方：`auto`（默认，有 base_url 就服务端直连，否则交给设备客户端）
    /// / `client` / `server`
    point_ai: Option<String>,
    /// 识别窗口时长上限（秒）：窗口按「停留时长 + 8」算，用它兜底（默认 300 = 停留上限）。
    /// 运维担心误报（长停留期间镜头抖动把别的东西认成目标）时压小即可。
    ai_window_max_secs: Option<u64>,
    /// 浏览器 → mediamtx（WHEP 信令 + 媒体）
    webrtc_url: Option<String>,
    /// 平台侧 mediamtx 的信令地址（服务端反代 WHEP 用）
    mediamtx_url: Option<String>,
    /// 浏览器 → 板端 Agent（直连模式；正式环境用这个）
    browser_url: Option<String>,
    /// 板端 Agent 隧道指定的设备码（多背包部署时用）
    tunnel_device: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ZentaiFile {
    /// 反向控制信道地址（内网部署必需）
    ws_url: Option<String>,
    /// 中台 HTTP 地址（建图回传、账号登录、设备同步用）
    base_url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    service_token: Option<String>,
}

impl ServerFile {
    fn read(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })
    }
}

impl Config {
    /// 正式入口：按 环境变量 > `server.toml` > 默认值 组装配置。
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_file_path(env::var(CONFIG_FILE_ENV).ok())?;
        let (file, base_dir) = match &path {
            Some(path) => (Some(ServerFile::read(path)?), base_dir_of(path)),
            None => (None, None),
        };
        let config = Self::merge(file, base_dir, |name| env::var(name).ok());
        if config.wecom_webhook_source_is_file(|name| env::var(name).ok()) {
            tracing::warn!(
                "企业微信 webhook 来自配置文件：该地址本身就是凭据，建议改用环境变量 {WECOM_WEBHOOK_ENV}"
            );
        }
        Ok(config)
    }

    /// 只用环境变量的入口（忽略配置文件），兼容旧部署与脚本。
    pub fn from_env() -> Self {
        Self::merge(None, None, |name| env::var(name).ok())
    }

    fn merge<F>(file: Option<ServerFile>, base_dir: Option<PathBuf>, env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let file = file.unwrap_or_default();
        // 内置默认值按**当前工作目录**解析（部署时 = 安装目录）。
        // 以前这里用 `env!("CARGO_MANIFEST_DIR")`，会把编译机路径烧进二进制，
        // 拷到别的机器后默认路径全部失效。
        let base_dir_default = PathBuf::new();

        let text = |name: &str, from_file: Option<String>, default: String| -> String {
            env_text(&env, name)
                .or_else(|| from_file.and_then(clean))
                .unwrap_or(default)
        };
        let path = |name: &str, from_file: Option<String>, default: PathBuf| -> PathBuf {
            match env_text(&env, name) {
                Some(value) => PathBuf::from(value),
                None => file_path(from_file, base_dir.as_deref(), default),
            }
        };
        let url = |name: &str, from_file: Option<String>| -> Option<String> {
            env_text(&env, name)
                .or_else(|| from_file.and_then(clean))
                .map(|value| value.trim_end_matches('/').to_string())
        };

        Self {
            port: env_text(&env, "ROBODOG_PORT")
                .and_then(|value| value.parse().ok())
                .or(file.port)
                .unwrap_or(3100),
            database_path: path(
                "ROBODOG_DB",
                file.database,
                base_dir_default.join("data/robodog.db"),
            ),
            jwt_secret: text(
                "ROBODOG_JWT_SECRET",
                file.jwt_secret,
                "robodog-ops-development-secret-change-me".to_string(),
            ),
            jwt_ttl_hours: env_text(&env, "ROBODOG_JWT_TTL_HOURS")
                .and_then(|value| value.parse().ok())
                .or(file.jwt_ttl_hours)
                .unwrap_or(24),
            device_timeout_secs: env_text(&env, "ROBODOG_DEVICE_TIMEOUT_SECS")
                .and_then(|value| value.parse().ok())
                .or(file.device_timeout_secs)
                .unwrap_or(90)
                .max(20),
            task_timeout_secs: env_text(&env, "ROBODOG_TASK_TIMEOUT_SECS")
                .and_then(|value| value.parse().ok())
                .or(file.task_timeout_secs)
                .unwrap_or(120)
                .max(30),
            task_stuck_secs: env_text(&env, "ROBODOG_TASK_STUCK_SECS")
                .and_then(|value| value.parse().ok())
                .or(file.task_stuck_secs)
                .unwrap_or(600)
                .max(120),
            web_dist: path(
                "ROBODOG_WEB_DIST",
                file.web_dist,
                base_dir_default.join("../web/dist"),
            ),
            backend_db_path: path(
                "ROBODOG_BACKEND_DB",
                file.backend_db,
                base_dir_default.join("../../backend/data/users.db"),
            ),
            zentai_base_url: url("ROBODOG_ZENTAI_URL", file.zentai.base_url),
            zentai_username: env_text(&env, "ROBODOG_ZENTAI_USER")
                .or_else(|| file.zentai.username.and_then(clean)),
            zentai_password: env_text(&env, "ROBODOG_ZENTAI_PASSWORD")
                .or_else(|| file.zentai.password.and_then(clean)),
            ingest_dir: path(
                "ROBODOG_INGEST_DIR",
                file.ingest_dir,
                base_dir_default.join("data/map_ingest"),
            ),
            service_token: env_text(&env, "ROBODOG_SERVICE_TOKEN")
                .or_else(|| file.zentai.service_token.and_then(clean)),
            zentai_ws_url: url("ROBODOG_ZENTAI_WS_URL", file.zentai.ws_url),
            backpack_base_url: url("ROBODOG_BACKPACK_URL", file.backpack.base_url),
            // 不在这里兜底推导端口：`url()` 会去掉结尾斜杠，推导需要原始 host，
            // 统一放在 `backpack.rs::webrtc_base` 里做，配置项保持「原样或空」。
            backpack_webrtc_url: env_text(&env, "ROBODOG_BACKPACK_WEBRTC_URL")
                .or_else(|| file.backpack.webrtc_url.and_then(clean))
                .map(|value| value.trim_end_matches('/').to_string()),
            backpack_browser_url: url("ROBODOG_BACKPACK_BROWSER_URL", file.backpack.browser_url),
            // 平台侧 mediamtx（板端 RTSP 推上来 → 浏览器 WHEP 拉下去）。
            // 默认不配：只有部署了 mediamtx 的环境才该开这条通路（见
            // deploy/external-hz/mediamtx/），否则前端会去协商一条不存在的流。
            mediamtx_url: url(
                "ROBODOG_MEDIAMTX_URL",
                file.backpack.mediamtx_url,
            ),            backpack_tunnel_device: env_text(&env, "ROBODOG_BACKPACK_TUNNEL_DEVICE")
                .or_else(|| file.backpack.tunnel_device.and_then(clean)),
            point_ai: point_ai_source(
                env_text(&env, "ROBODOG_POINT_AI")
                    .or_else(|| file.backpack.point_ai.and_then(clean)),
            ),
            // 窗口上限：默认等于路线停留上限（300 s）。低于 10 s 没有意义，按 10 处理。
            ai_window_max_secs: env_text(&env, "ROBODOG_AI_WINDOW_MAX_SECS")
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .or(file.backpack.ai_window_max_secs)
                .unwrap_or(crate::ai_patrol::DEFAULT_WINDOW_MAX_SECS)
                .max(10),
            public_url: url("ROBODOG_PUBLIC_URL", file.public_url),
            wecom_enabled: env_flag(&env, "ROBODOG_WECOM_ENABLED")
                .or(file.wecom.enabled)
                .unwrap_or(true),
            wecom_webhook: url(WECOM_WEBHOOK_ENV, file.wecom.webhook),
            wecom_min_level: text(
                "ROBODOG_WECOM_MIN_LEVEL",
                file.wecom.min_level,
                "警告".to_string(),
            ),
            wecom_mention_all: env_flag(&env, "ROBODOG_WECOM_MENTION_ALL")
                .or(file.wecom.mention_all)
                .unwrap_or(false),
        }
    }

    /// 浏览器侧 WHEP 基址：显式配置优先，否则由浏览器可达的 Agent 地址推导（同主机 :8889）。
    ///
    /// 直连模式看 `browser_url`（浏览器真正访问的地址），代理模式看 `base_url`。
    pub fn backpack_webrtc_base(&self) -> Option<String> {
        self.backpack_webrtc_url.clone().or_else(|| {
            self.backpack_browser_url
                .as_deref()
                .and_then(derive_webrtc_base)
                .or_else(|| {
                    self.backpack_base_url
                        .as_deref()
                        .and_then(derive_webrtc_base)
                })
        })
    }

    /// 背包接法：`direct`（浏览器直连 Agent）优先，其次 `proxy`（业务服务代理），
    /// 都没配就是 `None`（页面展示配置指引）。
    pub fn backpack_mode(&self) -> Option<&'static str> {
        if self.backpack_browser_url.is_some() {
            Some("direct")
        } else if self.backpack_base_url.is_some() {
            Some("proxy")
        } else {
            None
        }
    }

    pub fn database_url(&self) -> String {
        format!("sqlite://{}", self.database_path.display())
    }

    /// 与中台通信是否就绪：地址 + （共享服务令牌 或 服务账号）。
    ///
    /// 只要有令牌就走内部接口 `/api/dogInternal/*`，不需要中台账号。
    pub fn zentai_ready(&self) -> bool {
        self.zentai_base_url.is_some()
            && (self.has_service_token()
                || (self.zentai_username.is_some() && self.zentai_password.is_some()))
    }

    /// 是否配置了共享服务令牌（决定走内部接口还是账号登录）。
    pub fn has_service_token(&self) -> bool {
        self.service_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
    }

    /// 是否允许「用用户自己的中台账号密码」登录业务平台。
    ///
    /// 只需要中台地址：密码由用户本人提供，不依赖配置里的中台服务账号；
    /// 分配关系查询另走服务令牌（`has_service_token`）或服务账号。
    pub fn zentai_login_ready(&self) -> bool {
        self.zentai_base_url.is_some()
    }

    /// 中台服务账号是否可用（老配置：用固定账号访问中台公开接口）。
    pub fn zentai_account_ready(&self) -> bool {
        self.zentai_base_url.is_some()
            && self.zentai_username.is_some()
            && self.zentai_password.is_some()
    }

    /// 与中台通信的鉴权方式，用于健康检查与日志。
    pub fn zentai_auth_mode(&self) -> &'static str {
        if self.zentai_base_url.is_none() {
            "none"
        } else if self.has_service_token() {
            "service-token"
        } else if self.zentai_username.is_some() && self.zentai_password.is_some() {
            "account"
        } else {
            "incomplete"
        }
    }

    /// 反向控制信道是否可用（地址 + 共享令牌齐全）
    pub fn control_channel_ready(&self) -> bool {
        self.zentai_ws_url.is_some() && self.service_token.is_some()
    }

    /// 配置层面是否就绪：开关打开 + **环境变量里**有 webhook。
    ///
    /// 注意运行时判定用 `Notifier::is_enabled()`：它还会看平台设置页里
    /// 配的地址（[`crate::notify::stored_webhook`]），这里只反映部署侧配置。
    pub fn wecom_ready(&self) -> bool {
        self.wecom_enabled
            && self
                .wecom_webhook
                .as_deref()
                .is_some_and(|url| !url.trim().is_empty())
    }

    /// webhook 是否来自配置文件（而不是环境变量）。启动时打一条警告用。
    pub fn wecom_webhook_source_is_file<F>(&self, env: F) -> bool
    where
        F: Fn(&str) -> Option<String>,
    {
        self.wecom_webhook.is_some() && env_text(&env, WECOM_WEBHOOK_ENV).is_none()
    }
}

/// 由 Agent 地址推导浏览器侧 WHEP 基址：同主机、端口换成 mediamtx 的 8889。
///
/// Agent 在 `:8000`、mediamtx 在 `:8889` 是背包 Demo 的固定布局，现场只需要
/// 告诉平台「板子在哪」，不必再填第二个地址；端口不同时用
/// `ROBODOG_BACKPACK_WEBRTC_URL` 显式覆盖。
fn derive_webrtc_base(base: &str) -> Option<String> {
    let rest = base.split_once("://").map(|(_, rest)| rest).unwrap_or(base);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // 去掉 `user:pass@`
    let host = host.rsplit_once('@').map(|(_, host)| host).unwrap_or(host);
    let host = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };
    (!host.is_empty()).then(|| format!("http://{host}:8889"))
}

/// 定位配置文件：显式指定的必须存在，默认路径缺失视为「不读文件」。
fn config_file_path(explicit: Option<String>) -> Result<Option<PathBuf>, ConfigError> {
    match explicit.and_then(clean) {
        Some(value) => {
            let path = PathBuf::from(value);
            if !path.is_file() {
                return Err(ConfigError::Missing {
                    path: path.display().to_string(),
                    env: CONFIG_FILE_ENV.to_string(),
                });
            }
            Ok(Some(path))
        }
        None => {
            let default = PathBuf::from(DEFAULT_CONFIG_FILE);
            Ok(default.is_file().then_some(default))
        }
    }
}

/// 航点 AI 调用方：只认 `auto` / `client` / `server`，其他值退回 `auto` 并告警。
///
/// 配置写错时**不能悄悄变成 server**：正式环境那样会让航点 AI 一直连不上板端，
/// 现场只看得到「没告警」。所以非法值一律回到 `auto`，由 `auto` 按部署形态判断。
fn point_ai_source(raw: Option<String>) -> String {
    match raw.as_deref() {
        Some("client") => "client".to_string(),
        Some("server") => "server".to_string(),
        Some("auto") | None => "auto".to_string(),
        Some(other) => {
            tracing::warn!(
                "point_ai={other:?} 不合法（只支持 auto / client / server），按 auto 处理"
            );
            "auto".to_string()
        }
    }
}

/// 配置文件所在目录（相对路径的解析基准）。
fn base_dir_of(path: &Path) -> Option<PathBuf> {
    path.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

fn env_text<F>(env: &F, name: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    env(name).and_then(clean)
}

/// 布尔环境变量：`1/true/yes/on` 为真，`0/false/no/off` 为假，其他值视为未设置。
fn env_flag<F>(env: &F, name: &str) -> Option<bool>
where
    F: Fn(&str) -> Option<String>,
{
    match env_text(env, name)?.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn clean(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 文件里的相对路径按配置文件目录解析；绝对路径原样使用。
fn file_path(from_file: Option<String>, base_dir: Option<&Path>, default: PathBuf) -> PathBuf {
    match from_file.and_then(clean) {
        Some(value) => {
            let path = PathBuf::from(value);
            match base_dir {
                Some(dir) if path.is_relative() => dir.join(path),
                _ => path,
            }
        }
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ServerFile {
        toml::from_str(text).expect("toml must parse")
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_apply_without_file_or_env() {
        let config = Config::merge(None, None, no_env);
        assert_eq!(config.port, 3100);
        assert_eq!(config.jwt_ttl_hours, 24);
        assert_eq!(config.task_timeout_secs, 120);
        assert_eq!(config.task_stuck_secs, 600);
        assert!(config.zentai_ws_url.is_none());
        assert!(config.service_token.is_none());
        assert!(!config.control_channel_ready());
    }

    #[test]
    fn file_values_are_used() {
        let config = Config::merge(
            Some(parse(
                r#"
                port = 4100
                jwt_ttl_hours = 6
                [zentai]
                ws_url = "ws://10.0.0.8:3000/"
                base_url = "http://10.0.0.8:3000/"
                username = "sysadmin"
                service_token = "from-file"
                "#,
            )),
            None,
            no_env,
        );
        assert_eq!(config.port, 4100);
        assert_eq!(config.jwt_ttl_hours, 6);
        assert_eq!(config.zentai_ws_url.as_deref(), Some("ws://10.0.0.8:3000"));
        assert_eq!(
            config.zentai_base_url.as_deref(),
            Some("http://10.0.0.8:3000")
        );
        assert_eq!(config.service_token.as_deref(), Some("from-file"));
        assert!(config.control_channel_ready());
    }

    #[test]
    fn env_wins_over_file() {
        let env = |name: &str| match name {
            "ROBODOG_PORT" => Some("5000".to_string()),
            "ROBODOG_ZENTAI_WS_URL" => Some("ws://env-host:3000".to_string()),
            _ => None,
        };
        let config = Config::merge(
            Some(parse(
                r#"
                port = 4100
                [zentai]
                ws_url = "ws://file-host:3000"
                service_token = "from-file"
                "#,
            )),
            None,
            env,
        );
        assert_eq!(config.port, 5000);
        assert_eq!(config.zentai_ws_url.as_deref(), Some("ws://env-host:3000"));
        // 环境变量没有覆盖的项仍来自文件
        assert_eq!(config.service_token.as_deref(), Some("from-file"));
    }

    #[test]
    fn blank_env_value_falls_back_to_file() {
        let env = |name: &str| match name {
            "ROBODOG_ZENTAI_WS_URL" => Some("   ".to_string()),
            _ => None,
        };
        let config = Config::merge(
            Some(parse("[zentai]\nws_url = \"ws://file-host:3000\"")),
            None,
            env,
        );
        assert_eq!(config.zentai_ws_url.as_deref(), Some("ws://file-host:3000"));
    }

    #[test]
    fn relative_paths_resolve_against_config_dir() {
        let config = Config::merge(
            Some(parse(
                r#"
                database = "data/x.db"
                web_dist = "dist"
                ingest_dir = "/var/lib/robodog/maps"
                "#,
            )),
            Some(PathBuf::from("/etc/robodog")),
            no_env,
        );
        assert_eq!(
            config.database_path,
            PathBuf::from("/etc/robodog/data/x.db")
        );
        assert_eq!(config.web_dist, PathBuf::from("/etc/robodog/dist"));
        assert_eq!(config.ingest_dir, PathBuf::from("/var/lib/robodog/maps"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<ServerFile>("prot = 3100").is_err());
        assert!(toml::from_str::<ServerFile>("[zentai]\nwss_url = \"x\"").is_err());
    }

    #[test]
    fn service_token_alone_is_enough_to_talk_to_zentai() {
        let env = |name: &str| match name {
            "ROBODOG_ZENTAI_URL" => Some("http://10.0.0.8:3000".to_string()),
            "ROBODOG_SERVICE_TOKEN" => Some("token-only".to_string()),
            _ => None,
        };
        let config = Config::merge(None, None, env);
        assert!(config.zentai_ready(), "令牌足以完成台账同步与地图回传");
        assert_eq!(config.zentai_auth_mode(), "service-token");
        // 中台账号登录路径关闭（没有账号可代理登录）
        assert!(!config.zentai_account_ready());
    }

    #[test]
    fn account_mode_still_works_without_service_token() {
        let env = |name: &str| match name {
            "ROBODOG_ZENTAI_URL" => Some("http://10.0.0.8:3000".to_string()),
            "ROBODOG_ZENTAI_USER" => Some("sysadmin".to_string()),
            "ROBODOG_ZENTAI_PASSWORD" => Some("secret".to_string()),
            _ => None,
        };
        let config = Config::merge(None, None, env);
        assert!(config.zentai_ready());
        assert!(!config.has_service_token());
        assert!(config.zentai_account_ready());
        assert_eq!(config.zentai_auth_mode(), "account");
    }

    #[test]
    fn address_only_reports_incomplete() {
        let env = |name: &str| match name {
            "ROBODOG_ZENTAI_URL" => Some("http://10.0.0.8:3000".to_string()),
            _ => None,
        };
        let config = Config::merge(None, None, env);
        assert!(!config.zentai_ready(), "没有令牌也没有账号，视为未就绪");
        assert_eq!(config.zentai_auth_mode(), "incomplete");
    }

    #[test]
    fn bundled_example_file_parses() {
        // 仓里的 server.toml.example 必须始终能解析，否则部署教程就是错的
        let file: ServerFile =
            toml::from_str(include_str!("../server.toml.example")).expect("示例配置必须可解析");
        assert_eq!(
            file.zentai.ws_url.as_deref(),
            Some("ws://192.168.41.10:3000")
        );
        assert_eq!(file.port, Some(3100));
    }

    #[test]
    fn wecom_defaults_to_disabled_until_webhook_is_set() {
        let config = Config::merge(None, None, no_env);
        assert!(config.wecom_enabled, "没配 webhook 时开关本身没有意义");
        assert!(config.wecom_webhook.is_none());
        assert!(!config.wecom_ready(), "没有 webhook 就不算就绪");
        assert_eq!(config.wecom_min_level, "警告");
        assert!(!config.wecom_mention_all);
        assert!(config.public_url.is_none());
    }

    #[test]
    fn wecom_ready_needs_both_switch_and_webhook() {
        let env = |name: &str| match name {
            "ROBODOG_WECOM_WEBHOOK" => {
                Some(" https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=k ".to_string())
            }
            _ => None,
        };
        let config = Config::merge(None, None, env);
        assert_eq!(
            config.wecom_webhook.as_deref(),
            Some("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=k")
        );
        assert!(config.wecom_ready());

        // 开关关掉后即使有 webhook 也不发
        let env = |name: &str| match name {
            "ROBODOG_WECOM_WEBHOOK" => Some("https://example.com/hook".to_string()),
            "ROBODOG_WECOM_ENABLED" => Some("false".to_string()),
            _ => None,
        };
        let config = Config::merge(None, None, env);
        assert!(!config.wecom_enabled);
        assert!(!config.wecom_ready());
    }

    #[test]
    fn wecom_file_section_is_flagged() {
        let config = Config::merge(
            Some(parse(
                r#"
                [wecom]
                webhook = "https://example.com/hook"
                min_level = "严重"
                mention_all = true
                "#,
            )),
            None,
            no_env,
        );
        assert_eq!(config.wecom_min_level, "严重");
        assert!(config.wecom_mention_all);
        // 文件里的地址能被识别出来，启动时打警告提示改用环境变量
        assert!(config.wecom_webhook_source_is_file(no_env));
        // 环境变量给了地址则不再告警（环境变量优先）
        let env = |name: &str| match name {
            "ROBODOG_WECOM_WEBHOOK" => Some("https://env.example.com/hook".to_string()),
            _ => None,
        };
        let config = Config::merge(
            Some(parse(
                "[wecom]\nwebhook = \"https://file.example.com/hook\"",
            )),
            None,
            env,
        );
        assert_eq!(
            config.wecom_webhook.as_deref(),
            Some("https://env.example.com/hook")
        );
        assert!(!config.wecom_webhook_source_is_file(|name| match name {
            "ROBODOG_WECOM_WEBHOOK" => Some("https://env.example.com/hook".to_string()),
            _ => None,
        }));
    }

    #[test]
    fn env_flag_accepts_common_truthy_values() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            let env = move |_: &str| Some(value.to_string());
            assert_eq!(env_flag(&env, "X"), Some(true), "{value} 应为真");
        }
        for value in ["0", "false", "no", "off"] {
            let env = move |_: &str| Some(value.to_string());
            assert_eq!(env_flag(&env, "X"), Some(false), "{value} 应为假");
        }
        // 认不出来的值当作未设置，回落到文件/默认值
        let env = |_: &str| Some("maybe".to_string());
        assert_eq!(env_flag(&env, "X"), None);
    }

    #[test]
    fn explicit_missing_config_file_is_an_error() {
        let error = config_file_path(Some("/nonexistent/server.toml".to_string()))
            .expect_err("显式指定的文件不存在必须报错");
        assert!(matches!(error, ConfigError::Missing { .. }));
    }

    #[test]
    fn webrtc_base_is_derived_from_agent_host() {
        // 现场固定布局：Agent :8000、mediamtx :8889 同主机
        assert_eq!(
            derive_webrtc_base("http://192.168.234.200:8000").as_deref(),
            Some("http://192.168.234.200:8889")
        );
        assert_eq!(
            derive_webrtc_base("http://192.168.234.200:8000/").as_deref(),
            Some("http://192.168.234.200:8889")
        );
        // 没有端口、带账号、带路径也要能取到主机
        assert_eq!(
            derive_webrtc_base("https://user:pass@board.local").as_deref(),
            Some("http://board.local:8889")
        );
        assert_eq!(
            derive_webrtc_base("http://10.0.0.8:8000/base/path").as_deref(),
            Some("http://10.0.0.8:8889")
        );
    }

    #[test]
    fn webrtc_base_prefers_browser_url_and_mode_is_direct() {
        let mut config = Config::from_env();
        config.backpack_base_url = None;
        config.backpack_webrtc_url = None;
        config.backpack_browser_url = Some("http://192.168.41.246:8000".to_string());
        assert_eq!(config.backpack_mode(), Some("direct"));
        assert_eq!(
            config.backpack_webrtc_base().as_deref(),
            Some("http://192.168.41.246:8889")
        );

        // 显式配置的 webrtc 地址优先于推导
        config.backpack_webrtc_url = Some("http://10.0.0.9:18889".to_string());
        assert_eq!(
            config.backpack_webrtc_base().as_deref(),
            Some("http://10.0.0.9:18889")
        );

        // 只有 base_url（平台能反代到 Agent）时是代理模式
        config.backpack_browser_url = None;
        config.backpack_webrtc_url = None;
        config.backpack_base_url = Some("http://192.168.41.246:8000".to_string());
        assert_eq!(config.backpack_mode(), Some("proxy"));

        // 全空 = 未接入
        config.backpack_base_url = None;
        assert_eq!(config.backpack_mode(), None);
    }

    #[test]
    fn default_missing_config_file_is_skipped() {
        // 默认路径在测试工作目录下通常不存在；存在也不影响断言
        let path = config_file_path(None).expect("默认路径缺失不算错误");
        assert!(path.is_none() || path.is_some_and(|path| path.is_file()));
    }
}
