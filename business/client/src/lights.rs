//! 航点灯光：补光灯 + LED 灯带（板端 Agent 的 `/api/lights`）
//!
//! 为什么在客户端：灯是背包上的硬件，Agent 只在内网可达（正式环境业务平台在公网），
//! 而客户端与 Agent 同机（`127.0.0.1:8000`）。到点判定本来就在本地——5009 的
//! `stage` / `detail_index`，顺手拿来做开灯时机，不额外引入一套状态机。
//!
//! 配置来自两处，**各管各的**：
//!
//! 1. 路线动作（`routeData.points[].actions[]` 的 `lightFill` / `lightLed`）：
//!    到该航点开哪几盏灯。路线编辑器里和「AI 识别」动作并排配；
//! 2. 计划级「仅夜间自动开灯」（下发载荷 `lightNightOnly`，= `tasks.light_night_only`）：
//!    勾上时把 `nightOnly=true` 交给板端，由**板端**按本地时间窗
//!    （`LIGHT_NIGHT_START`/`LIGHT_NIGHT_END`，默认 18:00-06:00）判定白天还是夜间。
//!
//! 为什么夜间判定在板端：平台/客户端与板子可能不在同一时区，甚至平台时钟本身不准；
//! 灯亮了却不知道为什么亮最难查。判据只有一处（`control.is_night`），客户端只传开关。
//!
//! 关灯时机（缺一个就是「灯一直亮着耗电」）：
//! - 离开航点（阶段不再是 `patrol.point_*`）—— 与 AI 窗口同一条收口规则；
//! - 到达一个没有灯光动作的航点（上一个点的灯必须收掉）；
//! - 任务终态 / 取消 / 本地任务收尾；
//! - 板端看门狗（`LIGHT_MAX_ON_SECS`，默认 15 分钟）：客户端掉线 / 被 kill 时兜底。

use reqwest::Client;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// 板端开/关灯只是几次 GPIO 写 + 一次进程启停（同机 HTTP，毫秒级）。给 5s 是为了
/// 板子忙时留余量，又不能让一次卡死把 5009 处理链路拖太久。
const LIGHT_TIMEOUT: Duration = Duration::from_secs(5);

/// 路线里一个航点的灯光配置（只有至少要求开一盏灯的点才进结果）
#[derive(Debug, Clone, PartialEq)]
pub struct LightPoint {
    /// 航点下标（0-based，与 `routeData.points` / 5009 `detail_index` 一致）
    pub index: i64,
    /// 动作 `lightFill`：到点开补光灯
    pub fill: bool,
    /// 动作 `lightLed`：到点开 LED 灯带
    pub led: bool,
}

/// 解析 `routeData` 里所有带灯光动作的航点。
///
/// 同一个航点可能有多个动作（「AI 识别」一个、「补光」一个），按**或**合并：
/// 任意一条动作要求开灯就算这个点要开灯。
pub fn parse_light_points(route_data: &Value) -> Vec<LightPoint> {
    let Some(points) = route_data.get("points").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut merged: BTreeMap<i64, LightPoint> = BTreeMap::new();
    for (index, point) in points.iter().enumerate() {
        let Some(actions) = point.get("actions").and_then(Value::as_array) else {
            continue;
        };
        for action in actions {
            let fill = action
                .get("lightFill")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let led = action
                .get("lightLed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !fill && !led {
                continue;
            }
            let entry = merged.entry(index as i64).or_insert(LightPoint {
                index: index as i64,
                fill: false,
                led: false,
            });
            entry.fill |= fill;
            entry.led |= led;
        }
    }
    merged.into_values().collect()
}

/// 取指定航点的灯光配置
pub fn light_point_at(points: &[LightPoint], index: i64) -> Option<&LightPoint> {
    points.iter().find(|point| point.index == index)
}

