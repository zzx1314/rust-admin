//! 识别窗口的生命周期：起窗、消费 Agent SSE、命中/数量/未检出的记账、收口与看门狗。

// 同一模块拆出来的几块共用一个词汇表（`Window` / `WindowConfig` / `Hit` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/// 客户端窗口的超时看门狗。
///
/// 客户端进程崩了 / 业务 WS 断了，「窗口结束」就永远不来；不清窗口态的话
/// `GET /api/backpack/ai-window` 会一直显示「在识别」，下次到点还会被判成
/// 「这个点已经在识别」而跳过。窗口秒数 + 10s 兜底。
pub(crate) fn spawn_client_window_timeout(
    state: AppState,
    window_id: String,
    stop: Arc<Notify>,
    secs: u64,
) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(secs.saturating_add(10))).await;
        let ai = &state.ai_patrol;
        let taken = {
            let mut guard = ai.active.lock().await;
            match guard.as_ref() {
                Some(window) if Arc::ptr_eq(&window.stop, &stop) => guard.take(),
                _ => None,
            }
        };
        let Some(window) = taken else {
            return;
        };
        let hits = window.hits.lock().await.len();
        warn!(
            "航点 AI 识别：客户端窗口 {window_id} 超过 {}s 未回报结束，服务端清掉窗口态",
            secs.saturating_add(10)
        );
        if window.closing.is_some() {
            // 已经在收尾宽限里（平台先下了 `ai_stop`，看门狗却先到点）：
            // 按已上报的检出结算，「没找到」不能因为客户端没回话就永远丢。
            ai.settle_and_remember(&state, window).await;
            return;
        }
        // 这里**不**记「没找到」：客户端的结论（有没有检出 / 停够了没有）全在它手里，
        // 它一句话都没回来说明链路己经断了，推“未检出”只会是误报。
        let content = format!("AI 识别窗口超时未收到客户端结束回报（已上报 {hits} 类目标）");
        let cfg = window.cfg.clone();
        ai.remember(window).await;
        if let Err(error) = insert_task_event(&state.db, &cfg, &now(), &content).await {
            warn!("AI 识别：写任务事件失败: {error}");
        }
    });
}

/// 关窗后的收尾看门狗：等客户端把本窗口的结论说完，最多 [`CLOSE_GRACE`] 秒。
///
/// 正常路径是客户端 `ai_end` 先到（[`AiPatrol::on_client_end`] 摘窗口并结算），这里
/// 什么也不做。只有客户端进程崩了 / 业务 WS 断了才由这里按已上报的检出结算 ——
/// 「看满了却没找到」不能因为一条永远不来的 `ai_end` 就不告警。
pub(crate) fn spawn_close_grace(state: AppState, stop: Arc<Notify>) {
    tokio::spawn(async move {
        tokio::time::sleep(CLOSE_GRACE).await;
        let ai = &state.ai_patrol;
        let taken = {
            let mut guard = ai.active.lock().await;
            match guard.as_ref() {
                Some(window) if Arc::ptr_eq(&window.stop, &stop) && window.closing.is_some() => {
                    guard.take()
                }
                _ => None,
            }
        };
        let Some(window) = taken else {
            return;
        };
        let hits = window.hits.lock().await.len();
        warn!(
            "航点 AI 识别：窗口 {} 收尾 {}s 内没等到客户端回报结束，按已上报的 {hits} 类目标结算",
            window.window_id,
            CLOSE_GRACE.as_secs()
        );
        ai.settle_and_remember(&state, window).await;
    });
}

