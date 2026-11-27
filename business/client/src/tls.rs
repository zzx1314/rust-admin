//! 业务平台链路的 TLS 信任（`server.url = wss://…` 时才用得上）
//!
//! 现场把业务平台的明文入口（公网 `3008`）关掉、只留 https（`3443`）之后，设备客户端
//! 必须走 `wss://`；而平台用的是**自签证书**，不显式信任就会一直握手失败
//! （2026-11-26 现场：板子 18:05 之后一直离线，就是 3008 关了而客户端还在连 `ws://`）。
//!
//! 两条链路要共用一个信任库：
//!
//! - 设备 WS：`tokio-tungstenite` 的 [`Connector::Rustls`]
//! - 回传 / 同步用的 `reqwest`：[`reqwest::ClientBuilder::use_preconfigured_tls`]
//!
//! ## 为什么不是「把证书塞进 RootCertStore」了事
//!
//! 现场那张证书（`/opt/hz/rust-admin/tls/server.crt`）是**同一个自签证书既当 CA 又当
//! leaf**用（`basicConstraints=critical, CA:TRUE` + `subjectAltName=IP:…` + `serverAuth`）。
//! 把它当信任根之后，webpki 再看到它作为 end-entity 出示就直接拒绝——
//! `invalid peer certificate: CaUsedAsEndEntity`（2026-11-26 20:05 实测，改完配置
//! 立刻复现）。反过来它也不能当普通 leaf（webpki 要求 leaf 不带 `CA:TRUE`）。
//!
//! 所以这里做的是**证书钉死（certificate pinning）**：`server.tls_ca` 指向的 PEM 里
//! 每一张证书都允许被平台原样出示（逐字节比对）；同时也把它塞进根证书库，这样换成
//! 「正规 CA + leaf」的链（或者公网 CA 证书）时照旧按标准链校验。
//!
//! ⛔ 不做「跳过校验」（`danger_accept_invalid_certs` 那一套）：设备令牌就走这条 WS
//! 上行，信任任意证书等于送给中间人。自签就钉证书，一行配置的事；平台换证书
//! （重新生成 `tls/server.crt`）就把新文件再拷一份到板端（`deploy/backpack-246.md`）。

use std::path::Path;
use std::sync::{Arc, OnceLock};

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio_tungstenite::Connector;

/// 进程级的 rustls 配置：启动时 `init()` 一次，之后两条链路共用同一份信任库。
static CONFIG: OnceLock<Option<Arc<rustls::ClientConfig>>> = OnceLock::new();

/// 启动时调用一次：把 `server.tls_ca` 读成 rustls 配置并缓存。
///
/// 返回 `Err` 时**应该让进程起不来**（`main` 里直接退出）：现场「证书路径写错 / 证书
/// 没拷过来」表现为连不上平台，日志里只有一句 timed out，比启动失败难查得多。
pub fn init(ca_path: Option<&Path>) -> Result<(), String> {
    let built = match ca_path {
        Some(path) => Some(Arc::new(build(path)?)),
        None => None,
    };
    // 只初始化一次：重复调用（重连 / 测试）保留第一份
    let _ = CONFIG.set(built);
    Ok(())
}

fn build(path: &Path) -> Result<rustls::ClientConfig, String> {
    let verifier = pinned_verifier(path)?;
    Ok(rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}

/// 钉死 + 标准链校验的双重校验器（见模块注释里 `CaUsedAsEndEntity` 那段）。
fn pinned_verifier(path: &Path) -> Result<Arc<dyn ServerCertVerifier>, String> {
    let pem = std::fs::read(path).map_err(|error| {
        format!(
            "读不到 server.tls_ca（{}）: {error}（把平台 tls 目录下的证书拷过来，例如 \
             /opt/hz/rust-admin/tls/server.crt）",
            path.display()
        )
    })?;
    let mut pinned: Vec<CertificateDer<'static>> = Vec::new();
    let mut reader = pem.as_slice();
    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.map_err(|error| {
            format!(
                "server.tls_ca 不是 PEM 证书（{}）: {error}",
                path.display()
            )
        })?;
        pinned.push(cert);
    }
    if pinned.is_empty() {
        return Err(format!(
            "server.tls_ca（{}）里没有 BEGIN CERTIFICATE 段",
            path.display()
        ));
    }
    // 标准链校验兜底：公网 CA 证书 / 「CA + leaf」的正规自签链都走这条
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for cert in &pinned {
        // 带 CA:TRUE 的能进根库；不能进（普通 leaf）也不影响钉死那条路
        let _ = roots.add(cert.clone());
    }
    let webpki = WebPkiServerVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|error| format!("构造标准链校验器失败: {error}"))?;
    let provider = CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()));
    Ok(Arc::new(PinnedVerifier {
        pinned,
        webpki,
        provider,
    }))
}

