//! 识别窗口：启停板端 Agent、消费 SSE、关窗收尾（含晚到的存证图）。

use super::*;

impl AiPatrol {
    /// 窗口主流程：启停 Agent → 收 SSE → 逐类别上报 → 收尾
    pub(crate) async fn run_window(
        self: Arc<Self>,
        out_tx: mpsc::Sender<WsMessage>,
        start: AiStart,
        cancel: Arc<Notify>,
    ) {
        let body = json!({
            "task": start.task,
            "labels": if start.labels.is_empty() { Value::Null } else { json!(start.labels) },
            "conf": start.conf,
            "cams": [start.cam],
            "fps": start.fps,
            // 航点计划：板端据此把证据图 + result.json 落到
            // detect_jobs/<plan_id>/pt<航点下标>/，并按 expect_count 核对数量
            "plan_id": start.plan_id,
            "point_index": start.point_index,
            "check_type": start.check_type,
            "expect_count": start.expect_count,
            // 中台参数配置里的板端媒体目录 / 固定帧数（缺省由板端 config.py 兜底）
            "media_dir": start.media_dir,
            "save_frames": start.media_frames,
            // 航点自动采集总开关（中台 `media_capture_enabled`，默认关；
            // 老平台没下发时不传，板端按老行为——即「开」）
            "capture_enabled": start.media_capture,
        });
        let path = if start.action == "config" {
            "/api/detect/config"
        } else {
            "/api/detect/start"
        };
        // 证据图的「新鲜度」基准就是本轮窗口开始（板端写完图再发 SSE，留余量）
        let since = SystemTime::now() - EVIDENCE_MTIME_SLACK;
        match self.post_json(path, &body).await {
            Ok(_) => info!(
                "航点 AI 识别开始：场景={} 类别={} 阈值={:.2} 相机={} 窗口={}s",
                start.task,
                if start.labels.is_empty() {
                    "全部".to_string()
                } else {
                    start.labels.join("/")
                },
                start.conf,
                start.cam,
                start.secs
            ),
            Err(error) => {
                warn!(
                    "航点 AI 识别：启动失败（场景 {}，{path}）: {error}",
                    start.task
                );
                // 启动就失败：窗口没真正开起来，别让「未命中」被算成数量不一致
                self.finish(
                    &out_tx,
                    &start,
                    &format!("启动失败: {error}"),
                    0,
                    false,
                    Leftover::default(),
                    since,
                )
                .await;
                return;
            }
        }

        let (hits, stopped_early, leftover) = self.consume(&out_tx, &start, &cancel, since).await;
        let reason = if stopped_early {
            "任务结束 / 被新航点顶替".to_string()
        } else {
            format!("窗口到期（{}s）", start.secs)
        };
        // `expired = true` 只代表「窗口正常到期」：数量任务这时一个都没找到，
        // 服务端才把它算成数量不一致（0 ≠ N）；被任务结束/新航点打断不算账。
        // （平台另外还会把「停满 dwell 后离开」当看满——窗口是 dwell+8，狗停到 dwell
        // 就走，只认到期会让「没找到也告警」永远报不出来，见 server/src/ai_patrol.rs。）
        self.finish(
            &out_tx,
            &start,
            &reason,
            hits,
            !stopped_early,
            leftover,
            since,
        )
        .await;
    }

    /// 读 SSE，把超过阈值的类别按「本窗口首次出现」上报。
    ///
    /// 返回（上报类别数, 是否被提前打断, 收尾要补的活）。
    ///
    /// `leftover` 交给 [`Self::finish`] 停掉板端后再补：板端 flush 计划时才把采样中的
    /// 命中落盘，那时才拿得到图——挂起的补上卡片，已经发过卡片的单独补一张图（`ai_evidence`）。
    pub(crate) async fn consume(
        &self,
        out_tx: &mpsc::Sender<WsMessage>,
        start: &AiStart,
        cancel: &Arc<Notify>,
        since: SystemTime,
    ) -> (usize, bool, Leftover) {
        let response = self
            .stream
            .get(format!("{}/api/events", self.agent))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await;
        let mut response = match response {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                warn!("航点 AI 识别：Agent 拒绝事件流 HTTP {}", response.status());
                return (0, false, Leftover::default());
            }
            Err(error) => {
                warn!("航点 AI 识别：连不上 Agent 事件流: {error}");
                return (0, false, Leftover::default());
            }
        };

