use super::*;

fn route() -> Value {
    json!({
        "points": [
            {
                "name": "新航点 1",
                "dwell": 100,
                "actions": [{
                    "type": "AI 识别",
                    "scene": "firextinguisher",
                    "labels": ["extinguisher"],
                    "threshold": "置信度 ≥ 0.35",
                    "param": "灭火器 · 灭火器"
                }]
            },
            {"name": "新航点 2", "actions": [{"type": "可见光拍照", "param": "1080P"}]},
            {"name": "新航点 3", "dwell": 6, "actions": []}
        ]
    })
}

#[test]
fn ai_points_read_from_route_data() {
    let points = parse_ai_points(&route());
    assert_eq!(points.len(), 1);
    let point = &points[0];
    assert_eq!(point.index, 0);
    assert_eq!(point.name, "新航点 1");
    assert_eq!(point.dwell, 100);
    assert_eq!(point.scene.as_deref(), Some("firextinguisher"));
    assert_eq!(point.labels, vec!["extinguisher".to_string()]);
    assert_eq!(point.threshold, "置信度 ≥ 0.35");
    assert_eq!(point.hint, "灭火器 · 灭火器");
    assert_eq!(
        point.expect_count, None,
        "路线里没配就是 None（不核对数量）"
    );
    assert!(ai_point_at(&points, 1).is_none(), "拍照点不该触发识别");
    assert!(ai_point_at(&points, 0).is_some());
}

#[test]
fn expect_count_is_parsed_leniently_but_never_zero() {
    // 编辑器写数字，历史数据/手改可能是字符串
    assert_eq!(parse_expect_count(Some(&json!(3))), Some(3));
    assert_eq!(parse_expect_count(Some(&json!("4"))), Some(4));
    assert_eq!(parse_expect_count(Some(&json!(" 5 "))), Some(5));
    // 留空 / 0 / 负数 / 非法 = 不核对数量
    assert_eq!(parse_expect_count(None), None);
    assert_eq!(parse_expect_count(Some(&Value::Null)), None);
    assert_eq!(parse_expect_count(Some(&json!(0))), None);
    assert_eq!(parse_expect_count(Some(&json!(""))), None);
    assert_eq!(parse_expect_count(Some(&json!("abc"))), None);
    // 上限防止脏数据
    assert_eq!(parse_expect_count(Some(&json!(100000))), Some(999));

    let points = parse_ai_points(&json!({
        "points": [{"name": "P1", "actions": [{"type": "AI 识别", "expectCount": "2"}]}]
    }));
    assert_eq!(points[0].expect_count, Some(2));
}

#[test]
fn check_type_is_explicit_or_inferred_from_expect_count() {
    // 显式字段优先
    assert_eq!(normalize_check_type(Some("count"), None), "count");
    assert_eq!(normalize_check_type(Some("exist"), Some(5)), "exist");
    // 中文 / 别名
    assert_eq!(normalize_check_type(Some("数量"), None), "count");
    assert_eq!(
        normalize_check_type(Some("检测数量是否正确"), None),
        "count"
    );
    assert_eq!(normalize_check_type(Some("presence"), None), "exist");
    // 老数据没 checkType：给了 expectCount 就是数量任务，否则存在性任务
    assert_eq!(normalize_check_type(None, Some(3)), "count");
    assert_eq!(normalize_check_type(None, None), "exist");
}

#[test]
fn capture_switch_reaches_the_agent_and_defaults_to_legacy_when_absent() {
    // 中台 `media_capture_enabled` 关：平台下发 `captureEnabled=false`，客户端原样转给板端
    let off = AiStart::from_value(&json!({
        "windowId": "AIW-1-0-9",
        "task": "firextinguisher",
        "mediaDir": "/media/media-data/picture",
        "mediaFrames": 3,
        "captureEnabled": false,
    }))
    .expect("构造开窗指令失败");
    assert_eq!(off.media_capture, Some(false));
    let on = AiStart::from_value(&json!({
        "windowId": "AIW-1-0-10",
        "captureEnabled": true,
    }))
    .expect("构造开窗指令失败");
    assert_eq!(on.media_capture, Some(true));
    // 老平台没这个字段：不传，板端按老行为（即开），不能被升级顺手关掉
    let legacy = AiStart::from_value(&json!({"windowId": "AIW-1-0-11"})).expect("构造开窗指令失败");
    assert_eq!(legacy.media_capture, None);
}