/// 一个窗口的启动参数（算好后窗口任务只负责执行）
#[derive(Clone)]
pub(crate) struct WindowConfig {
    /// 设备编号（`0202222672400230`）：告警去重键与文案都用它
    pub(crate) device_code: String,
    pub(crate) device_id: Option<i64>,
    pub(crate) park_id: Option<i64>,
    pub(crate) task_code: String,
    pub(crate) task_name: String,
    pub(crate) point_index: i64,
    pub(crate) point_name: String,
    /// 动作 `param` 里的场景提示，如「烟火 / 人员 / 仪表读数」
    pub(crate) hint: String,
    /// 动作 `scene`：编辑器从实时视频模型目录里选的模型包（优先于 `hint` 猜词）
    pub(crate) scene: Option<String>,
    /// 动作 `labels`：勾选的识别类别（英文 `name`，空 = 该场景全部类别）
    pub(crate) labels: Vec<String>,
    /// 动作 `threshold` 原文，如「置信度 > 0.85」
    pub(crate) threshold_text: String,
    /// 动作 `expectCount`：检查目标数量（数量任务必填）
    pub(crate) expect_count: Option<i64>,
    /// 任务类型：`exist`（是否存在）/ `count`（数量是否正确）
    pub(crate) check_type: String,
    /// 动作 `alertWhenMissing`：`exist` 任务看满停留时长、**一个都没检出**时也告警
    ///
    /// `exist` 默认只在「检出」时告警；「应该有却没有」这种语义靠这个开关（否则只能用
    /// `count` + `expectCount`）。`count` 任务自带「0 ≠ N」告警，这个字段对它无效。
    pub(crate) alert_when_missing: bool,
    /// 动作 `notifyWhenCountOk`：`count` 任务数量**一致**时也推一条企业微信回执
    ///
    /// 只发群消息：不进告警中心、不派单、不建工单（见 `notify::count_matched`）。
    /// `exist` 任务没有「数量一致」这种结论，这个字段对它无效。
    pub(crate) notify_when_count_ok: bool,
    pub(crate) secs: u64,
    /// 航点停留时长（秒）：窗口开了这么久还没检出，才把「一个都没找到」算成结论
    pub(crate) dwell: u64,
}