        let notified = cancel.notified();
        tokio::pin!(notified);
        let deadline = Instant::now() + Duration::from_secs(start.secs.max(1));
        let mut buffer = String::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut hits = 0usize;
        // 数量核对告警每个窗口只上报一条（板端 `plan_hit` 也只发一次，双保险）
        let alerted = AtomicBool::new(false);
        // 等着带上证据图的检出（`label`, 置信度），见 [`EVIDENCE_PENDING_WAIT`] 的注释
        let mut pending: Vec<(String, f64)> = Vec::new();
        // 本窗口已拿到的命中帧（板端路径, base64）：同一窗口后来再命中别的类别直接复用
        let mut evidence: Option<(String, String)> = None;
        // 挂起检出的兜底上报时刻：一直等不到证据图也要把告警发出去
        let mut pending_deadline: Option<Instant> = None;
        // 有没有「卡片已经发出去、但没带上图」的检出：窗口收尾时补一张图（`ai_evidence`）
        let mut image_missing = false;

        let stopped_early = loop {
            let flush_at = pending_deadline.unwrap_or(deadline);
            tokio::select! {
                _ = &mut notified => break true,
                _ = tokio::time::sleep_until(deadline) => break false,
                // 兜底：板端迟迟不给证据图（老板端 / 画框失败）就先发卡片保告警
                _ = tokio::time::sleep_until(flush_at) => {
                    if pending.is_empty() {
                        // `flush_at` 就是窗口到期时刻，与上面那个分支同义
                        break false;
                    }
                    debug!(
                        "航点 AI 识别：{}s 内没等到证据图，先上报 {} 类检出",
                        EVIDENCE_PENDING_WAIT.as_secs(),
                        pending.len()
                    );
                    if let Some(found) = self.window_evidence(start, since, 1).await {
                        evidence = Some(found);
                    }
                    image_missing |= evidence.is_none();
                    for (label, conf) in std::mem::take(&mut pending) {
                        send_detect(out_tx, start, &label, conf, evidence.as_ref()).await;
                    }
                    pending_deadline = None;
                }
                chunk = response.chunk() => {
                    match chunk {
                        Ok(Some(bytes)) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));
                            if buffer.len() > SSE_BUF_LIMIT {
                                warn!("航点 AI 识别：SSE 缓冲超过上限，丢弃重来");
                                buffer.clear();
                                continue;
                            }
                            for payload in take_sse_events(&mut buffer) {
                                let Ok(event) = serde_json::from_str::<Value>(&payload) else {
                                    continue;
                                };
                                // 板端已判定「数量与检查目标数量不一致」：转发成 ai_alert
                                if let Some(mut alert) = count_alert(&event, start)
                                    && !alerted.swap(true, Ordering::Relaxed)
                                {
                                    let expect =
                                        alert.get("expectCount").and_then(|v| v.as_i64()).unwrap_or(0);
                                    let found =
                                        alert.get("foundCount").and_then(|v| v.as_i64()).unwrap_or(0);
                                    // 证据图只存在于板端磁盘，平台拿不到（Agent 没有取文件接口），
                                    // 只有本客户端能读。企业微信图片消息要的是字节不是路径，
                                    // 所以读出来随告警一起上报，由平台校验后发 msgtype=image。
                                    if let Some(image) = evidence_image_base64(&alert).await {
                                        alert["evidenceImage"] = json!(image);
                                    }
                                    warn!(
                                        "航点 AI 识别：数量核对不一致（检查 {expect} / 检出 {found}），上报告警"
                                    );
                                    send(out_tx, "ai_alert", alert).await;
                                }
                                // 板端已判定「数量与检查目标数量一致」：动作勾了
                                // `notifyWhenCountOk` 时转发成 `ai_count`。平台只发一条
                                // 企业微信回执，**不落告警、不建工单**（正常结论不该派单），
                                // 所以不能走 `ai_alert` 那条路。
                                if let Some(mut counted) = count_matched(&event, start)
                                    && !alerted.swap(true, Ordering::Relaxed)
                                {
                                    let expect = counted
                                        .get("expectCount")
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or(0);
                                    let found = counted
                                        .get("foundCount")
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or(0);
                                    // 证据图同告警：「一致」的回执带一张图才看得清是哪几个
                                    if let Some(image) = evidence_image_base64(&counted).await {
                                        counted["evidenceImage"] = json!(image);
                                    }
                                    info!(
                                        "航点 AI 识别：数量核对一致（检查 {expect} / 检出 {found}），上报通知"
                                    );
                                    send(out_tx, "ai_count", counted).await;
                                }
                                // 板端 `plan_hit` 里的证据图路径能反推出存证根目录：记下来，
                                // 下一轮（甚至本轮后续航点）扫目录时就不用猜 `detect_jobs` 在哪
                                if let Some(root) = jobs_root_from_evidence(&event) {
                                    self.learn_jobs_root(root).await;
                                }
                                // 板端 `plan_hit` 才是「命中帧已画框落盘」的信号（检出那一刻
                                // 还在采样）：拿它把挂起的检出补上图再上报。路径优先用事件里
                                // 的 `evidence`，读不到（bind-mount / 换了盘符）就退回扫存证目录。
                                if event.get("type").and_then(Value::as_str) == Some("plan_hit")
                                    && !pending.is_empty()
                                {
                                    evidence = match plan_hit_evidence(&event).await {
                                        Some(found) => Some(found),
                                        None => self.window_evidence(start, since, 4).await,
                                    };
                                    if evidence.is_none() {
                                        debug!(
                                            "航点 AI 识别：plan_hit 没有可读的证据图，{} 类检出只发卡片",
                                            pending.len()
                                        );
                                        image_missing = true;
                                    }
                                    for (label, conf) in std::mem::take(&mut pending) {
                                        send_detect(out_tx, start, &label, conf, evidence.as_ref())
                                            .await;
                                    }
                                    pending_deadline = None;
                                }
                                for (label, conf) in take_hits(&event, start.conf, &mut seen) {
                                    hits += 1;
                                    debug!("航点 AI 识别：检出 {label}（{conf:.2}）");
                                    // 证据图这时通常还没落盘（见 [`EVIDENCE_PENDING_WAIT`]）。
                                    // 先探一次盘（`plan_hit` 后来的那些命中，图已经在盘上），
                                    // 探不到就先挂起等 `plan_hit`——群里那条卡片晚几秒，
                                    // 但后面跟得上证据图。
                                    if evidence.is_none() {
                                        evidence = self.window_evidence(start, since, 1).await;
                                    }
                                    if let Some(found) = evidence.as_ref() {
                                        send_detect(out_tx, start, &label, conf, Some(found)).await;
                                        continue;
                                    }
                                    pending.push((label, conf));
                                    pending_deadline.get_or_insert_with(|| {
                                        Instant::now() + EVIDENCE_PENDING_WAIT
                                    });
                                }
                            }
                        }
                        Ok(None) => break false,
                        Err(error) => {
                            warn!("航点 AI 识别：事件流中断: {error}");
                            break false;
                        }
                    }
                }
            }
        };
        (
            hits,
            stopped_early,
            Leftover {
                pending,
                image_missing,
            },
        )
    }

    /// 收尾：按需停 Agent →（补发挂起的检出 / 补一张证据图）→ 上报 `ai_end` → 释放窗口槽位
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn finish(
        &self,
        out_tx: &mpsc::Sender<WsMessage>,
        start: &AiStart,
        reason: &str,
        hits: usize,
        expired: bool,
        leftover: Leftover,
        since: SystemTime,
    ) {
        // 与平台侧 `replaced_by_other` 同一语义：窗口已被**新窗口**顶替时不停 Agent
        // （新窗口可能继承着操作员手动开的识别）；被 stop()/任务终态摘掉时窗口为空，
        // 这时 `replaced = false`，必须把 Agent 停掉，否则 NPU 一直占着。
        let replaced = {
            let guard = self.window.lock().await;
            guard
                .as_ref()
                .map(|window| window.id != start.window_id)
                .unwrap_or(false)
        };
        if start.was_running {
            info!("航点 AI 识别窗口结束（保留操作员已开启的识别）：{reason}");
            // 识别留着，但计划必须摘掉：否则之后画面再命中会继续写这个航点的
            // 存证目录、并可能重复上报数量告警。
            if !replaced
                && let Err(error) = self.post_json("/api/detect/plan/clear", &json!({})).await
            {
                debug!("航点 AI 识别：清除板端计划失败（不影响后续巡检）: {error}");
            }
        } else if !replaced && let Err(error) = self.post_json("/api/detect/stop", &json!({})).await
        {
            warn!("航点 AI 识别：停识别失败: {error}");
        }
        // 到窗口结束还没等到 `plan_hit` 的检出（命中后板端还在采样）：停识别会让板端
        // `_plan_flush` 把采样中的命中画框存证，所以**先停再扫**一次盘，往往能补上图。
        // 停之前扫是白扫——那时图根本还没写。
        if !leftover.pending.is_empty() || leftover.image_missing {
            let evidence = self.window_evidence(start, since, 4).await;
            // 已经发过卡片的（`image_missing`）由后面单独补一张图，所以这里补发的卡片
            // 就不再带图了——否则同一窗口会在群里冒出两张一样的命中帧。
            let attach = if leftover.image_missing {
                None
            } else {
                evidence.as_ref()
            };
            if !leftover.pending.is_empty() && attach.is_none() {
                debug!(
                    "航点 AI 识别：窗口收尾时仍没有证据图，{} 类检出只发卡片",
                    leftover.pending.len()
                );
            }
            for (label, conf) in leftover.pending {
                send_detect(out_tx, start, &label, conf, attach).await;
            }
            if leftover.image_missing {
                match evidence {
                    Some((path, image)) => {
                        debug!("航点 AI 识别：卡片已发、图才落盘，单独补一张证据图（{path}）");
                        send_evidence(out_tx, start, &path, &image).await;
                    }
                    None => debug!("航点 AI 识别：窗口收尾时仍没有证据图，不再补图"),
                }
            }
        }
        info!(
            "航点 AI 识别结束：场景={} {reason}，上报 {hits} 类目标",
            start.task
        );

        send(
            out_tx,
            "ai_end",
            json!({
                "windowId": start.window_id,
                "task": start.task,
                "reason": reason,
                "hits": hits,
                // 窗口是否正常到期：数量任务未命中时服务端据此判「数量不一致」
                "expired": expired,
            }),
        )
        .await;

        if !replaced {
            *self.window.lock().await = None;
        }
        // 平台已经把这些航点的证据图推进群了：窗口收尾（含停识别后的补图扫描）
        // 之后才真删板端存证。还属于「当前活动窗口」的条目留着，不能提前删。
        let active = self
            .window
            .lock()
            .await
            .as_ref()
            .map(|window| (window.plan_id.clone(), window.point_index));
        let (ready, keep): (Vec<CleanupTarget>, Vec<CleanupTarget>) = {
            let mut pending = self.pending_cleanup.lock().await;
            std::mem::take(&mut *pending)
                .into_iter()
                .partition(|item| active.as_ref() != Some(item))
        };
        if !keep.is_empty() {
            *self.pending_cleanup.lock().await = keep;
        }
        for (plan_id, point_index) in ready {
            self.remove_point_dir(&plan_id, point_index, "窗口收尾")
                .await;
        }
    }

    /// 本轮窗口的一张命中帧：（板端路径, base64）。
    ///
    /// 这是「拿不到 `plan_hit` 的 path 时」的退路：扫板端存证目录
    /// `<存证根>/<计划 id>/pt<航点下标>/`，取「窗口开始之后写下的」最新一张。
    /// `attempts` 是重试次数（每次间隔 300ms）：窗口收尾时给几次就够，兜底上报可以只给 1。
    /// 失败一律 `None`：**不带图不能影响告警**。
    pub(crate) async fn window_evidence(
        &self,
        start: &AiStart,
        since: SystemTime,
        attempts: usize,
    ) -> Option<(String, String)> {
        let plan = start.plan_id.trim();
        if plan.is_empty() || attempts == 0 {
            return None;
        }
        for attempt in 0..attempts {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            let root = self.jobs_root().await?;
            let dir = Path::new(&root)
                .join(plan)
                .join(format!("pt{}", start.point_index));
            let Some(path) = newest_evidence_file(&dir, since).await else {
                continue;
            };
            if let Some(image) = evidence_image_from_path(&path).await {
                return Some((path, image));
            }
        }
        None
    }
}

