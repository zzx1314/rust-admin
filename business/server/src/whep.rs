//! 平台侧 WebRTC 出口的同源代理：`/api/backpack/whep/*` → 平台 mediamtx 的 WHEP
//!
//! ```text
//! 板端 relay ──RTSP/TCP :8554（bp_<stream>）──▶ 平台 mediamtx
//!                                                   │ WHEP（只绑 127.0.0.1:8889）
//! 浏览器 ── POST /api/backpack/whep/<stream>/whep ───┘（本模块反代）
//!        └── 媒体：浏览器 ⇄ mediamtx UDP 8189（不经本进程）
//! ```
//!
//! 为什么要反代，而不是让浏览器直连 mediamtx：
//!
//! 1. **mediamtx 只绑回环**：8889 上没有任何鉴权手段（WHEP 是匿名协议），开在公网
//!    等于谁都能拉流；放在 Rust 进程后面才能复用平台 JWT（`require_auth` 支持
//!    `?token=`，浏览器的 WHEP `fetch` 也不方便带 Authorization）。
//! 2. **同源**：页面在 `:3100`，直连 `:8889` 是另一个源，多一套 CORS 与「混合内容」
//!    的坑；反代之后 WHEP 与其它 `/api/*` 完全一样。
//! 3. **会话地址改写**：mediamtx 回的 `Location` 是 `/bp_dogcam/whep/<uuid>`，
//!    必须改写成同源路径并带上令牌，否则浏览器后续的 `PATCH` / `DELETE` 会打到 404 上
//!    （会话泄漏 → mediamtx 里堆垃圾 reader）。
//!
//! 只转发 WHEP 的四个动词（`OPTIONS` 取 ICE 服务器 / `POST` 协商 / `PATCH` trickle /
//! `DELETE` 收尾），不代理 mediamtx 的其它 HTTP 端点（控制 API 在 `:9997`，只绑回环）。
//!
//! ⚠️ 媒体（SRTP）**不经过这里**：浏览器拿到 answer 后直接跟 mediamtx 的 UDP 8189
//! 建立 ICE —— 所以安全组必须放通 8189（见 `deploy/external-hz/README.md`）。

use crate::app::AppState;
use crate::auth::Claims;
use crate::error::AppError;
use crate::video::{BACKPACK_MEDIAMTX_PATH_PREFIX, BACKPACK_STREAMS};
use axum::body::{Body, Bytes};
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{debug, warn};

/// WHEP 协商的超时：mediamtx 要等一路流的关键帧才会回 answer，正常 <1s，
/// 板端刚起 ffmpeg 时更慢；超过就给前端一个明确的 503，让它的指数退避重连接手。
const WHEP_TIMEOUT: Duration = Duration::from_secs(15);

/// 请求体上限：SDP 实际只有几 KB，给 256KB 富余（trickle 的 PATCH 也很小）
const MAX_SDP_BYTES: usize = 256 * 1024;

/// 反代专用的长连接复用客户端（WHEP 是短请求，但没有理由每次握手）
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(WHEP_TIMEOUT)
            .build()
            .expect("build whep client")
    })
}

/// `OPTIONS|POST|PATCH|DELETE /api/backpack/whep/<stream>/whep[/<session>]`
///
/// `<stream>` 必须在白名单里（与观看通道共用 [`BACKPACK_STREAMS`]）：路径来自 URL，
/// 不限制就能被拿来让 mediamtx 创建任意 path。
pub async fn proxy(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
    method: Method,
    uri: Uri,
    Path(rest): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    // 先校验路径、再看部署配置：白名单与端点形状跟「有没有开 mediamtx」无关，
    // 顺序反过来会让 `/whep/backdoor/...` 也回「本部署没开 WebRTC」这种误导性文案。
    let (stream, tail) = split_path(&rest)?;
    if !BACKPACK_STREAMS.contains(&stream.as_str()) {
        return Err(AppError::BadRequest(format!("未知的背包通道：{stream}")));
    }
    if body.len() > MAX_SDP_BYTES {
        return Err(AppError::BadRequest("SDP 太大".to_string()));
    }

    let Some(base) = state.config.mediamtx_url.clone() else {
        return Err(AppError::Upstream(
            "本部署没有启用平台 WebRTC 出口（未配置 ROBODOG_MEDIAMTX_URL）：\
             视频请走平台中继（FLV），或在服务器上部署 deploy/external-hz/mediamtx/"
                .to_string(),
        ));
    };

    let upstream_url = upstream_url(&base, &stream, &tail);
    debug!("WHEP {method} {upstream_url}");

    let mut request = client().request(method.clone(), &upstream_url);
    for name in [header::CONTENT_TYPE, header::ACCEPT, header::IF_MATCH] {
        if let Some(value) = headers.get(&name) {
            request = request.header(name, value);
        }
    }
    if !body.is_empty() {
        request = request.body(body);
    }

    let upstream = request.send().await.map_err(|error| {
        // mediamtx 没起来 / 端口不对：这是部署问题，文案要能直接定位
        warn!("WHEP 上游不可达（{upstream_url}）：{error}");
        AppError::Upstream(
            "平台 WebRTC 出口不可达（mediamtx 没在跑？）：systemctl status mediamtx".to_string(),
        )
    })?;

    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/sdp")
        .to_string();
    // 会话地址改写成同源路径：浏览器后续的 PATCH / DELETE 才会落到本代理上
    let location = upstream.headers().get(header::LOCATION).and_then(|value| {
        let raw = value.to_str().ok()?;
        let session = raw.trim_end_matches('/').rsplit('/').next()?;
        Some(format!(
            "/api/backpack/whep/{stream}/whep/{session}{}",
            token_query(uri.query())
        ))
    });
    let payload = upstream.bytes().await.unwrap_or_default();

    let mut response = Response::builder()
        .status(StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY))
        .header(header::CONTENT_TYPE, content_type);
    if let Some(location) = location {
        response = response.header(header::LOCATION, location);
    }
    Ok(response
        .body(Body::from(payload))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()))
}