/// 打开识别 → 收 SSE → 检出即告警 → 到期/抢占收尾
pub(crate) async fn run_window(
    state: AppState,
    config: WindowConfig,
    stop: Arc<Notify>,
    hits: Arc<Mutex<Vec<Hit>>>,
    alerted: Arc<AtomicBool>,
) {
    let started = Instant::now();
    let Ok(base) = backpack::agent_base(&state.config).map(str::to_string) else {
        // 用 WARN 而不是 DEBUG：配了「只给浏览器直连地址」（`ROBODOG_BACKPACK_BROWSER_URL`）
        // 的部署里，服务端根本连不到 Agent，航点 AI 永远起不来；现场只看得到「没告警」，
        // 必须让这条原因出现在日志里。
        warn!(
            "AI 识别：本部署的服务端访问不到背包 Agent，跳过航点「{}」。\
             需要服务端与板端同网段并配置 ROBODOG_BACKPACK_URL（浏览器直连模式不够）",
            config.point_name
        );
        // 没接背包时不要把窗口留在状态里（否则 `/backpack/ai-window` 会一直说「在识别」）
        stop_if_current(&state, &stop).await;
        return;
    };
    let Ok(client) = backpack::api_client() else {
        return;
    };

    // 1) 现状：是否已经在跑（操作员手动开的识别），以及可用的模型场景
    let tasks_json = backpack::fetch_json(&client, &format!("{base}/api/detect/tasks"))
        .await
        .ok();
    let status = backpack::fetch_json(&client, &format!("{base}/api/detect/status"))
        .await
        .ok();
    let was_running = status
        .as_ref()
        .and_then(|value| value.get("running"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let scene = resolve_scene(tasks_json.as_ref(), &config.hint, config.scene.as_deref());
    let threshold = parse_conf(&config.threshold_text)
        .or_else(|| scene.as_ref().map(|item| item.conf))
        .unwrap_or(0.5)
        .clamp(0.05, 0.95);
    // 勾选的类别只保留该场景真实存在的，全部失效时退回「全部类别」（`labels: null`）
    let labels = scene
        .as_ref()
        .map(|item| scene_labels(tasks_json.as_ref(), &item.task, &config.labels))
        .unwrap_or_default();
    let cam = status
        .as_ref()
        .and_then(|value| value.get("default_cam"))
        .and_then(Value::as_str)
        .unwrap_or("dogcam")
        .to_string();
    let task_name = scene
        .as_ref()
        .map(|item| item.task.clone())
        .unwrap_or_else(|| config.hint.clone());

    update_resolved(&state, &stop, &task_name, &labels, threshold, &cam).await;

    // 2) 启动（已在跑就热更新，避免「识别已在运行」被拒）
    //    计划 id / 航点下标 / 检查目标数量一并下发：板端命中即按计划目录存证，
    //    数量与 expect_count 不符就在 SSE 的 plan_hit 里带 count_alert。
    //    `capture_enabled` 是航点自动采集总开关（中台 `media_capture_enabled`，默认关）：
    //    直连模式以前不带媒体参数，板端按自己的 `MEDIA_DATA_DIR` 默默攒图，这条一并收口。
    let capture_enabled =
        crate::zentai::media_capture_config(&state.config, &state.caches.media_config)
            .await
            .enabled;
    let body = json!({
        "task": task_name,
        "labels": if labels.is_empty() {
            Value::Null
        } else {
            json!(labels)
        },
        "conf": threshold,
        "cams": [cam],
        "fps": DETECT_FPS,
        "plan_id": config.task_code,
        "point_index": config.point_index,
        "check_type": config.check_type,
        "expect_count": config.expect_count,
        "capture_enabled": capture_enabled,
    });
    if let Err(error) = start_detect(&client, &base, &body, was_running).await {
        warn!(
            "AI 识别：启动失败（航点「{}」场景 {task_name}）: {error}",
            config.point_name
        );
        stop_if_current(&state, &stop).await;
        return;
    }
    info!(
        "航点 AI 识别开始：任务 {} 航点 {}「{}」场景={task_name} 类别={} 阈值={threshold:.2} 相机={cam} 窗口={}s",
        config.task_code,
        config.point_index,
        config.point_name,
        if labels.is_empty() {
            "全部".to_string()
        } else {
            labels.join("/")
        },
        config.secs
    );

    // 3) 收事件（到期或被打断就收尾）
    let deadline = tokio::time::Instant::now() + Duration::from_secs(config.secs);
    let sink = WindowSink {
        hits: hits.clone(),
        alerted: alerted.clone(),
    };
    let expired = consume_events(&state, &base, &config, threshold, &stop, &sink, deadline).await;
    // 看满了一个都没找到：数量任务算「数量不一致」，exist + alertWhenMissing 算「未检出」。
    // 到期算看满；被「已离开航点」提前摘掉时，只要窗口已开够 dwell 也算看满
    // （否则这类告警永远报不出来，见 [`watched_enough`]）。
    let hit_seen = !hits.lock().await.is_empty();
    let enough = expired || watched_enough(&config, started.elapsed());
    settle_no_hit(&state, &config, hit_seen, &alerted, enough).await;

    // 只在「本窗口还是当前窗口」时才去停 Agent：
    // - 被新窗口顶替时，新窗口已经接管（且可能有操作员的识别在跑），不能停；
    // - 操作员本来开着识别（was_running）时，窗口结束也不该把人家的停掉。
    if was_running {
        info!("航点 AI 识别窗口结束（保留操作员已开启的识别）");
        // 识别留着，但计划必须摘掉：否则之后画面再命中会继续写这个航点的存证/告警
        let _ = client
            .post(format!("{base}/api/detect/plan/clear"))
            .json(&json!({}))
            .send()
            .await;
        return;
    }
    if !state.ai_patrol.replaced_by_other(&stop).await {
        let _ = client
            .post(format!("{base}/api/detect/stop"))
            .json(&json!({}))
            .send()
            .await;
        info!(
            "航点 AI 识别结束：任务 {} 航点 {}「{}」，上报 {} 类目标",
            config.task_code,
            config.point_index,
            config.point_name,
            hits.lock().await.len()
        );
    }
}

/// 读 SSE（`/api/events`），把窗口内的 `detect` 事件里的目标写进告警中心。
pub(crate) async fn consume_events(
    state: &AppState,
    base: &str,
    config: &WindowConfig,
    threshold: f64,
    stop: &Arc<Notify>,
    sink: &WindowSink,
    deadline: tokio::time::Instant,
) -> bool {
    // 返回值：窗口是否「正常到期」（false = 被 stop 打断 / 事件流断了）
    let hits = &sink.hits;
    let alerted = &sink.alerted;
    let Ok(client) = backpack::stream_client() else {
        return false;
    };
    let mut response = match client
        .get(format!("{base}/api/events"))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            warn!("AI 识别：Agent 拒绝事件流 HTTP {}", response.status());
            return false;
        }
        Err(error) => {
            warn!("AI 识别：连不上 Agent 事件流: {error}");
            return false;
        }
    };

    let notified = stop.notified();
    tokio::pin!(notified);
    let mut buffer = String::new();
    let mut seen: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            _ = &mut notified => return false,
            _ = tokio::time::sleep_until(deadline) => return true,
            chunk = response.chunk() => {
                match chunk {
                    Ok(Some(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        if buffer.len() > SSE_BUF_LIMIT {
                            warn!("AI 识别：SSE 缓冲超过上限，丢弃重来");
                            buffer.clear();
                            continue;
                        }
                        for payload in take_sse_events(&mut buffer) {
                            let Ok(event) = serde_json::from_str::<Value>(&payload) else {
                                continue;
                            };
                            // 板端已判定的「数量不一致」：每个窗口只落一条告警
                            if let Some((expect, found, evidence)) = plan_hit_alert(&event) {
                                if !alerted.swap(true, Ordering::Relaxed) {
                                    record_count_alert(state, config, found, expect, &evidence, None)
                                        .await;
                                }
                                continue;
                            }
                            // 板端已判定的「数量一致」：勾了「一致也通知」才推一条企业微信回执
                            // （不落告警、不建工单，见 [`record_count_ok`]）
                            if let Some((expect, found, evidence)) = plan_hit_count_ok(&event, config)
                            {
                                if !alerted.swap(true, Ordering::Relaxed) {
                                    record_count_ok(state, config, found, expect, &evidence, None)
                                        .await;
                                }
                                continue;
                            }
                            let Some((label, conf)) = detect_hit(&event, threshold) else {
                                continue;
                            };
                            if !seen.insert(label.clone()) {
                                continue;
                            }
                            record_hit(state, config, &label, conf, hits, "", None).await;
                        }
                    }
                    Ok(None) => return false,
                    Err(error) => {
                        warn!("AI 识别：事件流中断: {error}");
                        return false;
                    }
                }
            }
        }
    }
}

