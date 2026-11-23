//! 板端存证目录：证据图读取、`ai_cleanup` 删点、任务结束删计划目录。

use super::*;

impl AiPatrol {
    /// 存证根目录（`detect_jobs`）：显式配置优先，否则试内置默认位置并记住能用的那个。
    pub(crate) async fn jobs_root(&self) -> Option<String> {
        if let Some(dir) = self.jobs_dir.lock().await.clone() {
            return Some(dir);
        }
        for candidate in DEFAULT_JOBS_DIRS {
            if tokio::fs::metadata(candidate).await.is_ok() {
                *self.jobs_dir.lock().await = Some(candidate.to_string());
                debug!("航点 AI 识别：命中帧存证目录 {candidate}");
                return Some(candidate.to_string());
            }
        }
        None
    }

    /// 板端 `plan_hit.evidence` 反推出的真实存证根：比猜默认路径可靠，学到就钉住。
    pub(crate) async fn learn_jobs_root(&self, root: String) {
        if self.jobs_dir_pinned {
            return;
        }
        let mut guard = self.jobs_dir.lock().await;
        if guard.as_deref() != Some(root.as_str()) {
            debug!("航点 AI 识别：从板端事件学到存证目录 {root}");
            *guard = Some(root);
        }
    }

    /// 平台把证据图推进企业微信群之后，删掉板端这一轮的存证
    /// （`detect_jobs/<计划>/pt<航点>/` 里的画框 JPG + `result.json`）。
    ///
    /// 为什么要删：板端每命中一次写一张图，巡检一天几十轮，板子存储很快被图片吃满；
    /// 而图进了群之后没有第二次用途（平台读不到板端磁盘）。
    ///
    /// 窗口还开着就先记账（[`AiPatrol::pending_cleanup`]）：那时板端可能还在写图，
    /// 客户端收尾时也要再扫一次存证目录补图（`ai_evidence`），删早了会把还没推出去的
    /// 那张图毁掉。窗口收尾后由 [`AiPatrol::finish`] 统一删。
    pub(crate) async fn cleanup_evidence(&self, plan_id: &str, point_index: i64, reason: &str) {
        let plan_id = plan_id.trim();
        if !safe_plan_id(plan_id) || point_index < 0 {
            debug!(
                "航点 AI 识别：存证清理载荷不合法（plan={plan_id:?} point={point_index}），忽略"
            );
            return;
        }
        {
            let guard = self.window.lock().await;
            if let Some(window) = guard.as_ref()
                && window.plan_id == plan_id
                && window.point_index == point_index
            {
                debug!(
                    "航点 AI 识别：窗口还没收尾，存证 {plan_id}/pt{point_index} 等收尾后再删（{reason}）"
                );
                self.pending_cleanup
                    .lock()
                    .await
                    .push((plan_id.to_string(), point_index));
                return;
            }
        }
        self.remove_point_dir(plan_id, point_index, reason).await;
    }

