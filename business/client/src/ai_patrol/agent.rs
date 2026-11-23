//! 板端 Agent 的 HTTP 调用：拉 JSON、下发识别指令、回传检出与证据图。

use super::*;

impl AiPatrol {
    pub(crate) async fn fetch_json(&self, path: &str) -> Result<Value, String> {
        let response = self
            .probe
            .get(format!("{}{path}", self.agent))
            .send()
            .await
            .map_err(|error| format!("请求失败: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "上游返回 {status}: {}",
                text.chars().take(160).collect::<String>()
            ));
        }
        serde_json::from_str(&text).map_err(|error| format!("响应不是合法 JSON: {error}"))
    }

    pub(crate) async fn post_json(&self, path: &str, body: &Value) -> Result<Value, String> {
        let response = self
            .action
            .post(format!("{}{path}", self.agent))
            .json(body)
            .send()
            .await
            .map_err(|error| format!("请求失败: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if payload.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(payload);
        }
        let reason = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(&text)
            .to_string();
        Err(if reason.trim().is_empty() {
            format!("上游返回 {status}")
        } else {
            reason
        })
    }
}

/// 上报一条检出（`ai_detect`）。
///
/// `evidence` = （板端路径, base64）：拿到命中帧就带图，没拿到就只发卡片
/// （见 [`EVIDENCE_PENDING_WAIT`]——先挂起等板端落盘，等不到也不能把告警拖没了）。
pub(crate) async fn send_detect(
    out_tx: &mpsc::Sender<WsMessage>,
    start: &AiStart,
    label: &str,
    conf: f64,
    evidence: Option<&(String, String)>,
) {
    let mut detect = json!({
        "windowId": start.window_id,
        "task": start.task,
        "planId": start.plan_id,
        "pointIndex": start.point_index,
        "label": label,
        "conf": conf,
    });
    if let Some((path, image)) = evidence {
        detect["evidence"] = json!(path);
        detect["evidenceImage"] = json!(image);
    }
    send(out_tx, "ai_detect", detect).await;
}

/// 补发一张命中帧（`ai_evidence`）：卡片已经发过了，平台只发 `msgtype=image`。
///
/// 只在「告警早就发出去、板端到窗口收尾才把图落盘」时用（见 [`Leftover`]）：
/// 平台不会重发卡片，所以群里看到的就是「卡片在前、图在后」，与正常情况一致。
pub(crate) async fn send_evidence(
    out_tx: &mpsc::Sender<WsMessage>,
    start: &AiStart,
    path: &str,
    image: &str,
) {
    send(
        out_tx,
        "ai_evidence",
        json!({
            "windowId": start.window_id,
            "taskCode": start.task_code,
            "planId": start.plan_id,
            "pointIndex": start.point_index,
            "evidence": path,
            "evidenceImage": image,
        }),
    )
    .await;
}