/// 检出 → 告警 + 任务事件
///
/// **数量核对任务（`checkType=count`）例外**：检出只写任务事件，不落告警、不建工单、
/// 不推企业微信——数量任务只有「检查 N 个、实际检出 M 个」这一个结论要处置，
/// 把每个检出类别都报一遍会把那条结论淹掉（见 [`record_count_alert`]）。
///
/// `evidence` / `image` 是这一帧命中图的「板端路径」和「字节」：企业微信的图片消息
/// 要字节不要路径（见 [`crate::notify::evidence_image`]），路径只留在告警正文
/// （告警中心 / 工单详情能回看）与任务时间线里，**群卡片正文不带**
/// （`notify::without_evidence_path`：图就紧跟在卡片后面，路径是噪点）。
/// 客户端代调模式下由客户端扫板端存证目录给出；服务端直连模式
/// （`ROBODOG_BACKPACK_URL`）读不到板端磁盘，两个都是空 → 只发卡片。
pub(crate) async fn record_hit(
    state: &AppState,
    config: &WindowConfig,
    label: &str,
    conf: f64,
    hits: &Arc<Mutex<Vec<Hit>>>,
    evidence: &str,
    image: Option<NoticeImage>,
) {
    let at = now();
    let key = ai_dedup_key(config, label);
    let mut content = format!(
        "AI 识别：任务「{}」航点「{}」检出 {}（置信度 {:.2}）",
        config.task_name, config.point_name, label, conf
    );
    if !evidence.trim().is_empty() {
        content.push_str(&format!("（证据：{}）", evidence.trim()));
    }
    info!("{content}");

    // 数量核对任务（`checkType=count`）**只认数量结论**：检出本身不是结论，
    // 「检查 N 个，实际检出 M 个」才是。每检出一种目标就落一条告警，会把真正要处置的
    // 那条数量告警淹掉（一个航点检出 3 类目标就是 4 条告警 / 4 张工单 / 4 条群消息），
    // 现场要的是「只推数量不一致」。所以这里只留任务时间线：不落告警、不建工单、
    // 不推企业微信，数量结论统一由 [`record_count_alert`] 出。
    let count_task = config.check_type == "count";
    let has_image = image.is_some();
    let alert_code = if count_task {
        None
    } else {
        match insert_alert(&state.db, config, &content, &key).await {
            Ok(alert_code) => {
                crate::notify::alert_created_with_image(&state.notify(), &alert_code, image).await;
                Some(alert_code)
            }
            Err(error) => {
                warn!("AI 识别：写告警失败: {error}");
                None
            }
        }
    };
    hits.lock().await.push(Hit {
        at: at.clone(),
        label: label.to_string(),
        conf,
        alert_code,
        // 数量任务的检出没有告警可补图（`ai_evidence` 认领的是「还没带过图的检出」），
        // 标记成已带图，免得晚到的图白认领一次、还打一条「都已带过图」的日志
        image: has_image || count_task,
    });
    if let Err(error) = insert_task_event(&state.db, config, &at, &content).await {
        warn!("AI 识别：写任务事件失败: {error}");
    }
}