/// 把原请求里的 `?token=` 原样带到改写后的会话地址上
///
/// `whep.ts` 的会话地址来自 `Location`，它不会自己再拼令牌；不带就意味着收尾的
/// `DELETE` 会被 `require_auth` 挡在 401 上，mediamtx 里留一条僵尸会话到超时。
fn token_query(query: Option<&str>) -> String {
    query
        .and_then(|query| {
            query
                .split('&')
                .find(|pair| pair.starts_with("token="))
                .map(|pair| format!("?{pair}"))
        })
        .unwrap_or_default()
}

/// 上游地址：`{mediamtx}/bp_<stream>/whep[/<session>]`
///
/// ⚠️ `tail` 里**已经含** `/whep`（那是 mediamtx 的 WHEP 端点），不能再拼一次 ——
/// 多余的那一段会被 mediamtx 当成 path 名（它的报错是
/// `no stream is available on path 'bp_dogcam/whep'`，看着像「没推流」，其实是路径拼错）。
/// 只有裸 `<stream>`（前面不带 `/whep`）才需要补上。
fn upstream_url(base: &str, stream: &str, tail: &str) -> String {
    let tail = if tail.is_empty() { "/whep" } else { tail };
    format!(
        "{base}/{BACKPACK_MEDIAMTX_PATH_PREFIX}{stream}{tail}",
        base = base.trim_end_matches('/'),
    )
}

/// `dogcam/whep/<session>` → (`dogcam`, `/whep/<session>`)
fn split_path(rest: &str) -> Result<(String, String), AppError> {
    let rest = rest.trim_matches('/');
    let mut parts = rest.splitn(2, '/');
    let stream = parts.next().unwrap_or_default().trim();
    if stream.is_empty() {
        return Err(AppError::BadRequest("缺少通道名".to_string()));
    }
    // 只允许 `<stream>/whep[/<session>]`：mediamtx 的其它端点（配置 / 控制 API）不能透出去
    let tail = match parts.next() {
        None | Some("") => String::new(),
        Some(tail) => format!("/{}", tail.trim_matches('/')),
    };
    if !(tail.is_empty() || tail == "/whep" || tail.starts_with("/whep/")) {
        return Err(AppError::BadRequest(format!("不支持的 WHEP 路径：{tail}")));
    }
    Ok((stream.to_string(), tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_split_accepts_whep_shapes() {
        assert_eq!(split_path("dogcam").unwrap(), ("dogcam".into(), "".into()));
        assert_eq!(
            split_path("dogcam/whep").unwrap(),
            ("dogcam".into(), "/whep".into())
        );
        assert_eq!(
            split_path("/dogcam/whep/abc-123/").unwrap(),
            ("dogcam".into(), "/whep/abc-123".into())
        );
    }

    #[test]
    fn path_split_rejects_other_endpoints() {
        assert!(split_path("").is_err());
        assert!(split_path("dogcam/config/get").is_err());
        assert!(split_path("dogcam/../api").is_err());
    }

    #[test]
    fn upstream_url_points_at_the_whep_endpoint_exactly_once() {
        assert_eq!(
            upstream_url("http://127.0.0.1:8889", "dogcam", "/whep"),
            "http://127.0.0.1:8889/bp_dogcam/whep"
        );
        assert_eq!(
            upstream_url("http://127.0.0.1:8889/", "dogcam", "/whep/abc-1"),
            "http://127.0.0.1:8889/bp_dogcam/whep/abc-1"
        );
        assert_eq!(
            upstream_url("http://127.0.0.1:8889", "dogcam", ""),
            "http://127.0.0.1:8889/bp_dogcam/whep"
        );
    }

    #[test]
    fn token_query_only_forwards_token() {
        assert_eq!(token_query(Some("token=abc&stream=x")), "?token=abc");
        assert_eq!(token_query(Some("stream=x")), "");
        assert_eq!(token_query(None), "");
    }
}
