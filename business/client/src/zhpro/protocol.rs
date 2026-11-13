//! 智航 Pro 二开协议（v1.0.3）消息封包与类型常量。
//!
//! 全部消息统一为 `{ head: { type, time, src, msg_id }, data }`。
//! 请求 `src=1`（App），算法侧 `src=2`；网关侧 1201/1203/1207/1299/1301 使用 `src=5`，
//! 本体侧 1315/1317 使用 `src=4`。请求与响应通过 `msg_id` 关联，响应 `type = 请求 type + 1`。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ─── type 常量 ─────────────────────────────────────────────────

/// 握手请求（control 已建立，重新索取会话信息）
pub const T_HANDSHAKE: u32 = 1200;
/// 握手应答：client_id / session_id / bulkdata_bind_token
pub const T_HANDSHAKE_ACK: u32 = 1201;
/// bulkdata 绑定请求（bulkdata 首帧）
pub const T_BIND_BULKDATA: u32 = 1202;
/// bulkdata 绑定成功应答
pub const T_BIND_BULKDATA_ACK: u32 = 1203;
/// 获取 RoamerX 控制权（发送 4000~5999 前必须持有）
pub const T_TAKE_CONTROL: u32 = 1204;
/// RoamerX 控制权通知
pub const T_TAKE_CONTROL_NOTIFY: u32 = 1205;
/// 模块可用性广播（sdk_node 等）
pub const T_MODULE_NOTIFY: u32 = 1207;
/// 网关传输层错误
pub const T_GATEWAY_ERROR: u32 = 1299;
/// 网关心跳请求
pub const T_HEARTBEAT: u32 = 1300;
/// 网关心跳响应
pub const T_HEARTBEAT_ACK: u32 = 1301;
/// 本体命令请求（站立 / 卧倒 / 急停 / 速度档位 …）
pub const T_COMMAND: u32 = 1302;
/// 本体命令响应
pub const T_COMMAND_ACK: u32 = 1303;
/// 遥控摇杆
pub const T_REMOTE_CONTROL: u32 = 1304;
/// 充电桩信息请求
pub const T_CHARGING_PILE: u32 = 1310;
/// 充电桩信息通知
pub const T_CHARGING_PILE_ACK: u32 = 1311;
/// 本体状态上报（1Hz）
pub const T_ROBOT_STATUS: u32 = 1315;
/// 本体故障上报
pub const T_FAULT_STATUS: u32 = 1317;

/// RoamerX 控制权变更通知（网关广播，`client_id` 为当前 owner，无 owner 时UINT32_MAX）
pub const T_ROAMER_CONTROL_NOTIFY: u32 = 1523;