/// 数量核对不一致 → 告警中心 + 任务时间线 + 企业微信（带证据图）
///
/// 与 [`record_hit`] 平级：检出是「找到了」，这里是「找到了但数量对不上」，
/// 两者都要能推企业微信，所以告警入库后同样走 `notify::alert_created*`。
///
/// `image` 是板端证据图（客户端从磁盘读出后随 `ai_alert` 带上）：企业微信的图片消息
/// 要字节不要路径，见 [`crate::notify::evidence_image`]。服务端直连板端 Agent 的模式下
/// 平台读不到板端磁盘，这时只有路径、没有图，卡片照发。
pub(crate) async fn record_count_alert(
    state: &AppState,
    config: &WindowConfig,
    found: i64,
    expect: i64,
    evidence: &str,
    image: Option<NoticeImage>,
) {
    let at = now();
    let mut content = format!(
        "AI 识别数量核对不一致：任务「{}」航点「{}」检查 {} 个，实际检出 {} 个",
        config.task_name, config.point_name, expect, found
    );
    if !evidence.trim().is_empty() {
        content.push_str(&format!("（证据：{}）", evidence.trim()));
    }
    info!("{content}");

    let key = ai_dedup_key(config, "count");
    match insert_alert(&state.db, config, &content, &key).await {
        Ok(alert_code) => {
            crate::notify::alert_created_with_image(&state.notify(), &alert_code, image).await;
        }
        Err(error) => warn!("AI 识别：写数量告警失败: {error}"),
    }
    if let Err(error) = insert_task_event(&state.db, config, &at, &content).await {
        warn!("AI 识别：写任务事件失败: {error}");
    }
}

/// 数量核对一致 → 企业微信回执 + 任务时间线（**不落告警、不派单、不建工单**）
///
/// 与 [`record_count_alert`] 是两条路：那条是「对不上，要处置」，这条是「对上了，
/// 留个回执」。只在动作勾了 `notifyWhenCountOk` 时才会被调用（见
/// [`AiPatrol::on_client_count`] 与 `plan_hit_count_ok`）。
///
/// 为什么不能复用 `insert_alert` + `alert_created`：那条链路的第一步就是自动派单
/// （`alert_flow::on_alert`），工单列表里多一条「检查 3 个、检出 3 个」只会让真正
/// 要处置的单淹掉。
pub(crate) async fn record_count_ok(
    state: &AppState,
    config: &WindowConfig,
    found: i64,
    expect: i64,
    evidence: &str,
    image: Option<NoticeImage>,
) {
    let at = now();
    let mut content = format!(
        "AI 识别数量核对一致：任务「{}」航点「{}」检查 {} 个，实际检出 {} 个",
        config.task_name, config.point_name, expect, found
    );
    if !evidence.trim().is_empty() {
        content.push_str(&format!("（证据：{}）", evidence.trim()));
    }
    info!("{content}");

    crate::notify::count_matched(
        &state.notify(),
        crate::notify::CountMatched {
            device_code: &config.device_code,
            task_code: &config.task_code,
            task_name: &config.task_name,
            point_index: config.point_index,
            point_name: &config.point_name,
            expect,
            found,
            content: &content,
            image,
        },
    )
    .await;
    if let Err(error) = insert_task_event(&state.db, config, &at, &content).await {
        warn!("AI 识别：写任务事件失败: {error}");
    }
}

