//! 客户端代调模式的入口：`ai_point` / `ai_detect` / `ai_alert` / `ai_count` /
//! `ai_end` / `ai_evidence` 落到窗口状态机的处理。
// 同一模块拆出来的几块共用一个词汇表（`Window` / `WindowConfig` / `Hit` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/// 客户端代调模式的上报入口（`ai_point` / `ai_detect` / `ai_alert` /
/// `ai_count` / `ai_end` / `ai_evidence`）。
impl AiPatrol {
    /// 客户端到点后的开窗请求（`ai_point`）。
    ///
    /// 场景解析、类别过滤、阈值、窗口时长全部复用直连模式的函数；客户端只提供
    /// 「板端有什么模型、当前在不在跑」这两件服务端看不到的事实。
    pub async fn on_client_point(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiPointPayload,
    ) {
        if payload.task_code.is_empty() {
            return;
        }
        if source_for(&state.config) != PointAiSource::Client {
            debug!(
                "航点 AI 识别：point_ai={} 由服务端直连，忽略设备 {device_code} 的开窗请求",
                state.config.point_ai
            );
            return;
        }
        let task = match load_task(&state.db, &payload.task_code).await {
            Ok(Some(task)) => task,
            Ok(None) => {
                warn!(
                    "航点 AI 识别：任务 {} 不存在，忽略设备 {device_code} 的开窗请求",
                    payload.task_code
                );
                return;
            }
            Err(error) => {
                warn!("航点 AI 识别：查任务 {} 失败: {error}", payload.task_code);
                return;
            }
        };

        let tasks = payload.tasks.as_ref();
        let scene = resolve_scene(tasks, &payload.hint, payload.scene.as_deref());
        let threshold = parse_conf(&payload.threshold)
            .or_else(|| scene.as_ref().map(|item| item.conf))
            .unwrap_or(0.5)
            .clamp(0.05, 0.95);
        let labels = scene
            .as_ref()
            .map(|item| scene_labels(tasks, &item.task, &payload.labels))
            .unwrap_or_default();
        let task_name = scene
            .as_ref()
            .map(|item| item.task.clone())
            .unwrap_or_else(|| payload.hint.clone());
        let cam = payload
            .agent_cam
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("dogcam")
            .to_string();
        let secs = window_secs(payload.dwell.unwrap_or(0), state.config.ai_window_max_secs);
        let was_running = payload.agent_running.unwrap_or(false);
        let point_name = payload
            .point_name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "航点".to_string());
        let seq = WINDOW_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let window_id = format!(
            "AIW-{device_code}-{}-{}-{seq}",
            payload.task_code, payload.point_index
        );
        // 「检查目标数量」以路线数据为准（旧版客户端不会带 `expectCount`）；
        // 客户端带了而路线里没配时用客户端的值兜底。
        let mut expect_count = payload.expect_count.filter(|value| *value > 0);
        // 任务类型同样以路线数据为准；旧版客户端不带 checkType 时按 expectCount 推断
        let mut check_type = normalize_check_type(payload.check_type.as_deref(), expect_count);
        // 「未检出也告警」同样以路线数据为准（客户端只做转发）
        let mut alert_when_missing = payload.alert_when_missing.unwrap_or(false);
        // 「数量一致也通知」同理：新平台随 `ai_point` 带上，老平台没有就用路线数据
        let mut notify_when_count_ok = payload.notify_when_count_ok.unwrap_or(false);
        if let Some(route_id) = task.route_id
            && let Ok(Some(point)) = load_point(&state.db, route_id, payload.point_index).await
            && let Some(action) = point.find_ai_action()
        {
            expect_count = action.expect_count.or(expect_count);
            check_type = action.check_type.clone();
            alert_when_missing = action.alert_when_missing;
            notify_when_count_ok = action.notify_when_count_ok;
        }

