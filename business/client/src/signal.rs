//! 板端联网信号：读板端 Agent 的 `/api/network`，随心跳 `signal` / `signalSource`
//! 上报平台。
//!
//! 为什么由客户端报：智航协议 1315 里没有任何 rssi / csq / wifi / lte 指标（39 个字段
//! 逐一核对过），本体给不出信号强弱；但设备客户端就装在同一块背包板子（现场 `.246`）上，
//! 而板子的联网质量决定了平台能不能连上它。板端 Agent 的 `GET /api/network` 已经把
//! 「当前联网信号」统一成一份（WiFi 优先，没连 WiFi 时退回 4G 的 AT+CSQ 折算），
//! 这里周期读一次塞进心跳，业务平台「设备管理」就能看到「信号 优 / 良 / 中 / 差」。
//!
//! 三条约定（改这块先看）：
//!
//! 1. **读不到就不报**：Agent 没起 / 没连 WiFi 也没拿到 4G IP / 老 Agent 没有这个接口，
//!    一律 `None`，心跳里不带 `signal`（页面显示 `—`）。**绝不编一个值**——以前填配置里的
//!    假信号，现场拿它判断「是不是信号差导致掉线」直接误导排查。
//! 2. **绝不影响巡检**：纯后台任务，HTTP 失败只打 `debug`，不等不重试。
//! 3. **0-100**：与 `nmcli` 的 SIGNAL 同口径（4G 由 `dBm` 线性折算），页面按
//!    80 / 60 / 40 分 优 / 良 / 中 / 差；`source` 只可能是 `wifi` / `cellular`。

use reqwest::Client;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::debug;

/// 单次查询超时。板端 `/api/network` 只读 5s 遥测快照缓存（毫秒级），
/// 这里给 10s 纯属留余量；真正慢的是**老 Agent**没有该接口时退回的 `/api/wifi`
/// （那里会真跑一次 `nmcli` 扫描，最多 25s，所以它单独用 [`LEGACY_TIMEOUT`]）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// 老 Agent 的 `/api/wifi` 里有 `nmcli dev wifi list`（扫描最多 25s）
const LEGACY_TIMEOUT: Duration = Duration::from_secs(20);
/// 轮询间隔：心跳 10s，30s 一次足够（页面 1~2 个心跳内刷新），也不会和板端任务抢资源。
const PROBE_INTERVAL: Duration = Duration::from_secs(30);

/// 信号来源：WiFi 还是 4G
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalSource {
    Wifi,
    Cellular,
}

impl SignalSource {
    /// 上报给平台的取值（`devices.signal_source`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Cellular => "cellular",
        }
    }
}

/// 一次读到的联网信号
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkSignal {
    /// 0-100，与 `nmcli` SIGNAL 同口径
    pub value: i64,
    pub source: SignalSource,
}

/// 板端联网信号探针：后台轮询 + 读当前值
pub struct SignalProbe {
    agent: String,
    client: Client,
    /// 最近一次读到的信号；`None` = 读不到（没连网 / Agent 不可达）
    signal: Mutex<Option<NetworkSignal>>,
}

impl SignalProbe {
    pub fn new(agent: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            agent: agent.into().trim().trim_end_matches('/').to_string(),
            client: Client::builder()
                .timeout(PROBE_TIMEOUT)
                .build()
                .unwrap_or_else(|_| Client::new()),
            signal: Mutex::new(None),
        })
    }

    /// 起后台轮询任务（首次立即查一次，之后每 [`PROBE_INTERVAL`] 一次）
    pub fn spawn(self: &Arc<Self>) {
        let probe = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let signal = probe.fetch().await;
                if let Ok(mut slot) = probe.signal.lock() {
                    *slot = signal;
                }
                tokio::time::sleep(PROBE_INTERVAL).await;
            }
        });
    }

    /// 当前信号；读不到时为 `None`
    pub fn signal(&self) -> Option<NetworkSignal> {
        self.signal.lock().ok().and_then(|slot| *slot)
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    async fn fetch(&self) -> Option<NetworkSignal> {
        // 新 Agent：/api/network 已经把 WiFi / 4G 统一好（只读快照，毫秒级）
        let url = format!("{}/api/network", self.agent);
        match self.client.get(&url).send().await {
            Ok(response) if response.status().is_success() => {
                return match response.json::<Value>().await {
                    Ok(value) => parse_network(&value),
                    Err(error) => {
                        debug!("板端 /api/network 响应不是合法 JSON: {error}");
                        None
                    }
                };
            }
            Ok(response) => {
                debug!("板端没有 /api/network（{}），退回 /api/wifi", response.status());
            }
            Err(error) => {
                debug!("板端 /api/network 不可达（信号暂不上报）: {error}");
                return None;
            }
        }
        // 老 Agent（只升级了客户端）：只能报 WiFi，4G 不猜
        let url = format!("{}/api/wifi", self.agent);
        match self
            .client
            .get(&url)
            .timeout(LEGACY_TIMEOUT)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => match response.json::<Value>().await {
                Ok(value) => parse_wifi(&value),
                Err(error) => {
                    debug!("板端 /api/wifi 响应不是合法 JSON: {error}");
                    None
                }
            },
            Ok(response) => {
                debug!("板端 /api/wifi 返回 {}（信号暂不上报）", response.status());
                None
            }
            Err(error) => {
                debug!("板端 /api/wifi 不可达（信号暂不上报）: {error}");
                None
            }
        }
    }
}

