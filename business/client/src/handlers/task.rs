//! 任务下发：按 `task_dispatch` 载荷建图 / 巡检 / 回充。

use crate::ai_patrol::AiPatrol;
use crate::ai_patrol::parse_ai_points;
use crate::dispatch;
use crate::lights;
use crate::locate::LOCATE_ATTEMPTS;
use crate::locate::ensure_localized;
use crate::locate::parse_initial_pose;
use crate::maps::clear_local_runner;
use crate::maps::is_current_runner;
use crate::photo::Photographer;
use crate::photo::parse_photo_plan;
use crate::record::{Recorder, parse_record_plan};
use crate::report::report_progress;
use crate::speak::Speaker;
use crate::speak::parse_speak_plan;
use crate::state::ActiveTask;
use crate::state::RobotState;
use crate::state::Shared;
use crate::state::bind_nav_task;
use crate::state::clear_active_task;
use crate::state::clear_active_task_if;
use crate::wire::WsMessage;
use crate::zhpro;
use crate::zhpro::client::ZhProClient;
use crate::zhpro::mapping::MappingSession;
use crate::zhpro::route::RouteRunner;
use dispatch::TaskMode;
use dispatch::parse_flight_plan;
use dispatch::parse_map_binding;
use lights::PointLights;
use lights::parse_light_points;
use robodog_device_client::config::Config;
use robodog_device_client::config::RoadNetworkMode;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::error;
use tracing::info;
use tracing::warn;
use zhpro::mapping::MapManager;
use zhpro::route::Control;

use zhpro::protocol::*;

/// `ctrl_source = 3` 的等待窗口与轮询步长（见 [`wait_only_ctrl_preempt`]）
const CTRL_PREEMPT_GRACE: Duration = Duration::from_secs(5);
const CTRL_PREEMPT_GRACE_STEP: Duration = Duration::from_millis(500);

