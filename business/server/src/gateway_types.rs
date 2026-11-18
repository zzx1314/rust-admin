use serde::{Deserialize, Serialize};
use serde_json::Value;

/// WebSocket 消息信封
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WsMessage {
    /// 消息类型标识
    pub msg_type: String,
    /// 时间戳（仅用于日志排查；板端推流这类简化客户端可以不带）
    #[serde(default)]
    pub ts: String,
    /// 业务载荷
    pub payload: Value,
}

/// 客户端 → 服务端：设备注册
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterPayload {
    pub device_id: String,
    /// 可选设备密钥（后续可扩展）
    #[allow(dead_code)] // 协议预留字段，当前服务端不校验
    pub secret: Option<String>,
    /// 设备型号：客户端取本体 1201 的 `type`（如 `ZSL-2-Ultra`），取不到则不报
    pub model: Option<String>,
    /// 固件版本：客户端本地配置 / 本体上报，用于中台台账展示
    pub firmware: Option<String>,
}

/// 服务端 → 客户端：注册确认
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterAckPayload {
    pub success: bool,
    pub message: String,
    pub device_id: String,
    pub device_name: Option<String>,
    /// 设备绑定的园区；未绑定时不下发，客户端据此跳过地图回传
    #[serde(skip_serializing_if = "Option::is_none")]
    pub park_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub park_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub park_name: Option<String>,
    pub server_ts: String,
}

/// 客户端 → 服务端：心跳
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatPayload {
    pub battery: Option<i64>,
    pub temperature: Option<f64>,
    pub position: Option<String>,
    pub odometer: Option<f64>,
    pub runtime_minutes: Option<i64>,
    pub current_task: Option<String>,
    pub firmware: Option<String>,
    /// 设备型号（本体 1201 `type`）：首次报到后变化才触发中台台账重同步
    pub model: Option<String>,
    /// 本体链路是否在线（false = 客户端处于模拟模式 / 本体掉线）
    pub body_online: Option<bool>,
    /// 客户端工作模式：robot / sim
    pub mode: Option<String>,
    /// 充电状态：本体 1315 上报（充电桩接入 / 电池电源状态为充电或充满）
    pub charging: Option<bool>,
    /// 本体工作模式：1315 `mode`（协议里的 MachineStatus），如 `remote`
    pub robot_mode: Option<String>,
    /// 定位状态（智航 5009 `loc_state.status`）：100 已收敛 / 122 定位丢失 / 210 定位失败
    pub loc_status: Option<i64>,
    /// 机器人当前生效地图（业务侧 mapId，如 `ROS-MAP-3@v1.0`）
    pub map_id: Option<String>,
    /// 1315 `ctrl_source`：0 未知 / 1 APP / 2 SDK / 3 其他（含硬件遥控器）
    pub ctrl_source: Option<i64>,
    /// 1315 `motion_status`：stand_up / lie_down / walk …（卧倒时走不动）
    pub motion_status: Option<String>,
    /// 1315 `estop.software`：软件急停是否按下
    pub estop_software: Option<bool>,
    /// 1207 `modules[]` 的 `sdk_node`：本体算法节点是否就绪
    pub sdk_node_available: Option<bool>,
}

/// 服务端 → 客户端：心跳确认
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatAckPayload {
    pub server_ts: String,
    /// 待执行命令队列（可选）
    pub pending_commands: Vec<PendingCommand>,
    /// 是否处于「强制接管」（跳过了 `ctrl_source = 3` 门禁）
    ///
    /// 客户端重启后本地标记会丢，靠心跳每轮同步：平台强制接管期间摇杆才能继续下发。
    pub teleop_force: bool,
}