/// 航点灯光运行态（进程内唯一）
pub struct PointLights {
    /// 板端 Agent 基地址（与 `ai_patrol` 共用 `[ai] agent_url`）
    agent: String,
    enabled: bool,
    client: Client,
    /// 当前由计划点亮的 (任务号, 航点下标)：去重 + 判断离开时要不要发关灯
    active: Mutex<Option<(String, i64)>>,
    /// 已经为哪个 (任务号, 航点) 发过开灯请求：5009 每 500ms 一帧，不去重会刷屏
    requested: Mutex<Option<(String, i64)>>,
    /// 开关灯串行化：开灯在后台任务里跑（不能阻塞 5009 处理链路），
    /// 如果和紧随其后的「离开航点关灯」交错，会出现「关灯请求先到、开灯后到」→
    /// 灯一直亮到下一个点。这把锁保证两者排好队（`tokio::Mutex` 是 FIFO 的）。
    gate: Mutex<()>,
}

impl PointLights {
    pub fn new(agent: impl Into<String>, enabled: bool) -> Self {
        Self {
            agent: agent.into().trim_end_matches('/').to_string(),
            enabled,
            client: Client::builder()
                .timeout(LIGHT_TIMEOUT)
                .build()
                .unwrap_or_default(),
            active: Mutex::new(None),
            requested: Mutex::new(None),
            gate: Mutex::new(()),
        }
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// 到点开灯（5009 `patrol.point_arrived` / `patrol.waiting`）。
    ///
    /// `night_only` = 计划勾了「仅夜间自动开灯」，交给板端按本地时间窗判定：
    /// 白天的返回值是 `skipped="daytime"`，此时**不记活跃态**（本来就没开灯，
    /// 离开航点时也不需要发关灯）。
    pub async fn point_arrived(&self, task_code: &str, point: &LightPoint, night_only: bool) {
        if !self.enabled || (!point.fill && !point.led) {
            return;
        }
        let _gate = self.gate.lock().await;
        {
            let active = self.active.lock().await;
            if active.as_ref() == Some(&(task_code.to_string(), point.index)) {
                return;
            }
            let mut requested = self.requested.lock().await;
            if requested.as_ref() == Some(&(task_code.to_string(), point.index)) {
                return;
            }
            *requested = Some((task_code.to_string(), point.index));
        }

        let body = json!({
            "fill": point.fill,
            "led": point.led,
            "nightOnly": night_only,
            "taskCode": task_code,
            "pointIndex": point.index,
        });
        match self.post("/api/lights", body).await {
            Ok(value) if value.get("skipped").and_then(Value::as_str) == Some("daytime") => {
                info!(
                    "航点灯光：任务 {task_code} 航点 {} 在白天到达，跳过开灯（计划仅夜间自动开灯）",
                    point.index
                );
                *self.active.lock().await = None;
            }
            Ok(value) => {
                // 硬件不可用（补光灯 GPIO 被禁用 / LED 程序缺失）时 Agent 仍回 200，结论
                // 在 `ok` / `errors` / `applied` 里。只要有一盏真的亮了就记「已开灯」，
                // 离开航点时关回来；一盏都没亮（例如只配了补光而 GPIO 被禁用）就不记，
                // 免得每个航点都白发一条关灯。
                let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(true);
                let applied = value
                    .get("applied")
                    .and_then(Value::as_object)
                    .map(|applied| applied.values().any(|v| v.as_bool() == Some(true)))
                    .unwrap_or(false);
                if ok {
                    info!(
                        "航点灯光：任务 {task_code} 航点 {} 已开灯（补光={} LED={}{}）",
                        point.index,
                        point.fill,
                        point.led,
                        if night_only { "，仅夜间" } else { "" }
                    );
                } else {
                    warn!(
                        "航点灯光：任务 {task_code} 航点 {} 部分失败（{}）；巡检继续",
                        point.index,
                        value
                            .get("errors")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null)
                    );
                }
                *self.active.lock().await =
                    (ok || applied).then(|| (task_code.to_string(), point.index));
            }
            Err(error) => {
                warn!(
                    "航点灯光：任务 {task_code} 航点 {} 开灯失败（{error}）；巡检继续",
                    point.index
                );
                // 允许下一个点到点再试（Agent 可能只是刚重启）
                *self.requested.lock().await = None;
            }
        }
    }

