//! `robodog-device-client` 的库入口。
//!
//! 二进制是主体（`src/main.rs`），这里只暴露**纯逻辑、无副作用**的模块，
//! 让集成测试（`tests/`）可以像使用外部 crate 一样验证行为，而不用把测试
//! 代码写进 `src/`。
//!
//! 导出的是配置与本体 SN 读取：协议解析、任务编排等逻辑仍留在二进制内部
//! （有各自的内联单元测试，`#[cfg(test)]` 编译期剔除，不进入发布产物）。

pub mod config;
pub mod device_sn;