/// `ctrl_source = 3` 常常只是「上一轮本体任务刚结束、导航能力还没交回来」的瞬时状态：
/// 官方 App 里任务被本体接手后也会报 `3`（见 `deploy/robodog-body.md` §7.5），
/// 而平台台账（心跳 10s 一次）可能还停在上一次的 `1/2`，于是「平台已经派单、
/// 客户端当场拒单」——现场看到的就是「任务下发后狗永远不进任务状态」。
///
/// 所以这里给一个短窗口（[`CTRL_PREEMPT_GRACE`]）等它回落，其余门禁照旧即时拒绝。
/// 真的被物理遥控器占着时也只是晚几秒拒，不会在 `ctrl_source=3` 期间下发任何东西。
async fn wait_only_ctrl_preempt(state: &Shared<RobotState>) -> Option<String> {
    let mut block = state.lock().await.motion_block_reason(true);
    block.as_ref()?;
    let only_ctrl_preempt = |guard: &RobotState| {
        guard.ctrl_source == Some(3) && guard.motion_block_reason_with(true, true).is_none()
    };
    let started = std::time::Instant::now();
    while started.elapsed() < CTRL_PREEMPT_GRACE {
        if !only_ctrl_preempt(&*state.lock().await) {
            break;
        }
        tokio::time::sleep(CTRL_PREEMPT_GRACE_STEP).await;
        block = state.lock().await.motion_block_reason(true);
        if block.is_none() {
            info!("ctrl_source 已回落，任务继续下发");
            return None;
        }
    }
    block
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch(
    config: &Config,
    msg: WsMessage,
    out_tx: &mpsc::Sender<WsMessage>,
    state: &Shared<RobotState>,
    active: &Shared<Option<ActiveTask>>,
    zhpro: &Option<Arc<ZhProClient>>,
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    local_mapping: &Shared<Option<Arc<MappingSession>>>,
    ai: &Arc<AiPatrol>,
    lights: &Arc<PointLights>,
    recorder: &Arc<Recorder>,
    speaker: &Arc<Speaker>,
    photographer: &Arc<Photographer>,
) {
    let task_code = msg.payload["taskCode"].as_str().unwrap_or("").to_string();
    let task_name = msg.payload["taskName"]
        .as_str()
        .unwrap_or("未命名任务")
        .to_string();
    let task_type = msg.payload["taskType"].as_str().unwrap_or("巡检");
    let mode = TaskMode::parse(task_type);
    info!("收到任务: [{task_code}] {task_name} 类型={task_type} 模式={mode:?}");
    // 排障最有用的一行：把业务侧下发的整份任务载荷原样打出来（含 routeData.points /
    // map / initialPose）。整条链路的信息默认可见：服务端 `task_dispatch 下发载荷`
    // → 客户端这行 → 客户端的 `5000` 完整帧 → 本体的 `5001` / `5009`。
    // （模拟模式下同样会打，用来核对 map / routeData 是否与平台一致）
    info!("task_dispatch 载荷: {}", msg.payload);

    // 本条路线里带「AI 识别」动作的航点：到点由 5009 的 detail_index 查表触发
    let ai_points = parse_ai_points(&msg.payload["routeData"]);
    if !ai_points.is_empty() {
        info!(
            "任务 {task_code} 含 {} 个 AI 识别航点: {:?}",
            ai_points.len(),
            ai_points
                .iter()
                .map(|point| (point.index, point.name.as_str()))
                .collect::<Vec<_>>()
        );
    }
    // 航点灯光（与 AI 动作分开：有点只开灯不识别）+ 计划级「自动开灯 / 仅夜间」
    let light_points = parse_light_points(&msg.payload["routeData"]);
    // 计划级自动开灯：配了就**以计划为准**。航点动作里的到点开灯 / 离开关灯全不生效
    // （现场要的是「整个计划期间灯亮着」，不是每个点翻一遍），夜间限制仍交给板端。
    let plan_fill = msg.payload["lightFill"].as_bool().unwrap_or(false);
    let plan_led = msg.payload["lightLed"].as_bool().unwrap_or(false);
    let plan_lights = (plan_fill || plan_led).then_some(lights::LightPoint {
        index: lights::PLAN_LIGHT_INDEX,
        fill: plan_fill,
        led: plan_led,
    });
    if let Some(plan) = plan_lights.as_ref() {
        info!(
            "任务 {task_code} 计划级自动开灯（补光={} LED={}）：以计划为准，航点动作里的 \
             开灯 / 关灯不生效（{} 个灯光航点被忽略）",
            plan.fill,
            plan.led,
            light_points.len()
        );
    } else if !light_points.is_empty() {
        info!(
            "任务 {task_code} 含 {} 个灯光航点: {:?}",
            light_points.len(),
            light_points
                .iter()
                .map(|point| (point.index, point.fill, point.led))
                .collect::<Vec<_>>()
        );
    }
    // 老平台不发 `lightNightOnly`：按「到点就开」跑（历史行为=不开灯）
    let light_night_only = msg.payload["lightNightOnly"].as_bool().unwrap_or(false);
    // 计划里自定义的夜间时段（HH:MM）：只搬运给板端判夜，客户端不自己算
    let light_window = lights::NightWindow::from_payload(&msg.payload);
    if let Some(window) = light_window.as_ref() {
        info!(
            "任务 {task_code} 夜间时段：{}-{}（板端按板子本地时间判定）",
            window.start, window.end
        );
    }
    // 航点「录像采集」+ 航段「全程录像」：与 AI / 灯光同源（路线动作），
    // 但起停时机在 5009 上（到点 / 离开 / 终态），见 `record` 模块。
    let record_plan = parse_record_plan(&msg.payload["routeData"]);
    if !record_plan.is_empty() {
        info!(
            "任务 {task_code} 含 {} 个录像航点，全程录像={}",
            record_plan.points.len(),
            record_plan.task.is_some()
        );
    }
    recorder.start_task(&task_code, record_plan).await;
    // 航点「语音播报」+ 航段「语音播报」：与 AI / 灯光 / 录像同源（路线动作），
    // 起停时机同样在 5009 上（到点 / 离开 / 终态），见 `speak` 模块。
    let speak_plan = parse_speak_plan(&msg.payload["routeData"]);
    if !speak_plan.is_empty() {
        info!(
            "任务 {task_code} 含 {} 个播报航点，{} 个播报航段",
            speak_plan.points.len(),
            speak_plan.legs.len()
        );
    }
    speaker.start_task(&task_code, speak_plan).await;
    // 航点「可见光拍照」：到点由客户端调板端 Agent 抓 JPEG，任务终态回传平台。
    let photo_plan = parse_photo_plan(&msg.payload["routeData"]);
    if !photo_plan.is_empty() {
        info!("任务 {task_code} 含 {} 个拍照航点", photo_plan.points.len());
    }
    photographer.start_task(&task_code, photo_plan).await;
    *active.lock().await = Some(ActiveTask {
        task_code: task_code.clone(),
        task_name: task_name.clone(),
        nav_task_id: None,
        stage: None,
        point_index: None,
        ai_points,
        light_points,
        light_night_only,
        plan_lights,
        light_window,
    });
    state.lock().await.current_task = Some(task_name.clone());
    // 新任务开始：清掉上一轮同 taskCode 的「已请求 / 已收尾」痕迹，否则计划灯不亮
    lights.start_task(&task_code).await;

    let Some(client) = zhpro.clone() else {
        warn!("未连接本体，任务仅记录不执行");
        report_progress(
            out_tx,
            &task_code,
            "异常",
            0,
            Some("未连接机器人本体".to_string()),
            None,
            None,
            None,
        )
        .await;
        return;
    };

    // 官方 App 在 sdk_node 未就绪 / 急停未解除 / 控制权被外部抢占 /
    // 未站立时不会创建任务；这里在发 5000 之前统一拦掉，避免
    // “5000 受理但本体不执行，最后等 600s 超时”。
    //
    // 客户端刚启动/刚重连时，平台会立刻补发 pending 任务，而第一条
    // 1315 可能还没到；先等最多 3s，避免把“初始化中”误判成“离线”。
    if !state.lock().await.body_status_fresh() {
        for _ in 0..15 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if state.lock().await.body_status_fresh() {
                break;
            }
        }
    }
    let task_block = wait_only_ctrl_preempt(state).await;
    if let Some(reason) = task_block {
        warn!("任务 {task_code} 拒绝下发: {reason}");
        report_progress(
            out_tx,
            &task_code,
            "异常",
            0,
            Some(reason),
            None,
            None,
            None,
        )
        .await;
        clear_active_task(active, state).await;
        return;
    }

    match mode {
        TaskMode::Mapping => {
            let session = Arc::new(MappingSession::new(client.clone()));
            *local_mapping.lock().await = Some(session.clone());
            let kind = match config.map_kind.as_str() {
                "indoor" => zhpro::protocol::MappingType::Indoor,
                _ => zhpro::protocol::MappingType::Outdoor,
            };
            match session.start(&task_name, kind).await {
                Ok(task_id) => {
                    report_progress(
                        out_tx,
                        &task_code,
                        "执行中",
                        0,
                        Some("建图任务已下发".to_string()),
                        Some("mapping.preparing".to_string()),
                        Some(task_id),
                        None,
                    )
                    .await;
                }
                Err(e) => {
                    error!("建图任务下发失败: {e}");
                    report_progress(
                        out_tx,
                        &task_code,
                        "异常",
                        0,
                        Some(format!("建图任务下发失败: {e}")),
                        None,
                        None,
                        None,
                    )
                    .await;
                    // 建图没起来：别留着「正在建图」的占位，否则进度定时器会拿它刷假状态
                    *local_mapping.lock().await = None;
                    clear_active_task(active, state).await;
                }
            }
        }

        TaskMode::ReturnToCharge => {
            let binding = parse_map_binding(&msg.payload);
            // 5014 的 `map_id` 是本体目录名（4013 `path` 最后一段），不是业务 ID
            let manager = MapManager::new(client.clone());
            let robot_map = match manager
                .resolve(&[&binding.map_dir, &binding.map_path, &binding.map_id])
                .await
            {
                Ok(map) => map,
                Err(e) => {
                    warn!("任务 {task_code} 的地图不在本体上，拒绝返充: {e}");
                    report_progress(out_tx, &task_code, "异常", 0, Some(e), None, None, None).await;
                    clear_active_task(active, state).await;
                    return;
                }
            };
            match client
                .call(T_RETURN_TO_CHARGE, json!({ "map_id": robot_map.map_id }))
                .await
            {
                Ok(data) => {
                    let robot_task = data
                        .get("task_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(a) = active.lock().await.as_mut() {
                        a.nav_task_id = robot_task.clone();
                    }
                    report_progress(
                        out_tx,
                        &task_code,
                        "执行中",
                        0,
                        Some("返回充电点任务已受理".to_string()),
                        Some("return_to_charge.preparing".to_string()),
                        robot_task,
                        None,
                    )
                    .await;
                }
                Err(e) => {
                    report_progress(
                        out_tx,
                        &task_code,
                        "异常",
                        0,
                        Some(format!("返回充电点失败: {e}")),
                        None,
                        None,
                        None,
                    )
                    .await;
                }
            }
        }

        TaskMode::Recharge | TaskMode::Undock => {
            let kind = if mode == TaskMode::Recharge {
                TaskKind::Recharge
            } else {
                TaskKind::Undock
            };
            let data = json!({
                "kind": kind.code(),
                "name": task_name,
                "schedule": 0,
                "execute_at": now_ms().to_string(),
                "priority": 0,
                "record": false,
                "params": Value::Null
            });
            match client.call(T_TASK_CREATE, data).await {
                Ok(reply) => {
                    let robot_task = reply
                        .get("task_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(a) = active.lock().await.as_mut() {
                        a.nav_task_id = robot_task.clone();
                    }
                    report_progress(
                        out_tx,
                        &task_code,
                        "执行中",
                        0,
                        Some(format!("{}任务已受理", kind.as_str())),
                        None,
                        robot_task,
                        None,
                    )
                    .await;
                }
                Err(e) => {
                    report_progress(
                        out_tx,
                        &task_code,
                        "异常",
                        0,
                        Some(format!("任务下发失败: {e}")),
                        None,
                        None,
                        None,
                    )
                    .await;
                }
            }
        }

        TaskMode::Route => {
            let Some(mut plan) = parse_flight_plan(&msg.payload, &config.map_root) else {
                warn!("任务 {task_code} 缺少可执行的航点数据");
                report_progress(
                    out_tx,
                    &task_code,
                    "异常",
                    0,
                    Some("任务缺少航点数据（routeData.points 为空）".to_string()),
                    None,
                    None,
                    None,
                )
                .await;
                clear_active_task(active, state).await;
                return;
            };
            info!(
                "航线计划: {} 个航点, 地图={} (路径 {}), 结束动作={}",
                plan.waypoints.len(),
                plan.map_id,
                plan.map_path,
                plan.end_task_label()
            );
            // 注意：**不要**在这里把 `active_map` 改成计划里的地图。那个字段表示
            // 「本体当前真的载着哪张图」，是定位守卫判断「要不要重新载图」的依据；
            // 提前写进去会让守卫误以为已载好，从而跳过自动重定位。
            // 定位成功后由 `ensure_localized` 写回。

            // 上一轮遗留的本体任务必须先收尾：本体对「已有一个未结束任务」回
            // 13897 resource conflict (capability busy)，而任务被暂停（5004）后
            // 不会自动结束——平台直接重新下发时第一个 5000 必然被拒。
            if let Some(previous) = local_runner.lock().await.take() {
                let had_robot_task = previous.current_nav_task().await.is_some();
                match previous.control(Control::Cancel).await {
                    Ok(()) => {
                        if had_robot_task {
                            info!(
                                "任务 {task_code}：已 5002 取消上一轮本体导航任务，等本体释放导航能力"
                            );
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                    }
                    Err(e) => warn!("取消上一轮本体导航任务失败: {e}"),
                }
            }

            // 前置校验：业务 `mapId` 不是本体目录名，必须用 4013 列表解析出真实目录。
            // 对不上就不发 5000——否则本体只会回 13898 missing map.yaml。
            let manager = MapManager::new(client.clone());
            let robot_map = match manager.resolve(&[&plan.map_path, &plan.map_id]).await {
                Ok(map) => map,
                Err(e) => {
                    warn!("任务 {task_code} 的地图不在本体上，拒绝下发: {e}");
                    report_progress(out_tx, &task_code, "异常", 0, Some(e), None, None, None).await;
                    clear_active_task(active, state).await;
                    return;
                }
            };
            info!(
                "机器人地图就绪: {} 目录={} 路网={} 回充点={}",
                plan.map_id, robot_map.map_id, robot_map.route_exist, robot_map.charge_point_exist
            );
            if plan.end_task() == zhpro::route::EndTask::ReturnToCharge
                && !robot_map.charge_point_exist
            {
                // 官方 App 在 charge_point_exist=false 时直接置灰返充入口；
                // 这里提前失败，避免跑完整条航线才在 5014 收到 0x3672。
                warn!("任务 {task_code} 拒绝下发：地图未设置回充点");
                report_progress(
                    out_tx,
                    &task_code,
                    "异常",
                    0,
                    Some(
                        "该地图未设置回充点（charge_point_exist=false），无法返航充电；\
                                 请先在本体设置回充点（4018）或把结束动作改为「原地」"
                            .to_string(),
                    ),
                    None,
                    None,
                    None,
                )
                .await;
                clear_active_task(active, state).await;
                return;
            }
            // 用本体上报的真实目录路径（L2/M1 地图根不同，不能靠平台拼）
            plan.map_path = robot_map.path.clone();

            // 对齐官方 cruise_mode：只有整条路线都走「沿道路」且本体地图
            // 有 map.geojson 时才用路网（map_type=3）；否则一律 2D 栅格（1）。
            // 不能只看服务端 mapType：服务端可能按 road_count 给了 3，
            // 但这条路线实际是自由导航。`--road-network on/off` 可强制。
            let target_map_type = config
                .road_network
                .map_type(plan.road_network, robot_map.route_exist);
            info!(
                "地图类型：本体能力 map_type={target_map_type}（导航枚举 1/3）→ 巡检 \
                         5000 kind=4 会发 {}（0=2D 栅格 / 1=路网）（road_network 策略={}，\
                         航线全沿道路={}，本体 map.geojson={}）",
                if target_map_type == 3 { 1 } else { 0 },
                config.road_network.as_str(),
                plan.road_network,
                robot_map.route_exist
            );
            if config.road_network == RoadNetworkMode::Auto
                && plan.road_network
                && !robot_map.route_exist
                && target_map_type == 1
            {
                warn!("航线请求路网导航，但本体地图没有 map.geojson，回退 2D 栅格");
            }
            if plan.map_type != target_map_type {
                info!(
                    "5000 地图类型: 服务端下发 {} → 实际按本体能力用 {target_map_type}",
                    plan.map_type
                );
                plan.map_type = target_map_type;
            }

            // 官方链路是「1204 取控制权 → 1302 cmd=62 切导航模式 → 选图/定位 → 建任务」。
            // 巡检（kind=4）由本体自己走 `load_localization_map` + `capture_origin_pose`，
            // 这里保留一次起步前定位守卫：地图不在本体上 / 载的不是这张图 / 定位丢失时
            // **自动**载图重定位，无需人工点「重新定位」。
            if let Err(e) = zhpro::teleop::enter_navigation_mode(&client).await {
                warn!("任务 {task_code} 切导航模式失败（继续尝试，但本体可能拒任务）: {e}");
            }
            let map_path = plan.resolve_map_path(&config.map_root);
            let initial_pose = parse_initial_pose(msg.payload.get("initialPose"));
            match ensure_localized(&client, state, &plan.map_id, &map_path, initial_pose).await {
                Ok((status, loaded)) => info!(
                    "定位已就绪（status={status}，本次{}载图重定位），开始下发巡检任务\
                             （5000 kind=4：航点与到点动作由本体 nav_paths 里的路径执行）",
                    if loaded { "" } else { "未" }
                ),
                Err(e) => {
                    warn!("任务 {task_code} 定位未就绪，拒绝下发: {e}");
                    report_progress(
                        out_tx,
                        &task_code,
                        "异常",
                        0,
                        Some(format!(
                            "定位未就绪：{e}（已自动重定位 {LOCATE_ATTEMPTS} 次）。\
                                     请远程接管把设备移回地图覆盖范围，或下发「重新定位」\
                                     并给出初始位姿后重试"
                        )),
                        None,
                        None,
                        None,
                    )
                    .await;
                    clear_active_task(active, state).await;
                    return;
                }
            }

            // 软件急停 / sdk_node / ctrl_source / 卧倒姿态已由上面的
            // `motion_block_reason(true)` 统一拦过，这里不再重复检查
            let runner = Arc::new(RouteRunner::new(client.clone()));
            // 巡检下发前用它和 min_recharge_battery 做低电守卫（双电池取较低那节）；
            // 先取出来再 await，避免把 state 的锁带过 await 点
            let battery_percent = state.lock().await.battery_min;
            runner.set_battery_percent(battery_percent).await;
            runner
                .set_patrol_options(&config.patrol_path_prefix, config.min_recharge_battery)
                .await;
            // 巡检的总点数 = 路径点数（本体按 nav_paths 里的点算，不做「闭环补点」）
            let total_points = plan.waypoints.len() as i64;
            runner.prepare(plan).await;
            *local_runner.lock().await = Some(runner.clone());

            // 把 runner 拿到的本体任务号补进 `active`：5009 的跟随（航点 AI / 航点灯光 /
            // 阶段与终态上报）都要求先有 `nav_task_id`，而巡检的 5000 是 runner 自己
            // 发的（见 `state::bind_nav_task`）。**不能**再靠 5009 的 `current_task` 猜
            // ——那可能是上一轮还没交还的任务。200ms 一轮，跑到就退出。
            {
                let watcher_runner = runner.clone();
                let slot = local_runner.clone();
                let active = active.clone();
                let code = task_code.clone();
                tokio::spawn(async move {
                    for _ in 0..300 {
                        // 任务已经收尾（runner 被摘掉）或已被新一轮替换：不再等。
                        // 同一个 taskCode 会被反复下发（现场反复点「开始执行」），只比
                        // taskCode 会把上一轮的任务号绑到新一轮上。
                        if !is_current_runner(&slot, &watcher_runner).await {
                            return;
                        }
                        if let Some(id) = watcher_runner.current_nav_task().await {
                            if bind_nav_task(&active, &code, &id).await {
                                info!(
                                    "任务 {code} 绑定本体任务号 {id}：5009 的航点 AI / 航点灯光从此生效"
                                );
                            }
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    warn!("任务 {code} 60s 内没有拿到本体任务号：5009 不会跟随这条任务");
                });
            }

            let out = out_tx.clone();
            let code = task_code.clone();
            let state = state.clone();
            let active = active.clone();
            let runner_slot = local_runner.clone();
            let ai = ai.clone();
            let lights = lights.clone();
            let speaker = speaker.clone();
            let photographer = photographer.clone();
            tokio::spawn(async move {
                let outcome = runner.run().await;
                let (status, message) = match outcome {
                    Ok(o) if o.success => (
                        "已完成",
                        format!(
                            "{}（{}/{} 航点）",
                            o.message, o.completed_points, o.total_points
                        ),
                    ),
                    Ok(o) => ("异常", format!("{}（{}）", o.message, o.skipped)),
                    Err(e) => ("异常", format!("执行失败: {e}")),
                };
                report_progress(
                    &out,
                    &code,
                    status,
                    100,
                    Some(message),
                    None,
                    None,
                    Some(total_points),
                )
                .await;
                // 本地跑的路子同样要在任务收尾时清掉整个计划的板端存证
                ai.finish_plan(&code, "本地任务结束").await;
                // 本地跑不一定会走到 5009 的「离开航点」，这里直接收灯
                lights.finish_task(&code, "本地任务结束").await;
                // 语音播报同理：循环播报不能跟着狗回家
                speaker.finish_task(&code, "本地任务结束").await;
                // 照片：停拍 + 回传（本地跑不一定会走到 5009 的终态帧）
                photographer.finish_task(&code, "本地任务结束").await;
                // 收尾必须把槽位里的 runner 摘掉：业务侧的下发地图 / 继续任务等
                // 门禁都看这个槽位，残留的 runner 会让设备在任务早已结束（甚至
                // 取消）之后仍报「设备正在执行任务」（2026-11-19 现场事故）。
                clear_local_runner(&runner_slot, &runner).await;
                // 同理，`active` 也只能摘「自己这一份」：上一轮 runner 的收尾是异步的，
                // 跑到这里时平台可能已经下发了新任务；无条件 `*active = None` 会把新
                // 任务的上下文清掉（5009 不再跟随、进度定时器停报、AI/灯光全失效），
                // 平台看到的是「已下发但没有进度」，狗其实已经在跑。
                clear_active_task_if(&active, &state, &code).await;
            });
        }
    }
}
