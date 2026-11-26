//! 任务下发载荷组装。
//!
//! `tasks.rs`（创建即推送）与 `gateway.rs`（设备上线补发）都需要把一条 `tasks` 记录
//! 组装成 [`TaskDispatchPayload`]。这里集中实现，避免两处字段漂移。
//!
//! 地图绑定规则：
//! 1. 优先使用路线冻结的 `routes.map_version_id`（路线与地图的一致性由发布时保证）；
//! 2. 路线未冻结地图时，回退到任务所属园区的当前基线 `parks.map_version_id`；
//! 3. 都没有则不下发 `map`，客户端会拒绝执行并在事件流中报错。

use crate::error::AppError;
use crate::gateway_types::{DispatchMap, TaskDispatchPayload};
use crate::models::Task;
use serde_json::{Value, json};
use sqlx::SqlitePool;

/// 智航本体地图目录名。
///
/// 业务侧地图版本 ID 形如 `ROS-MAP-3@v1.0`，`@` 在本体侧文件系统里不安全，
/// 统一替换为 `_`。与客户端 `zhpro::mapping::to_map_dir_name` 保持一致。
pub fn to_map_dir_name(map_version_id: &str) -> String {
    crate::mapdir::synthetic_dir(map_version_id)
}

/// 行程结束动作。
///
/// `executeMode` 为前端枚举（周期任务 / 临时下发 / 立即执行，以及中台外部调用可能带的
/// 单次 / 循环 / 返航 / 返航充电），映射到智航侧的结束策略。注意「返航充电」包含「返航」，
/// 必须先判充电。
///
/// 这只是**历史兼容**的推断：控制台的执行模式里并没有「返航」这个值，所以网页建的任务
/// 永远落到 `none`，真正让路线跑完必返航的是客户端 `patrol_end_task` 的闭环兜底
/// （`loop_mode == "闭环"` → 返回起点）。要按任务控制请用 [`resolve_task_end`]。
pub fn resolve_end_task(execute_mode: Option<&str>) -> &'static str {
    let mode = execute_mode.unwrap_or_default();
    if mode.contains("返充") || mode.contains("返回充电") || mode.contains("充电") {
        "return_to_charge"
    } else if mode.contains("返航") || mode.contains("返回起点") {
        "return_to_start"
    } else {
        "none"
    }
}

/// 结束动作归一化：同时接受控制台中文枚举与载荷英文枚举。
///
/// 返回 `None` 表示「认不出来」——调用方应当拒绝请求，不要静默当成跟随路线
/// （否则用户选了「原地」却因为拼错单词而继续返航，现场很难查）。
pub fn normalize_end_task(value: &str) -> Option<&'static str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value == "return_to_charge" || value.contains("充电") || value.contains("返充") {
        Some("return_to_charge")
    } else if value == "return_to_start" || value.contains("返航") || value.contains("返回起点")
    {
        Some("return_to_start")
    } else if matches!(value, "none" | "return_none" | "stay")
        || value.contains("原地")
        || value.contains("待命")
    {
        Some("none")
    } else {
        None
    }
}

/// 任务级结束动作：显式 `tasks.end_task` 优先，否则沿用 `executeMode` 推断。
///
/// 返回 `(endTask, endTaskMode)`：
/// - `endTask` 始终有值（老客户端只认这个字段）；
/// - `endTaskMode` **只在任务显式配置时有值**。必须这样区分，因为老服务端对所有任务
///   都发 `endTask: "none"`，客户端把 `none` 当「没指定」；若直接让 `endTask` 承担
///   「显式原地」语义，老客户端会被误伤成「所有闭环路线都不返航」。
pub fn resolve_task_end(task: &Task) -> (&'static str, Option<&'static str>) {
    if let Some(explicit) = task.end_task.as_deref().and_then(normalize_end_task) {
        return (explicit, Some(explicit));
    }
    (resolve_end_task(task.execute_mode.as_deref()), None)
}

/// `resolve_map` 读取的地图行：(id, park, map_name, version, file_path, source_ref, road_count)
type MapRow = (
    String,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
);

