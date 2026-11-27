//! 断链时**没送出去**的「取消本体任务」（`5002`）登记簿。
//!
//! 现场 2026-11-27 `TK-1127-22`：17:14:03 遥控器（`client_id=4`）抢走 RoamerX 控制权，
//! 本体随即把我们踢下线，重连后**换了一个新的 `ZhProClient`**；而 17:12 那条还在跑的
//! `RouteRunner` 握着旧 client（通道永久关闭），它的 `5002` 只能报
//! `取消导航任务失败: control 通道已关闭`。结果：平台 17:18:46 判「巡检任务超时」，
//! 狗到 17:22:23 还在跑那条任务（最后是新 runner 也超时才把它取消掉）。
//!
//! 所以取消失败**不能只打一条 warn**：把本体任务号记在这里，等链路重建
//! （`body::finish_relink`）后由 `zhpro::route::flush_pending_cancels` 补发。
//!
//! 两条边界：
//! - **按任务号补发，不按「当前任务」补发**：本体任务号是 UUID、不会复用，补一条早就不在
//!   的只会拿到 `13896 task not found`，绝不会误伤重连后新下发的任务；
//! - 登记簿只在内存里（进程重启就没了）：那时旧 `5002` 最多让狗多跑一小会儿，
//!   重启本身已经打断了本地 runner，狗体上的残留任务由人工「取消」处理。
//!
//! 客户端一台机器只连一个本体，所以进程内一份就够，不需要挂到 `BodyContext` 上
//! （`RouteRunner` 拿不到 `BodyContext`，挂上去就得为它新开一条穿透到 `route.rs` 的引用）。

use std::sync::OnceLock;
use tokio::sync::Mutex;
use tracing::warn;

/// 最多记这么多条：现场不会有几十条任务同时断链，超过就是有东西在刷错误
const PENDING_LIMIT: usize = 32;

/// 进程内唯一一份
fn pending() -> &'static Mutex<Vec<String>> {
    static PENDING: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

/// 登记一条「重连后要补发 5002」的本体任务号（重复登记自动去重）
pub async fn remember(task_id: &str) {
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return;
    }
    let mut guard = pending().lock().await;
    if guard.iter().any(|id| id == task_id) {
        return;
    }
    if guard.len() >= PENDING_LIMIT {
        // 极端情况下丢最老的一条：宁可少补一次，也不要无界增长
        let dropped = guard.remove(0);
        warn!("待补发 5002 的任务号已满 {PENDING_LIMIT} 条，丢弃最老的 {dropped}");
    }
    guard.push(task_id.to_string());
}

/// 取走全部待补发的任务号（**取走即清空**：补发失败的一方负责重新登记）
pub async fn take_all() -> Vec<String> {
    std::mem::take(&mut *pending().lock().await)
}

/// 还有多少条等着补发
pub async fn len() -> usize {
    pending().lock().await.len()
}

/// 测试用：清空登记簿（全局单例，用例之间要隔离）
#[cfg(test)]
pub async fn reset() {
    pending().lock().await.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局登记簿只有一个用例碰它，所以不需要跨用例串行：
    /// 重复登记要去重、`take_all` 取走即清空、空任务号不收。
    #[tokio::test]
    async fn remember_dedups_and_take_all_drains() {
        reset().await;
        remember("task-a").await;
        remember("task-a").await;
        remember("  ").await;
        assert_eq!(
            len().await,
            1,
            "重复 / 空任务号不该多记：{:?}",
            take_all().await
        );

        let drained = take_all().await;
        assert_eq!(drained, vec!["task-a".to_string()]);
        assert_eq!(len().await, 0, "take_all 取走即清空");
        assert!(take_all().await.is_empty());
    }
}