#[test]
fn check_type_reaches_agent_and_route_parse() {
    let points = parse_ai_points(&json!({
        "points": [
            {"name": "存在性", "actions": [{"type": "AI 识别", "checkType": "exist"}]},
            {"name": "数量", "actions": [{"type": "AI 识别", "expectCount": 2}]},
            {"name": "老数据", "actions": [{"type": "AI 识别"}]}
        ]
    }));
    assert_eq!(points[0].check_type, "exist");
    assert_eq!(points[1].check_type, "count", "老数据靠 expectCount 推断");
    assert_eq!(points[2].check_type, "exist");
}

#[test]
fn count_alert_only_for_flagged_plan_hits() {
    let start = AiStart::from_value(&json!({
        "windowId": "AIW-1-0-1",
        "taskCode": "TK-1",
        "planId": "TK-1",
        "pointIndex": 0,
        "expectCount": 3,
    }))
    .expect("构造开窗指令失败");
    // 命中但数量一致：不告警
    assert!(
        count_alert(
            &json!({"type": "plan_hit", "count_ok": true, "count_alert": false,
                    "expect_count": 3, "found_count": 3}),
            &start,
        )
        .is_none()
    );
    // 普通 detect 事件不告警
    assert!(count_alert(&json!({"type": "detect", "boxes": []}), &start).is_none());
    let alert = count_alert(
        &json!({"type": "plan_hit", "count_alert": true, "expect_count": 3,
                "found_count": 1, "evidence": "/x/y.jpg"}),
        &start,
    )
    .expect("数量不一致要告警");
    assert_eq!(alert["windowId"], "AIW-1-0-1");
    assert_eq!(alert["planId"], "TK-1");
    assert_eq!(alert["expectCount"], 3);
    assert_eq!(alert["foundCount"], 1);
    assert_eq!(alert["evidence"], "/x/y.jpg");
}

/// 数量**一致**：只有动作勾了 `notifyWhenCountOk` 才转 `ai_count`；
/// 不一致 / 普通检出 / 存在性任务都不走这条路（告警另有一条 `count_alert`）。
#[test]
fn count_matched_only_when_the_waypoint_asks_for_it() {
    let mut start = AiStart::from_value(&json!({
        "windowId": "AIW-1-0-1",
        "taskCode": "TK-1",
        "planId": "TK-1",
        "pointIndex": 0,
        "checkType": "count",
        "expectCount": 3,
        "notifyWhenCountOk": true,
    }))
    .expect("构造开窗指令失败");
    let hit = json!({"type": "plan_hit", "count_ok": true, "count_alert": false,
                      "expect_count": 3, "found_count": 3, "evidence": "/x/y.jpg"});
    let counted = count_matched(&hit, &start).expect("勾了「一致也通知」时应该上报");
    assert_eq!(counted["windowId"], "AIW-1-0-1");
    assert_eq!(counted["planId"], "TK-1");
    assert_eq!(counted["expectCount"], 3);
    assert_eq!(counted["foundCount"], 3);
    assert_eq!(counted["evidence"], "/x/y.jpg");
    // 与告警互斥：不一致的那条不能被当成「一致」
    assert!(
        count_matched(
            &json!({"type": "plan_hit", "count_ok": false, "count_alert": true,
                    "expect_count": 3, "found_count": 2}),
            &start,
        )
        .is_none()
    );
    assert!(count_matched(&json!({"type": "detect", "boxes": []}), &start).is_none());

    // 没勾开关：默认安静（数量任务只报「不一致」）
    start.notify_when_count_ok = false;
    assert!(count_matched(&hit, &start).is_none());
    // 存在性任务没有「数量一致」这种结论
    start.notify_when_count_ok = true;
    start.check_type = "exist".to_string();
    assert!(count_matched(&hit, &start).is_none());
}