/// 建图保存结果通知
pub const T_MAP_SAVED: u32 = 4001;
/// 获取地图列表
pub const T_MAP_LIST: u32 = 4012;
/// 地图列表响应
pub const T_MAP_LIST_ACK: u32 = 4013;
/// 修改地图目录名
pub const T_MAP_RENAME: u32 = 4014;
/// 修改地图目录名响应
pub const T_MAP_RENAME_ACK: u32 = 4015;
/// 删除地图
pub const T_MAP_DELETE: u32 = 4016;
/// 删除地图响应
pub const T_MAP_DELETE_ACK: u32 = 4017;
/// 设置返航回充点位
pub const T_SET_CHARGE_POINT: u32 = 4018;
/// 设置返航回充点位响应
pub const T_SET_CHARGE_POINT_ACK: u32 = 4019;
/// 故障码上报
pub const T_FAULT_CODE: u32 = 4021;
/// 根据地图获取路径信息
pub const T_PATH_QUERY: u32 = 4022;
/// 路径信息响应
pub const T_PATH_QUERY_ACK: u32 = 4023;
/// 添加路径
pub const T_PATH_ADD: u32 = 4024;
/// 添加路径响应
pub const T_PATH_ADD_ACK: u32 = 4025;
/// 删除路径
pub const T_PATH_REMOVE: u32 = 4026;
/// 删除路径响应
pub const T_PATH_REMOVE_ACK: u32 = 4027;
/// 修改路径
pub const T_PATH_MODIFY: u32 = 4028;
/// 修改路径响应
pub const T_PATH_MODIFY_ACK: u32 = 4029;
/// 定位请求（载图 / 重定位）
pub const T_LOCATE: u32 = 4030;
/// 定位响应
pub const T_LOCATE_ACK: u32 = 4031;
/// 建图位姿与里程推送
pub const T_MAPPING_TELEMETRY: u32 = 4035;
/// 删除返航回充点位
pub const T_DELETE_CHARGE_POINT: u32 = 4036;
/// 删除返航回充点位响应
pub const T_DELETE_CHARGE_POINT_ACK: u32 = 4037;
/// 获取机器人时间
pub const T_ROBOT_TIME: u32 = 4038;
/// 获取机器人时间响应
pub const T_ROBOT_TIME_ACK: u32 = 4039;
/// 设置导航避障等级
pub const T_SET_AVOID_LEVEL: u32 = 4044;
/// 设置导航避障等级响应
pub const T_SET_AVOID_LEVEL_ACK: u32 = 4045;

/// 创建任务
pub const T_TASK_CREATE: u32 = 5000;
/// 创建任务响应
pub const T_TASK_CREATE_ACK: u32 = 5001;
/// 取消任务
pub const T_TASK_CANCEL: u32 = 5002;
/// 取消任务响应
pub const T_TASK_CANCEL_ACK: u32 = 5003;
/// 暂停任务
pub const T_TASK_PAUSE: u32 = 5004;
/// 暂停任务响应
pub const T_TASK_PAUSE_ACK: u32 = 5005;
/// 恢复任务
pub const T_TASK_RESUME: u32 = 5006;
/// 恢复任务响应
pub const T_TASK_RESUME_ACK: u32 = 5007;
/// 任务及定位状态上报（V2 快照，默认 500ms）
pub const T_TASK_STATUS: u32 = 5009;
/// 自主返航回充（新建返回充电点任务）
pub const T_RETURN_TO_CHARGE: u32 = 5014;
/// 自主返航回充响应
pub const T_RETURN_TO_CHARGE_ACK: u32 = 5015;
/// 回充 / 出桩底层结果通知
pub const T_CHARGE_RESULT: u32 = 5019;

/// 实时位姿推送（bulkdata）
pub const T_CURRENT_POSE: u32 = 1401;
/// 建图轨迹推送（bulkdata）
pub const T_SLAM_PATH: u32 = 1403;
/// 局部规划路径推送（bulkdata）
pub const T_LOCAL_PATH: u32 = 1405;
/// 全局规划路径推送（bulkdata）
pub const T_GLOBAL_PATH: u32 = 1407;

// ─── 任务类型 / 阶段枚举 ───────────────────────────────────────

/// 5000 `data.kind` 任务类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Mapping,
    Navigation,
    Recharge,
    Patrol,
    Undock,
}

impl TaskKind {
    pub fn code(self) -> u32 {
        match self {
            TaskKind::Mapping => 1,
            TaskKind::Navigation => 2,
            TaskKind::Recharge => 3,
            TaskKind::Patrol => 4,
            TaskKind::Undock => 5,
        }
    }

    /// 5009 `current_task.type` 字符串
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Mapping => "mapping",
            TaskKind::Navigation => "navigation",
            TaskKind::Recharge => "recharge",
            TaskKind::Patrol => "patrol",
            TaskKind::Undock => "undock",
        }
    }
}

/// 5009 `lifecycle` 终态判定
pub fn lifecycle_is_terminal(lifecycle: &str) -> bool {
    matches!(lifecycle, "success" | "failed" | "cancelled")
}

