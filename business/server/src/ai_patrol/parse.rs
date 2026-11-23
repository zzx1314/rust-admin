//! 纯函数：状态/阶段判定、阈值与数量解析、场景与类别解析、SSE 切帧。

// 同一模块拆出来的几块共用一个词汇表（`Window` / `WindowConfig` / `Hit` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/* ------------------------------- 纯函数 ------------------------------- */

pub(crate) fn is_terminal_status(status: &str) -> bool {
    crate::status::is_terminal(status)
}

pub(crate) fn is_arrival_stage(stage: &str) -> bool {
    matches!(stage, "patrol.point_arrived" | "patrol.waiting")
}

/// 动作是否属于「AI 识别」：动作名含 `AI` 或 `识别`。
///
/// 注意排除「红外测温」这类同样带中文的动作——它不含「识别」二字，不受影响。
pub(crate) fn is_ai_action(kind: &str) -> bool {
    kind.to_ascii_uppercase().contains("AI") || kind.contains("识别")
}

/// 窗口秒数：跟着停留时长走（`dwell + 8`），下限 10 s，上限由配置给（默认 300 s）。
///
/// 这里**不再**把长停留截断成 30 s：截断会让 `dwell=100` 的点只覆盖前 30 s，后面 70 s
/// 狗还站在那儿却不识别。离开航点由阶段收口（见 `is_point_stage`），不靠时长兜。
pub(crate) fn window_secs(dwell: i64, max_secs: u64) -> u64 {
    let base = dwell.max(4) + 8;
    let max = max_secs.max(WINDOW_MIN_SECS);
    (base as u64).clamp(WINDOW_MIN_SECS, max)
}

/// 「还在航点上」的阶段：这些阶段窗口可以继续识别。
///
/// 其余阶段（`patrol.navigating` 前往下一点、`patrol.returning*` 返航、建图/导航任务的
/// 阶段）说明狗已经离开，窗口必须立刻关掉——否则相机会把路过的东西当成这个点的目标。
pub(crate) fn is_point_stage(stage: &str) -> bool {
    matches!(
        stage,
        "patrol.point_arrived"
            | "patrol.waiting"
            | "patrol.photographing"
            | "patrol.point_completed"
    )
}

/// 置信度：`0.85` / `> 0.85` / `85%` 都认；`65℃` 这种温度阈值不算（>1 又会误判成 65 → 0.65）。
///
/// 规则：取第一个数字；`0 < x <= 1` 直接用，`1 < x <= 100` 且原文带 `%` 或整数时才 /100；
/// 其它情况返回 `None`，由场景默认值兜底。
pub(crate) fn parse_conf(text: &str) -> Option<f64> {
    let number = first_number(text)?;
    if (0.0..=1.0).contains(&number) && number > 0.0 {
        return Some(number);
    }
    if text.contains('%') && (1.0..=100.0).contains(&number) {
        return Some(number / 100.0);
    }
    None
}

pub(crate) fn first_number(text: &str) -> Option<f64> {
    let mut buf = String::new();
    let mut started = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() || (ch == '.' && started) {
            buf.push(ch);
            started = true;
        } else if started {
            break;
        }
    }
    if buf.is_empty() {
        return None;
    }
    buf.trim_end_matches('.').parse::<f64>().ok()
}

/// 动作里的「检查目标数量」：接受数字或数字字符串（编辑器可能是输入框），
/// 空/0/非法一律当「不核对数量」（`None`），上限 999 防脏数据。
pub(crate) fn parse_expect_count(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let number = value
        .as_i64()
        .or_else(|| value.as_f64().map(|raw| raw as i64))
        .or_else(|| {
            value
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .and_then(|text| text.parse::<f64>().ok())
                .map(|raw| raw as i64)
        })?;
    if number < 1 {
        return None;
    }
    Some(number.min(999))
}

