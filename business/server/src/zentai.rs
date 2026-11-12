//! 中台（仓库根目录 `backend`）对接：设备台账同步、分配查询、设备删除。
//!
//! 出站调用有两种鉴权方式，**优先共享服务令牌**：
//!
//! 1. `ROBODOG_SERVICE_TOKEN` → 中台内部接口 `/api/dogInternal/*`（推荐）：
//!    不需要中台账号，也没有会话过期问题，凭证只有一条共享令牌；
//! 2. 退化为 `ROBODOG_ZENTAI_USER` / `_PASSWORD` → 服务账号登录换 JWT，
//!    调用中台公开接口（兼容老部署）。
//!
//! 中台账号/令牌都只保存在业务服务端，设备客户端不持有任何中台凭证。

use crate::config::Config;
use crate::error::AppError;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

const TOKEN_TTL: Duration = Duration::from_secs(10 * 60);

/// 一次中台登录的结果
#[derive(Debug, Clone)]
pub struct ZentaiSession {
    pub token: String,
    pub user_id: i64,
    pub username: String,
    pub nickname: Option<String>,
    pub roles: Vec<i64>,
}

impl ZentaiSession {
    /// 中台内置超级管理员角色 id = 1（`p_sys_role.code = '110'`）。
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&1)
    }
}

/// 中台设备台账（只取业务平台关心的字段）
#[derive(Debug, Clone)]
pub struct ZentaiDevice {
    pub device_id: String,
    pub user_ids: Vec<i64>,
}

#[derive(Debug, Clone)]
struct CachedToken {
    base: String,
    username: String,
    session: ZentaiSession,
    fetched_at: std::time::Instant,
}

static SERVICE_TOKEN: OnceLock<Mutex<Option<CachedToken>>> = OnceLock::new();

fn service_token_cache() -> &'static Mutex<Option<CachedToken>> {
    SERVICE_TOKEN.get_or_init(|| Mutex::new(None))
}

fn http_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| AppError::Internal(format!("构造 HTTP 客户端失败: {error}")))
}

/// 出站调用的鉴权方式（建图回传等其它模块复用）。
pub(crate) enum Auth {
    /// 共享服务令牌（中台内部接口 `/api/dogInternal/*`）
    Service(String),
    /// 服务账号登录换来的 Bearer（中台公开接口）
    Bearer(String),
}

impl Auth {
    /// 优先共享服务令牌；未配置时退回账号登录。
    pub(crate) async fn resolve(config: &Config) -> Result<Self, AppError> {
        if let Some(token) = config
            .service_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            return Ok(Self::Service(token.to_string()));
        }
        let session = service_session(config).await?;
        Ok(Self::Bearer(session.token))
    }

    pub(crate) fn is_service(&self) -> bool {
        matches!(self, Self::Service(_))
    }

    pub(crate) fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::Service(token) => request.header("x-service-token", token),
            Self::Bearer(token) => request.bearer_auth(token),
        }
    }
}

/// 中台基地址（去掉尾部 `/`）。
pub(crate) fn zentai_base(config: &Config) -> Result<String, AppError> {
    config
        .zentai_base_url
        .as_deref()
        .map(str::trim)
        .filter(|base| !base.is_empty())
        .map(|base| base.trim_end_matches('/').to_string())
        .ok_or_else(|| AppError::Internal("未配置中台地址（ROBODOG_ZENTAI_URL）".to_string()))
}

/// 用指定账号登录中台。
pub async fn login(
    config: &Config,
    http: &reqwest::Client,
    username: &str,
    password: &str,
) -> Result<ZentaiSession, AppError> {
    let Some(base) = config.zentai_base_url.as_deref() else {
        return Err(AppError::Internal(
            "未配置中台地址（ROBODOG_ZENTAI_URL）".to_string(),
        ));
    };
    login_at(http, base, username, password).await
}

/// 用服务账号登录（带进程内缓存）。
pub async fn service_session(config: &Config) -> Result<ZentaiSession, AppError> {
    let (Some(base), Some(username), Some(password)) = (
        config.zentai_base_url.as_deref(),
        config.zentai_username.as_deref(),
        config.zentai_password.as_deref(),
    ) else {
        return Err(AppError::Internal(
            "未配置中台服务账号（ROBODOG_ZENTAI_URL / USER / PASSWORD）".to_string(),
        ));
    };

    let mut guard = service_token_cache().lock().await;
    if let Some(cached) = guard.as_ref()
        && cached.base == base
        && cached.username == username
        && cached.fetched_at.elapsed() < TOKEN_TTL
    {
        return Ok(cached.session.clone());
    }

    let http = http_client()?;
    let session = login_at(&http, base, username, password).await?;
    *guard = Some(CachedToken {
        base: base.to_string(),
        username: username.to_string(),
        session: session.clone(),
        fetched_at: std::time::Instant::now(),
    });
    Ok(session)
}