    /// 真删 `<存证根>/<计划>/pt<n>/`（图和 `result.json` 都在这一层）。
    pub(crate) async fn remove_point_dir(&self, plan_id: &str, point_index: i64, reason: &str) {
        let Some(root) = self.jobs_root().await else {
            debug!("航点 AI 识别：找不到存证根目录，跳过清理 {plan_id}/pt{point_index}");
            return;
        };
        let Some(dir) = point_dir(&root, plan_id, point_index) else {
            return;
        };
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => info!("航点 AI 识别：已清理板端存证 {}（{reason}）", dir.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                debug!("航点 AI 识别：板端存证已经不在了（{}）", dir.display())
            }
            Err(error) => warn!(
                "航点 AI 识别：清理板端存证失败（{}）: {error}",
                dir.display()
            ),
        }
        // 计划目录空了就顺手删掉，别在 detect_jobs 下留一堆空壳（非空时 remove_dir 会失败）
        let _ = tokio::fs::remove_dir(Path::new(&root).join(plan_id.trim())).await;
    }

    /// 计划（任务）跑完了（终态 / 取消）：把板端**整个计划目录**删掉。
    ///
    /// 为什么不能只靠 [`AiPatrol::cleanup_evidence`]：那条是「这一张证据图已经进群，
    /// 删这个航点」，只有真的推过图的航点才有。数量任务的检出只写任务时间线（不建单
    /// 不推群）、一个都没命中的航点只留一份 `result.json`、图被企业微信拒收 / 设备当时
    /// 不在线——这些都等不到 `ai_cleanup`，`detect_jobs/<计划>/` 于是长期堆在板子上。
    /// 而任务都跑完了，这些图已经没有任何用途（图只在板端、平台读不到）。
    ///
    /// 窗口还在收尾时不能当场删（收尾要停识别、板端 `_plan_flush` 落盘、再扫盘补图，
    /// 删早了会把还没推出去的那张图毁掉），所以先记账、起个短任务等它跑完再删。
    pub(crate) async fn finish_plan(self: &Arc<Self>, plan_id: &str, reason: &str) {
        let plan_id = plan_id.trim();
        if !safe_plan_id(plan_id) {
            debug!("航点 AI 识别：计划 id 不合法（{plan_id:?}），跳过存证清理");
            return;
        }
        {
            let mut ending = self.ending_plans.lock().await;
            if !ending.iter().any(|plan| plan == plan_id) {
                ending.push(plan_id.to_string());
            }
        }
        if self.plan_window_active(plan_id).await {
            debug!("航点 AI 识别：计划 {plan_id} 的窗口还在收尾，存证等它跑完再删（{reason}）");
            let this = self.clone();
            let plan_id = plan_id.to_string();
            let reason = reason.to_string();
            tokio::spawn(async move { this.reap_plan_dir(plan_id, reason).await });
            return;
        }
        self.ending_plans
            .lock()
            .await
            .retain(|plan| plan != plan_id);
        self.remove_plan_dir(plan_id, reason).await;
    }

    /// 计划目录是不是正被某个窗口用着（`pt<n>/` 还在写、还要扫图）
    pub(crate) async fn plan_window_active(&self, plan_id: &str) -> bool {
        self.window
            .lock()
            .await
            .as_ref()
            .map(|window| window.plan_id == plan_id)
            .unwrap_or(false)
    }

    /// 等这一轮窗口收尾，再删整个计划目录（[`AiPatrol::finish_plan`] 的延迟分支）。
    pub(crate) async fn reap_plan_dir(&self, plan_id: String, reason: String) {
        for _ in 0..PLAN_CLEANUP_ATTEMPTS {
            // 新一轮同名计划开始了：这次跑出来的存证不能删
            if !self
                .ending_plans
                .lock()
                .await
                .iter()
                .any(|plan| plan == &plan_id)
            {
                return;
            }
            if !self.plan_window_active(&plan_id).await {
                break;
            }
            tokio::time::sleep(PLAN_CLEANUP_RETRY).await;
        }
        if self.plan_window_active(&plan_id).await {
            warn!("航点 AI 识别：计划 {plan_id} 的窗口迟迟不收尾，存证先留着（下次启动兜底清理）");
            return;
        }
        self.ending_plans
            .lock()
            .await
            .retain(|plan| plan != &plan_id);
        self.remove_plan_dir(&plan_id, &reason).await;
    }

    /// 真删 `<存证根>/<计划>/`（整个计划，含从没推过图的航点）
    pub(crate) async fn remove_plan_dir(&self, plan_id: &str, reason: &str) {
        if !safe_plan_id(plan_id) {
            return;
        }
        let Some(root) = self.jobs_root().await else {
            debug!("航点 AI 识别：找不到存证根目录，跳过清理计划 {plan_id}");
            return;
        };
        let dir = Path::new(&root).join(plan_id.trim());
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => info!(
                "航点 AI 识别：已清理板端计划存证 {}（{reason}）",
                dir.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                debug!("航点 AI 识别：板端计划存证已经不在了（{}）", dir.display())
            }
            Err(error) => warn!(
                "航点 AI 识别：清理板端计划存证失败（{}）: {error}",
                dir.display()
            ),
        }
    }

    /// 客户端启动时的兜底清理：`detect_jobs` 下超过 [`STALE_PLAN_AGE`] 没动过的
    /// 目录直接删掉（返回删了几个）。
    ///
    /// 正常路径是 [`AiPatrol::finish_plan`] 在任务结束时清；这里兜的是「那次收尾没
    /// 跑到」——客户端中途被 kill / 断电 / 还是老版本（只删推过图的航点），板子上的
    /// `detect_jobs` 只增不减，很快就会把盘写满。
    pub(crate) async fn sweep_stale_plans(&self) -> usize {
        self.sweep_plans_older_than(STALE_PLAN_AGE).await
    }

    /// 按年龄清残留（`age` 只是给单测用的入口）
    pub(crate) async fn sweep_plans_older_than(&self, age: Duration) -> usize {
        let Some(root) = self.jobs_root().await else {
            return 0;
        };
        let now = SystemTime::now();
        let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
            return 0;
        };
        let mut removed = 0usize;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !entry
                .file_type()
                .await
                .map(|kind| kind.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !safe_plan_id(&name) {
                continue;
            }
            // 正在跑 / 刚跑完还没收尾的计划不能动
            if self.plan_window_active(&name).await
                || self
                    .ending_plans
                    .lock()
                    .await
                    .iter()
                    .any(|plan| plan == &name)
            {
                continue;
            }
            let Some(last_touch) = dir_last_touch(&path).await else {
                continue;
            };
            if now.duration_since(last_touch).unwrap_or_default() < age {
                continue;
            }
            match tokio::fs::remove_dir_all(&path).await {
                Ok(()) => {
                    removed += 1;
                    info!("航点 AI 识别：清理残留的计划存证 {}", path.display());
                }
                Err(error) => warn!(
                    "航点 AI 识别：清理残留的计划存证失败（{}）: {error}",
                    path.display()
                ),
            }
        }
        if removed > 0 {
            info!("航点 AI 识别：启动兜底清理了 {removed} 个残留的计划存证目录");
        }
        removed
    }
}