        let cfg = WindowConfig {
            device_code: device_code.to_string(),
            device_id: task.device_id,
            park_id: task.park_id,
            task_code: payload.task_code.clone(),
            task_name: task.name.clone(),
            point_index: payload.point_index,
            point_name: point_name.clone(),
            hint: payload.hint.clone(),
            scene: payload.scene.clone(),
            labels: payload.labels.clone(),
            threshold_text: payload.threshold.clone(),
            expect_count,
            check_type,
            alert_when_missing,
            notify_when_count_ok,
            secs,
            dwell: payload.dwell.unwrap_or(0).max(0) as u64,
        };
        let stop = Arc::new(Notify::new());
        let hits = Arc::new(Mutex::new(Vec::new()));
        let alerted = Arc::new(AtomicBool::new(false));
        let previous = {
            let mut guard = self.active.lock().await;
            let previous = guard.take();
            *guard = Some(Window {
                window_id: window_id.clone(),
                device_code: device_code.to_string(),
                task_code: payload.task_code.clone(),
                point_index: payload.point_index,
                point_name: point_name.clone(),
                scene: task_name.clone(),
                labels: labels.clone(),
                threshold,
                cam: cam.clone(),
                started_at: now(),
                started: Instant::now(),
                stop: stop.clone(),
                hits,
                alerted,
                cfg: cfg.clone(),
                closing: None,
            });
            previous
        };
        if let Some(previous) = previous {
            previous.stop.notify_one();
            if previous.closing.is_some() {
                // 上一个点还在收尾宽限里（新航点已经到点，客户端来不及回结论）：
                // 先把它结算掉，不能因为被顶替就把「有/没找到」吞掉。
                self.settle_and_remember(state, previous).await;
            }
        }

