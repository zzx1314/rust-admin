# 电话（对讲）：板端 ⇄ 现场 PC 的双向通话

「让狗和现场的人说话」这件事的完整说明。和「语音播报 / 呼叫」（单向 TTS）不同，
电话是**双向实时语音**，音频走**局域网 UDP**，不经过平台。

| 在哪 | 文件 | 职责 |
|---|---|---|
| 平台页面 | `business/web/src/views/tasks/index.vue`（「远程呼叫 → 电话（对讲）」） | 填对端 IP/端口、发起 / 挂断、显示通话态与 PC 端命令 |
| 业务平台 | `business/server/src/handlers/devices.rs::phone_start / phone_stop / phone_status` | 校验对端、下发 `电话` / `挂断电话`、经隧道读通话态 |
| 设备客户端 | `business/client/src/phone.rs` | 代调板端 `/api/phone`（与 Agent 同机），再过一遍地址白名单 |
| 板端 Agent | `business/backpack_dashboard/backend/phone.py` | 校验 + 写聆思 FIFO `phone <对端IP> <端口>` / `phone stop` |
| PC 端 | `business/backpack_dashboard/backend/pc_tools/voice_call_pc.py` | 声卡采集/播放 + 12 字节极简 RTP（板端 `:8000/api/lingsi/voice_call_pc.py` 可下载） |
| 网页直连 | `business/web/src/composables/usePhoneCall.ts` + `business/server/src/phone_audio.rs` + `business/client/src/phone_relay.rs` | 浏览器 ⇄ 板端音频经设备 WS 中继（**不需要填 IP**，公网也能用） |

---

## 1. 链路

```text
任务详情「电话（对讲）」 填 对端PC IP + 端口
  → POST /devices/{id}/phone {peer, peerPort}            （平台校验 IPv4 / 1~65535）
    → 设备指令 `电话`（WS，params.params.{peer,peerPort}）
      → 设备客户端 phone::start → POST 板端 /api/phone
        → phone.py：先 speak.stop（FIFO 单通道，先收播报）→ ensure_asr()
          → `phone <对端IP> <端口>`
            → asr/voice_call 把麦克风 PCM（16000Hz / 单声道 / S16_LE / 20ms）
              **UDP 直发对端 PC 的该端口**；同时接收对端 PCM 播放
挂断：POST /devices/{id}/phone/stop → 设备指令 `挂断电话` → /api/phone/stop → `phone stop`
通话态：GET /devices/{id}/phone → 设备客户端隧道 → 板端 /api/phone/status
```

- **板端固定监听 5000**（`phone.py::LOCAL_PORT`）。所以：
  - `phone <IP> <端口>` 里的端口是**对端 PC** 的端口；
  - PC 端命令是 `python voice_call_pc.py --peer <板端IP> --peer-port 5000 --local-port <同一端口>`。
- **PC 必须和板子同网段**（现场 `192.168.234.x` eth0 / `192.168.41.x` Wi-Fi）。
  「跨公网打电话」不成立——平台只下发起停，不中继音频。
- 页面会展示板端网卡（`/api/phone/status` 的 `interfaces`），优先 `eth0`，
  并拼好 PC 端命令与脚本下载链接（`http://<板端IP>:8000/api/lingsi/voice_call_pc.py`）。

## 2. 三条红线

1. **地址必须白名单校验**：板端这条命令最终是 `echo "phone <ip> <port>" > FIFO`（shell）。
   平台 / 客户端 / 板端三层都只认「点分十进制 IPv4 + 1~65535」，其它一律 400。
   要「支持域名」之前先想清楚这一点。回归：`phone_selftest.py` 的注入用例、
   `client phone::tests::peer_must_be_a_plain_ipv4_with_a_real_port`、
   `api_smoke::phone_dispatch_validates_peer_and_reaches_the_device`。
2. **进电话模式先收播报**：FIFO 单通道，电话进来会打断播报；`phone.start` 先调
   `speak.stop()`，否则客户端以为循环播报还在响（离开航点时会多写一次 stop 无所谓）。
3. **失败只提示，不影响任务**：asr 起不来 / FIFO 写不进去 / 对端没跑脚本，
   都只回 `ok=false + error`；客户端只回执 `command_result` + 一条设备事件。

## 3. 现场排查

| 现象 | 先看什么 |
|---|---|
| 页面报「未送达」 | 设备客户端不在线（业务平台设备列表 / `journalctl -u robodog-device-client`） |
| 页面报「asr 服务未启动」 | 板端 `systemctl status asr`；电话模式靠它读写 FIFO |
| 点「发起通话」成功但没声音 | PC 有没有跑脚本、防火墙放不放 UDP 入站、两边是不是同网段（`ip route get <对端IP>`）；板端日志 `journalctl -u backpack-agent \| grep '\[phone\]'` 里应有「通话中：对端 x:y」 |
| 页面「通话态不可查」 | 设备客户端没登记 Agent 隧道（板端 `:8000` 不通 / 旧客户端）；通话本身可能仍然正常 |
| 说完一句就被打断 | 板端 FIFO 单通道：同一条通道上还有播报/唤醒词在抢；确认没有循环播报在跑 |
| 声音断续 | 4G/Wi-Fi 抖动 + 20ms 帧无重传。优先走厂商内网 eth0（`192.168.234.x`） |

