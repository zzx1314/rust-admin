//! 地图链路：本体地图推送/回读、增量同步与中台上传。

use crate::locate::LOCATE_TIMEOUT;
use crate::mapupload::IngestConfig;
use crate::mapupload::UploadOutcome;
use crate::state::RobotState;
use crate::state::Shared;
use crate::wire::DeviceEventPayload;
use crate::wire::WsMessage;
use crate::wire::send;
use crate::zhpro::client::ZhProClient;
use crate::zhpro::mapping::MappingSession;
use crate::zhpro::route::RouteRunner;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use zhpro::mapping::MapManager;
use zhpro::mapping::MappingSaved;

use super::*;

/// 地图同步运行标记的释放器：任务结束或 panic 时都会把「正在同步」复位。
pub(crate) struct MapSyncRunning(pub(crate) Arc<std::sync::atomic::AtomicBool>);

impl Drop for MapSyncRunning {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 上报一条「地图保存」事件（信息流留痕，与任务终态分开）
pub(crate) async fn send_map_saved_event(
    out_tx: &mpsc::Sender<WsMessage>,
    ok: bool,
    message: impl AsRef<str>,
    map_path: Option<String>,
) {
    let message = message.as_ref();
    let event = DeviceEventPayload {
        event_type: if ok { "info" } else { "error" }.to_string(),
        message: format!(
            "地图保存{}: {}{}",
            if ok { "成功" } else { "失败" },
            message,
            map_path
                .as_deref()
                .map(|p| format!(" → {p}"))
                .unwrap_or_default()
        ),
        data: Some(json!({
            "mapPath": map_path,
            "success": ok,
        })),
    };
    send(
        out_tx,
        "event",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .await;
}

/// 建图完成后把地图回传中台（A 方案：本体取包 → 业务服务导流）
pub(crate) async fn upload_mapping_to_zentai(
    config: &Config,
    saved: &MappingSaved,
) -> UploadOutcome {
    if !config.upload_map {
        return UploadOutcome::Skipped("已用 --no-upload-map 关闭".to_string());
    }
    // 园区是可选的：地图只跟设备有关系，设备没绑园区也照样回传，
    // 设备绑定人之后创建园区时再从自己的设备地图里挑一张绑定。
    let park_id = match config.park_id {
        Some(park_id) => Some(park_id),
        // 未显式指定时，使用注册回执里的园区绑定
        None => *config.bound_park_id.lock().await,
    };
    let Some(map_dir) = saved.dir_name() else {
        return UploadOutcome::Failed {
            reason: "4001 未返回地图目录名".to_string(),
        };
    };

    let robot_host = config.robot.clone().unwrap_or_default();
    let file_base = mapupload::resolve_file_base(
        &robot_host,
        config.robot_file_base.as_deref(),
        config.robot_file_port,
    );
    let http = mapupload::build_http_client();

    info!("开始回传地图 {map_dir}，从 {file_base} 取包");
    let package =
        match mapupload::fetch_map_package(&http, &file_base, &map_dir, saved.map_path.as_deref())
            .await
        {
            Ok(bytes) => bytes,
            Err(e) => {
                return UploadOutcome::Failed {
                    reason: format!("{e}；本体若未开放文件服务，请用 --robot-file-base 指定"),
                };
            }
        };

    let ingest = IngestConfig {
        api_base: config.resolve_api_base(),
        username: config.api_username.clone(),
        password: config.api_password().to_string(),
        device_id: config.device_id.clone(),
        park_id,
        map_name: config.ingest_map_name.clone(),
        bind: park_id.is_some(),
        // 4001 自动回传只新增，永远不覆盖已经入库的地图
        refresh: false,
    };
    // 4001 偶尔只给目录名不给绝对路径；补一个与 4012 一致的文件服务路径，
    // 保证自动回传与手动同步使用同一个 sourceRef 去重。
    let mut saved = saved.clone();
    if saved.map_path.is_none() {
        saved.map_path = Some(format!(
            "{}/history_map/{}",
            config.map_root.trim_end_matches('/'),
            map_dir
        ));
    }
    mapupload::ingest_map(&http, &ingest, &saved, &package).await
}

/// 任务收尾时把 `local_runner` 槽位里的这一份摘掉（只摘自己）。
///
/// 用的是 `Arc::ptr_eq` 而不是无条件 `= None`：任务结束的瞬间平台可能已经下发了
/// 下一轮任务（槽位里换成了新 runner），无条件清空会误伤新任务。
pub(crate) async fn clear_local_runner(
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    finished: &Arc<RouteRunner>,
) {
    let mut slot = local_runner.lock().await;
    if slot
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, finished))
    {
        *slot = None;
    }
}

