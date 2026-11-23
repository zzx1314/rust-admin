//! 航点 AI 识别的单元测试（纯函数为主）。

// 同一模块拆出来的几块共用一个词汇表（`Window` / `WindowConfig` / `Hit` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expect_count_parsing_is_lenient_but_never_zero() {
        // 编辑器写数字；历史数据 / 手改可能是字符串
        assert_eq!(parse_expect_count(Some(&json!(3))), Some(3));
        assert_eq!(parse_expect_count(Some(&json!("4"))), Some(4));
        assert_eq!(parse_expect_count(Some(&json!(" 5 "))), Some(5));
        // 留空 / 0 / 负数 / 非法 = 不核对数量
        assert_eq!(parse_expect_count(None), None);
        assert_eq!(parse_expect_count(Some(&Value::Null)), None);
        assert_eq!(parse_expect_count(Some(&json!(0))), None);
        assert_eq!(parse_expect_count(Some(&json!(-2))), None);
        assert_eq!(parse_expect_count(Some(&json!(""))), None);
        assert_eq!(parse_expect_count(Some(&json!("abc"))), None);
        // 上限防脏数据
        assert_eq!(parse_expect_count(Some(&json!(100000))), Some(999));

        // 动作里的字段名是 `expectCount`（与路线编辑器一致）
        let point = PointRow {
            name: "P1".to_string(),
            dwell: 5,
            actions: vec![json!({"type": "AI 识别", "expectCount": "2", "scene": "fire"})],
        };
        let action = point.find_ai_action().expect("应识别出 AI 动作");
        assert_eq!(action.expect_count, Some(2));
    }

    #[test]
    fn check_type_is_explicit_or_inferred() {
        // 显式字段优先：存在性任务即使写了数量也不判数量
        assert_eq!(normalize_check_type(Some("exist"), Some(5)), "exist");
        assert_eq!(normalize_check_type(Some("count"), None), "count");
        // 中文 / 别名（手改 route_data 时常见）
        assert_eq!(normalize_check_type(Some("数量"), None), "count");
        assert_eq!(
            normalize_check_type(Some("检测数量是否正确"), None),
            "count"
        );
        assert_eq!(normalize_check_type(Some("检测是否存在"), None), "exist");
        // 老数据只有 expectCount：给了数量就是数量任务
        assert_eq!(normalize_check_type(None, Some(3)), "count");
        assert_eq!(normalize_check_type(None, None), "exist");

        // 路线里的动作字段名是 `checkType`，与编辑器一致
        let point = PointRow {
            name: "P1".to_string(),
            dwell: 5,
            actions: vec![json!({"type": "AI 识别", "checkType": "exist", "expectCount": 9})],
        };
        let action = point.find_ai_action().expect("应识别出 AI 动作");
        assert_eq!(action.check_type, "exist");
    }

    #[test]
    fn plan_hit_alerts_only_when_count_flagged() {
        // 命中且数量一致：不告警
        assert!(
            plan_hit_alert(&json!({
                "type": "plan_hit", "count_ok": true, "count_alert": false,
                "expect_count": 3, "found_count": 3
            }))
            .is_none()
        );
        // 普通检出帧不告警
        assert!(plan_hit_alert(&json!({"type": "detect", "boxes": []})).is_none());
        // 数量不一致：拿得到期望/实际/证据
        let (expect, found, evidence) = plan_hit_alert(&json!({
            "type": "plan_hit", "count_alert": true, "expect_count": 3,
            "found_count": 1, "evidence": "/x/y.jpg"
        }))
        .expect("数量不一致要告警");
        assert_eq!((expect, found), (3, 1));
        assert_eq!(evidence, "/x/y.jpg");
    }

    /// 数量**一致**的回执：只有「数量任务 + 动作里勾了 notifyWhenCountOk + 真的对上了」
    /// 才返回；其它一律 None（一致且没勾就该安静，不一致走告警那条路）。
    #[test]
    fn count_ok_notice_needs_the_waypoint_switch_and_a_real_match() {
        let cfg = |check_type: &str, notify: bool| WindowConfig {
            device_code: "D1".to_string(),
            device_id: None,
            park_id: None,
            task_code: "TK-1".to_string(),
            task_name: "巡检".to_string(),
            point_index: 0,
            point_name: "P1".to_string(),
            hint: String::new(),
            scene: None,
            labels: vec![],
            threshold_text: String::new(),
            expect_count: Some(3),
            check_type: check_type.to_string(),
            alert_when_missing: false,
            notify_when_count_ok: notify,
            secs: 20,
            dwell: 10,
        };
        let hit = json!({
            "type": "plan_hit", "count_ok": true, "count_alert": false,
            "expect_count": 3, "found_count": 3, "evidence": "/x/y.jpg"
        });
        let (expect, found, evidence) =
            plan_hit_count_ok(&hit, &cfg("count", true)).expect("勾了开关且一致时应该发回执");
        assert_eq!((expect, found), (3, 3));
        assert_eq!(evidence, "/x/y.jpg");
        // 没勾：默认安静（数量任务只报不一致）
        assert!(plan_hit_count_ok(&hit, &cfg("count", false)).is_none());
        // 存在性任务没有「数量一致」这种结论
        assert!(plan_hit_count_ok(&hit, &cfg("exist", true)).is_none());
        // 不一致：那是 `plan_hit_alert` 的事
        assert!(
            plan_hit_count_ok(
                &json!({"type": "plan_hit", "count_ok": false, "count_alert": true,
                         "expect_count": 3, "found_count": 2}),
                &cfg("count", true),
            )
            .is_none()
        );
    }

    #[test]
    fn notify_when_count_ok_is_parsed_from_the_action() {
        let point = PointRow {
            name: "P1".to_string(),
            dwell: 5,
            actions: vec![json!({
                "type": "AI 识别", "checkType": "count", "expectCount": 3,
                "notifyWhenCountOk": true
            })],
        };
        let action = point.find_ai_action().expect("应识别出 AI 动作");
        assert!(
            action.notify_when_count_ok,
            "编辑器写的 notifyWhenCountOk 要生效"
        );
        // 老数据没这个字段：默认关
        let legacy = PointRow {
            name: "P1".to_string(),
            dwell: 5,
            actions: vec![json!({"type": "AI 识别", "checkType": "count", "expectCount": 3})],
        };
        assert!(!legacy.find_ai_action().unwrap().notify_when_count_ok);
    }

    #[test]
    fn point_ai_source_follows_config_and_deployment() {
        // 显式配置优先于部署形态
        assert_eq!(resolve_source("client", true), PointAiSource::Client);
        assert_eq!(resolve_source("server", false), PointAiSource::Server);
        // auto：服务端能直连板端（配了 ROBODOG_BACKPACK_URL）就自己调
        assert_eq!(resolve_source("auto", true), PointAiSource::Server);
        // auto + 正式环境（云上，连不到板端）→ 客户端代调
        assert_eq!(resolve_source("auto", false), PointAiSource::Client);
        // 未知值走 auto，不能悄悄变成 server（否则航点 AI 又会「配了却没告警」）
        assert_eq!(resolve_source("", false), PointAiSource::Client);
        assert_eq!(resolve_source("garbage", true), PointAiSource::Server);
    }

    #[test]
    fn ai_action_is_recognized_and_others_are_not() {
        assert!(is_ai_action("AI 识别"));
        assert!(is_ai_action("ai detect"));
        assert!(is_ai_action("目标识别"));
        assert!(!is_ai_action("可见光拍照"));
        assert!(!is_ai_action("红外测温"));
        assert!(!is_ai_action("静默通行"));
    }

    #[test]
    fn arrival_stages_are_the_patrol_point_events() {
        assert!(is_arrival_stage("patrol.point_arrived"));
        assert!(is_arrival_stage("patrol.waiting"));
        assert!(!is_arrival_stage("patrol.navigating"));
        assert!(!is_arrival_stage("patrol.completed"));
    }

    #[test]
    fn terminal_status_closes_window() {
        assert!(is_terminal_status("已完成"));
        assert!(is_terminal_status("异常"));
        assert!(is_terminal_status("已暂停"));
        assert!(!is_terminal_status("执行中"));
    }

    #[test]
    fn window_follows_dwell_up_to_configured_max() {
        let max = DEFAULT_WINDOW_MAX_SECS;
        // dwell 0 → max(4)+8 = 12
        assert_eq!(window_secs(0, max), 12);
        assert_eq!(window_secs(6, max), 14);
        // 下限 4 s 的 dwell 已经给出 12 s，所以 2 s 的点也是 12 s（模型加载要 1~3 s）
        assert_eq!(window_secs(2, max), 12);
        // 长停留不再被截断：停 100 s 就识别 108 s（以前是 30 s）
        assert_eq!(window_secs(100, max), 108);
        // 停留顶格 300 s（路线单点上限）时，+8 s 的余量被配置上限吃掉，正好 300 s
        assert_eq!(window_secs(300, max), 300);
        // 上限可收紧（运维怕误报就压小），且不会低于下限
        assert_eq!(window_secs(100, 30), 30);
        assert_eq!(window_secs(100, 1), WINDOW_MIN_SECS);
    }

    #[test]
    fn point_stages_keep_window_others_do_not() {
        for stage in [
            "patrol.point_arrived",
            "patrol.waiting",
            "patrol.photographing",
            "patrol.point_completed",
        ] {
            assert!(is_point_stage(stage), "{stage} 是在航点上");
        }
        for stage in [
            "patrol.navigating",
            "patrol.returning",
            "patrol.returning_to_charge",
            "navigation.navigating",
            "mapping.running",
            "",
        ] {
            assert!(!is_point_stage(stage), "{stage} 已经离开航点");
        }
    }

    #[test]
    fn conf_accepts_decimal_and_percent_only() {
        assert_eq!(parse_conf("置信度 > 0.85"), Some(0.85));
        assert_eq!(parse_conf("0.6"), Some(0.6));
        assert_eq!(parse_conf("> 85%"), Some(0.85));
        // 温度阈值不是置信度（没有 % 且 >1）
        assert_eq!(parse_conf("> 65℃ 生成告警"), None);
        assert_eq!(parse_conf("—"), None);
    }

    #[test]
    fn sse_blocks_are_split_and_partial_stays() {
        let mut buffer = String::from(
            "data: {\"type\":\"detect\",\"boxes\":[{},{}]}\n\ndata: {\"type\":\"telemetry\"}\n\ndata: {\"partial\":",
        );
        let events = take_sse_events(&mut buffer);
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("detect"));
        // 没收到空行的那半截留在缓冲里等下一块
        assert!(buffer.contains("partial"));
    }

    #[test]
    fn detect_hit_needs_type_and_threshold() {
        let event = json!({
            "type": "detect",
            "boxes": [
                {"zh": "烟", "conf": 0.4},
                {"zh": "火焰", "conf": 0.91}
            ]
        });
        assert_eq!(detect_hit(&event, 0.8), Some(("火焰".to_string(), 0.91)));
        assert_eq!(detect_hit(&event, 0.95), None);
        assert_eq!(detect_hit(&json!({"type": "telemetry"}), 0.1), None);
    }

    #[test]
    fn scene_falls_back_to_first_ready_task() {
        let tasks = json!({
            "tasks": [
                {"task": "fire", "name": "烟火检测", "conf": 0.4, "ok": true,
                 "labels": [{"name": "smoke", "zh": "烟雾"}]},
                {"task": "meter", "name": "仪表读数", "conf": 0.6, "ok": true,
                 "labels": [{"name": "gauge", "zh": "仪表"}]}
            ]
        });
        let scene = resolve_scene(Some(&tasks), "仪表读数", None).expect("scene");
        assert_eq!(scene.task, "meter");
        assert_eq!(scene.conf, 0.6);
        // 完全不匹配 → 退回第一个可用场景，而不是什么都不跑
        let scene = resolve_scene(Some(&tasks), "不存在的目标", None).expect("fallback");
        assert_eq!(scene.task, "fire");
        assert!(resolve_scene(None, "烟火", None).is_none());
    }

    #[test]
    fn editor_scene_wins_over_keyword_matching() {
        let tasks = json!({
            "tasks": [
                {"task": "fire", "name": "烟火检测", "conf": 0.4, "ok": true,
                 "labels": [{"name": "smoke", "zh": "烟雾"}, {"name": "flame", "zh": "火焰"}]},
                {"task": "meter", "name": "仪表读数", "conf": 0.6, "ok": true,
                 "labels": [{"name": "gauge", "zh": "仪表"}]}
            ]
        });
        // 编辑器选的模型包优先，即使关键词能命中别的场景
        let scene = resolve_scene(Some(&tasks), "仪表", Some("fire")).expect("scene");
        assert_eq!(scene.task, "fire");
        // 选错了（模型包已下线）→ 退回关键词匹配，不让窗口直接不跑
        let scene = resolve_scene(Some(&tasks), "仪表", Some("gone")).expect("fallback");
        assert_eq!(scene.task, "meter");
    }

    #[test]
    fn labels_are_filtered_to_the_resolved_scene() {
        let tasks = json!({
            "tasks": [{
                "task": "fire", "name": "烟火检测", "ok": true,
                "labels": [
                    {"name": "smoke", "zh": "烟雾"},
                    {"name": "flame", "zh": "火焰"},
                    {"name": "person", "zh": "人员", "ignored": true}
                ]
            }]
        });
        // 不存在的 / 被板端标记 ignored 的类别都丢掉
        let labels = scene_labels(
            Some(&tasks),
            "fire",
            &[
                "flame".to_string(),
                "person".to_string(),
                "ghost".to_string(),
            ],
        );
        assert_eq!(labels, vec!["flame".to_string()]);
        // 一个都没勾 / 全部无效 → 空，调用方传 `labels: null`（该场景全部类别）
        assert!(scene_labels(Some(&tasks), "fire", &[]).is_empty());
        assert!(scene_labels(Some(&tasks), "fire", &["ghost".to_string()]).is_empty());
        assert!(scene_labels(Some(&tasks), "meter", &["flame".to_string()]).is_empty());
    }

    #[test]
    fn point_reads_ai_action_from_route_data() {
        let point = PointRow {
            name: "配电柜".to_string(),
            dwell: 6,
            actions: vec![
                json!({"type": "可见光拍照", "param": "1080P"}),
                json!({"type": "AI 识别", "param": "烟火 / 人员", "threshold": "> 0.8"}),
            ],
        };
        let action = point.find_ai_action().expect("ai action");
        assert_eq!(action.hint, "烟火 / 人员");
        assert_eq!(parse_conf(&action.threshold), Some(0.8));
        assert!(action.scene.is_none());
        assert!(action.labels.is_empty());

        // 路线编辑器新写法：模型目录里选的场景 + 类别
        let structured = PointRow {
            name: "配电柜".to_string(),
            dwell: 6,
            actions: vec![json!({
                "type": "AI 识别",
                "param": "烟火检测 · 火焰 / 烟雾",
                "threshold": "置信度 ≥ 0.40",
                "scene": " fire ",
                "labels": ["flame", "smoke"]
            })],
        };
        let action = structured.find_ai_action().expect("ai action");
        assert_eq!(action.scene.as_deref(), Some("fire"));
        assert_eq!(
            action.labels,
            vec!["flame".to_string(), "smoke".to_string()]
        );
        assert_eq!(parse_conf(&action.threshold), Some(0.4));

        let plain = PointRow {
            name: "过道".to_string(),
            dwell: 0,
            actions: vec![json!({"type": "静默通行"})],
        };
        assert!(plain.find_ai_action().is_none());
    }
}
