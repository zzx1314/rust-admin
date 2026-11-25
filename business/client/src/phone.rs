//! 模拟电话（phone 模式）：板端 ↔ 同网段 PC 的双向对讲
//!
//! 平台点「发起通话 / 挂断」时，走的还是这条已有信道：
//!
//! ```text
//! 平台 POST /devices/{id}/phone ──设备指令 `电话`──▶ 本进程
//!     ──HTTP──▶ 板端 127.0.0.1:8000/api/phone {peer, peerPort}
//!         ──FIFO──▶ `phone <对端IP> <对端端口>`
//! 平台 POST /devices/{id}/phone/stop ──`挂断电话`──▶ /api/phone/stop ──▶ `phone stop`
//! ```
//!
//! 为什么由客户端代调：Agent 只在板子内网（客户端与它同机 `127.0.0.1:8000`），
//! 操作员在公网控制台上点按钮。**音频不经平台**：板子把麦克风 PCM（16kHz/S16/20ms）
//! 直接 UDP 打到对端 PC，所以 PC 必须和板子同网段并跑 `pc_tools/voice_call_pc.py`。
//!
//! 三条约定（改这块先看）：
//!
//! 1. **对端地址再过一遍白名单**：平台已经校验，但设备指令可以被伪造，而板端那条命令
//!    最终进的是 shell `echo` —— 只认 IPv4 点分十进制 + 1~65535，其它一律拒。
//! 2. **失败只回 command_result**：电话是运维手段，不是巡检步骤，任何失败都不能影响
//!    正在跑的任务（不抛错、不打断 5009 处理链路）。
//! 3. **不碰本体**：`handlers::command` 把这两个指令放在本体门禁之前，本体关机
//!    （or 断链）时也要能打电话找现场人。
use crate::speak;
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use tracing::info;

/// 同机 HTTP，5s 足够（板端可能要先 `systemctl start asr` 再等 2s）
const AGENT_TIMEOUT: Duration = Duration::from_secs(8);

/// 校验对端地址：点分十进制 IPv4 + 端口 1~65535。
///
/// 这是防注入的白名单（板端 `phone.py::normalize_peer` 还会再校验一次），
/// 不要为了「支持域名」放宽：FIFO 那条路走的是 shell。
pub fn normalize_peer(peer: &str, port: u64) -> Result<(String, u16), String> {
    // 不做 trim：这条命令最后要拼进 shell 的 `echo "phone <ip> <port>"`，
    // 带空白 / 换行的输入一律当非法，别指望下游去猜。
    let text = peer;
    let parts: Vec<&str> = text.split('.').collect();
    let ipv4 = parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 3
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && part.parse::<u16>().is_ok_and(|value| value <= 255)
        })
        && parts
            .iter()
            .any(|part| part.parse::<u16>().unwrap_or_default() > 0);
    if !ipv4 {
        return Err(format!(
            "对端 IP 不合法（{text:?}）：只支持 IPv4 点分十进制（不能有空格/换行），例如 192.168.234.101"
        ));
    }
    if !(1..=65535).contains(&port) {
        return Err(format!("对端端口不合法（{port}）：需要 1~65535"));
    }
    Ok((text.to_string(), port as u16))
}

/// 发起通话：板端 `/api/phone` 写 `phone <对端IP> <端口>`，回到 `ok:true` 才算接上。
pub async fn start(agent: &str, peer: &str, port: u64) -> Result<String, String> {
    let (peer, port) = normalize_peer(peer, port)?;
    let client = agent_client()?;
    let reply = speak::post_agent(
        &client,
        agent,
        "/api/phone",
        json!({ "peer": peer, "peerPort": port }),
        "电话",
    )
    .await?;
    if reply.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
        let error = reply
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("板端拒绝发起通话");
        return Err(error.to_string());
    }
    info!("电话：通话中，对端 {peer}:{port}（板端监听 5000）");
    Ok(format!("已发起通话：对端 {peer}:{port}"))
}

