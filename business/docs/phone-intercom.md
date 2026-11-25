# 电话（对讲）：浏览器 ⇄ 板端的双向通话

「和现场的人说话」这件事的完整说明。平台侧**只有一个入口**：任务详情 →
「远程呼叫」→「网页直连通话」。音频经设备客户端中继，**不需要填 IP、公网浏览器可用**。

（板端仍保留 `/api/phone` / `/api/phone/stop` 与 `/api/lingsi/mode` 的 phone 通道，
现场调试可以照旧用 `pc_tools/voice_call_pc.py`，但平台已经不提供「拨给某台 PC」的入口。）

| 在哪 | 文件 | 职责 |
|---|---|---|
| 页面 | `business/web/src/composables/usePhoneCall.ts`、`views/tasks/index.vue` | AudioWorklet 采集/播放 + 抖动缓冲，WS 连接与状态展示 |
| 业务平台 | `business/server/src/phone_audio.rs` | 浏览器 WS 会话、一次一路、把帧转给客户端 |
| 设备客户端 | `business/client/src/phone_relay.rs`（+`phone.rs`） | 本机当**板端 UDP 对端**，UDP ⇄ 设备 WS 双向搬运 |
| 板端 Agent | `business/backpack_dashboard/backend/phone.py` | 严格的 IPv4/端口校验 + 写成聆思 FIFO 命令 |
| 板端语音模块 | `asr.service`（厂商 `voice_call`） | 麦克风/喇叭 + 16kHz PCM 收发 |

---

## 1. 链路

```text
浏览器 ──WS /api/devices/{id}/audio/ws?token=…──▶ 平台 phone_audio.rs
       ◀──── 二进制 PCM16@16k / 20ms（640B）────
                 │  ▲
      device WS  │  │ phone_audio{base64}
                 ▼  │
        客户端 phone_relay.rs（与板端同机）
                 │  ▲
      UDP 127.0.0.1:5000 │ │ 12B 头 + 640B PCM
                 ▼  │
        板端 voice_call（asr.service 内）
```

1. 浏览器连 WS（`?token=`，浏览器 WS 带不上 Header）→ 平台给客户端下发
   `phone_stream {action:"start"}`；
2. 客户端 `bind 127.0.0.1:0`，调板端 `/api/phone {peer:"127.0.0.1", peerPort}`，
   板子从此以为在跟一台本机 PC 通话（**板端一行代码都不用改**）；
3. 下行：板端 UDP → 客户端剥 12 字节头 → base64 `phone_audio` → 平台 → 浏览器二进制帧
   → AudioWorklet 抖动缓冲（攒 3 帧/60ms）+ 16k→48k 播放；
4. 上行：麦克风 48k → AudioWorklet 3 取平均降到 16k → 二进制帧 → 平台 → 客户端补
   12 字节头 → UDP 板端 5000；
5. 挂断 / 关弹窗 / 切任务 / 关页面 / 断网 → 平台下发 `phone_stream {action:"stop"}`
   → 客户端 `phone stop` + 收 socket。

**一次只允许一路**：板端 FIFO / UDP 都是单通道。同一设备第二个页面会接管，先来的
收到「另一个页面接管了这次通话」并挂断（`PhoneAudioHub::attach`）。

## 2. 三条红线

1. **地址白名单**：客户端当「对端」时把 `127.0.0.1:<本机端口>` 写进 `phone <ip> <port>`，
   而这条命令在板端是 shell `echo`。所以 `phone.py::normalize_peer` /
   `client/src/phone.rs::normalize_peer` 都只认「点分十进制 IPv4 + 1~65535」，
   不做 trim，带空格/换行一律拒。回归：`phone_selftest.py` 的注入用例、
   `client phone::tests::peer_must_be_a_plain_ipv4_with_a_real_port`。
2. **收尾是硬要求**：任何退出路径都要 `phone stop`，否则板子停在 phone 模式
   （唤醒词失效、还占着 FIFO）。三层兜底：页面 `onBeforeUnmount` / `onDetailClosed`；
   平台会话结束/超时/被接管下发 stop；客户端 `recv_loop` 发现平台连接断了也自己收口。
3. **平台不落库、不混音**：只搬字节，通话内容平台留不下。

## 2.5 必须用 https 打开（否则按钮是灰的）

