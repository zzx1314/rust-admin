//! 业务服务 WebSocket 线协议：消息信封、心跳/进度/事件载荷与发送辅助。

use chrono::Local;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use tokio::sync::mpsc;

// ─── 业务服务协议（与 server/src/gateway_types.rs 一致） ────────

/// WebSocket 消息信封
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WsMessage {
    pub(crate) msg_type: String,
    pub(crate) ts: String,
    pub(crate) payload: Value,
}

/// 心跳载荷
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeartbeatPayload {
    pub(crate) battery: i64,
    pub(crate) temperature: f64,
    pub(crate) position: String,
    pub(crate) odometer: f64,
    pub(crate) runtime_minutes: i64,
    pub(crate) current_task: Option<String>,
    pub(crate) firmware: String,
    /// 本体型号（智航 1201 `type`），未知时不报
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    /// 本体链路是否在线：false 时电量/温度等数值不可信（本服务据此不再当真实数据展示）
    pub(crate) body_online: bool,
    /// 客户端工作模式：robot（接本体）/ sim（纯模拟）
    pub(crate) mode: String,
    /// 充电状态：本体 1315 `charging_pile_connect` / 电池 `power_supply_status`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) charging: Option<bool>,
    /// 本体工作模式（1315 `mode` / MachineStatus，如 `remote`、`auto`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) robot_mode: Option<String>,
    /// 定位状态（智航 loc_state.status）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) loc_status: Option<i64>,
    /// 机器人当前生效地图
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) map_id: Option<String>,
    /// 1315 `ctrl_source`：0 未知 / 1 APP / 2 SDK / 3 其他（含硬件遥控器）
    ///
    /// 平台据此判断「到底谁在控制」：拿到 1204 但 `ctrl_source=3` 说明遥控器还压着。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ctrl_source: Option<i64>,
    /// 1315 `motion_status`（stand_up / lie_down / walk …）：卧倒时摇杆走不动
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) motion_status: Option<String>,
    /// 1315 `estop.software`：软件急停是否按下；平台据此禁止遥控/任务
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) estop_software: Option<bool>,
    /// 1207 模块可用性：本体 SDK 节点是否就绪；false 时所有运动/导航指令都会被拒
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sdk_node_available: Option<bool>,
    /// WiFi / 4G 信号强度（0-100）：**板端**联网信号，见 [`crate::signal`]。
    ///
    /// WiFi 优先，没连 WiFi 时由板端用 4G 的 `AT+CSQ` 折算。本体协议里没有信号指标，
    /// 这个值来自背包板子自己；读不到时不带该字段（平台显示 `—`），
    /// **不要**用配置值或 0 冒充。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) signal: Option<i64>,
    /// 信号来源：`wifi` / `cellular`（4G），与 `signal` 成对上报
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) signal_source: Option<String>,
}

/// 任务进度上报
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskProgressPayload {
    pub(crate) task_code: String,
    pub(crate) status: String,
    pub(crate) progress: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) point_index: Option<i64>,
    /// 当前执行航点的 **0 基下标**（5009 的 `detail_index`）。
    ///
    /// 必须和 `point_index` 分开：`point_index` 是「已完成航点数」（1 基计数），
    /// 定时摘要里它等于「当前点 + 1」，平台拿它开 AI 窗口会查到下一个航点。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) waypoint_index: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    /// 智航 5009 的阶段串，便于前端展示执行细节
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stage: Option<String>,
    /// 智航侧任务 ID（5001 返回），便于与机器人日志对账
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) robot_task_id: Option<String>,
    /// 航点总数，前端据此展示「已完成 / 总数」
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total_points: Option<i64>,
}

/// 设备事件
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceEventPayload {
    pub(crate) event_type: String,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<Value>,
}

pub(crate) async fn send(tx: &mpsc::Sender<WsMessage>, msg_type: &str, payload: Value) {
    let msg = WsMessage {
        msg_type: msg_type.to_string(),
        ts: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        payload,
    };
    let _ = tx.send(msg).await;
}