/// 解析任务应绑定的地图
async fn resolve_map(db: &SqlitePool, task: &Task) -> Result<Option<DispatchMap>, AppError> {
    // 1) 路线冻结的地图版本
    // 2) 任务所属园区的当前基线
    // 本体目录名：`source_ref`（建图回传写入）优先，缺失才退回业务 ID 合成名。
    // 合成名在本体上可能根本不存在（目录名是建图会话名），客户端会用 4013
    // 列表核对，对不上就不发 5000。
    let row: Option<MapRow> = sqlx::query_as(
        "SELECT mv.id, mv.park_id, mv.map_name, mv.version, mv.file_path, mv.source_ref,
                COALESCE(mv.road_count, 0)
         FROM map_versions mv
         WHERE mv.id = COALESCE(
             (SELECT map_version_id FROM routes WHERE id = ?1),
             (SELECT map_version_id FROM parks WHERE id = ?2)
         )",
    )
    .bind(task.route_id)
    .bind(task.park_id)
    .fetch_optional(db)
    .await?;

    let Some((map_id, park_id, map_name, version, _file_path, source_ref, road_count)) = row else {
        return Ok(None);
    };
    let map_dir = source_ref
        .as_deref()
        .and_then(crate::mapdir::dir_from_source_ref)
        .unwrap_or_else(|| crate::mapdir::synthetic_dir(&map_id));

    Ok(Some(DispatchMap {
        map_path: map_dir.clone(),
        map_dir,
        map_id,
        // 有路网段的地图按 3（Route）下发；客户端还会用 4013 的
        // map_route_exist 再核对一次，缺失时回退 1（2D 栅格）。
        map_type: if road_count > 0 { 3 } else { 1 },
        park_id,
        map_name,
        version,
    }))
}

/// 组装一条任务的下发载荷
pub async fn build_dispatch_payload(
    db: &SqlitePool,
    task: &Task,
) -> Result<TaskDispatchPayload, AppError> {
    // 路线数据与速度上限
    let route_row: Option<(Option<String>, Option<f64>, Option<String>)> = match task.route_id {
        Some(route_id) => {
            sqlx::query_as("SELECT route_data, default_speed, loop_mode FROM routes WHERE id = ?1")
                .bind(route_id)
                .fetch_optional(db)
                .await?
        }
        None => None,
    };
    let (route_data_text, default_speed, _loop_mode) = route_row.unwrap_or((None, None, None));
    let route_data: Option<Value> = route_data_text.and_then(|s| serde_json::from_str(&s).ok());

    let map = resolve_map(db, task).await?;
    let (end_task, end_task_mode) = resolve_task_end(task);

    // 坐标守卫：航点必须是地图米制且在所选地图范围内（详见 [`check_route_data`]）。
    if let Some(data) = route_data.as_ref() {
        let frame =
            crate::coords::frame_for_map(db, map.as_ref().map(|item| item.map_id.as_str())).await?;
        check_route_data(task.route_code.as_deref().unwrap_or("—"), data, &frame)?;
    }

    // 断点续跑：`tasks.resume_index` 是下次下发的起点（0 基）
    let start_index = resolve_start_index(task, route_data.as_ref())?;

    Ok(TaskDispatchPayload {
        task_id: task.id,
        task_code: task.task_code.clone(),
        task_name: task.task_name.clone(),
        task_type: task.task_type.clone(),
        execute_mode: task.execute_mode.clone(),
        route: route_data.as_ref().map(|_| {
            json!({
                "routeId": task.route_id,
                "routeCode": task.route_code,
                "routeName": task.route_name,
            })
        }),
        route_data,
        plan_start: task.plan_start.clone(),
        plan_end: task.plan_end.clone(),
        dispatch_policy: task.dispatch_policy.clone(),
        description: task.description.clone(),
        map,
        speed_limit: default_speed,
        avoid_level: Some(40),
        end_task: Some(end_task.to_string()),
        // 仅显式配置才带：客户端用它覆盖「闭环路线自动返航」的兜底
        end_task_mode: end_task_mode.map(str::to_string),
        // 计划级「仅夜间自动开灯」：客户端原样转给板端 Agent，判定在板端
        light_night_only: task.light_night_only,
        // 计划级「自动开灯」：配了就以计划为准（全程开、忽略航点动作），总是下发
        light_fill: task.light_fill,
        light_led: task.light_led,
        // 计划里自定义的夜间时段：只有两个都有才带（缺省 = 板端默认）
        light_night_start: task.light_night_start.clone(),
        light_night_end: task.light_night_end.clone(),
        // 断点续跑起点：只有 > 0 才带（`0` 就是「从起点跑」，没必要下发）
        start_index,
    })
}

