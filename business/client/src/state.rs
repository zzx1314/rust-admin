//! 设备运行时状态：本体 1315 数值、任务上下文与运动门禁。

use crate::ai_patrol::AiPoint;
use crate::lights::LightPoint;
use crate::lights::NightWindow;
use crate::lights::light_point_at;
use crate::wire::HeartbeatPayload;
use rand::Rng;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use super::*;

// ─── 机器人运行时状态 ──────────────────────────────────────────

/// 设备运行时状态：数值优先取本体 1315 上报，否则按任务推演
pub(crate) struct RobotState {
    pub(crate) battery: i64,
    /// 双电池里**较低**的那一节（`power1/power2` 取 min）——平台展示用 max，
    /// 但低电保护/巡检阈值必须按较低的那节算（ALG 就是按这个值判低电的）
    pub(crate) battery_min: i64,
    pub(crate) temperature: f64,
    pub(crate) position: String,
    pub(crate) runtime_minutes: i64,
    pub(crate) odometer: f64,
    pub(crate) current_task: Option<String>,
    pub(crate) firmware: String,
    /// 本体型号（智航 1201 `type`），未知时为空
    pub(crate) model: Option<String>,
    pub(crate) loc_status: i64,
    pub(crate) loc_fresh: bool,
    /// 1315 `ctrl_source`：谁在控制本体（遥控器 / APP / SDK）
    pub(crate) ctrl_source: Option<i64>,
    pub(crate) active_map: Option<String>,
    /// 最近一次收到本体 1315 的时刻：超过 `BODY_STATUS_FRESH` 视为本体掉线
    pub(crate) body_status_at: Option<Instant>,
    /// 工作模式：robot（接本体）/ sim（纯模拟）
    pub(crate) mode: &'static str,
    /// 充电状态：来自 1315（充电桩接入 / 电池电源状态），模拟模式恒 false
    pub(crate) charging: bool,
    /// 本体工作模式：1315 `mode`（协议里的 MachineStatus），如 remote / auto
    pub(crate) robot_mode: Option<String>,
    /// 运动状态：1315 `motion_status`（stand_up / balance_stand / lie_down …）
    ///
    /// 固件不回 `1303`，所以站立 / 卧倒是否生效只能看这个字段的变化
    pub(crate) motion_status: Option<String>,
    /// 软件急停是否处于按下态：1315 `estop.software`
    pub(crate) estop_software: Option<bool>,
    /// 1207 `modules[]` 里的 `sdk_node` 是否可用；false 时本体不会执行运动/导航
    pub(crate) sdk_node_available: Option<bool>,
    /// 前补光灯开关：1315 `fill_light.front`
    pub(crate) fill_light_front: Option<bool>,
    /// 最近一次「摇杆被门禁拒绝」的原因与上报时刻，用于限频（否则 10Hz 摇杆会刷爆事件流）
    pub(crate) teleop_block_notice: Option<(String, Instant)>,
    /// 强制接管标记：平台二次确认后忽略 `ctrl_source = 3`（遥控器占用）这一条门禁
    ///
    /// 现场：遥控器关了但本体还在报 `ctrl_source = 3`，常规门禁会把摇杆全部拦掉。
    /// 这个开关只放行 ctrl_source 一条，其余（离线 / sdk_node / 急停 / 卧倒）照旧。
    pub(crate) teleop_force: bool,
}

/// 本体状态新鲜窗口：1315 是 1Hz，30s 收不到就认为本体链路已不可信
pub(crate) const BODY_STATUS_FRESH: Duration = Duration::from_secs(30);

/// 「摇杆被门禁拒绝」上报平台的限频窗口（原因相同则最多这么久一条）
pub(crate) const TELEOP_BLOCK_NOTICE_INTERVAL: Duration = Duration::from_secs(30);