/// 从平台下发指令里取 `requestId`。
///
/// 两种形态都要认：
/// - 「下发地图」这类由业务平台自己组参数：`params.requestId`；
/// - 中台转发过来的普通指令走 `push_command`，业务平台会把中台参数**再包一层**：
///   `params.params.requestId`（与 `media_sync::parse_params` 兼容的两层一致）。
///   少了这一层，`同步媒体` 的进度/结论回执会因为取不到 requestId 直接不发，
///   中台审计就一直停在「已下发」。
pub(crate) fn command_request_id(msg: &WsMessage) -> Option<String> {
    let params = msg.payload.get("params");
    params
        .and_then(|params| params.get("params"))
        .and_then(|params| params.get("requestId"))
        .or_else(|| params.and_then(|params| params.get("requestId")))
        .or_else(|| msg.payload.get("requestId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// 回报远程指令的**真实执行结果**。
///
/// 平台只看 `wsDelivered` 无法区分「指令进了设备连接」和「设备真的执行成功」；
/// 业务平台按 `requestId` 等待这条回执，用它决定地图下发成功还是失败。
pub(crate) async fn send_command_result(
    out_tx: &mpsc::Sender<WsMessage>,
    msg: &WsMessage,
    command: &str,
    result: &Result<String, String>,
) {
    send_command_result_with(out_tx, msg, command, result, json!({})).await;
}

/// 同上，带结构化细节（响应目录是否已清空等）。
pub(crate) async fn send_command_result_with(
    out_tx: &mpsc::Sender<WsMessage>,
    msg: &WsMessage,
    command: &str,
    result: &Result<String, String>,
    extra: Value,
) {
    let Some(request_id) = command_request_id(msg) else {
        return;
    };
    let (ok, message) = match result {
        Ok(note) => (true, note.clone()),
        Err(error) => (false, error.clone()),
    };
    let mut payload = json!({
        "requestId": request_id,
        "command": command,
        "ok": ok,
        "message": message,
    });
    if let (Some(payload), Value::Object(extra)) = (payload.as_object_mut(), extra) {
        for (key, value) in extra {
            payload.insert(key, value);
        }
    }
    send(out_tx, "command_result", payload).await;
}

#[cfg(test)]
mod command_result_tests {
    use super::{WsMessage, command_request_id};
    use serde_json::json;

    fn message(payload: serde_json::Value) -> WsMessage {
        WsMessage {
            msg_type: "command".to_string(),
            ts: "2026-11-16 10:00:00".to_string(),
            payload,
        }
    }

    #[test]
    fn reads_request_id_from_nested_command_params() {
        let msg = message(json!({
            "command": "下发地图",
            "params": { "mapId": "ROS-MAP-1@v1.0", "requestId": "req-push-1" }
        }));
        assert_eq!(command_request_id(&msg).as_deref(), Some("req-push-1"));
    }

    #[test]
    fn reads_request_id_when_the_platform_wraps_params_twice() {
        // 中台 → 业务平台 push_command → 设备 WS：中台参数被包成 params.params
        let msg = message(json!({
            "command": "同步媒体",
            "params": {
                "dispatchedAt": "2026-11-20 17:21:38",
                "operator": "admin",
                "params": {
                    "pictureDir": "/media/media-data/picture",
                    "frames": 3,
                    "requestId": "cmd-9f2"
                }
            }
        }));
        assert_eq!(
            command_request_id(&msg).as_deref(),
            Some("cmd-9f2"),
            "同步媒体的进度/结论就是靠这个 requestId 回写到中台审计的"
        );
    }

    #[test]
    fn falls_back_to_top_level_request_id_and_rejects_blank() {
        let msg = message(json!({ "requestId": "req-2" }));
        assert_eq!(command_request_id(&msg).as_deref(), Some("req-2"));

        let blank = message(json!({ "params": { "requestId": "   " } }));
        assert!(command_request_id(&blank).is_none());
    }
}
