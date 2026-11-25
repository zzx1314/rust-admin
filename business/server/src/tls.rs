//! HTTPS 监听（自签证书即可）
//!
//! 为什么需要它：浏览器只在**安全上下文**里开放 `AudioWorklet` 与
//! `navigator.mediaDevices.getUserMedia`（麦克风）—— 网页对讲的两个前提。
//! `http://<公网IP>:3008` 打开时点击呼叫会直接抛
//! `Cannot read properties of undefined (reading 'addModule')`。
//!
//! 目标机 443 已经被厂商 OTA 的 nginx 占着，所以这里另起一个端口
//! （`[server] https_port`，默认部署用 3443），**同一个 router 同时挂在 http 与
//! https 两个监听上**：
//!
//! ```text
//! :3008 http   设备客户端（ws://）+ 老书签（浏览器走 https 更安全）
//! :3443 https  操作员浏览器（https://，自签证书；导入到信任根后无警告）
//! ```
//!
//! 设备侧**不需要改地址**：保留明文监听就是为了不把现场机器人一起迁到 wss
//! （板子在 4G 上，改配置要上塔）。
//!
//! 三条约定（改这块先看）：
//!
//! 1. **只跑 http/1.1**：WebSocket 升级走 1.1，浏览器不需要 h2；少一层 ALPN 分支。
//! 2. **ConnectInfo 照样注入**：设备接入 IP 由 handler 的 `ConnectInfo` 取，
//!    TLS 这条链路要自己塞进请求扩展（`tower::service_fn`），否则 wss 下的设备会 500。
//! 3. **TLS 起不来不能拖垮明文**：证书/私钥有问题时只记错误，`main.rs` 里明文监听照常。

use axum::Router;
use axum::extract::ConnectInfo;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tracing::{debug, info, warn};

/// 读取 PEM 证书链 + 私钥，构造 TLS 接受器。
///
/// 自签证书与正式证书一视同仁：只看 PEM 能不能解析、私钥与证书是否匹配
/// （`with_single_cert` 会校验），配置错了要在启动时喊出来，而不是等第一次握手失败。
pub fn acceptor(cert_path: &Path, key_path: &Path) -> Result<TlsAcceptor, String> {
    let cert_pem = std::fs::read(cert_path)
        .map_err(|error| format!("读证书 {} 失败: {error}", cert_path.display()))?;
    let key_pem = std::fs::read(key_path)
        .map_err(|error| format!("读私钥 {} 失败: {error}", key_path.display()))?;
    let certs = rustls_pemfile::certs(&mut cert_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("解析证书失败: {error}"))?;
    if certs.is_empty() {
        return Err(format!("{} 里没有 PEM 证书", cert_path.display()));
    }
    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())
        .map_err(|error| format!("解析私钥失败: {error}"))?
        .ok_or_else(|| {
            format!(
                "{} 里没有私钥（期望 PKCS#8 或 RSA 的普通 PEM）",
                key_path.display()
            )
        })?;
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|error| format!("证书与私钥不匹配: {error}"))?;
    // 只要 http/1.1：WS 升级走 1.1，浏览器也不需要 h2
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// 监听并服务 HTTPS（HTTP/1.1 + WebSocket 升级），连上就 spawn 一条连接任务。
///
/// 返回 `Err` 只有一种情况：监听 socket 本身出错（进程该重启了）。单条连接的错误
/// （浏览器中途关页、证书告警没点继续）只记 debug，不打断监听。
pub async fn serve(
    router: Router,
    listener: TcpListener,
    acceptor: TlsAcceptor,
) -> std::io::Result<()> {
    let local = listener.local_addr()?;
    info!(address = %format!("https://{local}"), "HTTPS 监听已启动（自签证书即可，浏览器首次会提示不安全）");
    loop {
        let (stream, peer) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let router = router.clone();
        tokio::spawn(async move {
            let tls = match acceptor.accept(stream).await {
                Ok(tls) => tls,
                Err(error) => {
                    // 证书告警没点「继续前往」/ 端口探测都会走到这里
                    debug!(%peer, "TLS 握手未完成: {error}");
                    return;
                }
            };
            // hyper 交过来的是 `Request<Incoming>`，转成 axum 的 `Request<Body>`
            // 并补上 ConnectInfo（设备接入 IP 与明文监听一致）
            let service = hyper_util::service::TowerToHyperService::new(tower::service_fn(
                move |request: hyper::Request<hyper::body::Incoming>| {
                    let mut router = router.clone();
                    async move {
                        let mut request = request.map(axum::body::Body::new);
                        request.extensions_mut().insert(ConnectInfo(peer));
                        tower::Service::call(&mut router, request).await
                    }
                },
            ));
            if let Err(error) = auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(tls), service)
                .await
            {
                debug!(%peer, "HTTPS 连接结束: {error}");
            }
        });
    }
}

/// 启动 HTTPS 监听（证书没配 / 端口为 0 时什么都不做）。
///
/// 故意 **不阻塞调用方**：起不来只记 error，明文监听照常跑（设备不能因为
/// 浏览器那点证书问题掉线）。调用方在主线程里 `tokio::spawn` 即可。
pub async fn spawn(
    router: Router,
    port: u16,
    cert: Option<&Path>,
    key: Option<&Path>,
) -> Result<(), String> {
    if port == 0 {
        info!("HTTPS 未启用（[server] https_port = 0）：浏览器用 http 打开时麦克风/网页对讲不可用");
        return Ok(());
    }
    let (Some(cert), Some(key)) = (cert, key) else {
        warn!(
            "HTTPS 已配置端口 {port} 但缺少 tls_cert / tls_key：跳过（浏览器网页对讲需要 https）"
        );
        return Ok(());
    };
    // rustls 0.23 的 `ServerConfig::builder()` 取进程默认 provider：这里显式装 ring，
    // 免得以后哪个依赖带进 aws-lc-rs 导致运行时 panic（重复安装只返回 Err，忽略）
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let acceptor = match acceptor(cert, key) {
        Ok(acceptor) => acceptor,
        Err(error) => {
            warn!("HTTPS 启动失败（明文监听不受影响）：{error}");
            return Err(error);
        }
    };
    let listener = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(listener) => listener,
        Err(error) => {
            let message = format!("绑定 HTTPS 端口 {port} 失败: {error}");
            warn!("{message}（明文监听不受影响）");
            return Err(message);
        }
    };
    tokio::spawn(serve(router, listener, acceptor));
    Ok(())
}
