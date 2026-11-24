//! 进程内可变缓存（重启即失效的软状态）。
//!
//! 这些以前是各自模块里的 `static`：集成测试跑在**同一个进程**里，于是会互相
//! 读到上一个用例留下的值（`MEDIA_CONFIG_CACHE` 甚至没有按中台地址分键，
//! 换了假中台端口照样命中）。只能靠「用独立任务号 / 别并发」的注释兜着。
//!
//! 现在挂在 [`crate::app::AppState`] 上：每个 `test_state()` 一份，
//! 每个进程一份 —— 这也符合它们的本质。
//!
//! 注意**不是**业务状态：丢了只会多查一次库 / 多登录一次中台，不影响正确性；
//! 所以允许失败时沿用旧值（见各自模块的注释）。

/// 进程内可变缓存的集合。
///
/// 新加缓存请加在这里，而不是再写一个模块级 `static`：`AppState` 的字段
/// 每加一次，20 多处构造点都要跟着改。
#[derive(Default)]
pub struct Caches {
    /// 中台媒体采集配置（60s TTL，按中台地址分键）
    pub media_config: crate::zentai::MediaConfigCache,
    /// 计划「到点未下发」事件的计次与节流
    pub wait_states: crate::schedule::WaitStates,
    /// 地图状态从中台拉回的节流（`GET /api/maps` 顺手拉一次）
    pub map_status_sync: MapStatusSync,
}

/// 让「中台发布后立刻能在业务平台看到已发布」不用等 30s 的调度扫描。
///
/// 只做节流，不存状态：即使进程重启丢了，也只是多拉一次（幂等）。
#[derive(Default)]
pub struct MapStatusSync(std::sync::Mutex<Option<std::time::Instant>>);

impl MapStatusSync {
    /// 距上次同步是否已经超过 `interval`；返回 `true` 时调用方负责去做同步
    pub fn due(&self, interval: std::time::Duration) -> bool {
        // 锁中毒（其它线程 panic）时保守地不同步，不能让一次地图列表请求 500
        let Ok(mut last) = self.0.lock() else {
            return false;
        };
        let now = std::time::Instant::now();
        match *last {
            Some(at) if now.duration_since(at) < interval => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}