/// 客户端 → 服务端：任务进度上报
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressPayload {
    #[allow(dead_code)] // 协议预留字段，任务匹配以 task_code 为准
    pub task_id: Option<i64>,
    pub task_code: String,
    pub status: String,
    pub progress: i64,
    /// **已完成航点数**（1 基计数），前端据此显示「进度 n/N」。
    ///
    /// 它**不是**航点下标，而且客户端两处语义并不一致：5s 定时摘要里是「已完成口径」
    /// （当前点 + 1），5009 驱动的到点事件里却直接放的 `detail_index`（0 基下标）。
    /// 要定位「现在站在哪个点」请一律用 [`Self::waypoint_index`]。
    pub point_index: Option<i64>,
    /// 当前执行航点的**0 基下标**（= 5009 的 `detail_index`），查路线 `points[]` 用它。
    ///
    /// 老客户端不发这个字段，只能退回 `point_index`（语义不同，会差一位）。
    #[serde(default)]
    pub waypoint_index: Option<i64>,
    pub message: Option<String>,
    /// 智航 Pro 本体侧任务标识（客户端从 5009 快照中取得）
    #[serde(default)]
    pub robot_task_id: Option<String>,
    /// 智航 Pro 执行阶段，如 `navigation.path_running`
    #[serde(default)]
    pub stage: Option<String>,
    /// 航点总数，便于前端展示 `已完成/总数`
    #[serde(default)]
    pub total_points: Option<i64>,
}

/// 客户端 → 服务端：航点 AI 识别请求
///
/// 正式环境的业务平台在公网，连不到现场内网的背包 Agent；设备客户端与 Agent
/// 同机（`127.0.0.1:8000`），所以由它把「动作参数 + 板端模型目录」一起报上来，
/// 服务端用与直连模式**完全同一套**规则（场景解析 / 类别过滤 / 阈值 / 窗口时长）
/// 算好参数，再用 `ai_start` 下发。
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiPointPayload {
    pub task_code: String,
    pub point_index: i64,
    #[serde(default)]
    pub point_name: Option<String>,
    #[serde(default)]
    pub dwell: Option<i64>,
    /// 动作 `param`：历史数据的关键词提示
    #[serde(default)]
    pub hint: String,
    /// 动作 `scene`：编辑器选的模型包 id
    #[serde(default)]
    pub scene: Option<String>,
    /// 动作 `labels`：勾选的识别类别
    #[serde(default)]
    pub labels: Vec<String>,
    /// 动作 `threshold` 原文（「置信度 ≥ 0.35」）
    #[serde(default)]
    pub threshold: String,
    /// 动作 `expectCount`：检查目标数量（数量任务必填；存在性任务不参与判定）
    #[serde(default)]
    pub expect_count: Option<i64>,
    /// 动作 `checkType`：任务类型，`exist` = 检测是否存在 / `count` = 检测数量是否正确
    #[serde(default)]
    pub check_type: Option<String>,
    /// 动作 `alertWhenMissing`：`exist` 任务未检出时是否也告警（路线数据优先）
    #[serde(default)]
    pub alert_when_missing: Option<bool>,
    /// 板端 `GET /api/detect/tasks` 原样
    #[serde(default)]
    pub tasks: Option<Value>,
    /// 板端当前是否已在识别（操作员手动开的）：决定 `start` 还是 `config`
    #[serde(default)]
    pub agent_running: Option<bool>,
    /// 板端默认相机（`default_cam`）
    #[serde(default)]
    pub agent_cam: Option<String>,
}

/// 客户端 → 服务端：识别窗口内的检出（客户端已按阈值过滤 + 同类别去重）
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiDetectPayload {
    pub window_id: String,
    #[serde(default)]
    pub task: String,
    pub label: String,
    pub conf: f64,
    /// 命中帧路径（板端本地绝对路径，仅作运维线索）。
    ///
    /// 检出是**实时**推的：那一刻板端还没发 `plan_hit`，客户端是自己扫
    /// `detect_jobs/<planId>/pt<n>/` 取「本轮最新一张」得到的路径。
    #[serde(default)]
    pub evidence: String,
    /// 命中帧本体（JPG/PNG 字节的 base64），由客户端读盘后随检出带上。
    ///
    /// 与 [`AiAlertPayload::evidence_image`] 同一套约束（≤2MB / JPG-PNG / 不带换行），
    /// 服务端用 `notify::evidence_image` 再校一次；不合格就只发卡片。
    #[serde(default)]
    pub evidence_image: Option<String>,
}