        let scene_name = task_name.clone();
        // 板端媒体目录 / 固定帧数：中台参数配置是唯一真源（带 60s 缓存），
        // 客户端原样转给板端 Agent 决定「存哪、存几张」，「同步媒体」按同一目录回传。
        let media =
            crate::zentai::media_capture_config(&state.config, &state.caches.media_config).await;
        let body = json!({
            "windowId": window_id,
            "action": if was_running { "config" } else { "start" },
            "task": task_name,
            "taskCode": payload.task_code,
            "pointIndex": payload.point_index,
            "pointName": point_name,
            "planId": payload.task_code,
            "checkType": cfg.check_type,
            "expectCount": expect_count,
            // 数量一致也通知：客户端据此把板端 `plan_hit.count_ok=true` 转成 `ai_count`
            // （平台只推企业微信回执，不落告警、不建工单）
            "notifyWhenCountOk": cfg.notify_when_count_ok,
            "labels": labels,
            "conf": threshold,
            "cam": cam,
            "fps": DETECT_FPS,
            "secs": secs,
            "wasRunning": was_running,
            "mediaDir": media.picture_dir,
            "mediaFrames": media.frames,
            // 航点自动采集总开关（中台 `media_capture_enabled`，默认关）：
            // 关时板端只写告警证据图，不再往媒体归档目录攒训练图。
            "captureEnabled": media.enabled,
        });
        if !crate::gateway::send_message_to_device(
            &state.device_connections,
            device_code,
            "ai_start",
            body,
        )
        .await
        {
            warn!(
                "航点 AI 识别：设备 {device_code} 不在线，航点 {}「{point_name}」跳过",
                payload.point_index
            );
            stop_if_current(state, &stop).await;
            return;
        }
        info!(
            "航点 AI 识别（客户端代调）开始：任务 {} 航点 {}「{point_name}」场景={} 类别={} 阈值={threshold:.2} 相机={cam} 窗口={secs}s 模式={}",
            payload.task_code,
            payload.point_index,
            scene_name,
            if labels.is_empty() {
                "全部".to_string()
            } else {
                labels.join("/")
            },
            if was_running {
                "config（复用已开的识别）"
            } else {
                "start"
            }
        );
        spawn_client_window_timeout(state.clone(), window_id, stop, secs);
    }

    /// 客户端回传的检出（`ai_detect`）：写告警 + 任务事件 + 企业微信推送。
    ///
    /// 客户端已按阈值过滤、并按类别去重过一次；这里再做一次双保险（窗口可能已被
    /// 新航点顶替，迟到的事件必须丢掉）。
    pub async fn on_client_detect(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiDetectPayload,
    ) {
        let (cfg, threshold, hits) = {
            let guard = self.active.lock().await;
            let Some(window) = guard.as_ref() else {
                debug!("航点 AI 识别：没有活动窗口，丢弃检出 {}", payload.label);
                return;
            };
            if window.device_code != device_code
                || (!window.window_id.is_empty() && window.window_id != payload.window_id)
            {
                debug!(
                    "航点 AI 识别：窗口 {} 已失效，丢弃检出 {}",
                    payload.window_id, payload.label
                );
                return;
            }
            (window.cfg.clone(), window.threshold, window.hits.clone())
        };
        if payload.conf + 1e-9 < threshold {
            debug!(
                "航点 AI 识别：类别 {} 置信度 {:.2} 低于阈值 {threshold:.2}，忽略",
                payload.label, payload.conf
            );
            return;
        }
        if hits
            .lock()
            .await
            .iter()
            .any(|hit| hit.label == payload.label)
        {
            return;
        }
        // 客户端随检出带上来的命中帧（路径 + base64）：读不到图就只发卡片
        let image = payload.evidence_image.as_deref().and_then(|raw| {
            match crate::notify::evidence_image(raw) {
                Ok(image) => Some(image),
                Err(error) => {
                    warn!("AI 识别：客户端上报的检出证据图不可用，只发卡片: {error}");
                    None
                }
            }
        });
        record_hit(
            state,
            &cfg,
            &payload.label,
            payload.conf,
            &hits,
            &payload.evidence,
            image,
        )
        .await;
    }

    /// 客户端回报窗口结束（`ai_end`）：收尾 + 记一条任务事件。
    pub async fn on_client_end(&self, state: &AppState, device_code: &str, payload: &AiEndPayload) {
        let taken = {
            let mut guard = self.active.lock().await;
            let matches = guard
                .as_ref()
                .map(|window| {
                    window.device_code == device_code
                        && (window.window_id.is_empty() || window.window_id == payload.window_id)
                })
                .unwrap_or(false);
            if !matches {
                // 服务端已关窗（任务终态 / 离开航点 / 被新航点顶替）后才到的 ai_end：
                // 不重复记事件，但留一行 debug——排查「窗口为什么没结束记录」时能对上
                debug!(
                    "航点 AI 识别：窗口 {} 已失效或已关闭，忽略 ai_end（{}）",
                    payload.window_id, payload.reason
                );
                return;
            }
            guard.take()
        };
        let Some(window) = taken else {
            return;
        };
        let hits = window.hits.lock().await.len();
        let at = now();
        // 「看满了一个都没找到」：数量任务算数量不一致，exist + alertWhenMissing 算未检出。
        // 看满 = 客户端回报窗口正常到期，或窗口实际已开够 `dwell` 秒（见 [`settle_no_hit`]）。
        let enough = payload.expired || watched_enough(&window.cfg, window.watched());
        settle_no_hit(
            state,
            &window.cfg,
            payload.hits > 0 || hits > 0,
            &window.alerted,
            enough,
        )
        .await;
        info!(
            "航点 AI 识别结束（客户端回报）：任务 {} 航点 {}「{}」{}，上报 {hits} 类目标",
            window.task_code, window.point_index, window.point_name, payload.reason
        );
        let content = format!(
            "AI 识别窗口结束：{}（上报 {hits} 类目标，客户端回报 {}）",
            if payload.reason.trim().is_empty() {
                "无原因"
            } else {
                payload.reason.as_str()
            },
            payload.hits
        );
        if let Err(error) = insert_task_event(&state.db, &window.cfg, &at, &content).await {
            warn!("AI 识别：写任务事件失败: {error}");
        }
        // 客户端已经收尾了，但图可能还在后面（`ai_evidence` 跟着 `ai_end` 到）：留下窗口痕迹
        self.remember(window).await;
    }

    /// 客户端转发的「数量不一致」（`ai_alert`）：告警中心 + 任务时间线 + 企业微信。
    ///
    /// 判定在板端做（Agent 知道本帧到底画出了几个框，也有证据图和 `result.json`）；
    /// 这里只做归属校验与去重：同一个窗口只落一条告警，迟到（窗口已被新航点顶替）
    /// 的直接丢弃，避免旧航点的结论污染新航点。
    pub async fn on_client_alert(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiAlertPayload,
    ) {
        let (cfg, alerted, expect, check_type) = {
            let guard = self.active.lock().await;
            let Some(window) = guard.as_ref() else {
                debug!("航点 AI 识别：没有活动窗口，丢弃数量告警");
                return;
            };
            if window.device_code != device_code
                || (!window.window_id.is_empty() && window.window_id != payload.window_id)
            {
                debug!(
                    "航点 AI 识别：窗口 {} 已失效，丢弃数量告警",
                    payload.window_id
                );
                return;
            }
            (
                window.cfg.clone(),
                window.alerted.clone(),
                window.cfg.expect_count,
                window.cfg.check_type.clone(),
            )
        };
        if check_type != "count" {
            // 存在性任务不判数量：脏数据/旧客户端误报一律丢弃
            debug!("航点 AI 识别：本窗口是存在性任务，忽略数量告警");
            return;
        }
        let Some(expect) = payload.expect_count.filter(|value| *value > 0).or(expect) else {
            // 没配「检查目标数量」却收到告警：按脏数据丢弃（不猜数量）
            warn!("航点 AI 识别：收到数量告警但本窗口没配检查数量，忽略: {payload:?}");
            return;
        };
        if alerted.swap(true, Ordering::Relaxed) {
            debug!("航点 AI 识别：本窗口已报过数量告警，忽略重复上报");
            return;
        }
        let found = payload.found_count.unwrap_or(0).max(0);
        // 客户端随告警带上来的证据图（base64）；校验不过就只发卡片
        let image = payload.evidence_image.as_deref().and_then(|raw| {
            match crate::notify::evidence_image(raw) {
                Ok(image) => Some(image),
                Err(error) => {
                    warn!("AI 识别：客户端上报的证据图不可用，只发卡片: {error}");
                    None
                }
            }
        });
        record_count_alert(state, &cfg, found, expect, &payload.evidence, image).await;
    }

    /// 客户端转发的「数量核对一致」（`ai_count`）：**只推企业微信回执**。
    ///
    /// 数量任务默认只有「不一致」才出声（[`record_count_alert`]）；有的现场想把
    /// 「这轮核对上了」也留在群里，动作里勾了 `notifyWhenCountOk` 才走到这里。
    /// 这类消息不是告警：不落 `alerts`、不派单、不建工单（见 [`crate::notify::count_matched`]）。
    pub async fn on_client_count(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiCountPayload,
    ) {
        let (cfg, alerted, expect) = {
            let guard = self.active.lock().await;
            let Some(window) = guard.as_ref() else {
                // 板端 `plan_hit` 可能在窗口收尾（`ai_stop`）之后才落盘：那时没有活动窗口，
                // 没有告警要补图，也没有任务上下文，只能丢（下面留一行 debug 可查）
                debug!("航点 AI 识别：没有活动窗口，丢弃数量一致通知");
                return;
            };
            if window.device_code != device_code
                || (!window.window_id.is_empty() && window.window_id != payload.window_id)
            {
                debug!(
                    "航点 AI 识别：窗口 {} 已失效，丢弃数量一致通知",
                    payload.window_id
                );
                return;
            }
            (
                window.cfg.clone(),
                window.alerted.clone(),
                window.cfg.expect_count,
            )
        };
        if cfg.check_type != "count" {
            // 存在性任务没有「数量一致」这种结论：脏数据 / 旧客户端误报一律丢弃
            debug!("航点 AI 识别：本窗口是存在性任务，忽略数量一致通知");
            return;
        }
        if !cfg.notify_when_count_ok {
            // 动作里没勾「数量一致也通知」：客户端本不该发（新客户端会按 `ai_start` 自己门控）
            debug!("航点 AI 识别：本窗口未开启「数量一致也通知」，忽略 ai_count");
            return;
        }
        let Some(expect) = payload.expect_count.filter(|value| *value > 0).or(expect) else {
            warn!("航点 AI 识别：收到数量一致通知但本窗口没配检查数量，忽略: {payload:?}");
            return;
        };
        // 只认「确实对上了」的结论：不一致（或没带检出数）走 `ai_alert` 那条路，
        // 这里**不能**拿 expect 当成 found 凑一个一致出来
        let Some(found) = payload.found_count.filter(|value| *value >= 0) else {
            warn!("航点 AI 识别：数量一致通知没带 foundCount，忽略: {payload:?}");
            return;
        };
        if found != expect {
            debug!(
                "航点 AI 识别：数量一致通知的检出色数 {} ≠ 检查数量 {expect}，忽略（不一致走 ai_alert）",
                found
            );
            return;
        }
        // 与数量告警共用「本窗口结论已上报」标志：`plan_hit` 只发一次，抖动重发也合并
        if alerted.swap(true, Ordering::Relaxed) {
            debug!("航点 AI 识别：本窗口结论已上报过，忽略重复的数量一致通知");
            return;
        }
        let image = payload.evidence_image.as_deref().and_then(|raw| {
            match crate::notify::evidence_image(raw) {
                Ok(image) => Some(image),
                Err(error) => {
                    warn!("AI 识别：数量一致通知的证据图不可用，只发卡片: {error}");
                    None
                }
            }
        });
        record_count_ok(state, &cfg, found, expect, &payload.evidence, image).await;
    }

    /// 客户端补发的命中帧（`ai_evidence`）：告警卡片早就发过，这里**只补一张图**。
    ///
    /// 什么时候会走这条路：板端 `_plan_frame` 要连采 3 帧命中才画框存证，命中帧稀疏时
    /// 采样一直到窗口收尾（客户端 POST `/api/detect/stop` → `_plan_flush`）才结束，
    /// 那时客户端已经用 8s 兜底把卡片发出去了。停识别后图才落盘，所以单独补报。
    ///
    /// 归到哪条告警：先找还开着的窗口，再找刚关掉的（`recent`）——平台「已离开航点」
    /// 关窗在前、客户端扫盘上报在后，所以**后者才是常态**；两边都按「本窗口里还没
    /// 带过图的那条检出」定位，已带过就不重复发。
    pub async fn on_client_evidence(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiEvidencePayload,
    ) {
        let Some(image) = payload.evidence_image.as_deref().and_then(|raw| {
            match crate::notify::evidence_image(raw) {
                Ok(image) => Some(image),
                Err(error) => {
                    warn!("AI 识别：补发的证据图不可用，跳过: {error}");
                    None
                }
            }
        }) else {
            return;
        };
        let Some(alert_code) = self
            .claim_alert_for_evidence(device_code, &payload.window_id)
            .await
        else {
            // 窗口既不在 `active` 也不在 `recent`，或者本窗口的检出都带过图：
            // 前者是真丢图（现场只看得到「没图」），用 info 留一条线索
            info!(
                "航点 AI 识别：窗口 {} 的检出都带过图（或窗口已过期），忽略补发的证据图",
                payload.window_id
            );
            return;
        };
        info!(
            "航点 AI 识别：补发命中帧证据图（{} 字节，告警 {alert_code}，任务 {} 航点 {:?}，证据：{}）",
            image.bytes,
            payload.task_code,
            payload.point_index,
            payload.evidence.trim()
        );
        crate::notify::alert_evidence(&state.notify(), &alert_code, image).await;
    }
}