/// 任务类型归一化：`exist`（检测是否存在）/ `count`（检测数量是否正确）。
///
/// 兼容三种来源：路线编辑器写 `checkType`；老数据只有 `expectCount`（给了数量就按
/// 数量任务算）；两者都没有就是存在性任务。中文别名也认（手改 route_data 时不踩坑）。
pub(crate) fn normalize_check_type(explicit: Option<&str>, expect: Option<i64>) -> String {
    let text = explicit.map(str::trim).unwrap_or_default();
    if text.is_empty() {
        return if expect.is_some() { "count" } else { "exist" }.to_string();
    }
    let lower = text.to_ascii_lowercase();
    if lower == "count" || lower == "quantity" || text.contains("数量") {
        "count".to_string()
    } else {
        "exist".to_string()
    }
}

/// 板端 `plan_hit` 事件里的数量核对结论：只在 `countAlert` 为真时返回
/// （期望数量, 实际检出, 证据图路径）。
pub(crate) fn plan_hit_alert(event: &Value) -> Option<(i64, i64, String)> {
    if event.get("type").and_then(Value::as_str) != Some("plan_hit") {
        return None;
    }
    if event.get("count_alert").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let expect = event.get("expect_count").and_then(Value::as_i64)?;
    let found = event
        .get("found_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let evidence = event
        .get("evidence")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some((expect, found, evidence))
}

/// 板端 `plan_hit` 事件里的「数量核对一致」：只有**数量任务 + 开了「一致也通知」+
/// `count_ok=true`** 才返回（期望数量, 实际检出, 证据图路径）。
///
/// 服务端直连板端时走这条路；客户端代调模式下由客户端把同一事件转成 `ai_count`
/// （见 [`AiPatrol::on_client_count`]），两边最后都到 [`record_count_ok`]。
pub(crate) fn plan_hit_count_ok(
    event: &Value,
    config: &WindowConfig,
) -> Option<(i64, i64, String)> {
    if config.check_type != "count" || !config.notify_when_count_ok {
        return None;
    }
    if event.get("type").and_then(Value::as_str) != Some("plan_hit") {
        return None;
    }
    if event.get("count_ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let expect = event.get("expect_count").and_then(Value::as_i64)?;
    let found = event
        .get("found_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if found != expect {
        return None;
    }
    let evidence = event
        .get("evidence")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some((expect, found, evidence))
}

/// `[name, {properties:{...}}]` 之类都无所谓的宽松查找：把 SSE 里的一帧 `detect` 事件
/// 翻成「第一个超过阈值的类别」。
pub(crate) fn detect_hit(event: &Value, threshold: f64) -> Option<(String, f64)> {
    if event.get("type").and_then(Value::as_str) != Some("detect") {
        return None;
    }
    let boxes = event.get("boxes").and_then(Value::as_array)?;
    boxes.iter().find_map(|item| {
        let conf = item.get("conf").and_then(Value::as_f64)?;
        if conf < threshold {
            return None;
        }
        let label = item
            .get("zh")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .or_else(|| item.get("label").and_then(Value::as_str))
            .unwrap_or("未知目标");
        Some((label.to_string(), conf))
    })
}

/// 从 SSE 文本缓冲里切出完整事件（按空行分隔，取 `data:` 行）。
pub(crate) fn take_sse_events(buffer: &mut String) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(position) = buffer.find("\n\n") {
        let block: String = buffer.drain(..position + 2).collect();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                let payload = rest.trim();
                if !payload.is_empty() {
                    out.push(payload.to_string());
                }
            }
        }
    }
    out
}

pub(crate) struct Scene {
    pub(crate) task: String,
    pub(crate) conf: f64,
}