/// 客户端 → 服务端：识别窗口结束
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiEndPayload {
    pub window_id: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub hits: i64,
    /// 窗口是否**正常到期**（`false` = 任务结束 / 被新航点顶替而提前结束）。
    /// 数量任务靠它区分「到期没找到 = 数量不一致」和「被打断，不记账」。
    #[serde(default)]
    pub expired: bool,
}

/// 客户端 → 服务端：AI 检出数量与「检查目标数量」不一致
///
/// 判定在板端做（Agent 知道本帧到底画出了几个框，且有证据图/`result.json`）；
/// 客户端只把这个结论转发上来，服务端负责告警落库 + 企业微信推送。
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiAlertPayload {
    pub window_id: String,
    #[serde(default)]
    pub task_code: String,
    /// 计划 id（= 业务任务号），板端按它建存证目录
    #[serde(default)]
    pub plan_id: String,
    #[serde(default)]
    pub point_index: Option<i64>,
    /// 动作里的「检查目标数量」
    #[serde(default)]
    pub expect_count: Option<i64>,
    /// 板端 AI 实际检出的目标数
    #[serde(default)]
    pub found_count: Option<i64>,
    /// 证据图路径（板端本地绝对路径，仅作运维线索）
    #[serde(default)]
    pub evidence: String,
    /// 证据图本体（JPG/PNG 字节的 base64），由客户端从板端磁盘读出后随告警带上。
    ///
    /// 为什么要客户端带：图片在板端 `/root/…/detect_jobs/<plan>/pt<n>/*.jpg`，
    /// 平台拿不到那个文件系统（Agent 隧道白名单里也没有取图接口）。企业微信图片
    /// 消息要的是**字节**（base64 + md5），不是路径，所以只能让能读盘的客户端给。
    /// 超 2MB / 不是 JPG/PNG 时服务端会丢掉图片只发卡片（见 `notify::evidence_image`）。
    #[serde(default)]
    pub evidence_image: Option<String>,
    /// 板端给的描述（可选，服务端会自己拼一条更全的）
    #[serde(default)]
    pub message: String,
}

/// 客户端 → 服务端：补发本窗口的命中帧（卡片已经发过，只补一张图）
///
/// 正常路径下检出与图是**一起**上报的（客户端等板端 `plan_hit` 里的 `evidence`）。
/// 但板端的「连采 3 帧命中」采样要是一直没跑完，图要到窗口收尾
/// （`/api/detect/stop` → `_plan_flush`）才写出来 —— 那时告警早就发了，
/// 所以客户端停识别、扫到图之后再单独报一次，服务端只发 `msgtype=image`。
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiEvidencePayload {
    pub window_id: String,
    #[serde(default)]
    pub task_code: String,
    /// 计划 id（= 业务任务号），板端按它建存证目录
    #[serde(default)]
    pub plan_id: String,
    #[serde(default)]
    pub point_index: Option<i64>,
    /// 证据图路径（板端本地绝对路径，仅作运维线索）
    #[serde(default)]
    pub evidence: String,
    /// 证据图本体（JPG/PNG 字节的 base64）：校验与约束同
    /// [`AiAlertPayload::evidence_image`]
    #[serde(default)]
    pub evidence_image: Option<String>,
}

/// 客户端 → 服务端：本地接管态变化
///
/// 设备客户端做**进程内重连**时，与业务平台的 WebSocket 不会断开，服务端也就
/// 收不到「断连即清 `teleop_holder`」那条清理路径。而客户端在重建 control 通道后
/// 必然丢掉 RoamerX 控制权（`1204` 是会话级的），所以必须由它显式上报「holder 已
/// 不在本端」，否则平台会一直以为还握着控制权，摇杆被网关静默丢弃。
///
/// 只用于**释放**方向：holder 的建立仍由平台指令（「远程接管」）写入，客户端上报
/// `holder: true` 会被忽略，避免两个来源互相覆盖。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeleopStatePayload {
    /// 本地是否仍持有控制权（当前只有 `false` 生效）
    pub holder: bool,
    /// 上报原因，落到事件流便于运维追溯（如 `body-link-lost`）
    pub reason: Option<String>,
}