impl RobotState {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            battery: config.battery,
            temperature: 32.0,
            position: config.position.clone(),
            runtime_minutes: 0,
            odometer: 0.0,
            current_task: None,
            firmware: config.firmware.clone(),
            model: None,
            loc_status: 255,
            loc_fresh: false,
            ctrl_source: None,
            active_map: None,
            body_status_at: None,
            mode: "sim",
            charging: false,
            robot_mode: None,
            motion_status: None,
            battery_min: 100,
            estop_software: None,
            sdk_node_available: None,
            fill_light_front: None,
            teleop_block_notice: None,
            teleop_force: false,
        }
    }

    /// 本体链路是否在线：以 1315 心跳新鲜度为准（TCP 还连着但本体不出数也算掉线）
    pub(crate) fn body_status_fresh(&self) -> bool {
        self.body_status_at
            .map(|at| at.elapsed() < BODY_STATUS_FRESH)
            .unwrap_or(false)
    }

    /// 运动/任务前置条件（对齐官方 App 的 ready 门禁）。
    ///
    /// 官方 App 在 `sdk_node` 未就绪时显示“设备初始化中”，在 `ctrl_source`
    /// 被外部程序/遥控器抢占时暂停遥控，在急停未解除时不允许运动；这里把
    /// 同样的判断收敛到客户端，避免“平台显示已下发、本体实际拒收”。
    pub(crate) fn motion_block_reason(&self, require_posture: bool) -> Option<String> {
        self.motion_block_reason_with(require_posture, false)
    }

    /// 同上，但可选择忽略 `ctrl_source = 3` 这一条
    ///
    /// `allow_ctrl_preempt` 只给强制接管后的手动动作与摇杆用：遥控器断电后本体
    /// 仍可能报 `ctrl_source = 3`，不放行就永远接不回来。任务/导航类指令不进这个开关。
    pub(crate) fn motion_block_reason_with(
        &self,
        require_posture: bool,
        allow_ctrl_preempt: bool,
    ) -> Option<String> {
        if self.body_status_at.is_none() {
            return Some("尚未收到本体 1315 状态（设备初始化中），请稍后重试".to_string());
        }
        if !self.body_status_fresh() {
            return Some("本体 1315 状态超过 30s 未更新（本体离线或算法节点未启动）".to_string());
        }
        if self.sdk_node_available == Some(false) {
            return Some(
                "本体 SDK 节点未就绪（sdk_node=false），请等待初始化或重启本体算法节点".to_string(),
            );
        }
        if self.estop_software == Some(true) {
            return Some("本体软件急停已触发，请先点「解除急停」".to_string());
        }
        if self.ctrl_source == Some(3) && !allow_ctrl_preempt {
            return Some(
                "本体控制权不在平台（ctrl_source=3）：可能是物理遥控器 / 第三方 App，\
                 也可能是本体内部残留任务占着导航能力；请先结束外部控制或重启本体算法服务"
                    .to_string(),
            );
        }
        if require_posture && matches!(self.motion_status.as_deref(), Some("lie_down")) {
            return Some("本体当前为卧倒姿态，请先点「站立」再执行".to_string());
        }
        None
    }

    /// 处理 1315 `kRobotStatus` 本体状态上报。
    ///
    /// 现场实测字段（`data`）：`battery{power1,power2,present1,present2,temperature1,
    /// temperature2,voltage1,current1,power_supply_status1...}`、`temp`、`mile_data`、
    /// `charging_pile_connect`、`mode`、`motion_status`、`speed{}`、`estop{}` 等。
    /// 电量 / 温度 / 里程 / 充电状态全部只从这里取，不做任何推断或随机漂移。
    pub(crate) fn apply_robot_status(&mut self, data: &Value) {
        self.body_status_at = Some(Instant::now());

        if let Some(battery) = data.get("battery") {
            // 双电池取较高的那一节作为展示电量
            let p1 = battery.get("power1").and_then(Value::as_f64).unwrap_or(0.0);
            let p2 = battery.get("power2").and_then(Value::as_f64).unwrap_or(0.0);
            let present2 = battery
                .get("present2")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let power = if present2 { p1.max(p2) } else { p1 };
            let power_min = if present2 { p1.min(p2) } else { p1 };
            self.battery = power.round() as i64;
            self.battery_min = power_min.round() as i64;
            if present2 && (p1 - p2).abs() >= 5.0 {
                // 平台上展示的是较高那节，本体 ALG 用的是可用电量（较低那节），差值大时
                // 会出现「平台 52%、ALG 23%」这种误判，巡检查电量阈值时要用 battery_min
                warn!(
                    "两节电池差异较大：power1={p1:.0}% power2={p2:.0}%，平台展示取较高值 {}%，\
                     按可用电量 {}% 判断低电保护",
                    self.battery, self.battery_min
                );
            }

            // 充电判定：充电桩接入，或电池电源状态为 charging(1) / full(4)。
            // power_supply_status 用的是标准枚举：0=unknown 1=charging 2=discharging
            // 3=not_charging 4=full（现场实测放电时是 2），所以只认 1 和 4。
            let pile = data
                .get("charging_pile_connect")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let supply = [1_i64, 4].iter().any(|status| {
                [
                    battery.get("power_supply_status1"),
                    battery.get("power_supply_status2"),
                ]
                .iter()
                .any(|value| value.and_then(Value::as_i64) == Some(*status))
            });
            self.charging = pile || supply;

            // 温度优先取机体 `temp`，缺省（0）时退回电池温度
            let body_temp = data.get("temp").and_then(Value::as_f64).unwrap_or(0.0);
            let battery_temp = ["temperature1", "temperature2"]
                .iter()
                .filter_map(|key| battery.get(*key).and_then(Value::as_f64))
                .find(|value| *value > 0.0)
                .unwrap_or(0.0);
            let temperature = if body_temp > 0.0 {
                body_temp
            } else {
                battery_temp
            };
            if temperature > 0.0 {
                self.temperature = temperature;
            }
        }

        // 本体工作模式：协议里叫 MachineStatus，网关以字符串 `mode` 下发
        // （现场实测 `remote`）；个别固件给数值枚举，原样保留成字符串由前端展示
        if let Some(mode) = data
            .get("mode")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                data.get("machine_status")
                    .or_else(|| data.get("machineStatus"))
                    .and_then(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| value.as_i64().map(|code| code.to_string()))
                    })
            })
        {
            self.robot_mode = Some(mode);
        }

        // 控制源：0 未知 / 1 APP / 2 SDK / 3 其他（硬件遥控器走这条）
        if let Some(source) = data.get("ctrl_source").and_then(Value::as_i64)
            && self.ctrl_source != Some(source)
        {
            info!("本体控制源变化: ctrl_source={source}");
            self.ctrl_source = Some(source);
        }

        if let Some(mile) = data.get("mile_data").and_then(Value::as_f64) {
            self.odometer = mile;
        }

        // 运动状态 / 急停 / 补光灯：
        //
        // 固件不回 1303（实测），下发 1302 后只能靠这三个字段确认是否真的生效，
        // 所以变化时打一条 info，运维在 journalctl 里就能看到「站立命令到底动没动」。
        if let Some(status) = data
            .get("motion_status")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        {
            if self.motion_status.as_deref() != Some(status.as_str()) {
                info!("本体运动状态: {status}");
            }
            self.motion_status = Some(status);
        }
        if let Some(estop) = data
            .get("estop")
            .and_then(|value| value.get("software"))
            .and_then(Value::as_bool)
        {
            if self.estop_software != Some(estop) {
                if estop {
                    warn!("本体软件急停按下中（estop.software=true）：运动指令会被拒");
                } else {
                    info!("本体软件急停已解除");
                }
            }
            self.estop_software = Some(estop);
        }
        if let Some(front) = data
            .get("fill_light")
            .and_then(|value| value.get("front"))
            .and_then(Value::as_bool)
        {
            if self.fill_light_front != Some(front) {
                info!("前补光灯: {}", if front { "开" } else { "关" });
            }
            self.fill_light_front = Some(front);
        }

        self.runtime_minutes += 1;
    }

    /// 处理 1401 实时位姿（bulkdata）
    pub(crate) fn apply_pose(&mut self, data: &Value) {
        let Some(pose) = data.get("pose") else {
            return;
        };
        let x = pose
            .get("position")
            .and_then(|p| p.get("x"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let y = pose
            .get("position")
            .and_then(|p| p.get("y"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        self.position = format!("{x:.2}, {y:.2}");
    }

    /// 模拟模式下的状态漂移
    pub(crate) fn tick(&mut self, running: bool) {
        let mut rng = rand::thread_rng();
        let drain = if running { 1 } else { 0 };
        self.battery = (self.battery - drain).clamp(0, 100);
        self.temperature = (self.temperature + rng.gen_range(-0.1..0.3)).clamp(20.0, 65.0);
        self.runtime_minutes += 1;
    }

    pub(crate) fn heartbeat(&self) -> HeartbeatPayload {
        HeartbeatPayload {
            battery: self.battery,
            temperature: f64::trunc(self.temperature * 10.0) / 10.0,
            position: self.position.clone(),
            odometer: f64::trunc(self.odometer * 10.0) / 10.0,
            runtime_minutes: self.runtime_minutes,
            current_task: self.current_task.clone(),
            firmware: self.firmware.clone(),
            model: self.model.clone(),
            body_online: self.body_status_fresh(),
            mode: self.mode.to_string(),
            charging: self.body_status_fresh().then_some(self.charging),
            robot_mode: self.robot_mode.clone(),
            loc_status: Some(self.loc_status),
            map_id: self.active_map.clone(),
            ctrl_source: self.ctrl_source,
            motion_status: self.motion_status.clone(),
            estop_software: self.estop_software,
            sdk_node_available: self.sdk_node_available,
            // 由 `session` 从板端联网信号探针填（`crate::signal`）：状态里没有这个字段，
            // 因为信号来自背包板子而不是本体
            signal: None,
            signal_source: None,
        }
    }
}

/// 任务执行上下文：把业务 taskCode 与智航任务绑定
#[derive(Default, Clone)]
pub(crate) struct ActiveTask {
    pub(crate) task_code: String,
    pub(crate) task_name: String,
    pub(crate) nav_task_id: Option<String>,
    pub(crate) stage: Option<String>,
    /// 当前航点下标（5009 `detail_index` + 断点偏移，0 基原始下标）：定时摘要里
    /// 单独作为 `waypointIndex` 上报
    pub(crate) point_index: Option<i64>,
    /// 断点续跑起点（0 基，原始路线下标）：本轮本体路径里只有这之后的航点，
    /// 5009 的 `detail_index` 是本轮下标，查本地动作表 / 上报平台都要加回这个偏移。
    /// `0` = 从起点跑（正常一轮）。
    pub(crate) start_index: i64,
    /// 本条路线里带「AI 识别」动作的航点（到点时按 `detail_index` 查）
    pub(crate) ai_points: Vec<AiPoint>,
    /// 本条路线里带「开补光灯 / 开 LED」动作的航点**和航段**（与 `ai_points` 分开：
    /// 有点只开灯不识别）。航段灯用起点航点下标 + `LightScope::Leg`，
    /// 同一个下标两处都配时航点动作优先（见 `lights::light_point_at`）
    pub(crate) light_points: Vec<LightPoint>,
    /// 计划级「仅夜间自动开灯」（`tasks.light_night_only` → 载荷 `lightNightOnly`）：
    /// 为 true 时把 `nightOnly` 交给板端按本地时间窗判，白天到达不开灯
    pub(crate) light_night_only: bool,
    /// 计划级「自动开灯」（`tasks.light_fill` / `light_led` → 载荷 `lightFill` / `lightLed`）：
    /// `Some` = **以计划为准**，航点动作里的到点开 / 离开关全不生效，任务一开始就开、
    /// 终态 / 取消才关。下标固定 `PLAN_LIGHT_INDEX`（不是真航点）。
    pub(crate) plan_lights: Option<LightPoint>,
    /// 计划里自定义的「仅夜间」时段（`lightNightStart` / `lightNightEnd`）：
    /// **只搬运给板端判夜**，客户端自己不算时间；`None` = 板端默认。
    pub(crate) light_window: Option<NightWindow>,
}

impl ActiveTask {
    /// 到了 `index` 这个航点要开哪几盏灯。
    ///
    /// **计划级自动开灯优先**：配了计划灯就忽略航点 / 航段动作（包括「这个点没配灯」），
    /// 这样 5009 里的航点切换（到点 / 离开）都不会把灯翻来翻去。
    /// 没配计划灯时：航点动作优先，没配才用「从这里出发的航段」的灯
    /// （`light_at` 返回的 `scope` 决定开关灯时机：航点灯到点开 / 离开关，
    /// 航段灯到点只记账、**起步（离开航点）才开**，见 `PointLights::leave_point`）。
    pub(crate) fn light_at(&self, index: i64) -> Option<LightPoint> {
        if let Some(plan) = self.plan_lights.as_ref() {
            return Some(plan.clone());
        }
        light_point_at(&self.light_points, index).cloned()
    }

    /// 是不是按计划开灯（全程开、不跟航点动作）
    pub(crate) fn plan_lighting(&self) -> bool {
        self.plan_lights.is_some()
    }
}

pub(crate) type Shared<T> = Arc<Mutex<T>>;

/// 复位本地任务上下文。
///
/// 任务没真正跑起来（航点缺失 / 地图不在本体 / 定位未就绪）时必须调这个：
/// 否则 5s 进度定时器会继续拿这条已失败的任务上报「已暂停 / 等待本体连接」，
/// 把刚发出去的「异常」覆盖掉，平台事件流也会被刷屏。
pub(crate) async fn clear_active_task(
    active: &Shared<Option<ActiveTask>>,
    state: &Shared<RobotState>,
) {
    *active.lock().await = None;
    state.lock().await.current_task = None;
}

/// 只在当前上下文仍是 `task_code` 时才复位。
///
/// 上一轮的收尾是**异步**的（本地 runner 的 `tokio::spawn`、建图 4001、任务取消），
/// 收尾跑到时 `active` 里可能已经换成了新下发的任务。无条件 `*active = None`
/// 会把新任务的上下文清掉，后果是：5009 不再跟随（`handle_status_frame` 找不到
/// 绑定的 `nav_task_id`）、5s 进度定时器停报、AI 窗口/航点灯光全部失效——
/// 平台看到的是「任务已下发但没有任何进度」，而狗其实已经在跑。
/// 所以收尾一律按自己的 `task_code` 比对，不是自己的就不许动。
pub(crate) async fn clear_active_task_if(
    active: &Shared<Option<ActiveTask>>,
    state: &Shared<RobotState>,
    task_code: &str,
) -> bool {
    let mut guard = active.lock().await;
    let ours = guard
        .as_ref()
        .is_some_and(|task| task.task_code == task_code);
    if !ours {
        return false;
    }
    *guard = None;
    drop(guard);
    let mut st = state.lock().await;
    st.current_task = None;
    true
}

/// 把本体任务号补进 `active`（只在「还是这条任务、且尚未绑定」时写一次）。
///
/// 巡检（`TaskMode::Route`）的 5000 是客户端 `RouteRunner` 自己发的，本体任务号只存在于
/// runner 里，`ActiveTask.nav_task_id` 一直是 `None`。而 5009 的跟随（航点 AI、航点灯光、
/// 阶段 / 终态上报）都以 `active.nav_task_id` 为前提：以前靠 `task_type == "patrol"` 兜底
/// 绑定，改成「必须已绑定」之后兜底没了，每一帧都在 `bound_id?` 处返回——表现就是
/// 「计划勾了自动开灯，到点也不开灯」「AI 识别不触发」（2026-11-23 19:35 现场）。
///
/// 为什么从 runner 取而不是从 5009 的 `current_task` 猜：`current_task` 可能是上一轮
/// 还没交还的任务，猜错就会把整条任务绑到别人的 `task_id` 上（且不会自我纠正）。
pub(crate) async fn bind_nav_task(
    active: &Shared<Option<ActiveTask>>,
    task_code: &str,
    nav_task_id: &str,
) -> bool {
    let mut guard = active.lock().await;
    let Some(task) = guard.as_mut() else {
        return false;
    };
    if task.task_code != task_code || task.nav_task_id.is_some() {
        return false;
    }
    task.nav_task_id = Some(nav_task_id.to_string());
    true
}

#[cfg(test)]
mod active_task_light_tests {
    use super::*;
    use crate::lights::LightScope;
    use crate::lights::PLAN_LIGHT_INDEX;

    fn light(index: i64, fill: bool, led: bool) -> LightPoint {
        LightPoint {
            index,
            scope: LightScope::Point,
            fill,
            led,
        }
    }

    fn leg_light(index: i64, fill: bool, led: bool) -> LightPoint {
        LightPoint {
            index,
            scope: LightScope::Leg,
            fill,
            led,
        }
    }

    /// **计划级自动开灯优先**：配了计划灯就忽略航点动作（到点开 / 离开关都不做）。
    ///
    /// 现场要求「计划里配了自动开灯，航点上的开灯关灯不生效，以计划为准」——灯从任务
    /// 开始亮到结束，不在每个航点上翻一遍。
    #[test]
    fn plan_lights_override_waypoint_actions() {
        let plan = light(PLAN_LIGHT_INDEX, false, true);
        let mut task = ActiveTask {
            light_points: vec![light(0, true, false), light(2, false, false)],
            plan_lights: Some(plan.clone()),
            ..ActiveTask::default()
        };

        assert!(task.plan_lighting());
        // 航点 0 自己配了补光灯，但以计划为准（只开 LED）
        assert_eq!(task.light_at(0), Some(plan.clone()));
        // 没配灯的航点也返回计划灯：5009 的「到达无灯光动作的航点 → 关灯」不会触发
        assert_eq!(task.light_at(3), Some(plan));

        // 没配计划灯 → 回到航点动作（老行为）
        task.plan_lights = None;
        assert!(!task.plan_lighting());
        assert_eq!(task.light_at(0), Some(light(0, true, false)));
        assert_eq!(task.light_at(1), None);
    }

    /// 航段灯：这个点没配航点灯时，「从这里出发的航段」的灯就是这个点的灯；
    /// 两处都配时**航点动作优先**（与语音播报同一条规则）。
    #[test]
    fn leg_lights_apply_at_the_leg_start_with_point_priority() {
        let mut task = ActiveTask {
            light_points: vec![
                light(0, true, false),
                leg_light(0, false, true),
                leg_light(2, true, true),
            ],
            ..ActiveTask::default()
        };

        // P1 上既有航点灯又有航段灯 → 航点优先（走起来就关，航段那盏不生效）
        assert_eq!(task.light_at(0), Some(light(0, true, false)));
        // P3 只有航段灯（返程段）→ 用航段的：`scope` 让它在到点时只记账、起步才亮
        assert_eq!(task.light_at(2), Some(leg_light(2, true, true)));
        // 什么都没配的点：不用开灯
        assert_eq!(task.light_at(1), None);

        // 计划灯仍然压倒两者
        task.plan_lights = Some(light(PLAN_LIGHT_INDEX, true, true));
        assert_eq!(task.light_at(2), Some(light(PLAN_LIGHT_INDEX, true, true)));
    }
}
