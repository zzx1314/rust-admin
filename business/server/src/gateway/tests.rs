//! 网关的单元测试（连接池身份判定 / 故障码解析 / 看门狗纯函数）。

// 测的是模块内部的纯函数（连接池身份判定 / 故障码解析 / 状态推导），
// 走对外的 `gateway::*` 反而看不到它们。
use super::{
    DeviceEventPayload, derive_device_status, event_alert_level, fault_code, is_current_connection,
    release_connection,
};
use crate::gateway_types::PendingCommand;
use std::collections::HashMap;
use tokio::sync::broadcast;

type Pool = HashMap<String, broadcast::Sender<PendingCommand>>;

/// 自动工单的去重键靠故障码：同一台设备的同一个码 = 同一个问题。
/// 文案里的坐标 / 耗时 / 英文描述一变就拆单，是 2026-11-17 之前的老毛病。
#[test]
fn fault_code_is_extracted_from_message() {
    assert_eq!(
        fault_code("感知障碍物检测延迟（code=13382, 级别=1）"),
        Some("13382")
    );
    assert_eq!(
        fault_code("导航指令非法（code=13440, 级别=2）"),
        Some("13440")
    );
    // 英文 / 带前缀的写法也能取到
    assert_eq!(
        fault_code("RS 与 lidar 时间不同步（code=12608, 级别=1）"),
        Some("12608")
    );
    // 没有码（翻译文案 / 人工事件）→ 返回 None，工单退回按标题去重
    assert_eq!(
        fault_code("The localization system has poor accuracy"),
        None
    );
    assert_eq!(fault_code("code="), None);
    assert_eq!(fault_code(""), None);
}

fn channel() -> broadcast::Sender<PendingCommand> {
    // 保留 receiver 否则通道无接收者（本用例只关心 sender 身份）
    let (tx, _rx) = broadcast::channel(8);
    tx
}

#[test]
fn stale_connection_cannot_unregister_the_new_one() {
    // 现场故障复现：客户端重连，旧 TCP 还没死透。
    // 旧连接退出时如果无条件 remove，就会把新连接摘走，台账仍是「在线待命」，
    // 中台「同步地图」却报「设备未连接」。
    let device = "0202222672400230";
    let stale = channel();
    let fresh = channel();
    let mut pool: Pool = HashMap::new();

    // 旧连接注册 → 新连接注册（顶替）
    pool.insert(device.to_string(), stale.clone());
    pool.insert(device.to_string(), fresh.clone());
    assert!(is_current_connection(&pool, device, &fresh));

    // 旧连接退出：不得摘掉新登记
    assert!(!release_connection(&mut pool, device, &stale));
    assert!(
        is_current_connection(&pool, device, &fresh),
        "新连接必须还在连接池里"
    );

    // 新连接自己退出才真的摘掉
    assert!(release_connection(&mut pool, device, &fresh));
    assert!(!pool.contains_key(device));
}

#[test]
fn heartbeat_can_re_register_connection() {
    // 心跳自愈：池里没登记本连接时就补登记（进程重启 / 历史残留状态）
    let device = "RD-001";
    let tx = channel();
    let mut pool: Pool = HashMap::new();
    assert!(!is_current_connection(&pool, device, &tx));
    pool.insert(device.to_string(), tx.clone());
    assert!(is_current_connection(&pool, device, &tx));
}

#[test]
fn unbound_device_stays_pending() {
    // 未绑园区：不管充不充电，都必须保持「待绑定」，否则会被任务调度选中
    assert_eq!(
        derive_device_status("待绑定", false, Some(true), Some(80), None),
        None
    );
    assert_eq!(
        derive_device_status("充电中", false, Some(true), Some(80), None),
        Some("待绑定")
    );
}

#[test]
fn charging_comes_from_body_report() {
    assert_eq!(
        derive_device_status("在线待命", true, Some(true), Some(88), None),
        Some("充电中")
    );
    assert_eq!(
        derive_device_status("充电中", true, Some(false), Some(88), None),
        Some("在线待命")
    );
    // 电量低但本体说没在充电：不再瞎猜成「充电中」
    assert_eq!(
        derive_device_status("在线待命", true, Some(false), Some(5), None),
        None
    );
    // 电量低且老客户端没上报充电状态：兼容兜底
    assert_eq!(
        derive_device_status("在线待命", true, None, Some(5), None),
        Some("充电中")
    );
}

#[test]
fn running_task_and_manual_statuses_are_kept() {
    assert_eq!(
        derive_device_status("执行任务", true, Some(false), Some(60), None),
        None
    );
    assert_eq!(
        derive_device_status("执行任务", true, Some(true), Some(60), None),
        None
    );
    assert_eq!(
        derive_device_status("故障", true, Some(false), Some(60), None),
        None
    );
    assert_eq!(
        derive_device_status("离线", true, Some(false), Some(60), None),
        Some("在线待命")
    );
}