手工验（板端同机）：

```bash
curl -s 127.0.0.1:8000/api/phone/status
curl -s -XPOST 127.0.0.1:8000/api/phone -H 'Content-Type: application/json' \
  -d '{"peer":"192.168.234.101","peerPort":5000}'
curl -s -XPOST 127.0.0.1:8000/api/phone/stop -d '{"reason":"manual"}'
# PC 端（同网段）：
pip install sounddevice
python voice_call_pc.py --peer 192.168.234.200 --peer-port 5000 --local-port 5000
```

## 4. 回归

```bash
# 板端：IPv4/端口白名单（含注入）、asr 门禁、先收播报、挂断幂等
python3 business/backpack_dashboard/backend/tools/phone_selftest.py

# 客户端：地址白名单 / 起停代调 / 板端拒绝要冒泡成错误
cd business/client && cargo test phone::

# 业务平台：校验 + WS 下发 + 通话态不可达不报 500
cd business/server && cargo test --test api_smoke phone_dispatch

# 页面：类型 + 构建
cd business/web && pnpm build
```

## 5. 网页直连对讲（公网浏览器，不用填 IP）

上面那套「PC 脚本」要求对端在板子同网段。**公网浏览器**怎么办：让设备客户端
（与板端同机，`127.0.0.1` 一定通）当那个「UDP 对端」，音频借已有的设备 WS 中继。

```text
浏览器 ──WS /api/devices/{id}/audio/ws?token=…──▶ 平台(phone_audio.rs) ──设备 WS──▶ 客户端
       ◀──────── 二进制 PCM16@16k / 20ms（640B） ─────────────────────────────
                                                       客户端 ──UDP 127.0.0.1:5000──▶ 板端
```

1. 浏览器连上 WS → 平台给客户端下发 `phone_stream {action:"start"}`；
2. 客户端 `bind 127.0.0.1:0` 拿到端口，调板端 `/api/phone {peer:"127.0.0.1", peerPort}`，
   板子从此以为在跟一台本机 PC 通话（**板端一行代码都不用改**）；
3. 板端 UDP 帧（12B 头 + 640B PCM）→ 客户端剥头 → `phone_audio{base64}` → 平台 →
   二进制帧给浏览器 → AudioWorklet 抖动缓冲 + 16k→48k 播放；
4. 浏览器麦克风 48k → AudioWorklet 降到 16k → 二进制帧 → 平台 → 客户端补 12B 头 →
   UDP 板端 5000；
5. 挂断 / 关页面 / 换任务 / 断网 → 平台下发 `phone_stream {action:"stop"}` → 客户端
   `phone stop` + 收 socket。

要点：

- **一次只允许一路**：板端 FIFO / UDP 都是单通道。同一设备第二个浏览器会话会接管，
  先来的那条收到「另一个页面接管了这次通话」并挂断（`PhoneAudioHub::attach`）。
- **只搬字节，不落库不混音**：平台不留任何通话内容；两端都不做编码（裸 PCM）。
- **带宽**：16k × 16bit = 256kbps/方向。板端上行是 4G 上最贵的那一段，和视频中继抢
  带宽；长时间通话建议先把实时视频停掉。要省带宽先加 Opus/μ-law 编码（协议里已有
  `codec` 字段，别偷偷改字节语义）。
- **建议戴耳机**：浏览器 AEC 只能压掉一部分回授，外放 + 大音量会啸叫。
- **收尾是硬要求**：`onDetailClosed` / 切任务 / `onBeforeUnmount` 都会 `stop()`；
  客户端侧 `recv_loop` 在平台连接断开时也会自己 `phone stop`（板子不会停在 phone 模式）。
- 与「现场 PC」模式**互斥**：两种方式都是同一路 FIFO/UDP，谁后到谁生效。
- 排查：页面状态一直「等待板端接通」→ 客户端日志 `journalctl -u robodog-device-client |
  grep 对讲`（应有「本机 UDP 对端 127.0.0.1:x」）；「通话中（没有音频到达）」→ 板端
  `journalctl -u backpack-agent | grep '\[phone\]'`，看 asr 是否真的在跑。

## 6. 回归（网页直连）

```bash
# 平台：会话表 / 一次一路 / 坏 base64 不炸
cd business/server && cargo test phone_audio::

# 客户端：RTP 头形态与 20ms 帧长、未通话时上行要报错
cd business/client && cargo test phone_relay::

# 页面：类型 + 构建
cd business/web && pnpm build
```

真机验证（会短暂让板端进 phone 模式，但无人出声）：用 `websocket-client` 连
`/api/devices/{id}/audio/ws?token=…`，应收到 `{"type":"phone_state","state":"started"}`，
关掉连接后板端 `/api/phone/status` 的 `active` 回到 false。
