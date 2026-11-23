//! 定位门禁：载图定位、初始位姿解析与地图匹配。

use crate::state::RobotState;
use crate::state::Shared;
use crate::zhpro::client::ZhProClient;
use serde_json::Value;

use super::*;
use zhpro::protocol::*;

/// 定位收敛等待上限：设备被搬运后重定位通常 10~30s，超过就该判失败而不是干等
pub(crate) const LOCATE_TIMEOUT: Duration = Duration::from_secs(60);

/// 自动重定位最多试几次（含首次）
///
/// 现场 SLAM 有一定概率第一轮不收敛（贴着墙 / 特征太少），自动再试一次能消掉大部分
/// 「必须人工点重新定位」的情况；再多就只是把异常拖长，不如早点把原因报给运维。
pub(crate) const LOCATE_ATTEMPTS: u32 = 2;

/// 两次自动重定位之间的间隔：给本体把上一轮「重定位中」的现场清干净
pub(crate) const LOCATE_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// 平台下发的初始位姿：`initialPose: { x, y, heading }`（heading 单位度，与路线航点一致）
///
/// 缺字段或未下发时返回 `None`，交给 SLAM 读取上一份位姿文件做完整重定位。
pub(crate) fn parse_initial_pose(value: Option<&Value>) -> Option<Pose> {
    let pose = value?;
    let x = pose.get("x").and_then(Value::as_f64)?;
    let y = pose.get("y").and_then(Value::as_f64)?;
    let heading = pose.get("heading").and_then(Value::as_f64).unwrap_or(0.0);
    Some(Pose::new(x, y, heading.to_radians()))
}

/// 巡检起步前的定位守卫（**自动**：执行计划时不依赖人工点「重新定位」）。
///
/// 判据是「**这条路线绑的那张图** + 定位收敛到 `100`」，缺一项就当场 `4030 op=1` 载图
/// 重定位：
/// - 定位没收敛（`121/122/203…`）→ 重定位；
/// - 已收敛但载的不是这张图（`active_map` 对不上）→ 也重定位，否则本体会在错的地图
///   坐标系里跑完整条航线才报错；
/// - 已经是「这张图 + 100」→ 直接放行，**不重载**（重载会把位姿重置回上一份位姿文件）。
///
/// 重定位默认下发**零四元数**（[`Pose::unspecified`]），由 SLAM 读上一份位姿文件做完整
/// 重定位——现场绝大多数「设备没动过，只是没载图 / 定位丢了」靠这条自愈。平台显式下发了
/// `initialPose` 时以平台为准（设备被搬动过的场景）。
///
/// 不收敛会再自动试一次（SLAM 有一定概率第一轮不收敛）；`203/210` 是 SLAM 明确失败，
/// 不白等第二轮。
///
/// 返回 `(定位状态, 本次是否真的载图定位过)`。
pub(crate) async fn ensure_localized(
    client: &ZhProClient,
    state: &Shared<RobotState>,
    map_id: &str,
    map_path: &str,
    initial_pose: Option<Pose>,
) -> Result<(i64, bool), String> {
    // 4030 在 4000~5999 非白名单区间，必须先拿到 RoamerX 控制权，否则网关静默丢弃
    if let Err(e) = client.ensure_control().await {
        warn!("取 RoamerX 控制权失败（4030 可能被网关丢弃）: {e}");
    }
    // 先取快照再 await，避免把 state 的锁带过 await 点
    let active_map = state.lock().await.active_map.clone();
    let same_map = map_matches(active_map.as_deref(), map_id, map_path);
    match zhpro::mapping::latest_loc_status(client, Duration::from_secs(2)).await {
        Some(status) if zhpro::mapping::loc_status_ready(status) && same_map => {
            return Ok((status, false));
        }
        Some(status) if zhpro::mapping::loc_status_ready(status) => info!(
            "定位正常（status={status}）但本体载的不是本任务地图（当前 {}，需要 {}），\
             自动载图重定位 map={map_path}",
            active_map.as_deref().unwrap_or("<未知>"),
            if map_id.trim().is_empty() {
                map_path
            } else {
                map_id.trim()
            }
        ),
        Some(status) => info!(
            "定位未就绪（{}，status={status}），自动载图重定位 map={map_path}",
            zhpro::mapping::describe_loc_status(status)
        ),
        None => warn!("2s 内未收到 5009 定位状态，直接尝试自动载图重定位 map={map_path}"),
    }
    let mut last_error = String::new();
    for attempt in 1..=LOCATE_ATTEMPTS {
        if attempt > 1 {
            info!("自动重定位第 {attempt}/{LOCATE_ATTEMPTS} 次（上次失败：{last_error}）");
            tokio::time::sleep(LOCATE_RETRY_BACKOFF).await;
        }
        match zhpro::mapping::relocalize(client, map_path, initial_pose, LOCATE_TIMEOUT).await {
            Ok(status) => {
                // 只有真的载进去了才更新 active_map（它同时被心跳的 map_id、
                // 「召回充电」5014 的校验参数复用）
                let loaded = if map_id.trim().is_empty() {
                    map_path.to_string()
                } else {
                    map_id.trim().to_string()
                };
                state.lock().await.active_map = Some(loaded);
                return Ok((status, true));
            }
            // 203/210：SLAM 自己判了失败，再等一轮 60s 只是拖长异常
            Err(e) if zhpro::mapping::is_fatal_locate_error(&e) => return Err(e),
            Err(e) => {
                warn!("自动重定位第 {attempt}/{LOCATE_ATTEMPTS} 次失败: {e}");
                last_error = e;
            }
        }
    }
    Err(last_error)
}