/// 登录中台拿 Bearer token。
///
/// 中台密码走 AES-128-CBC（ZeroPadding，key=iv=`Welcome Superred`）加密后
/// 以表单提交，与前端 `utils/aes.ts` 保持一致。
pub async fn login_at(
    http: &reqwest::Client,
    base: &str,
    username: &str,
    password: &str,
) -> Result<ZentaiSession, AppError> {
    use aes::Aes128;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::NoPadding};

    type Aes128CbcEnc = cbc::Encryptor<Aes128>;
    const KEY: &[u8; 16] = b"Welcome Superred";

    let plain = password.as_bytes();
    let padded_len = plain.len().div_ceil(16).max(1) * 16;
    let mut buf = vec![0u8; padded_len];
    buf[..plain.len()].copy_from_slice(plain);
    let cipher = Aes128CbcEnc::new(KEY.into(), KEY.into())
        .encrypt_padded_mut::<NoPadding>(&mut buf, padded_len)
        .map_err(|error| AppError::Internal(format!("密码加密失败: {error}")))?;
    let encrypted = BASE64.encode(cipher);

    let url = format!("{}/api/token", base.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .form(&[
            ("username", username),
            ("password", encrypted.as_str()),
            ("grant_type", "password"),
            ("scope", "server"),
        ])
        .send()
        .await
        .map_err(|error| {
            // 连不上中台 ≠ 账号密码错，必须区分开：前者是部署/网络问题（503）
            AppError::Upstream(format!("中台不可达（{base}），无法校验中台账号: {error}"))
        })?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::BAD_REQUEST
        {
            return Err(AppError::Unauthorized("中台账号或密码错误".to_string()));
        }
        return Err(AppError::Upstream(format!(
            "中台登录接口异常 HTTP {status}: {text}"
        )));
    }

    let value: Value = serde_json::from_str(&text)
        .map_err(|error| AppError::Upstream(format!("中台登录响应不是 JSON: {error}")))?;
    let data = value.get("data").unwrap_or(&value);
    let token = data
        .get("accessToken")
        .or_else(|| data.get("access_token"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::Upstream(format!("中台登录响应缺少 accessToken: {text}")))?;
    let user_id = data
        .get("userId")
        .or_else(|| data.get("user_id"))
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::Upstream(format!("中台登录响应缺少 userId: {text}")))?;
    let username = data
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or(username)
        .to_string();
    let nickname = data
        .get("nickname")
        .and_then(Value::as_str)
        .map(str::to_string);
    let roles = data
        .get("roles")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();

    Ok(ZentaiSession {
        token,
        user_id,
        username,
        nickname,
        roles,
    })
}

/// 读取中台设备台账与分配关系。
///
/// 内部接口 `/api/dogInternal/devices/assignments` 与公开的
/// `/api/dogDevice/getAll` 返回同一结构（`DeviceVO` 数组），解析逻辑共用一份。
pub async fn device_assignments(config: &Config) -> Result<Vec<ZentaiDevice>, AppError> {
    let base = zentai_base(config)?;
    let auth = Auth::resolve(config).await?;
    let http = http_client()?;
    let url = if auth.is_service() {
        format!("{base}/api/dogInternal/devices/assignments")
    } else {
        format!("{base}/api/dogDevice/getAll")
    };
    let resp = auth
        .apply(http.get(&url))
        .send()
        .await
        .map_err(|error| AppError::Internal(format!("请求中台设备台账失败: {error}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::Internal(format!(
            "中台设备台账返回 HTTP {status}: {text}"
        )));
    }
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| AppError::Internal(format!("中台设备台账不是 JSON: {error}")))?;
    let records = value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let devices = records
        .into_iter()
        .filter_map(|item| {
            let device_id = item.get("deviceId").and_then(Value::as_str)?.to_string();
            let user_ids = item
                .get("users")
                .and_then(Value::as_array)
                .map(|users| {
                    users
                        .iter()
                        .filter_map(|user| user.get("id").and_then(Value::as_i64))
                        .collect()
                })
                .unwrap_or_default();
            Some(ZentaiDevice {
                device_id,
                user_ids,
            })
        })
        .collect();
    Ok(devices)
}

/// 设备编号 → 中台分配用户 id 列表
pub async fn assignment_map(config: &Config) -> Result<HashMap<String, Vec<i64>>, AppError> {
    Ok(device_assignments(config)
        .await?
        .into_iter()
        .map(|device| (device.device_id, device.user_ids))
        .collect())
}

/// 设备编号 → 第一个被分配的中台用户（地图归属）
pub async fn device_owner(config: &Config, device_id: &str) -> Result<Option<i64>, AppError> {
    Ok(device_assignments(config)
        .await?
        .into_iter()
        .find(|device| device.device_id == device_id.trim())
        .and_then(|device| device.user_ids.into_iter().min()))
}