浏览器只在**安全上下文**里开放 `AudioWorklet` 与 `navigator.mediaDevices.getUserMedia`：
`https://`、`http://localhost`，或被浏览器手动加白的 http 源。用
`http://39.107.55.13:3008` 这种明文 IP 打开时两者都是 `undefined`，点「网页直连通话」
会抛 `Cannot read properties of undefined (reading 'addModule')`。

页面现在会**提前拦**：按钮置灰 + 一行提示 + 开启办法（不再抛 JS 错）。三种解法：

| 办法 | 适合 | 代价 |
|---|---|---|
| **给控制台配 https**（域名 + 证书，反代或服务端 TLS） | 长期、团队 | 需要域名与证书 |
| Chrome / Edge：`chrome://flags/#unsafely-treat-insecure-origin-as-secure` 填该源 → Enabled → 重启 | 临时、单台机器 | 每个浏览器 profile 都要手动设一次 |
| Firefox：`about:config` 把 `media.devices.insecure.enabled`、`media.getusermedia.insecure.enabled` 设为 true | 临时、单台机器 | 同上 |

注意「现场入口」（`deploy/site-console`，:8080）也是 http，同样受限。

## 3. 带宽与音质

| 项 | 值 |
|---|---|
| 带宽 | 裸 PCM 16k×16bit = **256kbps/方向**（板端上行最贵，和视频中继抢 4G） |
| 延迟 | 4G 单程 +100~200ms，加上 60ms 抖动缓冲；对话慢半拍是正常的 |
| 回声 | 浏览器 AEC 能压一部分，**建议戴耳机**（外放 + 大音量会啸叫） |
| 并发 | 一次一路；与板端 Dashboard / PC 脚本的 phone 会话互斥（同一路 FIFO） |

要更省流：把协议里的 `codec` 换成 Opus/μ-law（两端各加一个编解码器，约半天）。
**不要**偷偷改字节语义 —— 板端那 12 字节头是跟 `voice_call` 对齐的。

## 4. 现场排查

| 现象 | 先看什么 |
|---|---|
| 页面「等待板端接通…」超时 | 客户端日志 `journalctl -u robodog-device-client \| grep 对讲`：应有「本机 UDP 对端 127.0.0.1:x」。没有 = 客户端没收到 start（看平台日志「对讲开始」） |
| 页面「板端没有响应（asr 是否在跑？）」 | 板端 `systemctl status asr`；phone 模式靠它读写 FIFO |
| 「通话中（没有音频到达）」 | 板端 `journalctl -u backpack-agent \| grep '\\[phone\\]'`，应有「通话中：对端 127.0.0.1:port」；再确认板端 `voice_call` 真在推流 |
| 声音断续/卡顿 | 4G 抖动；先把实时视频停掉（抢上行），确认 `ip route get 39.107.55.13` 走 4G 而不是抖的 Wi-Fi |
| 啸叫 | 戴耳机 / 调小音量；或点「静音」（只掐麦克风，仍能听见现场） |
| 页面报「另一个页面接管了这次通话」 | 有人开了第二个标签页；一次只允许一路 |

手工验（板端同机）：

```bash
curl -s 127.0.0.1:8000/api/phone/status
curl -s -XPOST 127.0.0.1:8000/api/phone -H 'Content-Type: application/json' \
  -d '{"peer":"192.168.234.101","peerPort":5000}'      # 现场调试：直接跟 PC 脚本通话
curl -s -XPOST 127.0.0.1:8000/api/phone/stop -d '{"reason":"manual"}'
```

## 5. 回归

```bash
# 板端：IPv4/端口白名单（含注入）、asr 门禁、先收播报、挂断幂等
python3 business/backpack_dashboard/backend/tools/phone_selftest.py

# 客户端：RTP 头形态与 20ms 帧长、未通话时上行要报错、地址白名单
cd business/client && cargo test phone

# 平台：会话表 / 一次一路 / 坏 base64 不炸
cd business/server && cargo test phone_audio::

# 页面：类型 + 构建
cd business/web && pnpm build
```

真机验证（无人出声）：用 `websocket-client` 连
`/api/devices/{id}/audio/ws?token=…`，应收到 `{"type":"phone_state","state":"started"}`，
随后持续收到二进制帧（≈50 帧/秒）；断开后板端 `/api/phone/status` 的 `active` 回到 false。