#[test]
fn fault_severity_maps_to_alert_level() {
    let event = |event_type: &str, data: Option<serde_json::Value>| DeviceEventPayload {
        event_type: event_type.to_string(),
        message: "内部节点异常".to_string(),
        data,
    };
    // 13504 level=3 必须落「严重」，不能被普通 fault 淹没
    assert_eq!(
        event_alert_level(&event("fault", Some(serde_json::json!({ "severity": 3 })))),
        "严重"
    );
    assert_eq!(
        event_alert_level(&event("fault", Some(serde_json::json!({ "severity": 1 })))),
        "警告"
    );
    assert_eq!(event_alert_level(&event("fault", None)), "警告");
    assert_eq!(event_alert_level(&event("error", None)), "严重");
}

#[test]
fn body_offline_is_fault_and_recovers() {
    // WS 还连着但本体 1315 超时：不能继续显示“在线待命”
    assert_eq!(
        derive_device_status("在线待命", true, Some(false), Some(80), Some(false)),
        Some("故障")
    );
    assert_eq!(
        derive_device_status("充电中", true, Some(true), Some(80), Some(false)),
        Some("故障")
    );
    // 本体恢复后自动回到待命
    assert_eq!(
        derive_device_status("故障", true, Some(false), Some(80), Some(true)),
        Some("在线待命")
    );
}

#[tokio::test]
async fn command_result_wakes_waiter_by_request_id() {
    let waiters = super::new_command_result_waiters();
    let rx = super::expect_command_result(&waiters, "req-map-push").await;
    super::complete_command_result(
        &waiters,
        "req-map-push",
        super::CommandResult {
            ok: true,
            message: "本体地图已更新".to_string(),
            data: None,
        },
    )
    .await;
    let result = super::wait_command_result(
        &waiters,
        "req-map-push",
        rx,
        std::time::Duration::from_secs(1),
    )
    .await
    .expect("result must arrive");
    assert!(result.ok);
    assert_eq!(result.message, "本体地图已更新");
}

#[tokio::test]
async fn command_result_times_out_and_is_cleaned_up() {
    let waiters = super::new_command_result_waiters();
    let rx = super::expect_command_result(&waiters, "req-timeout").await;
    let error = super::wait_command_result(
        &waiters,
        "req-timeout",
        rx,
        std::time::Duration::from_millis(20),
    )
    .await
    .expect_err("must time out");
    assert!(error.contains("未回报执行结果"));
    // 超时后等待表要清空，迟到回执不能命中下一条请求
    assert!(waiters.lock().await.is_empty());
}

/// 断点续跑：中断后从第几个航点继续。
///
/// 这条规则错了现场有两种后果：跳过没巡检的点（漏检）或把走过的点再跑一遍
/// （每条航点都有 AI 识别 / 拍照 / 停留，白跑一轮就是几分钟）。
mod resume_point {
    use super::super::tasks::resume_point_index;

    #[test]
    fn arrived_point_is_retried_when_the_failure_happened_on_it() {
        // 在航点上中断（停留 / 拍照 / 识别没做完）：重跑这个点
        assert_eq!(
            resume_point_index(Some(2), Some("patrol.waiting"), Some(5)),
            Some(2)
        );
        assert_eq!(
            resume_point_index(Some(2), Some("patrol.point_completed"), Some(5)),
            Some(2)
        );
    }

    #[test]
    fn next_point_is_used_when_the_dog_was_already_moving() {
        // 已经离开 P3、正在前往 P4 时中断：P3 的作业做完了，从 P4 开始
        assert_eq!(
            resume_point_index(Some(2), Some("patrol.navigating"), Some(5)),
            Some(3)
        );
        assert_eq!(
            resume_point_index(Some(2), Some("patrol.returning"), Some(5)),
            Some(3)
        );
    }

    #[test]
    fn unknown_stage_falls_back_to_the_arrived_point() {
        // 老客户端 / 客户端一次性报的「异常」不带 stage：保守地重跑中断点，绝不跳过
        assert_eq!(resume_point_index(Some(1), None, Some(4)), Some(1));
    }

    #[test]
    fn nothing_to_resume_when_every_point_was_visited() {
        // 最后一个点之后（返程失败）：没有可续跑的航点
        assert_eq!(
            resume_point_index(Some(4), Some("patrol.returning"), Some(5)),
            None
        );
        // 在最后一个点上中断仍然可以只重跑这个点
        assert_eq!(
            resume_point_index(Some(4), Some("patrol.waiting"), Some(5)),
            Some(4)
        );
    }

    #[test]
    fn no_progress_report_means_start_over() {
        assert_eq!(
            resume_point_index(None, Some("patrol.waiting"), Some(5)),
            None
        );
        // 负数（老客户端 / 解析异常）同样不能当成「第 -1 个点」
        assert_eq!(resume_point_index(Some(-3), None, Some(5)), None);
        // 航点数未知时不做越界判断，交给下发前校验
        assert_eq!(
            resume_point_index(Some(9), Some("patrol.waiting"), None),
            Some(9)
        );
    }
}
