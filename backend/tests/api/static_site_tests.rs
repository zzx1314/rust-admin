//! 前端静态托管（`[server] static_dir`）测试。
//!
//! 这层替代的是生产环境里的 nginx，所以要验证的都是 nginx 原本负责的事：
//!   1. `/` 与任意 SPA 路由都回 `index.html`（`try_files` 回落）；
//!   2. `/static/*` 命中真实文件，并带长缓存头；
//!   3. `index.html` 带 `no-cache`，发版后用户不会拿到旧入口；
//!   4. 文本类响应支持 gzip；
//!   5. `/api/*` 未命中仍然是 404 JSON，不会被前端回落兜成 200 的 HTML；
//!   6. 没配 `static_dir` / 目录里没有 `index.html` 时，服务退回纯 API 行为。

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use x_rust::api::static_site;
use x_rust::app::App;
use x_rust::config::AppConfig;

const JWT_SECRET: &str = "0123456789abcdef0123456789abcdef";

/// 造一个最小的 `dist`：`index.html` + `static/js/app-<hash>.js`。
fn fake_dist() -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("rust_admin_dist_{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(dir.join("static/js")).expect("create dist");

    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><body><div id=\"app\"></div></body></html>",
    )
    .expect("write index.html");
    // 内容足够长，保证会走 gzip 而不是被跳过。
    let script = "console.log('rust-admin');".repeat(400);
    std::fs::write(dir.join("static/js/app-abc123.js"), script).expect("write asset");

    dir
}

fn unused_dist() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rust_admin_missing_{}",
        uuid::Uuid::new_v4().simple()
    ))
}

/// 只落一个最小可解析的配置；静态目录在测试里单独传给 `static_site::attach`。
fn minimal_config() -> AppConfig {
    let toml = format!(
        r#"
jwt_secret = "{JWT_SECRET}"
database_url = "sqlite::memory:"

[server]
host = "127.0.0.1"
port = 3000

[redis]
host = "localhost"
port = 6379
"#
    );
    let path = std::env::temp_dir().join(format!(
        "rust_admin_static_cfg_{}.toml",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&path, toml).expect("write config");
    let config = AppConfig::load(&path).expect("config must parse");
    std::fs::remove_file(&path).ok();
    config
}

fn router_with(dist: Option<&Path>) -> Router {
    static_site::attach(Router::new(), dist)
}

async fn get(
    router: &Router,
    path: &str,
    accept_encoding: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().uri(path);
    if let Some(encoding) = accept_encoding {
        request = request.header(header::ACCEPT_ENCODING, encoding);
    }
    let request = request.body(Body::empty()).expect("request");

    use tower::ServiceExt;
    router.clone().oneshot(request).await.expect("response")
}

#[tokio::test]
async fn serves_index_and_falls_back_for_spa_routes() {
    let dist = fake_dist();
    let router = router_with(Some(&dist));

    for path in ["/", "/sys/user", "/dogDevice/device?id=1"] {
        let response = get(&router, path, None).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "路径 {path} 应回 index.html"
        );

        let cache = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(cache, "no-cache", "路径 {path} 的入口不能长缓存");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert!(
            String::from_utf8_lossy(&body).contains("id=\"app\""),
            "路径 {path} 应返回 index.html"
        );
    }

    std::fs::remove_dir_all(&dist).ok();
}

#[tokio::test]
async fn serves_hashed_assets_with_long_cache_and_gzip() {
    let dist = fake_dist();
    let router = router_with(Some(&dist));

    let response = get(&router, "/static/js/app-abc123.js", Some("gzip")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("gzip"),
        "带 hash 的静态资源应支持 gzip"
    );
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=31536000, immutable")
    );

    std::fs::remove_dir_all(&dist).ok();
}

#[tokio::test]
async fn api_paths_still_404_instead_of_returning_index_html() {
    // 真实路由：/api/... 由接口处理，未命中必须 404，不能落到静态回落。
    let app = App::new("sqlite::memory:")
        .await
        .expect("in-memory database must migrate");
    let config = minimal_config();
    let dist = fake_dist();

    let router = static_site::attach(
        x_rust::api::routes::create_router(app.build_state(&config)),
        Some(&dist),
    );

    let response = get(&router, "/api/definitely-not-a-route", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .starts_with("application/json"),
        "未命中的 /api 必须是 JSON 而不是 HTML"
    );

    // 消息里要带完整的 `/api/...` 路径（嵌套路由会剥掉前缀，必须用 OriginalUri）
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("/api/definitely-not-a-route"),
        "404 消息应包含完整路径，实际：{body}"
    );

    std::fs::remove_dir_all(&dist).ok();
}

#[tokio::test]
async fn missing_or_empty_static_dir_keeps_api_only_mode() {
    // 没配 static_dir。
    let router = router_with(None);
    assert_eq!(
        get(&router, "/", None).await.status(),
        StatusCode::NOT_FOUND
    );

    // 配了但目录里没有 index.html（比如忘了 `pnpm build`）。
    let dir = unused_dist();
    std::fs::create_dir_all(&dir).expect("create empty dir");
    let router = router_with(Some(&dir));
    assert_eq!(
        get(&router, "/", None).await.status(),
        StatusCode::NOT_FOUND,
        "缺少 index.html 时必须退回纯 API 模式"
    );

    std::fs::remove_dir_all(&dir).ok();
}
