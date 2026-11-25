//! `robodog-ops-server` 的库入口。
//!
//! 二进制 `src/main.rs` 只负责装配进程（读配置 → 连库 → 迁移 → 播种 → 监听），
//! 业务逻辑全部在库里，这样：
//!
//! - 单元测试（`#[cfg(test)]`）继续跟被测代码同文件，编译器在 release 下剔除；
//! - 集成测试放在 `tests/`，像外部调用方一样通过 HTTP 路由验证接口契约。
//!
//! 目前 `tests/api_smoke.rs` 覆盖健康检查、鉴权与驾驶舱接口的冒烟路径。

pub mod agent_tunnel;
pub mod ai_patrol;
pub mod alert_flow;
pub mod app;
pub mod auth;
pub mod backpack;
pub mod cache;
pub mod config;
pub mod control_plane;
pub mod coords;
pub mod db;
pub mod dispatch;
pub mod error;
pub mod gateway;
pub mod gateway_types;
pub mod geo;
pub mod handlers;
pub mod ids;
pub mod ingest;
pub mod mapdir;
pub mod models;
pub mod notify;
pub mod phone_audio;
pub mod photos;
pub mod recordings;
pub mod schedule;
pub mod seed;
pub mod staff_import;
pub mod status;
pub mod tls;
pub mod video;
pub mod whep;
pub mod xlsx;
pub mod zentai;