/// 导航地图类型（5000 key=2 `params.map_type`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavMapType {
    /// 2D 栅格
    Grid2D,
    /// Route 路网
    Route,
}

impl NavMapType {
    pub fn code(self) -> u32 {
        match self {
            NavMapType::Grid2D => 1,
            NavMapType::Route => 3,
        }
    }
}

/// 建图类型（5000 kind=1 `params.map_type`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingType {
    Outdoor,
    Indoor,
}

impl MappingType {
    pub fn code(self) -> u32 {
        match self {
            MappingType::Outdoor => 0,
            MappingType::Indoor => 2,
        }
    }
}

/// 导航避障等级（4044）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvoidLevel {
    /// 关闭感知与碰撞停止
    None,
    /// 保留碰撞停止，不绕障
    StopOnly,
    /// 感知停车等待，不绕障
    Perceive,
    /// 完整避障（默认）
    Full,
}

impl AvoidLevel {
    pub fn code(self) -> u32 {
        match self {
            AvoidLevel::None => 10,
            AvoidLevel::StopOnly => 20,
            AvoidLevel::Perceive => 30,
            AvoidLevel::Full => 40,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AvoidLevel::None => "none",
            AvoidLevel::StopOnly => "stop_only",
            AvoidLevel::Perceive => "perceive",
            AvoidLevel::Full => "full",
        }
    }
}

/// 巡检点作业动作（`goals[].actions[].type`，闭集）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointAction {
    /// 停留（原地等待，固定时长）
    Wait,
    /// 拍照（上装 `/capture_image`）
    Photo,
}

impl PointAction {
    pub fn code(self) -> u32 {
        match self {
            PointAction::Wait => 0,
            PointAction::Photo => 1,
        }
    }
}

// ─── 几何 ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct Vector3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Default for Quaternion {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }
}

impl Quaternion {
    /// 由偏航角（弧度）构造平面四元数
    pub fn from_yaw(yaw: f64) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: (yaw / 2.0).sin(),
            w: (yaw / 2.0).cos(),
        }
    }

    /// 取偏航角（弧度），用于计算转向是否满足到点容差
    pub fn yaw(&self) -> f64 {
        let siny_cosp = 2.0 * (self.w * self.z + self.x * self.y);
        let cosy_cosp = 1.0 - 2.0 * (self.y * self.y + self.z * self.z);
        siny_cosp.atan2(cosy_cosp)
    }

    pub fn is_zero(&self) -> bool {
        self.x == 0.0 && self.y == 0.0 && self.z == 0.0 && self.w == 0.0
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct Pose {
    pub position: Vector3,
    pub orientation: Quaternion,
}

impl Pose {
    pub fn new(x: f64, y: f64, yaw: f64) -> Self {
        Self {
            position: Vector3 { x, y, z: 0.0 },
            orientation: Quaternion::from_yaw(yaw),
        }
    }

    pub fn distance_to(&self, other: &Pose) -> f64 {
        (self.position.x - other.position.x).hypot(self.position.y - other.position.y)
    }
}

// ─── 信封 ─────────────────────────────────────────────────────

/// 统一消息头
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Head {
    #[serde(rename = "type")]
    pub msg_type: u32,
    /// Unix 毫秒
    pub time: i64,
    pub src: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<u32>,
}

/// 统一信封
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub head: Head,
    #[serde(default)]
    pub data: Value,
}

impl Envelope {
    /// 构造外部端（src=1）请求
    pub fn request(msg_type: u32, msg_id: u32, data: Value) -> Self {
        Self {
            head: Head {
                msg_type,
                time: now_ms(),
                src: SRC_APP,
                msg_id: Some(msg_id),
            },
            data,
        }
    }