/// 板端 `/api/network`：`{source: "wifi"|"cellular"|null, signal: 0-100|null, ...}`
///
/// 新接口是唯一真源：`source` 不认识的响应一律当作「读不到」，
/// 不要瞎猜成 WiFi —— 页面上「WiFi 良」和「4G 良」是两件事。
pub fn parse_network(value: &Value) -> Option<NetworkSignal> {
    let source = match value.get("source").and_then(Value::as_str) {
        Some("wifi") => SignalSource::Wifi,
        Some("cellular") => SignalSource::Cellular,
        _ => return None,
    };
    let signal = value
        .get("signal")
        .and_then(Value::as_i64)
        .filter(|signal| (0..=100).contains(signal))?;
    Some(NetworkSignal {
        value: signal,
        source,
    })
}

/// 老 Agent 的 `/api/wifi`：`current: {connected, ssid, signal, quality, ip}`
///
/// 没连 WiFi 时 `signal = null`；没有 `current` 段的更老版本同样返回 `None`。
/// 这条路只有 WiFi，**不做 4G 兜底**（老接口里没有 4G 的 CSQ）。
pub fn parse_wifi(value: &Value) -> Option<NetworkSignal> {
    let current = value.get("current")?;
    let connected = current
        .get("connected")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !connected {
        return None;
    }
    let signal = current
        .get("signal")
        .and_then(Value::as_i64)
        .filter(|signal| (0..=100).contains(signal))?;
    Some(NetworkSignal {
        value: signal,
        source: SignalSource::Wifi,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_unified_wifi_signal() {
        let value = json!({
            "source": "wifi", "signal": 66, "quality": "良",
            "ssid": "POE-2.4G", "ip": "192.168.41.246", "rssi_dbm": null
        });
        assert_eq!(
            parse_network(&value),
            Some(NetworkSignal {
                value: 66,
                source: SignalSource::Wifi
            })
        );
    }

    #[test]
    fn reads_the_cellular_fallback() {
        // 没连 WiFi，4G CSQ 折算成 0-100：-85dBm → 约 45（中）
        let value = json!({
            "source": "cellular", "signal": 45, "quality": "中",
            "ssid": null, "ip": "10.64.3.21", "rssi_dbm": -85
        });
        assert_eq!(
            parse_network(&value),
            Some(NetworkSignal {
                value: 45,
                source: SignalSource::Cellular
            })
        );
    }

    #[test]
    fn offline_board_reports_nothing() {
        // 没连 WiFi 也没拿到 4G IP：宁可页面显示「—」，也不要报一个 0 让人以为信号极差
        let value = json!({ "source": null, "signal": null, "quality": "未知" });
        assert_eq!(parse_network(&value), None);
    }

    #[test]
    fn unknown_source_is_not_guessed() {
        // 「WiFi 良」和「4G 良」是两件事，来源不明就别报
        let value = json!({ "source": "satellite", "signal": 70 });
        assert_eq!(parse_network(&value), None);
        let value = json!({ "signal": 70 });
        assert_eq!(parse_network(&value), None);
    }

    #[test]
    fn out_of_range_signal_is_ignored() {
        let value = json!({ "source": "wifi", "signal": 130 });
        assert_eq!(parse_network(&value), None);
        let value = json!({ "source": "wifi", "signal": -3 });
        assert_eq!(parse_network(&value), None);
    }

    #[test]
    fn legacy_wifi_endpoint_still_reports_wifi_only() {
        let value = json!({
            "iface": "wlan0",
            "total": 1,
            "networks": [],
            "current": { "connected": true, "ssid": "POE-2.4G", "signal": 72,
                          "quality": "良", "ip": "192.168.41.246" }
        });
        assert_eq!(
            parse_wifi(&value),
            Some(NetworkSignal {
                value: 72,
                source: SignalSource::Wifi
            })
        );
        // 没连 WiFi：老接口没有 4G 兜底，就是不报
        let value = json!({ "current": { "connected": false, "signal": null } });
        assert_eq!(parse_wifi(&value), None);
        // 连 current 段都没有的老 Agent
        let value = json!({ "iface": "wlan0", "total": 0, "networks": [] });
        assert_eq!(parse_wifi(&value), None);
    }
}