/// 槽位里现在是不是这一轮 runner。
///
/// 同一个 `task_code` 可能被反复下发（现场就是反复点「开始执行」），所以判断
/// 「这条任务还在跑」一律按 `Arc` 身份比，**不比 `task_code`**——否则上一轮的任务号
/// 会被绑到新一轮上（5009 每帧都会 `task.task_id != bound_id`，航点 AI / 灯光全哑）。
pub(crate) async fn is_current_runner(
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    runner: &Arc<RouteRunner>,
) -> bool {
    local_runner
        .lock()
        .await
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, runner))
}

/// 「下发地图」的忙碌门禁：只有**真的在跑**的任务 / 建图才拒绝。
///
/// 不要退回去看 `local_runner.lock().await.is_some()`：槽位里可能残留着上一轮的
/// runner（任务取消 / 执行失败后没摘干净），那时平台侧任务早已是「异常/已完成」，
/// 但设备会永远回「设备正在执行任务，请先暂停/结束任务后再下发地图」。
/// 2026-11-19 现场 `0202222672400230` 连续 6 次下发地图失败就是踩在这里（13 号任务
/// 已于 16:13:48 取消，设备状态「在线待命」，下发地图仍被拒）。
pub(crate) async fn map_push_block(
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    local_mapping: &Shared<Option<Arc<MappingSession>>>,
) -> Option<String> {
    if local_runner
        .lock()
        .await
        .as_ref()
        .is_some_and(|runner| runner.is_running())
    {
        return Some("设备正在执行任务，请先暂停/结束任务后再下发地图".to_string());
    }
    if local_mapping.lock().await.is_some() {
        return Some("设备正在建图，请先结束建图后再下发地图".to_string());
    }
    None
}