    /// 关灯：离开航点 / 任务终态 / 取消 / 本地任务收尾。
    ///
    /// 没开过灯就什么都不发（白天跳过、路线没配灯光、Agent 不可用都走这里）——
    /// 否则每个航点离开时都会多一条无意义的 HTTP。
    pub async fn leave(&self, reason: &str) {
        let _gate = self.gate.lock().await;
        *self.requested.lock().await = None;
        let taken = self.active.lock().await.take();
        if taken.is_none() || !self.enabled {
            return;
        }
        match self
            .post("/api/lights/off", json!({ "reason": reason }))
            .await
        {
            Ok(_) => info!("航点灯光：关灯（{reason}）"),
            Err(error) => warn!("航点灯光：关灯失败（{error}）；交给板端看门狗兜底"),
        }
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let response = self
            .client
            .post(format!("{}{path}", self.agent))
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("{path}: {error}"))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| format!("{path}: {error}"))?;
        if !status.is_success() {
            return Err(format!("{path} HTTP {status}: {text}"));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::post;
    use std::sync::Arc;
    use tokio::net::TcpListener;

    type Calls = Arc<Mutex<Vec<(String, Value)>>>;

    fn route() -> Value {
        json!({
            "points": [
                {
                    "name": "新航点 1",
                    "actions": [
                        {"type": "AI 识别", "scene": "fire", "labels": ["fire"]},
                        {"type": "可见光拍照", "lightFill": true}
                    ]
                },
                {"name": "新航点 2", "actions": [{"type": "可见光拍照", "param": "1080P"}]},
                {
                    "name": "新航点 3",
                    "actions": [
                        {"type": "补光", "lightLed": true},
                        {"type": "AI 识别", "lightFill": true, "lightLed": true}
                    ]
                }
            ]
        })
    }

    #[test]
    fn light_actions_are_read_and_merged_per_point() {
        let points = parse_light_points(&route());
        assert_eq!(
            points,
            vec![
                LightPoint {
                    index: 0,
                    fill: true,
                    led: false
                },
                LightPoint {
                    index: 2,
                    fill: true,
                    led: true
                },
            ]
        );
        // 没有灯光动作的点不进结果：到点判定用「查不到 = 不用开灯」
        assert!(light_point_at(&points, 1).is_none());
        assert_eq!(light_point_at(&points, 2).map(|p| p.index), Some(2));
    }

    #[test]
    fn empty_route_has_no_light_points() {
        assert!(parse_light_points(&json!({})).is_empty());
        assert!(parse_light_points(&json!({"points": []})).is_empty());
        assert!(parse_light_points(&route()["points"][1]["actions"]).is_empty());
    }

    /// 假 Agent：`on_result` 是 `/api/lights` 的固定回复（白天跳过 / 硬件部分失败都靠它模拟）
    async fn spawn_agent(calls: Calls, on_result: Value) -> String {
        let app = Router::new()
            .route(
                "/api/lights",
                post(
                    move |axum::extract::State(calls): axum::extract::State<Calls>,
                          axum::Json(body): axum::Json<Value>| {
                        let on_result = on_result.clone();
                        async move {
                            calls.lock().await.push(("on".to_string(), body));
                            axum::Json(on_result)
                        }
                    },
                ),
            )
            .route(
                "/api/lights/off",
                post(
                    |axum::extract::State(calls): axum::extract::State<Calls>,
                     axum::Json(body): axum::Json<Value>| async move {
                        calls.lock().await.push(("off".to_string(), body));
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .with_state(calls);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    fn light_point(index: i64) -> LightPoint {
        LightPoint {
            index,
            fill: true,
            led: true,
        }
    }

    #[tokio::test]
    async fn arrival_turns_lights_on_once_and_leave_turns_them_off() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent(
            calls.clone(),
            json!({"ok": true, "skipped": null, "night": true,
                   "applied": {"fill": true, "led": true}}),
        )
        .await;
        let lights = PointLights::new(agent, true);

        lights.point_arrived("TK-1", &light_point(2), true).await;
        // 5009 每 500ms 一帧：同一点不能反复发请求
        lights.point_arrived("TK-1", &light_point(2), true).await;
        let seen = calls.lock().await.clone();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].0, "on");
        assert_eq!(
            seen[0].1,
            json!({"fill": true, "led": true, "nightOnly": true,
                   "taskCode": "TK-1", "pointIndex": 2})
        );

        lights.leave("已离开航点").await;
        // 关灯只发一次：终端 / 取消 / 离开航点可能连着来
        lights.leave("任务终态").await;
        let seen = calls.lock().await.clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[1].0, "off");
        assert_eq!(seen[1].1["reason"], "已离开航点");
    }

    #[tokio::test]
    async fn daytime_skip_is_not_recorded_as_lit() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent(
            calls.clone(),
            json!({"ok": true, "skipped": "daytime", "night": false}),
        )
        .await;
        let lights = PointLights::new(agent, true);

        lights.point_arrived("TK-1", &light_point(0), true).await;
        assert_eq!(calls.lock().await.len(), 1);
        // 板端说「白天跳过」= 灯没开：离开航点不该再发关灯（否则会顺手关掉
        // 操作员手动开的灯）
        lights.leave("已离开航点").await;
        assert_eq!(calls.lock().await.len(), 1, "{:?}", calls.lock().await);
    }

    #[tokio::test]
    async fn disabled_lights_never_touch_the_agent() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent(calls.clone(), json!({"ok": true})).await;
        let lights = PointLights::new(agent, false);
        lights.point_arrived("TK-1", &light_point(0), false).await;
        lights.leave("已离开航点").await;
        assert!(calls.lock().await.is_empty());
    }

    #[tokio::test]
    async fn fill_light_failure_alone_does_not_mark_the_point_as_lit() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        // 只配了补光灯、而板端 GPIO 被禁用：一盏都没亮
        let agent = spawn_agent(
            calls.clone(),
            json!({"ok": false, "skipped": null,
                   "applied": {"fill": false}, "errors": ["补光灯已禁用"]}),
        )
        .await;
        let lights = PointLights::new(agent, true);
        let point = LightPoint {
            index: 4,
            fill: true,
            led: false,
        };
        lights.point_arrived("TK-1", &point, false).await;
        lights.leave("已离开航点").await;
        // 没有灯亮过 → 不该白发一条关灯（会把操作员手动开的灯关掉）
        assert_eq!(calls.lock().await.len(), 1, "{:?}", calls.lock().await);
    }

    #[tokio::test]
    async fn led_success_after_fill_failure_still_gets_turned_off() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent(
            calls.clone(),
            json!({"ok": false, "skipped": null,
                   "applied": {"fill": false, "led": true},
                   "errors": ["补光灯已禁用（config: FILL_LIGHT_GPIO=-1）"]}),
        )
        .await;
        let lights = PointLights::new(agent, true);
        lights.point_arrived("TK-1", &light_point(1), false).await;
        lights.leave("已离开航点").await;
        let seen = calls.lock().await.clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[1].0, "off", "LED 亮了就必须在离开航点时关掉");
    }

    #[tokio::test]
    async fn unreachable_agent_is_skipped_without_panicking() {
        let lights = PointLights::new("http://127.0.0.1:1", true);
        lights.point_arrived("TK-1", &light_point(0), false).await;
        lights.leave("已离开航点").await;
    }
}