    /// 构造外部端（src=1）单向消息（不需要响应的控制帧，如 1304）
    ///
    /// 文档里 1304 没有对应的响应类型，发 `msg_id` 会让调用方白白等一个
    /// 永远不来的应答，所以这类消息不带 `msg_id`。
    pub fn notify(msg_type: u32, data: Value) -> Self {
        Self {
            head: Head {
                msg_type,
                time: now_ms(),
                src: SRC_APP,
                msg_id: None,
            },
            data,
        }
    }

    /// 响应是否成功（`code` / `status_code` 为 0）
    pub fn is_ok(&self) -> bool {
        self.code() == 0
    }

    /// 响应码
    ///
    /// 协议里有两套约定，这里统一成一个取值：
    /// - 控制类（`1303` / `1311` / `1205`）：`data.status_code`
    /// - 算法侧（`4001` / `4013` / `5001` …）：`data.code`
    pub fn code(&self) -> i64 {
        if let Some(status) = self.data.get("status_code").and_then(Value::as_i64) {
            return status;
        }
        self.data.get("code").and_then(Value::as_i64).unwrap_or(-1)
    }

    /// 响应说明：控制类字段名是 `message`，算法侧是 `msg`
    pub fn message(&self) -> String {
        self.data
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| self.data.get("msg").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string()
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

pub const SRC_APP: u32 = 1;
pub const SRC_ALG: u32 = 2;
pub const SRC_MACHINE: u32 = 4;
pub const SRC_GATEWAY: u32 = 5;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 空 data 对象
pub fn empty() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_roundtrip() {
        for deg in [-180.0_f64, -90.0, 0.0, 45.0, 90.0, 179.0] {
            let yaw = deg.to_radians();
            let got = Quaternion::from_yaw(yaw).yaw();
            assert!((got - yaw).abs() < 1e-6, "deg={deg} got={got}");
        }
    }

    #[test]
    fn control_reply_uses_status_code() {
        let env: Envelope = serde_json::from_str(
            r#"{"head":{"type":1303,"time":1,"src":4},
                "data":{"status_code":0,"message":"ok","cmd":10}}"#,
        )
        .unwrap();
        assert!(env.is_ok());
        assert_eq!(env.message(), "ok");

        let failed: Envelope = serde_json::from_str(
            r#"{"head":{"type":1303,"time":1,"src":4},
                "data":{"status_code":5,"message":"busy","cmd":10}}"#,
        )
        .unwrap();
        assert!(!failed.is_ok());
        assert_eq!(failed.code(), 5);
        assert_eq!(failed.message(), "busy");
    }

    #[test]
    fn alg_reply_uses_code_and_msg() {
        let env: Envelope = serde_json::from_str(
            r#"{"head":{"type":4001,"time":1,"src":2,"msg_id":3},
                "data":{"code":13980,"msg":"mapping: save map timed out"}}"#,
        )
        .unwrap();
        assert!(!env.is_ok());
        assert_eq!(env.code(), 13980);
        assert_eq!(env.message(), "mapping: save map timed out");
    }

    #[test]
    fn notify_omits_msg_id() {
        let env = Envelope::notify(T_REMOTE_CONTROL, json!({ "lx": 0.0 }));
        assert!(env.head.msg_id.is_none());
        let text = env.to_text();
        assert!(!text.contains("msg_id"), "{text}");
    }

    #[test]
    fn envelope_serializes_head_shape() {
        let env = Envelope::request(T_MAP_LIST, 7, Value::Null);
        let value = serde_json::to_value(&env).unwrap();
        assert_eq!(value["head"]["type"], json!(4012));
        assert_eq!(value["head"]["src"], json!(1));
        assert_eq!(value["head"]["msg_id"], json!(7));
        assert!(value["head"]["time"].as_i64().is_some_and(|t| t > 0));
        assert!(value["data"].is_null());
    }

    #[test]
    fn terminal_lifecycle() {
        assert!(lifecycle_is_terminal("success"));
        assert!(lifecycle_is_terminal("failed"));
        assert!(!lifecycle_is_terminal("running"));
    }
}