/// 「应该有、但一个都没找到」→ 告警中心 + 任务时间线 + 企业微信。
///
/// 只给 `exist` + `alertWhenMissing` 的航点用：`exist` 本来只会在**检出**时告警，
/// 「灭火器不见了」这类「缺失」语义以前根本报不出来（只能用 `count` + `expectCount`）。
/// 与数量告警同理，只在「狗看够了」时才算（见 [`watched_enough`]）
/// ——被任务结束/新航点打断且停留时间没走完时，狗可能还没看够。
pub(crate) async fn record_missing_alert(state: &AppState, config: &WindowConfig) {
    let at = now();
    let targets = if config.labels.is_empty() {
        config.scene.clone().unwrap_or_else(|| config.hint.clone())
    } else {
        config.labels.join("/")
    };
    let content = format!(
        "AI 识别未检出：任务「{}」航点「{}」窗口 {}s 内没找到 {}（场景 {}）",
        config.task_name,
        config.point_name,
        config.secs,
        if targets.trim().is_empty() {
            "目标"
        } else {
            targets.as_str()
        },
        if config
            .scene
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            "-"
        } else {
            config.scene.as_deref().unwrap_or_default()
        }
    );
    info!("{content}");

    let key = ai_dedup_key(config, "missing");
    match insert_alert(&state.db, config, &content, &key).await {
        Ok(alert_code) => crate::notify::alert_created(&state.notify(), &alert_code).await,
        Err(error) => warn!("AI 识别：写未检出告警失败: {error}"),
    }
    if let Err(error) = insert_task_event(&state.db, config, &at, &content).await {
        warn!("AI 识别：写任务事件失败: {error}");
    }
}

/// 「一个都没找到」的记账入口：数量任务落「数量不一致（0 ≠ N）」，
/// `exist` + `alertWhenMissing` 落「AI 识别未检出」。
///
/// 两个调用前提都由调用方给：
/// - `hit_seen`：本窗口有没有报过检出（报过就不再判「没找到」）；
/// - `enough`：狗在这个点上看够了没有（见 [`watched_enough`]）。
///
/// 三条路（直连到期 / 客户端 `ai_end` / 平台「已离开航点」关窗）都会走到这里，
/// 用共享的 `alerted` 保证一个窗口只落一条告警。
pub(crate) async fn settle_no_hit(
    state: &AppState,
    config: &WindowConfig,
    hit_seen: bool,
    alerted: &Arc<AtomicBool>,
    enough: bool,
) {
    if hit_seen || !enough {
        return;
    }
    if config.check_type == "count" {
        if let Some(expect) = config.expect_count
            && !alerted.swap(true, Ordering::Relaxed)
        {
            record_count_alert(state, config, 0, expect, "", None).await;
        }
    } else if config.check_type == "exist"
        && config.alert_when_missing
        && !alerted.swap(true, Ordering::Relaxed)
    {
        record_missing_alert(state, config).await;
    }
}

/// 狗在这个点上看够了没有：窗口开了至少 `dwell.max(4)` 秒。
///
/// 为什么不用「窗口到期」当唯一标准：窗口时长是 `dwell + 8`（给 Agent 加载模型留的余量），
/// 而狗停到 `dwell` 就往下一点走了——平台收到「已离开航点」时窗口往往还差几秒没到点，
/// 于是 `expired` 恒为 false、窗口也早被摘掉，「没找到也告警」永远报不出来
/// （2026-11-18 办公室巡检「垃圾桶溢出」漏报：pt1 停 65 s、窗口 68 s）。
/// 反过来，停留 0 秒的「路过点」`dwell.max(4)` 要求至少看 4 秒，也不至于误报。
pub(crate) fn watched_enough(config: &WindowConfig, watched: Duration) -> bool {
    watched >= Duration::from_secs(config.dwell.max(4))
}

/// 把解析出来的场景/阈值/相机回填到窗口记录（`GET /api/backpack/ai-window` 看得到）
pub(crate) async fn update_resolved(
    state: &AppState,
    stop: &Arc<Notify>,
    scene: &str,
    labels: &[String],
    threshold: f64,
    cam: &str,
) {
    let mut guard = state.ai_patrol.active.lock().await;
    if let Some(window) = guard.as_mut()
        && Arc::ptr_eq(&window.stop, stop)
    {
        window.scene = scene.to_string();
        window.labels = labels.to_vec();
        window.threshold = threshold;
        window.cam = cam.to_string();
    }
}

/// 启动失败时把自己从状态里摘掉（只摘自己，不误伤已经接管的新窗口）
pub(crate) async fn stop_if_current(state: &AppState, stop: &Arc<Notify>) {
    let mut guard = state.ai_patrol.active.lock().await;
    if guard
        .as_ref()
        .map(|window| Arc::ptr_eq(&window.stop, stop))
        .unwrap_or(false)
    {
        guard.take();
    }
}