/// 断点续跑起点：`tasks.resume_index` → 载荷 `startIndex`。
///
/// 校验放在下发前，而不是在设置 `resume_index` 时：航点数会随路线编排变化，
/// 一条「中断在 P4」的计划被改成 3 个点后，续跑就没有意义了——这时要报错让现场
/// 重新选择（从起点重跑），而不是静默地从头跑一遍或下发一个越界下标。
///
/// 返回 `None` = 从起点跑（0 或没配）。`routeData` 缺失（临时路径没有航点）时
/// 也不带：客户端本来就会拒绝执行。
fn resolve_start_index(task: &Task, route_data: Option<&Value>) -> Result<Option<i64>, AppError> {
    let Some(index) = task.resume_index else {
        return Ok(None);
    };
    if index <= 0 {
        return Ok(None);
    }
    let total = route_data
        .and_then(|data| data.get("points"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if total == 0 {
        return Err(AppError::BadRequest(format!(
            "任务 {} 没有可续跑的航点（路线缺失或 routeData.points 为空），只能从起点执行",
            task.task_code
        )));
    }
    if index as usize >= total {
        return Err(AppError::BadRequest(format!(
            "任务 {} 的断点 P{} 已超出航点数（共 {total} 个）：上一轮所有航点都走过了，\
             中断发生在返程；没有可续跑的航点，请从起点重跑",
            task.task_code,
            index + 1
        )));
    }
    Ok(Some(index))
}

/// 下发前的设备就绪快照。
#[derive(sqlx::FromRow)]
struct DeviceGate {
    device_code: String,
    park_id: Option<i64>,
    body_online: i64,
    estop_software: Option<i64>,
    sdk_node_available: Option<i64>,
    ctrl_source: i64,
    run_mode: Option<String>,
    teleop_holder: Option<String>,
    teleop_at: Option<String>,
}

/// 设备当前是否满足下发条件：`None` 可下发，`Some(原因)` 不可下发。
///
/// 网页端「开始执行」、建任务即下发、设备上线补发三条路径必须共用这套门禁：
/// 只在一处检查时，另外两条会把任务推给本体还没起来的设备，客户端只能回一条
/// 「未连接机器人本体」，任务停在「执行中」等看门狗收。
pub async fn device_block_reason(
    db: &SqlitePool,
    device_pk: i64,
) -> Result<Option<String>, AppError> {
    let gate: Option<DeviceGate> = sqlx::query_as(
        "SELECT device_id AS device_code, park_id, COALESCE(body_online, 0) AS body_online,
                estop_software, sdk_node_available, COALESCE(ctrl_source, 0) AS ctrl_source,
                run_mode, teleop_holder, teleop_at
         FROM devices WHERE id = ?1",
    )
    .bind(device_pk)
    .fetch_optional(db)
    .await?;
    let Some(gate) = gate else {
        return Ok(Some(format!("设备 {device_pk} 不存在")));
    };

    if gate.park_id.is_none() {
        return Ok(Some(format!(
            "设备 {} 尚未绑定园区，无法下发任务",
            gate.device_code
        )));
    }

    // 对齐官方 App 的 ready 门禁：本体节点/急停/控制权任一不满足都不下发，
    // 否则 5000 可能被受理但一直不执行，最后拖到任务看门狗判异常。
    // 纯模拟设备没有本体，不做这层校验（前端联调用）。
    if gate.run_mode.as_deref() != Some("sim") {
        if gate.body_online == 0 {
            return Ok(Some(format!(
                "设备 {} 本体未连接（body_online=false，1315 状态超时），无法下发任务",
                gate.device_code
            )));
        }
        if gate.sdk_node_available == Some(0) {
            return Ok(Some(format!(
                "设备 {} 本体 SDK 节点未就绪（sdk_node=false），请等待初始化或重启本体算法节点后再下发",
                gate.device_code
            )));
        }
        if gate.estop_software == Some(1) {
            return Ok(Some(format!(
                "设备 {} 处于软件急停（estop.software=true），请先解除急停再下发任务",
                gate.device_code
            )));
        }
        if gate.ctrl_source == 3 {
            // `3` 是「其他控制源」，不一定是物理遥控器：也可能是本体内残留任务把导航
            // 能力占住了（这种一定伴随 13897 / 13440），文案要给对方向，否则运维会
            // 一直去关一个本来就关着的遥控器。
            let reason = crate::gateway::ctrl_preempt_reason(db, &gate.device_code).await;
            return Ok(Some(format!(
                "设备 {} 的控制权不在平台（1315 ctrl_source=3）：{reason}。\
                 请先在本体取消残留任务 / 重启本体算法服务，或关闭遥控器电源后再下发任务",
                gate.device_code
            )));
        }
    }

    // 远程接管中不下发：本体停在通用（手动）模式，导航能力被判给手动控制，
    // 每个航点的 5000 都会被回 13897 resource conflict (capability busy)。
    if let Some(holder) = gate.teleop_holder.as_deref()
        && !crate::handlers::teleop::teleop_expired(gate.teleop_at.as_deref())
    {
        return Ok(Some(format!(
            "设备 {} 正在被 {holder} 远程接管（本体处于手动模式），\
             请先释放接管再下发任务，否则本体回 13897 capability busy",
            gate.device_code
        )));
    }

    Ok(None)
}

/// 航点坐标能否下发：米制标记 + 非示意坐标系 + 全部落在图内。
///
/// 智航 5000 kind=2 的 `goals[].position` 是地图坐标系下的米（协议 §2.5.1.4），
/// 而路线编辑器用的是 800×460 画布像素：没有这道检查，`x=335` 会被当成 335 米
/// 发给本体，整条航线落到图外，现场只能看到一句 `invalid request parameters`。
pub fn check_route_data(
    route_code: &str,
    data: &Value,
    frame: &crate::coords::MapFrame,
) -> Result<(), AppError> {
    if !crate::coords::is_map_frame(data) {
        return Err(AppError::BadRequest(format!(
            "路线 {route_code} 的航点还是画布像素坐标（缺 `coordFrame=map` 标记），不能下发；\
             请在路线编辑器里重新保存一次完成米制换算"
        )));
    }
    if frame.schematic {
        return Err(AppError::BadRequest(format!(
            "路线 {route_code} 绑定的地图没有真实 ROS 地图包（示意坐标系，1 画布像素按 {:.1} m 估算），\
             无法下发；请先在地图基线里发布真实地图再重新编排路线",
            crate::coords::SCHEMATIC_RESOLUTION
        )));
    }
    if let Some((index, x, y)) = crate::coords::points_outside(data, frame).first() {
        let (min_x, min_y, max_x, max_y) = frame.extents();
        return Err(AppError::BadRequest(format!(
            "路线 {route_code} 的第 {index} 个航点 ({x:.2}, {y:.2}) 超出地图范围\
             x[{min_x:.1}, {max_x:.1}] y[{min_y:.1}, {max_y:.1}]，已拒绝下发"
        )));
    }
    Ok(())
}

/// 建任务前的坐标预检：避免「任务插入成功但下发报错」的半截状态。
///
/// 地图解析顺序与 [`resolve_map`] 保持一致：路线冻结版本 → 园区当前基线。
pub async fn ensure_route_dispatchable(
    db: &SqlitePool,
    route_id: Option<i64>,
    park_id: Option<i64>,
) -> Result<(), AppError> {
    let Some(route_id) = route_id else {
        return Ok(());
    };
    let row: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT route_data, route_code FROM routes WHERE id = ?1")
            .bind(route_id)
            .fetch_optional(db)
            .await?;
    let Some((route_data, route_code)) = row else {
        return Ok(());
    };
    let Some(raw) = route_data else {
        return Ok(());
    };
    let Ok(data) = serde_json::from_str::<Value>(&raw) else {
        return Ok(());
    };
    // 与 resolve_map 相同的 COALESCE：路线冻结版本优先，否则园区基线
    let map_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM map_versions WHERE id = COALESCE(
            (SELECT map_version_id FROM routes WHERE id = ?1),
            (SELECT map_version_id FROM parks WHERE id = ?2))",
    )
    .bind(route_id)
    .bind(park_id)
    .fetch_optional(db)
    .await?;
    let frame = crate::coords::frame_for_map(db, map_id.as_deref()).await?;
    check_route_data(&route_code, &data, &frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_dir_name_replaces_at_sign() {
        assert_eq!(to_map_dir_name("ROS-MAP-3@v1.0"), "ROS-MAP-3_v1.0");
        assert_eq!(to_map_dir_name("MAP-ZJ-01"), "MAP-ZJ-01");
    }

    #[test]
    fn end_task_prefers_charge_over_return() {
        assert_eq!(resolve_end_task(Some("返航充电")), "return_to_charge");
        assert_eq!(resolve_end_task(Some("返航")), "return_to_start");
        assert_eq!(resolve_end_task(Some("单次")), "none");
        assert_eq!(resolve_end_task(None), "none");
    }

    #[test]
    fn explicit_end_task_wins_over_execute_mode() {
        let mut task = Task {
            id: 1,
            task_code: "TK-1".to_string(),
            task_name: "巡检".to_string(),
            task_type: None,
            // 历史枚举「返航」本来会推断成返回起点
            execute_mode: Some("返航".to_string()),
            end_task: Some("none".to_string()),
            light_night_only: false,
            light_fill: false,
            light_led: false,
            light_night_start: None,
            light_night_end: None,
            level: None,
            device_id: None,
            device_code: None,
            park_id: None,
            park_name: None,
            zone_id: None,
            zone_name: None,
            route_id: None,
            route_code: None,
            route_name: None,
            plan_start: None,
            plan_end: None,
            schedule_days: None,
            last_dispatched_on: None,
            publish_status: "已发布".to_string(),
            published_at: None,
            task_status: "待执行".to_string(),
            progress: 0,
            last_waypoint_index: None,
            resume_index: None,
            dispatch_policy: None,
            description: None,
            robot_task_id: None,
            robot_stage: None,
            robot_updated_at: None,
            created_at: String::new(),
            updated_at: String::new(),
            created_by: None,
        };
        assert_eq!(resolve_task_end(&task), ("none", Some("none")));

        // 未显式配置：沿用 executeMode 推断，且不带 endTaskMode（老客户端 / 老行为）
        task.end_task = None;
        task.execute_mode = Some("周期任务".to_string());
        assert_eq!(resolve_task_end(&task), ("none", None));

        task.execute_mode = Some("返航充电".to_string());
        assert_eq!(
            resolve_task_end(&task),
            ("return_to_charge", None),
            "外部调用仍可按 executeMode 文案指定"
        );
    }

    #[test]
    fn end_task_normalization() {
        assert_eq!(normalize_end_task("原地待命"), Some("none"));
        assert_eq!(normalize_end_task("none"), Some("none"));
        assert_eq!(normalize_end_task("返回起点"), Some("return_to_start"));
        assert_eq!(
            normalize_end_task("return_to_charge"),
            Some("return_to_charge")
        );
        assert_eq!(normalize_end_task("瞎写"), None);
        assert_eq!(normalize_end_task("  "), None);
    }
}
