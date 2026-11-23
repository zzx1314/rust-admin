//! 调板端 Agent 启停识别（`POST /api/detect/*`）。

// 同一模块拆出来的几块共用一个词汇表（`Window` / `WindowConfig` / `Hit` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/* ------------------------------- 启停 ------------------------------- */

pub(crate) async fn start_detect(
    client: &reqwest::Client,
    base: &str,
    body: &Value,
    was_running: bool,
) -> Result<(), String> {
    // 已在跑（操作员开的识别 / 上一个窗口没收干净）→ 热更新，别硬停人家的会话
    let action = if was_running { "config" } else { "start" };
    let response = client
        .post(format!("{base}/api/detect/{action}"))
        .json(body)
        .send()
        .await
        .map_err(|error| format!("请求失败: {error}"))?;
    let text = response.text().await.unwrap_or_default();
    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        return Ok(());
    }
    // `config` 被拒（例如识别没在跑）时补一次 `start`
    if was_running {
        let retry = client
            .post(format!("{base}/api/detect/start"))
            .json(body)
            .send()
            .await
            .map_err(|error| format!("请求失败: {error}"))?;
        let text = retry.text().await.unwrap_or_default();
        let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if payload.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(());
        }
        return Err(payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(&text)
            .to_string());
    }
    Err(payload
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or(&text)
        .to_string())
}
