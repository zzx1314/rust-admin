//! 由中台服务自己托管前端构建产物（`frontend/dist`）。
//!
//! 部署时把 `frontend` 的 `dist` 放到 `[server] static_dir` 指向的目录，
//! 就只需要一个端口：`/api/*` 交给接口路由，其余路径落到这里。前端是 SPA
//! （`VITE_ROUTER_HISTORY` 默认 `hash`，但历史模式也要能用），所以任何未命中
//! 的路径都回 `index.html`，由前端路由接管。
//!
//! 这样生产环境不再需要 nginx：原来 nginx 提供的 `try_files` 回落、gzip、
//! 静态资源长缓存都由这一层补齐。`/api/*` 的 404 在 `routes.rs` 内部处理掉，
//! 不会被这里的回落兜成 200 的 HTML。

use std::path::Path;

use axum::Router;
use axum::http::header::{CACHE_CONTROL, HeaderValue};
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

/// 文件名带内容的 hash，可以放心让浏览器长期缓存。
const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";
/// `index.html` 必须每次回源：发版后如果浏览器还用旧入口，
/// 会去请求已经被删掉的 chunk 而白屏。
const NO_CACHE: &str = "no-cache";

/// 给 `router` 挂上前端静态托管。`static_dir` 为 `None`、目录不存在或
/// 缺少 `index.html` 时原样返回——保持「纯 API」行为，不报错。
pub fn attach(router: Router, static_dir: Option<&Path>) -> Router {
    let Some(dir) = static_dir else {
        return router;
    };
    let index = dir.join("index.html");
    if !index.is_file() {
        tracing::warn!(
            static_dir = %dir.display(),
            "static_dir 下没有 index.html，跳过前端静态托管（只提供 /api）"
        );
        return router;
    }

    tracing::info!(static_dir = %dir.display(), "已开启前端静态托管");

    // Vite 把带 hash 的产物放在 `dist/static/` 下（见 frontend/vite.config.ts）。
    let assets = ServiceBuilder::new()
        .layer(CompressionLayer::new())
        .layer(cache_header(IMMUTABLE_CACHE))
        .service(ServeDir::new(dir.join("static")));

    // 注意用 `fallback` 而不是 `not_found_service`：后者会把回落响应的状态码
    // 改成 404，SPA 深链接（如 `/sys/user`）就变成 404 + index.html 了。
    let spa = ServiceBuilder::new()
        .layer(CompressionLayer::new())
        .layer(cache_header(NO_CACHE))
        .service(ServeDir::new(dir).fallback(ServeFile::new(index)));

    router.nest_service("/static", assets).fallback_service(spa)
}

fn cache_header(value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(CACHE_CONTROL, HeaderValue::from_static(value))
}
