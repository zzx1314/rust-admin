//! 智航 Pro（v1.0.3）二开协议对接层。
//!
//! 分层：
//! - [`protocol`] 消息信封、type 常量、几何与枚举
//! - [`client`] control + bulkdata 双通道连接、请求响应关联、心跳
//! - [`status`] 5009 任务状态快照解析与丢帧/去重
//! - [`mapping`] 建图任务、地图列表、回充点、载图定位
//! - [`route`] 航线（巡检）任务执行引擎
//! - [`teleop`] 远程接管、本体命令（1302）与遥控摇杆（1304）
//!
//! 各子模块按 `zhpro::<module>::Type` 路径直接引用，这里不再集中 re-export，
//! 避免出现「导出但未使用」的噪声。

pub mod client;
pub mod mapping;
pub mod protocol;
pub mod route;
pub mod status;
pub mod teleop;