/// 这些中台地图 id 中还有哪些「活着」（未被软删除）。
///
/// 中台是地图主库：业务库里导入过的地图被中台删掉后不能再算「已同步」，
/// 否则设备端不会重传，中台列表里也永远看不到这张图。
///
/// 只走共享服务令牌内部接口；未配置令牌或中台不可达时返回 `Err`，
/// 由调用方退回旧行为（把业务库记录都当作已同步），避免把中台抖动
/// 放大成「每台设备都重新下载几十兆地图」。
pub async fn alive_map_ids(config: &Config, ids: &[i64]) -> Result<HashSet<i64>, AppError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let base = zentai_base(config)?;
    let token = config
        .service_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            AppError::Internal(
                "未配置中台服务令牌（ROBODOG_SERVICE_TOKEN），无法校验中台地图是否仍存在"
                    .to_string(),
            )
        })?;
    let http = http_client()?;
    let url = format!("{base}/api/dogInternal/maps/exists");
    let resp = http
        .post(&url)
        .header("x-service-token", token)
        .json(&json!({ "ids": ids }))
        .send()
        .await
        .map_err(|error| AppError::Internal(format!("请求中台地图存在性接口失败: {error}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::Internal(format!(
            "中台地图存在性接口返回 HTTP {status}: {text}"
        )));
    }
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| AppError::Internal(format!("中台地图存在性响应不是 JSON: {error}")))?;
    Ok(value
        .get("data")
        .and_then(|data| data.get("existing"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default())
}

/// 把业务平台设备台账同步到中台（幂等 upsert）。
pub async fn upsert_device(config: &Config, device: &DeviceUpsert) -> Result<(), AppError> {
    let base = zentai_base(config)?;
    let auth = Auth::resolve(config).await?;
    let http = http_client()?;
    let url = if auth.is_service() {
        format!("{base}/api/dogInternal/devices/upsert")
    } else {
        format!("{base}/api/dogDevice/upsert")
    };
    let payload = json!({
        "deviceId": device.device_id,
        "deviceName": device.device_name,
        "model": device.model,
        "status": device.status,
        "firmware": device.firmware,
        "deviceIp": device.device_ip,
        "parkCode": device.park_code,
        "parkName": device.park_name,
        "remarks": device.remarks,
        "userIds": device.user_ids,
    });
    let resp = auth
        .apply(http.post(&url).json(&payload))
        .send()
        .await
        .map_err(|error| AppError::Internal(format!("同步设备到中台失败: {error}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::Internal(format!(
            "同步设备到中台失败 HTTP {status}: {text}"
        )));
    }
    Ok(())
}

/// 业务平台同步到中台的设备字段。
#[derive(Debug, Clone)]
pub struct DeviceUpsert {
    pub device_id: String,
    pub device_name: Option<String>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub firmware: Option<String>,
    pub device_ip: Option<String>,
    pub park_code: Option<String>,
    pub park_name: Option<String>,
    pub remarks: Option<String>,
    /// 需要追加分配的中台用户（仅管理员服务账号有效）
    pub user_ids: Vec<i64>,
}

/// 尽力同步：失败只记录日志，不阻断业务平台的设备增改。
pub async fn try_upsert_device(config: &Config, device: &DeviceUpsert) {
    if !config.zentai_ready() {
        return;
    }
    match upsert_device(config, device).await {
        Ok(()) => info!("设备 {} 已同步到中台", device.device_id),
        Err(error) => warn!("设备 {} 同步到中台失败: {error}", device.device_id),
    }
}

/// 删除中台设备台账（内部接口按编号直接删；兼容模式先查 id 再删）。
pub async fn delete_device(config: &Config, device_id: &str) -> Result<(), AppError> {
    let base = zentai_base(config)?;
    let auth = Auth::resolve(config).await?;
    let http = http_client()?;

    if auth.is_service() {
        let url = format!(
            "{base}/api/dogInternal/devices/{}",
            urlencoding::encode(device_id.trim())
        );
        let resp = auth
            .apply(http.delete(&url))
            .send()
            .await
            .map_err(|error| AppError::Internal(format!("删除中台设备失败: {error}")))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AppError::Internal(format!("删除中台设备失败: {text}")));
        }
        return Ok(());
    }

    let resp = auth
        .apply(http.get(format!("{base}/api/dogDevice/byCode/{device_id}")))
        .send()
        .await
        .map_err(|error| AppError::Internal(format!("查询中台设备失败: {error}")))?;
    let text = resp.text().await.unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let Some(id) = value
        .get("data")
        .and_then(|data| data.get("id"))
        .and_then(Value::as_i64)
    else {
        return Ok(());
    };
    let resp = auth
        .apply(http.delete(format!("{base}/api/dogDevice/{id}")))
        .send()
        .await
        .map_err(|error| AppError::Internal(format!("删除中台设备失败: {error}")))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!("删除中台设备失败: {text}")));
    }
    Ok(())
}

/// 判断当前业务用户是否有权访问某台设备。
///
/// - 没有中台身份（本地服务账号）→ 不限制；
/// - 中台管理员 → 不限制；
/// - 其余账号 → 必须在中台分配关系里。
pub async fn user_can_access_device(
    config: &Config,
    zentai_user_id: Option<i64>,
    is_admin: bool,
    device_id: &str,
) -> Result<bool, AppError> {
    if zentai_user_id.is_none() || is_admin || !config.zentai_ready() {
        return Ok(true);
    }
    let user_id = zentai_user_id.expect("checked above");
    let assignments = assignment_map(config).await?;
    Ok(assignments
        .get(device_id.trim())
        .map(|users| users.contains(&user_id))
        .unwrap_or(false))
}