/// 中台设备页「同步地图」：4012 拉取地图列表，只下载业务库缺失的地图并回传。
///
/// 不要求设备绑定园区：地图跟着设备走，园区在控制台创建时再挑图。
/// 中台编辑地图后的「下发地图」：拉业务平台最新包 → 写本体文件服务 → 重新载图。
///
/// 本体上没有「上传地图」协议，只能通过文件服务写文件；写之前先确认：
/// 1. 设备当前没有执行任务 / 建图（避免改图与导航互相打架）；
/// 2. 本体上确实存在该地图目录（不能凭名字造目录）；
/// 3. 写完 `map_edit/map.pgm` / `map_edit/map.yaml`（有编辑目录时）或根目录
///    `map.pgm` / `map.yaml`（老固件）后读回校验，内容一致才算成功；包内若带
///    `map_edit/edit_info.json`（中台橡皮擦的擦除圆），会与本体现有记录合并后写入；
/// 4. 最后 4030 重新载图，让导航立刻用上新栅格。
pub(crate) async fn push_map_to_body(
    config: &Config,
    client: &Arc<ZhProClient>,
    state: &Shared<RobotState>,
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    local_mapping: &Shared<Option<Arc<MappingSession>>>,
    msg: &WsMessage,
) -> Result<String, String> {
    if let Some(block) = map_push_block(local_runner, local_mapping).await {
        return Err(block);
    }

    let outer = msg.payload.get("params");
    let params = outer.and_then(|p| p.get("params")).or(outer);
    let map_id = params
        .and_then(|p| p.get("mapId"))
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_i64().map(|id| id.to_string()))
        })
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "下发地图缺少 mapId".to_string())?;
    let map_dir_hint = params
        .and_then(|p| p.get("mapDir"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let source_ref_hint = params
        .and_then(|p| p.get("sourceRef"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let dir_hint = if map_dir_hint.trim().is_empty() {
        source_ref_hint
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    } else {
        map_dir_hint
    };

    let http = mapupload::build_http_client();
    let ingest = IngestConfig {
        api_base: config.resolve_api_base(),
        username: config.api_username.clone(),
        password: config.api_password().to_string(),
        device_id: config.device_id.clone(),
        park_id: None,
        map_name: None,
        bind: false,
        // 写本体不经过 ingest，字段只为满足结构体
        refresh: false,
    };
    let package = mapupload::download_map_package(&http, &ingest, &map_id).await?;
    info!(
        "已下载业务平台最新地图包: map={map_id} {} 字节",
        package.len()
    );

    // 本体目录名以 4013 列表为准（平台给的目录名只作候选）
    let manager = MapManager::new(client.clone());
    let robot_map = manager
        .resolve(&[&dir_hint, &map_id])
        .await
        .map_err(|error| {
            format!("本体上没有对应地图目录（候选 {dir_hint} / {map_id}）：{error}")
        })?;

    let file_base = mapupload::resolve_file_base(
        config.robot.as_deref().unwrap_or_default(),
        config.robot_file_base.as_deref(),
        config.robot_file_port,
    );
    let summary =
        mapupload::push_map_to_robot(&http, &file_base, &robot_map.map_id, &package).await?;

    // 重新载图，让本体导航立刻使用新栅格
    let status = zhpro::mapping::relocalize(client, &robot_map.path, None, LOCATE_TIMEOUT)
        .await
        .map_err(|error| format!("{summary}；但重新载图失败：{error}"))?;
    state.lock().await.active_map = Some(map_id.clone());
    if let Err(error) = zhpro::teleop::enter_navigation_mode(client).await {
        warn!("下发地图后切导航模式失败: {error}");
    }
    Ok(format!(
        "{summary}；已重新载图加载最新地图（{map_id}，status={status}）"
    ))
}

/// 一轮地图同步的结果：给平台回执用的文案 + 结构化统计。
///
/// 中台「同步地图」现在按长任务处理（审计停在 `pending`，设备回报才出结论），
/// 所以调用方拿得到 `report` 才能把「共几张 / 传了几张 / 哪几张失败」写回调执。
pub(crate) struct MapSyncOutcome {
    pub summary: String,
    pub report: mapupload::SyncReport,
}

/// 同步本体地图到平台。
///
/// - `refresh`：`true` = 中台/控制台显式「同步地图」，要求把本体最新图全部刷上去（内容未变的仍会跳过）；
///   `false` = 设备定时轮询，只补平台没有的图，**绝不覆盖已有记录**。
/// - `announce`：为 `false` 时不往事件流里发「无事可做」汇总，避免轮询每十分钟刷一条；
///   新增/更新/失败仍然会发，运维能看到实际动作。
/// - `progress`：中台手动触发时上报「已处理 n/m」（设备页看得到进度，不用干等）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn sync_maps_from_robot(
    config: &Config,
    client: &Arc<ZhProClient>,
    out_tx: &mpsc::Sender<WsMessage>,
    park_hint: Option<Option<i64>>,
    refresh: bool,
    announce: bool,
    progress: Option<mapupload::MapSyncProgressFn<'_>>,
) -> Result<MapSyncOutcome, String> {
    // 服务端下发的园区优先；没有就用本地配置 / 注册回执里的绑定
    let park_id = match park_hint {
        Some(park_id) => park_id,
        None => match config.park_id {
            Some(park_id) => Some(park_id),
            None => *config.bound_park_id.lock().await,
        },
    };
    let robot_host = config.robot.clone().unwrap_or_default();
    let file_base = mapupload::resolve_file_base(
        &robot_host,
        config.robot_file_base.as_deref(),
        config.robot_file_port,
    );
    let http = mapupload::build_http_client();
    let maps = zhpro::mapping::MapManager::new(client.clone())
        .list()
        .await
        .map_err(|e| format!("4012 获取地图列表失败: {e}"))?;
    info!(
        "{}同步地图：本体共有 {} 张地图",
        if refresh {
            "中台触发全量刷新"
        } else {
            "定时增量"
        },
        maps.len()
    );

    let ingest = IngestConfig {
        api_base: config.resolve_api_base(),
        username: config.api_username.clone(),
        password: config.api_password().to_string(),
        device_id: config.device_id.clone(),
        park_id,
        map_name: None,
        bind: false,
        refresh,
    };
    let report = mapupload::sync_robot_maps(&http, &ingest, &file_base, &maps, progress).await;
    let summary = report.summary();
    if announce || report.has_changes() {
        let event = DeviceEventPayload {
            event_type: if report.failed > 0 { "error" } else { "info" }.to_string(),
            message: summary.clone(),
            data: Some(json!({
                "total": report.total,
                "uploaded": report.uploaded,
                "skipped": report.skipped,
                "unchanged": report.unchanged,
                "failed": report.failed,
                "mode": if refresh { "refresh" } else { "incremental" },
            })),
        };
        send(
            out_tx,
            "event",
            serde_json::to_value(event).unwrap_or_default(),
        )
        .await;
    } else {
        debug!("定时同步无变化: {summary}");
    }
    Ok(MapSyncOutcome { summary, report })
}

#[cfg(test)]
mod map_push_guard_tests {
    use super::{clear_local_runner, is_current_runner, map_push_block};
    use crate::mock_body;
    use crate::zhpro::client::ZhProClient;
    use crate::zhpro::mapping::MappingSession;
    use crate::zhpro::route::RouteRunner;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    type RunnerSlot = Arc<Mutex<Option<Arc<RouteRunner>>>>;
    type MappingSlot = Arc<Mutex<Option<Arc<MappingSession>>>>;

    async fn client() -> Arc<ZhProClient> {
        // 假本体（`--robot mock`）是仓库里唯一不依赖真机的 ZhProClient 来源
        let host = mock_body::start(0, Duration::from_millis(100), vec![], true)
            .await
            .expect("假本体启动失败");
        Arc::new(ZhProClient::connect(&host).await.expect("连接假本体失败"))
    }

    /// 回归 2026-11-19 现场：任务取消后 `local_runner` 槽位残留，
    /// 「下发地图」永久报「设备正在执行任务」（设备其实已经待命）
    #[tokio::test]
    async fn stale_runner_does_not_block_map_push() {
        let client = client().await;
        let slot: RunnerSlot = Arc::new(Mutex::new(Some(Arc::new(RouteRunner::new(client)))));
        let mapping: MappingSlot = Arc::new(Mutex::new(None));
        assert_eq!(map_push_block(&slot, &mapping).await, None);
    }

    /// 真在跑的任务仍然要挡住下发地图（否则改图与导航互相打架）
    #[tokio::test]
    async fn running_task_still_blocks_map_push() {
        let client = client().await;
        let runner = Arc::new(RouteRunner::new(client));
        runner.mark_running(true);
        let slot: RunnerSlot = Arc::new(Mutex::new(Some(runner)));
        let mapping: MappingSlot = Arc::new(Mutex::new(None));
        let blocked = map_push_block(&slot, &mapping).await;
        assert!(
            blocked
                .as_deref()
                .unwrap_or_default()
                .contains("正在执行任务"),
            "运行中的任务必须拒绝下发地图: {blocked:?}"
        );
    }

    /// `run()` 无论从哪条分支返回都要复位「执行中」标记
    #[tokio::test]
    async fn run_clears_running_flag_on_early_return() {
        let client = client().await;
        let runner = RouteRunner::new(client);
        assert!(runner.run().await.is_err(), "没有执行计划时必须报错");
        assert!(!runner.is_running(), "run() 退出后必须复位执行中标记");
    }

    /// 收尾只摘自己那一份：新任务已经写进槽位时不能被上一轮的收尾清空
    #[tokio::test]
    async fn clear_local_runner_only_removes_itself() {
        let client = client().await;
        let previous = Arc::new(RouteRunner::new(client.clone()));
        let current = Arc::new(RouteRunner::new(client));
        let slot: RunnerSlot = Arc::new(Mutex::new(Some(current.clone())));

        clear_local_runner(&slot, &previous).await;
        assert!(
            slot.lock().await.is_some(),
            "上一轮的收尾不能清掉新任务的 runner"
        );

        clear_local_runner(&slot, &current).await;
        assert!(slot.lock().await.is_none(), "自己收尾时必须摘掉槽位");
    }

    /// 绑定本体任务号前先问「槽位里还是不是这一轮 runner」：
    /// 同一个 taskCode 被反复下发时，上一轮的任务号绝不能写给新一轮。
    #[tokio::test]
    async fn is_current_runner_matches_by_identity_not_task_code() {
        let client = client().await;
        let previous = Arc::new(RouteRunner::new(client.clone()));
        let current = Arc::new(RouteRunner::new(client));
        let slot: RunnerSlot = Arc::new(Mutex::new(Some(current.clone())));

        assert!(is_current_runner(&slot, &current).await);
        assert!(
            !is_current_runner(&slot, &previous).await,
            "上一轮的 runner 不许被当成当前任务"
        );

        clear_local_runner(&slot, &current).await;
        assert!(
            !is_current_runner(&slot, &current).await,
            "槽位清空后谁都不是"
        );
    }
}