/// 板端 `plan_hit` 事件 → `ai_alert` 载荷。
///
/// 只有 `countAlert` 为真时才转发（板端已经把「没配检查数量」的情况过滤掉了，
/// 这里再判一次，免得将来事件字段改动就把普通命中当成告警刷上去）。
pub(crate) fn count_alert(event: &Value, start: &AiStart) -> Option<Value> {
    if event.get("type").and_then(Value::as_str) != Some("plan_hit") {
        return None;
    }
    if event.get("count_alert").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    Some(json!({
        "windowId": start.window_id,
        "taskCode": start.task_code,
        "planId": start.plan_id,
        "pointIndex": start.point_index,
        "expectCount": event
            .get("expect_count")
            .and_then(Value::as_i64)
            .or(start.expect_count),
        "foundCount": event.get("found_count").and_then(Value::as_i64).unwrap_or(0),
        "evidence": event.get("evidence").and_then(Value::as_str).unwrap_or_default(),
    }))
}

/// 板端 `plan_hit` 事件 → `ai_count` 载荷（数量**一致**）。
///
/// 只在「数量核对任务 + 动作勾了 `notifyWhenCountOk` + 板端 `count_ok=true`」时转发：
/// 一致是正常结论，默认不打扰群；不一致走 [`count_alert`] → `ai_alert`（会建单）。
/// 平台收到 `ai_count` 后**只推企业微信，不落告警、不建工单**。
pub(crate) fn count_matched(event: &Value, start: &AiStart) -> Option<Value> {
    if start.check_type != "count" || !start.notify_when_count_ok {
        return None;
    }
    if event.get("type").and_then(Value::as_str) != Some("plan_hit") {
        return None;
    }
    if event.get("count_ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let expect = event
        .get("expect_count")
        .and_then(Value::as_i64)
        .or(start.expect_count)?;
    let found = event.get("found_count").and_then(Value::as_i64)?;
    // 双保险：只有真的对上了才当「一致」报，别把不一致的通知成正常
    if found != expect {
        return None;
    }
    Some(json!({
        "windowId": start.window_id,
        "taskCode": start.task_code,
        "planId": start.plan_id,
        "pointIndex": start.point_index,
        "expectCount": expect,
        "foundCount": found,
        "evidence": event.get("evidence").and_then(Value::as_str).unwrap_or_default(),
    }))
}

/* ------------------------------- 证据图 ------------------------------- */