/// 证据图随告警上报：客户端与板端 Agent 同机，**只有它**能读到板端磁盘上的图，
/// 企业微信图片消息要的是字节不是路径。读不到 / 不是 JPG-PNG / 超 2MB 一律不传，
/// 告警本身不受影响（卡片里仍有路径）。
#[tokio::test]
async fn evidence_image_reads_board_file_and_skips_unusable_ones() {
    let dir = std::env::temp_dir().join(format!("robodog-evidence-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // ① 正常 JPEG（只看文件头）
    let jpeg = dir.join("dogcam_1.jpg");
    std::fs::write(&jpeg, [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10]).unwrap();
    let alert = json!({ "evidence": jpeg.to_string_lossy() });
    let encoded = evidence_image_base64(&alert)
        .await
        .expect("JPEG 应能读出来");
    assert_eq!(encoded, "/9j/4AAQ", "base64 不能带换行");

    // ② 文件不在（客户端不在板端 / 路径不在本机）→ 不传图
    let missing = json!({ "evidence": dir.join("nope.jpg").to_string_lossy() });
    assert!(evidence_image_base64(&missing).await.is_none());

    // ③ 不是图片（只看文件头，不信扩展名）
    let fake = dir.join("fake.jpg");
    std::fs::write(&fake, b"hello world").unwrap();
    let fake_alert = json!({ "evidence": fake.to_string_lossy() });
    assert!(evidence_image_base64(&fake_alert).await.is_none());

    // ④ 超过企业微信 2MB 上限
    let huge = dir.join("huge.jpg");
    let mut bytes = vec![0xFF_u8, 0xD8, 0xFF];
    bytes.resize(EVIDENCE_IMAGE_MAX_BYTES as usize + 1, 0);
    std::fs::write(&huge, &bytes).unwrap();
    let huge_alert = json!({ "evidence": huge.to_string_lossy() });
    assert!(evidence_image_base64(&huge_alert).await.is_none());

    // ⑤ 没给路径（老客户端 / 事件没带 evidence）
    assert!(evidence_image_base64(&json!({})).await.is_none());
    assert!(
        evidence_image_base64(&json!({ "evidence": "  " }))
            .await
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 板端 `plan_hit.evidence` 能反推出存证根目录（板子路径前缀不统一，不硬编码）
#[test]
fn jobs_root_is_learned_from_plan_hit_evidence() {
    assert_eq!(
        jobs_root_from_evidence(&json!({
            "type": "plan_hit",
            "evidence": "/root/261113/backpack_dashboard/backend/detect_jobs/TK-1/pt0/dogcam_1.jpg"
        })),
        Some("/root/261113/backpack_dashboard/backend/detect_jobs".to_string())
    );
    // 不是 plan_hit / 没给路径 / 布局不是 `<根>/<计划>/pt<n>/<图>`：一律不猜
    assert!(
        jobs_root_from_evidence(&json!({"type": "detect", "evidence": "/a/b/pt0/x.jpg"})).is_none()
    );
    assert!(jobs_root_from_evidence(&json!({"type": "plan_hit"})).is_none());
    assert!(jobs_root_from_evidence(&json!({"type": "plan_hit", "evidence": "  "})).is_none());
    assert!(
        jobs_root_from_evidence(&json!({"type": "plan_hit", "evidence": "/x/y.jpg"})).is_none(),
        "目录不叫 pt<n> 就说明板端换了存证布局，反推出来的根不可信"
    );
}

/// 显式配的 `jobs_dir` 不能被板端事件覆盖（现场可能 bind-mount 到别处）
#[tokio::test]
async fn pinned_jobs_dir_wins_over_what_the_board_reports() {
    let ai = AiPatrol::with_jobs_dir("http://127.0.0.1:9", true, Some("/pinned/".to_string()));
    ai.learn_jobs_root("/from/board".to_string()).await;
    assert_eq!(ai.jobs_root().await.as_deref(), Some("/pinned"));

    // 没配时才学板端给的路径
    let ai = AiPatrol::with_jobs_dir("http://127.0.0.1:9", true, None);
    ai.learn_jobs_root("/from/board/detect_jobs".to_string())
        .await;
    assert_eq!(
        ai.jobs_root().await.as_deref(),
        Some("/from/board/detect_jobs")
    );
}

/// 存证目录里只认「本轮写下的」最新一张：上一轮巡检的图不能当这一帧的证据
#[tokio::test]
async fn newest_evidence_file_skips_stale_and_picks_freshest() {
    let dir = std::env::temp_dir().join(format!(
        "robodog-jobs-scan-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF, 0x00]).unwrap();
    std::fs::write(dir.join("notes.txt"), b"not an image").unwrap();

    // 窗口开始时间在未来（= 这些文件都是「上一轮的」）→ 不带图
    let future = SystemTime::now() + Duration::from_secs(60);
    assert!(newest_evidence_file(&dir, future).await.is_none());

    // 正常窗口：取到那张 jpg（非图片后缀直接忽略）
    let past = SystemTime::now() - Duration::from_secs(60);
    let picked = newest_evidence_file(&dir, past)
        .await
        .expect("应扫到命中帧");
    assert!(picked.ends_with("dogcam_1.jpg"), "{picked}");

    // 目录不存在（客户端不在板端 / 板端换了目录）：安静返回 None
    assert!(
        newest_evidence_file(&dir.join("nope"), past)
            .await
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ai_action_matches_ai_and_chinese_word_only() {
    assert!(is_ai_action(&json!({"type": "AI 识别"})));
    assert!(is_ai_action(&json!({"type": "ai detect"})));
    assert!(is_ai_action(&json!({"type": "智能识别"})));
    assert!(!is_ai_action(&json!({"type": "可见光拍照"})));
    assert!(!is_ai_action(&json!({"type": "红外测温"})));
}

#[test]
fn empty_route_has_no_ai_points() {
    assert!(parse_ai_points(&json!({})).is_empty());
    assert!(parse_ai_points(&json!({"points": []})).is_empty());
}

#[test]
fn start_parses_platform_payload() {
    let start = AiStart::from_value(&json!({
        "windowId": "w-1",
        "action": "config",
        "task": "firextinguisher",
        "taskCode": "TK-1112-11",
        "pointIndex": 0,
        "labels": ["extinguisher"],
        "conf": 0.35,
        "cam": "dogcam",
        "fps": 3,
        "secs": 30,
        "wasRunning": true
    }))
    .expect("payload parses");
    assert_eq!(start.window_id, "w-1");
    assert_eq!(start.action, "config");
    assert_eq!(start.task, "firextinguisher");
    assert_eq!(start.task_code, "TK-1112-11");
    assert_eq!(start.point_index, 0);
    assert_eq!(start.plan_id, "TK-1112-11", "没带 planId 时退回 taskCode");
    assert_eq!(start.expect_count, None);
    assert_eq!(start.labels, vec!["extinguisher".to_string()]);
    assert!((start.conf - 0.35).abs() < 1e-9);
    assert_eq!(start.secs, 30);
    assert!(start.was_running);
    assert!(AiStart::from_value(&json!({"action": "start"})).is_none());
}

#[test]
fn hits_filter_by_conf_and_dedupe() {
    let mut seen = HashSet::new();
    let frame = json!({
        "type": "detect",
        "boxes": [
            {"label": "extinguisher", "conf": 0.92},
            {"label": "extinguisher", "conf": 0.91},
            {"label": "smoke", "conf": 0.20}
        ]
    });
    let first = take_hits(&frame, 0.35, &mut seen);
    assert_eq!(first, vec![("extinguisher".to_string(), 0.92)]);
    // 同一类别只报一次（窗口内 3~6fps 会重复出现）
    assert!(take_hits(&frame, 0.35, &mut seen).is_empty());
    // 低于阈值的不算
    assert!(
        take_hits(
            &json!({"type": "detect", "boxes": [{"label": "smoke", "conf": 0.30}]}),
            0.35,
            &mut seen
        )
        .is_empty()
    );
    // 非 detect 事件忽略
    assert!(take_hits(&json!({"type": "telemetry"}), 0.0, &mut seen).is_empty());
}

#[test]
fn point_stages_keep_window_others_do_not() {
    // 到点阶段（触发开窗）必须包含在「还在航点上」（保持窗口）里
    for stage in ["patrol.point_arrived", "patrol.waiting"] {
        assert!(is_arrival_stage(stage));
        assert!(is_point_stage(stage), "{stage} 是在航点上");
    }
    // 点上的作业阶段：窗口继续开着（狗还站在那儿）
    for stage in ["patrol.photographing", "patrol.point_completed"] {
        assert!(!is_arrival_stage(stage));
        assert!(is_point_stage(stage), "{stage} 也是在本航点上");
    }
    for stage in [
        "patrol.navigating",
        "patrol.returning",
        "navigation.navigating",
        "",
    ] {
        assert!(!is_point_stage(stage), "{stage} 已经离开航点");
    }
}

#[test]
fn sse_blocks_are_split_and_partial_stays() {
    let mut buffer = String::from("data: {\"a\":1}\n\ndata: {\"b\":2}\n\ndata: {\"part");
    assert_eq!(take_sse_events(&mut buffer), vec!["{\"a\":1}", "{\"b\":2}"]);
    assert_eq!(buffer, "data: {\"part");
    buffer.push_str("ial\":3}\n\n");
    assert_eq!(take_sse_events(&mut buffer), vec!["{\"partial\":3}"]);
}