/// 「出示的证书就是 `server.tls_ca` 里那张」→ 直接放行，其余交给标准链校验。
#[derive(Debug)]
struct PinnedVerifier {
    pinned: Vec<CertificateDer<'static>>,
    webpki: Arc<WebPkiServerVerifier>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self.pinned.iter().any(|cert| cert == end_entity) {
            return Ok(ServerCertVerified::assertion());
        }
        self.webpki.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        )
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// 缓存的配置；没配 `server.tls_ca` 时是 `None`（走 tungstenite / reqwest 的默认根）
fn config() -> Option<Arc<rustls::ClientConfig>> {
    CONFIG.get().cloned().flatten()
}

/// 设备 WS 的 TLS 连接器：钉了证书才返回 `Some`，否则交给 tungstenite 默认（webpki 根）。
pub fn ws_connector() -> Option<Connector> {
    config().map(Connector::Rustls)
}

/// 平台侧 HTTP 客户端构造器（回传录像 / 同步媒体 / 照片 / 地图上传都走它）。
///
/// 没配 `server.tls_ca` 时与 `reqwest::Client::builder()` 完全等价，行为不变。
///
/// ⛔ **不要把 config 包成 `Some(config)`**：`use_preconfigured_tls` 自己会包一层
/// `Option`（内部按 `Option<rustls::ClientConfig>` 做 downcast），再包一次就
/// downcast 失败 → `build()` 报 `Unknown TLS backend passed to use_preconfigured_tls`；
/// 调用方以前用 `.build().unwrap_or_default()` 兜底，等于退回一个**没有 TLS 后端**的
/// 客户端，之后所有 `https://` 请求都只报一句 `error sending request for url`。
/// 2026-11-26 20:06 之后同步地图 / 同步媒体 / 回传录像 / 照片上传全挂在这上面
/// （现场表现就是「点了同步地图，中台一直没图、也不报错」）。
pub fn http_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    match config() {
        Some(config) => builder.use_preconfigured_tls((*config).clone()),
        None => builder,
    }
}