/// 把动作参数对到 Agent 的模型场景上。
///
/// 两个来源，优先级从高到低：
///
/// 1. `scene`：路线编辑器从「实时视频 → AI 识别」模型目录里选的模型包 id（如 `fire`），
///    精确匹配 `task`，不做猜词——现场选什么就跑什么；
/// 2. `hint`（动作 `param`）：历史数据 / 手填的中文提示（「烟火 / 人员 / 仪表读数」）。
///    按 `/ 、 , 空格` 切词，匹配 `task` / `name` / `desc` 与每个类别的 `name`、`zh`，
///    命中最多者胜；一个都不命中时退回第一个可用场景（并打 WARN），总比「什么都不跑」好排查。
pub(crate) fn resolve_scene(
    tasks: Option<&Value>,
    hint: &str,
    scene: Option<&str>,
) -> Option<Scene> {
    let list = tasks?.get("tasks")?.as_array()?;
    let wanted = scene.map(str::trim).filter(|value| !value.is_empty());

    if let Some(wanted) = wanted
        && let Some(found) = list.iter().find(|item| {
            item.get("task")
                .and_then(Value::as_str)
                .is_some_and(|task| task.eq_ignore_ascii_case(wanted))
        })
    {
        let scene = Scene {
            task: found
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            conf: found.get("conf").and_then(Value::as_f64).unwrap_or(0.5),
        };
        debug!(
            "AI 识别场景：按路线编辑器选的模型包直接命中 → {}",
            scene.task
        );
        return Some(scene);
    }
    if let Some(wanted) = wanted {
        warn!(
            "AI 识别：路线里指定的场景「{wanted}」不在板端模型列表里（模型包可能已更新），\
             退回关键词匹配，建议到路线编辑里重选场景"
        );
    }

    let tokens: Vec<String> = hint
        .split(['/', '、', ',', '，', ' '])
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| token.to_lowercase())
        .collect();

    let mut best: Option<(usize, Scene)> = None;
    let mut fallback: Option<Scene> = None;
    for item in list {
        let ok = item.get("ok").and_then(Value::as_bool).unwrap_or(true);
        let task = item
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if task.is_empty() {
            continue;
        }
        let scene = Scene {
            task: task.clone(),
            conf: item.get("conf").and_then(Value::as_f64).unwrap_or(0.5),
        };
        if ok && fallback.is_none() {
            fallback = Some(Scene {
                task: scene.task.clone(),
                conf: scene.conf,
            });
        }
        let mut haystack = String::new();
        for key in ["task", "name", "desc"] {
            if let Some(text) = item.get(key).and_then(Value::as_str) {
                haystack.push_str(&text.to_lowercase());
                haystack.push(' ');
            }
        }
        if let Some(labels) = item.get("labels").and_then(Value::as_array) {
            for label in labels {
                for key in ["name", "zh"] {
                    if let Some(text) = label.get(key).and_then(Value::as_str) {
                        haystack.push_str(&text.to_lowercase());
                        haystack.push(' ');
                    }
                }
            }
        }
        let score = tokens
            .iter()
            .filter(|token| haystack.contains(token.as_str()))
            .count();
        if score > 0 && best.as_ref().map(|(top, _)| score > *top).unwrap_or(true) {
            best = Some((score, scene));
        }
    }

    if let Some((score, scene)) = best {
        debug!(
            "AI 识别场景匹配：提示「{hint}」命中 {} 个关键词 → {}",
            score, scene.task
        );
        return Some(scene);
    }
    if let Some(scene) = fallback {
        warn!(
            "AI 识别：动作参数「{hint}」没有匹配到任何模型场景，退回 {}（可在动作参数里填模型中文名）",
            scene.task
        );
        return Some(scene);
    }
    None
}

/// 把动作里勾选的识别类别对到解析出的场景上。
///
/// 只保留该场景真实存在（且未被板端标记 `ignored`）的类别名——英文 `name`，
/// 与「实时视频」下发的 `labels` 是同一套。全部失效时返回空，调用方传
/// `labels: null` 让 Agent 用该场景的全部类别（比「一个都不识别」安全）。
pub(crate) fn scene_labels(tasks: Option<&Value>, scene: &str, wanted: &[String]) -> Vec<String> {
    if wanted.is_empty() {
        return Vec::new();
    }
    let Some(item) = tasks
        .and_then(|value| value.get("tasks"))
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .find(|item| item.get("task").and_then(Value::as_str) == Some(scene))
        })
    else {
        return Vec::new();
    };
    let known: Vec<&str> = item
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter(|label| {
                    !label
                        .get("ignored")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .filter_map(|label| label.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let keep: Vec<String> = wanted
        .iter()
        .filter(|name| known.contains(&name.as_str()))
        .cloned()
        .collect();
    if keep.len() != wanted.len() {
        warn!(
            "AI 识别：场景 {scene} 上不存在的类别已忽略（勾了 {} 个，保留 {} 个）：{wanted:?}",
            wanted.len(),
            keep.len()
        );
    }
    keep
}
