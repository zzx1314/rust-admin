//! 板端 WiFi 信号：读板端 Agent 的 `/api/wifi`，随心跳 `signal` 上报平台。
//!
//! 为什么由客户端报：智航协议 1315 里没有任何 rssi / csq / wifi / lte 指标（39 个字段
//! 逐一核对过），本体给不出信号强弱；但设备客户端就装在同一块背包板子（现场 `.246`）上，
//! 而板子的联网质量决定了平台能不能连上它。板端 Agent 的 `GET /api/wifi` 已经带
//! `current`（`nmcli` 的 SSID / SIGNAL 0-100 / IP），这里周期读一次塞进心跳，
//! 业务平台「设备管理」就能看到「信号 优 / 良 / 中 / 差」。
//!
//! 三条约定（改这块先看）：
//!
//! 1. **读不到就不报**：Agent 没起 / 板子没连 WiFi / 网卡不在，一律 `None`，
//!    心跳里不带 `signal`（页面显示 `—`）。**绝不编一个值**——以前填配置里的假信号，
//!    现场拿它判断「是不是信号差导致掉线」直接误导排查。
//! 2. **绝不影响巡检**：纯后台任务，HTTP 失败只打 `debug`，不等不重试；
//! 3. **0-100**：与 `nmcli` 的 SIGNAL 同口径，页面按 80 / 60 / 40 分档。

use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use tracing::debug;

/// 单次查询超时。
///
/// 给 20s 是因为板端 `/api/wifi` 是**真的跑一次 `nmcli dev wifi list`**（扫描最多 25s，
/// 现场通常 1~3s）。同机 HTTP 本身毫秒级，但扫描慢的时候不能把「没扫完」当成「没连 WiFi」：
/// 这个请求在后台任务里等，不堵心跳和 5009 处理链路。
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// 轮询间隔：`nmcli` 一次扫描要 1~20s，取 30s（心跳 10s，页面 1~2 个心跳内刷新）。
/// 更密会与板端 5s 遥测快照抢 `nmcli`，现场实测会拖慢整个快照。
const PROBE_INTERVAL: Duration = Duration::from_secs(30);
/// 「读不到」的哨兵值（`AtomicI64` 没有 Option，用负数表示未知）
const UNKNOWN: i64 = -1;

/// 板端 WiFi 信号探针：后台轮询 + 读当前值
pub struct WifiProbe {
    agent: String,
    client: Client,
    /// 最近一次读到的信号（0-100）；[`UNKNOWN`] = 读不到
    signal: AtomicI64,
}

impl WifiProbe {
    pub fn new(agent: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            agent: agent.into().trim().trim_end_matches('/').to_string(),
            client: Client::builder()
                .timeout(PROBE_TIMEOUT)
                .build()
                .unwrap_or_else(|_| Client::new()),
            signal: AtomicI64::new(UNKNOWN),
        })
    }

    /// 起后台轮询任务（首次立即查一次，之后每 [`PROBE_INTERVAL`] 一次）
    pub fn spawn(self: &Arc<Self>) {
        let probe = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let signal = probe.fetch().await;
                probe.signal.store(signal.unwrap_or(UNKNOWN), Ordering::Relaxed);
                tokio::time::sleep(PROBE_INTERVAL).await;
            }
        });
    }

    /// 当前信号（0-100）；没连 WiFi / 板端不可达时为 `None`
    pub fn signal(&self) -> Option<i64> {
        let value = self.signal.load(Ordering::Relaxed);
        (value >= 0).then_some(value)
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    async fn fetch(&self) -> Option<i64> {
        let url = format!("{}/api/wifi", self.agent);
        let response = match self.client.get(&url).send().await {
            Ok(response) => response,
            Err(error) => {
                debug!("板端 /api/wifi 不可达（信号暂不上报）: {error}");
                return None;
            }
        };
        match response.json::<Value>().await {
            Ok(value) => parse_signal(&value),
            Err(error) => {
                debug!("板端 /api/wifi 响应不是合法 JSON: {error}");
                None
            }
        }
    }
}

/// 从 `/api/wifi` 的响应里取当前连接的信号强度（0-100）。
///
/// 形状见板端 `telemetry.get_wifi()`：`current: {connected, ssid, signal, quality, ip}`。
/// 没连 WiFi 时 `signal = null`；老版本 Agent 没有 `current` 段，同样返回 `None`。
pub fn parse_signal(value: &Value) -> Option<i64> {
    let current = value.get("current")?;
    let connected = current
        .get("connected")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !connected {
        return None;
    }
    current
        .get("signal")
        .and_then(Value::as_i64)
        .filter(|signal| (0..=100).contains(signal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_current_connection_signal() {
        let value = json!({
            "iface": "wlan0",
            "total": 3,
            "networks": [{ "ssid": "POE-2.4G", "signal": 66, "quality": "良" }],
            "current": { "connected": true, "ssid": "POE-2.4G", "signal": 66,
                          "quality": "良", "ip": "192.168.41.246" }
        });
        assert_eq!(parse_signal(&value), Some(66));
    }

    #[test]
    fn disconnected_board_reports_nothing() {
        // 没连 WiFi：宁可页面显示「—」，也不要报一个 0 让人以为信号极差
        let value = json!({ "current": { "connected": false, "ssid": null, "signal": null } });
        assert_eq!(parse_signal(&value), None);
    }

    #[test]
    fn old_agent_without_current_section_reports_nothing() {
        let value = json!({ "iface": "wlan0", "total": 1, "networks": [] });
        assert_eq!(parse_signal(&value), None);
    }

    #[test]
    fn out_of_range_signal_is_ignored() {
        let value = json!({ "current": { "connected": true, "signal": 130 } });
        assert_eq!(parse_signal(&value), None);
        let value = json!({ "current": { "connected": true, "signal": -3 } });
        assert_eq!(parse_signal(&value), None);
    }
}