/// 到平台的 HTTP 客户端（带超时），**构建失败直接炸**。
///
/// 以前各处写的是 `crate::tls::http_builder().timeout(..).build().unwrap_or_default()`：
/// 一旦 TLS 配置没生效，兜底出来的客户端连 https 都发不出去，而报错只剩
/// `error sending request`，现场根本查不到原因。宁可启动/执行时炸出清楚的原因。
pub fn platform_client(timeout: std::time::Duration) -> reqwest::Client {
    http_builder().timeout(timeout).build().unwrap_or_else(|error| {
        panic!(
            "构造平台 HTTP 客户端失败：{error}（TLS 配置没有生效，检查 server.tls_ca / tls::http_builder）"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 用 openssl 现造一张自签证书（形状同现场那颗：`CA:TRUE` + SAN 带 IP）。
    /// 测试机没装 openssl 时返回空，调用方跳过。
    fn openssl_self_signed(dir: &Path, cn: &str, ip: &str) -> Vec<u8> {
        std::fs::create_dir_all(dir).expect("建临时目录");
        let crt = dir.join(format!("{cn}.crt"));
        let output = std::process::Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "1",
                "-subj",
                &format!("/O=RoboDog/CN={cn}"),
                "-addext",
                &format!("subjectAltName=IP:{ip}"),
                "-addext",
                "basicConstraints=critical,CA:TRUE",
                "-keyout",
            ])
            .arg(dir.join(format!("{cn}.key")))
            .arg("-out")
            .arg(&crt)
            .output();
        match output {
            Ok(output) if output.status.success() => std::fs::read(&crt).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn write_pem(path: &Path, pem: &[u8]) {
        std::fs::File::create(path)
            .and_then(|mut file| file.write_all(pem))
            .expect("写证书");
    }

    fn now() -> UnixTime {
        UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_800_000_000))
    }

    #[test]
    fn init_is_infallible_without_ca_and_reports_bad_files() {
        // 没配 = 不钉证书：连接器是 None，reqwest 用默认根（行为与以前一致）
        init(None).expect("没配 tls_ca 时初始化必须成功");

        let missing = Path::new("/nonexistent/robodog/platform.crt");
        let error = build(missing).expect_err("读不到文件必须报错，不能静默跳过");
        assert!(
            error.contains("server.tls_ca"),
            "错误要指明是哪个配置：{error}"
        );
    }

    /// 现场那张证书是「CA 和 leaf 同一张」（`CA:TRUE`），webpki 会拿
    /// `CaUsedAsEndEntity` 拒掉——所以必须靠钉死这条路过（2026-11-26 20:05 现场）。
    /// 回归 2026-11-27：`use_preconfigured_tls` 必须收到**裸的** `rustls::ClientConfig`。
    ///
    /// 以前传的是 `Some(config)`，reqwest 内部再包一层 `Option` 后 downcast 打不中，
    /// `build()` 直接报 `Unknown TLS backend`；调用方的 `.unwrap_or_default()` 把它
    /// 变成一个没有 TLS 后端的客户端，所有 https 请求只报 `error sending request`
    /// → 现场「同步地图」连登录都过不去（2026-11-26 20:06 ~ 11-27 11:47 全量失败）。
    #[test]
    fn preconfigured_tls_accepts_the_bare_client_config() {
        let dir = std::env::temp_dir().join(format!("tls-reqwest-{}", std::process::id()));
        let pem = openssl_self_signed(&dir, "39.107.55.13", "39.107.55.13");
        if pem.is_empty() {
            eprintln!("跳过：本机没有 openssl");
            return;
        }
        let path = dir.join("platform.crt");
        write_pem(&path, &pem);
        let config = build(&path).expect("自签证书应当能钉");
        reqwest::Client::builder()
            .use_preconfigured_tls(config)
            .build()
            .expect("预置 TLS 配置必须能被 reqwest 接受（不能包成 Some）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ca_used_as_end_entity_is_accepted_only_when_pinned() {
        let dir = std::env::temp_dir().join(format!("tls-selftest-{}", std::process::id()));
        let pem = openssl_self_signed(&dir, "39.107.55.13", "39.107.55.13");
        if pem.is_empty() {
            eprintln!("跳过：本机没有 openssl");
            return;
        }
        let path = dir.join("platform.crt");
        write_pem(&path, &pem);
        let verifier = pinned_verifier(&path).expect("自签证书应当能钉");
        let pinned = rustls_pemfile::certs(&mut pem.as_slice())
            .next()
            .expect("至少一张证书")
            .expect("PEM 可解析");
        let name = ServerName::try_from("39.107.55.13").expect("IP ServerName");
        verifier
            .verify_server_cert(&pinned, &[], &name, &[], now())
            .expect("钉死的证书必须放行（哪怕它是 CA:TRUE）");

        // 换一张没钉的证书：必须拒（不能变成「信任任意证书」）
        let other = openssl_self_signed(&dir, "evil.example", "127.0.0.1");
        if !other.is_empty() {
            let other_cert = rustls_pemfile::certs(&mut other.as_slice())
                .next()
                .expect("至少一张证书")
                .expect("PEM 可解析");
            assert!(
                verifier
                    .verify_server_cert(&other_cert, &[], &name, &[], now())
                    .is_err(),
                "没钉的证书必须被拒"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_pem_file_is_rejected_with_a_readable_error() {
        let dir = std::env::temp_dir().join(format!("tls-selftest-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let broken = dir.join("broken.crt");
        std::fs::write(&broken, b"not a certificate").expect("写坏文件");
        let error = build(&broken).expect_err("不是 PEM 必须报错");
        assert!(error.contains("没有 BEGIN CERTIFICATE"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