/// 挂断：板端 `/api/phone/stop`（`phone stop` 回 voice 模式，不在通话中时幂等）。
pub async fn stop(agent: &str, reason: &str) -> Result<String, String> {
    let client = agent_client()?;
    let reply = speak::post_agent(
        &client,
        agent,
        "/api/phone/stop",
        json!({ "reason": reason }),
        "电话",
    )
    .await?;
    if reply.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
        let error = reply
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("板端拒绝挂断");
        return Err(error.to_string());
    }
    info!("电话：已挂断（{reason}）");
    Ok("已挂断".to_string())
}

fn agent_client() -> Result<Client, String> {
    Client::builder()
        .timeout(AGENT_TIMEOUT)
        .build()
        .map_err(|error| format!("建 HTTP 客户端失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::extract::State;
    use axum::routing::post;
    use axum::{Json, http::StatusCode};
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    type Calls = Arc<Mutex<Vec<Value>>>;

    /// 假 Agent：记录 `/api/phone` 与 `/api/phone/stop` 的请求体
    async fn spawn_agent(calls: Calls) -> String {
        let app = Router::new()
            .route(
                "/api/phone",
                post(
                    |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(serde_json::json!({"path": "start", "body": body}));
                        Json(serde_json::json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/phone/stop",
                post(
                    |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(serde_json::json!({"path": "stop", "body": body}));
                        Json(serde_json::json!({"ok": true, "changed": true}))
                    },
                ),
            )
            .with_state(calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn peer_must_be_a_plain_ipv4_with_a_real_port() {
        assert_eq!(
            normalize_peer("192.168.234.101", 5000).ok(),
            Some(("192.168.234.101".to_string(), 5000))
        );
        // 注入 / 空白类输入（板端那条命令进的是 shell `echo`，不做 trim）
        for bad in [
            " 192.168.234.101 ",
            "192.168.1.1; rm -rf /tmp/x",
            "$(reboot)",
            "1.2.3",
            "256.1.1.1",
            "0.0.0.0",
            "",
            "192.168.1.1\n",
            "abc.def.ghi.jkl",
        ] {
            assert!(normalize_peer(bad, 5000).is_err(), "应拒绝 {bad:?}");
        }
        for bad_port in [0u64, 65536, 99999] {
            assert!(
                normalize_peer("192.168.234.101", bad_port).is_err(),
                "应拒绝端口 {bad_port}"
            );
        }
    }

    #[tokio::test]
    async fn start_and_stop_hit_the_board_agent() {
        let recorded: Calls = Default::default();
        let agent = spawn_agent(recorded.clone()).await;

        let note = start(&agent, "192.168.234.101", 5000)
            .await
            .expect("发起通话失败");
        assert!(note.contains("192.168.234.101:5000"), "{note}");
        stop(&agent, "平台手动挂断").await.expect("挂断失败");

        let calls = recorded.lock().await.clone();
        assert_eq!(calls.len(), 2, "应有 start + stop 两条：{calls:?}");
        assert_eq!(calls[0]["path"], "start");
        assert_eq!(calls[0]["body"]["peer"], "192.168.234.101");
        assert_eq!(calls[0]["body"]["peerPort"], 5000);
        assert_eq!(calls[1]["path"], "stop");
        assert_eq!(calls[1]["body"]["reason"], "平台手动挂断");

        // 非法对端：本地就拒，连请求都不发
        assert!(start(&agent, "192.168.1.1 -oops", 5000).await.is_err());
        assert_eq!(recorded.lock().await.len(), 2);
    }

    /// 板端返回 ok=false（asr 没起 / FIFO 不可用）时要冒泡成错误，不能当成功
    #[tokio::test]
    async fn board_rejection_is_an_error() {
        let app = Router::new().route(
            "/api/phone",
            post(|Json(_body): Json<Value>| async move {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"ok": false, "error": "asr 服务未启动"})),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let error = start(&format!("http://127.0.0.1:{port}"), "192.168.234.101", 5000)
            .await
            .expect_err("板端拒绝时必须报错");
        assert!(error.contains("asr"), "{error}");
    }
}