/// `active_map` 是否就是本任务要用的地图。
///
/// `active_map` 绝大多数时候存的是**业务地图 ID**（`MAP-ZJ-01@v2.1`），但也可能被写成
/// 本体目录名，所以三种写法都认：业务 ID、`to_map_dir_name(业务 ID)`、`map_path` 最后一段。
pub(crate) fn map_matches(active_map: Option<&str>, map_id: &str, map_path: &str) -> bool {
    let Some(active) = active_map.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let map_id = map_id.trim();
    let dir = zhpro::mapping::candidate_dir_name(map_path).unwrap_or_default();
    active == map_id
        || (!map_id.is_empty() && active == zhpro::mapping::to_map_dir_name(map_id))
        || (!dir.is_empty() && active == dir)
}

#[cfg(test)]
mod locate_guard_tests {
    use super::map_matches;

    const MAP_PATH: &str = "/ota/alg_data/map/history_map/MAP-ZJ-01_v2.1";

    /// 业务地图 ID / 本体目录名两种写法都算「同一张图」
    #[test]
    fn recognises_business_id_and_dir_name() {
        assert!(map_matches(
            Some("MAP-ZJ-01@v2.1"),
            "MAP-ZJ-01@v2.1",
            MAP_PATH
        ));
        assert!(map_matches(
            Some("MAP-ZJ-01_v2.1"),
            "MAP-ZJ-01@v2.1",
            MAP_PATH
        ));
        assert!(map_matches(
            Some("  MAP-ZJ-01@v2.1 "),
            "MAP-ZJ-01@v2.1",
            MAP_PATH
        ));
    }

    /// 对不上 / 没有记录 → 必须当作「载的不是这张图」，让定位守卫重新载图
    #[test]
    fn rejects_other_or_unknown_map() {
        assert!(!map_matches(
            Some("MAP-ZJ-02@v2.1"),
            "MAP-ZJ-01@v2.1",
            MAP_PATH
        ));
        assert!(!map_matches(None, "MAP-ZJ-01@v2.1", MAP_PATH));
        assert!(!map_matches(Some("  "), "MAP-ZJ-01@v2.1", MAP_PATH));
    }
}