/// 客户端 → 服务端：远程指令执行结果
///
/// 业务平台下发「下发地图」等指令后，只有 `wsDelivered` 不能说明设备真的执行成功。
/// 客户端执行完（成功或失败）用这条消息按 `requestId` 回报，业务平台据此把
/// 「已送达」和「执行成功」区分开。
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommandResultPayload {
    /// 与平台下发指令时的 `requestId` 对应
    pub request_id: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// 客户端 → 服务端：设备事件
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEventPayload {
    pub event_type: String,
    pub message: String,
    #[allow(dead_code)] // 协议预留字段，用于后续事件详情展示
    pub data: Option<Value>,
}

/// 服务端 → 客户端：任务下发
///
/// `route_data` 是前端三栏编辑器产出的 `RouteData`（`points/legs/returnLeg`）。
/// `map` 携带客户端驱动智航本体时需要的载图信息；缺省时客户端会按任务所属园区
/// 的当前基线回退。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskDispatchPayload {
    pub task_id: i64,
    pub task_code: String,
    pub task_name: String,
    pub task_type: Option<String>,
    pub execute_mode: Option<String>,
    pub route: Option<Value>,
    pub route_data: Option<Value>,
    pub plan_start: Option<String>,
    pub plan_end: Option<String>,
    pub dispatch_policy: Option<String>,
    pub description: Option<String>,
    /// 智航本体载图信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map: Option<DispatchMap>,
    /// 巡航速度上限（m/s），取自路线 `default_speed`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_limit: Option<f64>,
    /// 避障等级：10 关闭 / 20 仅急停 / 30 感知 / 40 全避障
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avoid_level: Option<i64>,
    /// 结束动作：`return_to_start` | `return_to_charge` | `none`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_task: Option<String>,
    /// 任务级显式结束动作（**只有**在任务里配过 `/api/tasks` 的 `endTask` 时才带）：
    /// `none` | `return_to_start` | `return_to_charge`。
    ///
    /// 为什么不直接复用 `end_task`：历史服务端对**所有**任务都发 `end_task: "none"`，
    /// 客户端把 `none` 当成「没指定」再按闭环路线兜底“返回起点”。如果让 `end_task`
    /// 承担「显式原地」语义，新旧版本混合部署时会让所有闭环路线静默停止返航。
    /// 老客端忽略本字段，行为不变。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_task_mode: Option<String>,
}

/// 下发载荷中的地图绑定
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DispatchMap {
    /// 业务侧地图版本 ID，如 `ROS-MAP-3@v1.0`
    pub map_id: String,
    /// 本体地图目录名/路径（相对本体 map_root，客户端会用 4013 列表核对）
    pub map_path: String,
    /// 本体地图目录名（4013 `path` 的最后一段）
    pub map_dir: String,
    /// 1 = 2D 栅格, 3 = Route 路网
    pub map_type: u32,
    pub park_id: i64,
    pub map_name: String,
    pub version: String,
}

/// 服务端 → 客户端：任务取消/暂停
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskControlPayload {
    pub task_id: i64,
    pub task_code: String,
    pub action: String, // "pause" | "resume" | "cancel"
    pub reason: Option<String>,
}

/// 服务端 → 客户端：远程指令
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommandPayload {
    pub command: String,
    pub params: Option<Value>,
    pub issued_at: String,
}

/// 待执行命令（心跳响应中捎带）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingCommand {
    pub cmd_type: String, // "task_dispatch" | "task_control" | "remote_command"
    pub payload: Value,
}

/// 连接中的设备会话信息
///
/// 目前连接池只保存广播发送端，本结构用于后续展示会话详情。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DeviceSession {
    pub device_id: String,
    pub device_name: Option<String>,
    pub connected_at: String,
    pub last_heartbeat: String,
}
