# 数据采集背包 Demo 分析 · 视频与 AI 识别接入业务平台

> 分析对象：`~/IdeaProjects/backpack_dashboard`（板端 Python Agent + mediamtx + RKNN 旁路识别）
> 集成目标：`business/`（RoboDog OPS，Rust 服务 3100 + Vue 3 控制台 5273）
> 本文第一阶段只落地**单路视频 + AI 识别 + 两种画框方式**，其余能力列在路线图里。

---

## 1. Demo 是什么

一套**离线直连**的数据采集背包演示：板子（RK3576）自带 4 路 USB 摄像头、红外网络
摄像头、温湿度 / 4G / WiFi / SSD / 风扇 / 聆思语音小板 / SOS/STOP 实体按键，
外加 RKNN 旁路 AI 识别。部署形态是「Windows 笔记本用网线直连背包，Chrome 打开
`http://192.168.234.200:8000`」，**零安装、无云、无平台**。

```
USB 摄像头 usbcam0~3 (MJPG 720p30)
      │ ffmpeg -f v4l2 → H264(h264_rkmpp)
      ▼
   mediamtx :8554 (RTSP)  ──▶ :8889 WHEP(WebRTC) ──▶ 浏览器 <video>
      ▲   ▲
      │   └── dog-ipcam（红外，现场已有 mediamtx path）
      └── dogcam（机器狗前置，relay.py 从 3588 的 RTSP 拉回来重编码归一化）

Agent (FastAPI :8000)  ── REST/SSE ──▶ Dashboard
   遥测采集 / LED·补光·聆思控制 / SOS·STOP 监听 / 定位 / AI 识别编排
```

四个关键设计决策（决定了平台侧该怎么接）：

| 决策 | 原因 |
| --- | --- |
| 视频统一走 **mediamtx + WHEP**，不用 HLS/FLV | 浏览器不能播 RTSP；WHEP 端到端 ~200ms，HLS 至少 2s |
| 摄像头 **MJPG → H264 硬编** 后再推 RTSP | UVC 只出 MJPG，WebRTC 只吃 H264 |
| AI 走 **RTSP 旁路抽帧**，不碰 v4l2 设备节点 | 主推流不受影响；4 路 USB 都在同一条 USB 总线上 |
| **NPU 独占单路** + 板端 OSD 烧框 | RK3576 单路推理 ~116ms，多路分时会拖垮帧率 |

---

## 2. 背包现有功能清单

| 域 | 能力 | 实现 | 平台价值 |
| --- | --- | --- | --- |
| 视频 | 4 路 USB（左前/右前/右后/左后）+ 红外 + 机器狗前置 | `stream.py` / `relay.py` + mediamtx | ★★★ 本次接入 |
| AI | 模型包热扫、类别勾选、阈值、帧率、单路 | `detect.py` + RKNN `--serve` 常驻进程 | ★★★ 本次接入 |
| 画框 | 板端 OSD 烧进视频 / 浏览器 SVG 叠加 | `osd.py`（纯标准库 RGBA + 预烘焙中文字模） | ★★★ 本次接入 |
| 检出事件 | SSE `detect` 事件 + JSONL 落盘（取证回溯） | `detect.py::_emit` | ★★★ 后续接告警中心 |
| 遥测 | 温湿度 SHT4x、4G(AT/SIM/信号/流量)、WiFi 扫描、核心温度、SSD、系统、SN | `telemetry.py` | ★★ 设备详情页 |
| 控制 | LED 灯带闪烁、补光灯开关、聆思 call/voice/phone/音量/播报 | `control.py` / `telemetry.py` | ★★ 设备遥控面板 |
| 安全 | SOS / STOP 实体按键实时上报（evdev） | `buttons.py` | ★★★ 告警中心（急停级别） |
| 定位 | 4G 小区 → OpenCellID/cellocation → 经纬度，本地轨迹 | `location.py` + Leaflet 页 | ★★ 轨迹页 |
| 工程 | 5s 看门狗自动拉起 ffmpeg / relay 退避重启 / 无缓存中间件 / WHEP 同源代理 | — | ★★ 可移植的经验 |

### 2.1 Agent HTTP 接口（平台代理需要转发的部分）

```text
GET  /api/cameras            摄像头映射 + WebRTC 信令地址（不含任何带密码的源 URL）
GET  /api/relay/status       dogcam 链路 {enabled,running,restarts,last_error,stream}
GET  /api/detect/tasks       模型包清单（静态契约校验，不占 NPU）
GET  /api/detect/status      运行态：state/cams/fps/stats/osd/label_format/relay
POST /api/detect/start       {task,labels,conf,cams,fps}   cams 只取一路
POST /api/detect/stop        停止推理 + 关闭 OSD + 清框
POST /api/detect/config      热更新场景/类别/阈值/目标路/帧率
GET  /api/events             SSE：telemetry / button / lingsi / detect / detect_state
*    /api/whep/<stream>      WHEP 信令同源代理（OPTIONS/POST/PATCH/DELETE）
```

### 2.2 视频链路参数（平台侧必须保持一致）

- 采集/编码：`1280x720@30`、`H264 High`、`2Mbps`、`GOP30`、无 B 帧、无音轨
- dogcam 归一化：源 1080p60 HL4.0 full-range → 同一套参数（`relay.py`）
- 红外：`dog-ipcam`，由现场已有 mediamtx 承载，**不参与 AI**
- 播放：mediamtx WHEP（`:8889`），agent 在 `/api/whep/*` 再包一层同源代理规避 CORS

---

## 3. AI 识别链路（决定了「画框方式」这个需求）

三条互相独立的节拍：

| 环节 | 频率 | 说明 |
| --- | --- | --- |
| 视频采集/推流 | 30 fps | 与 AI 无关 |
| 旁路抓帧 | 8 fps | `ffmpeg -f mjpeg` 只保留最新一帧覆盖写 `/tmp/detect_frames` |
| NPU 推理 | 默认 6 次/秒 | 单次 ~116ms，物理上限 ~8.6 次/秒 |
| 框叠加 | 视频 30fps / 画布 8fps | 一次结果被多帧复用 |

两种画框方式并存，前端可切换（**这正是本次需求**）：

| | 板端烧框（OSD） | 浏览器叠加（SVG） |
| --- | --- | --- |
| 链路 | 源流 → `osd.py` ffmpeg overlay → mediamtx `preview` → WHEP | 干净流 + SSE `detect` 事件 → SVG |
| 额外成本 | 二次编码 + 多一次 mediamtx 转发（延时 +200~400ms） | 零编码，纯前端绘制 |
| 优点 | 录屏/截图/转发到第三方时框自带；任何播放器都能看到 | 延时与其它路一致；框可平滑插值、可关 |
| 缺点 | 只有一路 `preview`；出图依赖 mediamtx 重启登记 path；编码失败要回退 | 客户端必须解析 SSE；iframe 降级时无法对齐 |
| 标签 | 中文字模烧进画面（`火焰 3处`） | 同文字由后端 `box.text` 统一下发 |

**关键结论：框的语义（中文名 + 同类数量 + 颜色）由后端 `_emit()` 只算一次**，
OSD、SSE、JSONL 三处复用，平台前端不要自己再数框，直接用 `counts` / `text`。

坑（平台接入要写进运维手册）：

- `preview` path 必须在 `mediamtx.yml` 里登记 `source: publisher`，改完**重启 mediamtx**；
  否则 OSD 起不来，只能回退浏览器叠加（demo 有 8s 启动确认窗口 + 6s 切流确认）。
- 画布 FIFO 写帧率（8fps）不能追平视频 30fps，否则 overlay 反压主视频，表现为「AI 那路慢几秒」。
- 红外不支持 AI；dogcam 未配置源时 demo 会自动把 AI 目标改成 cam0。
- 改中文类别名后必须重跑 `backend/tools/gen_cjk_font.py`，否则画面上是空心方框。

---

## 3.5 正式环境部署形态（2026-11-14 上线）

线上不是「平台后端代理背包」那么简单：**平台在公网、背包在现场内网**，两个硬约束决定了
最终形态。

### 约束一：业务服务连不上板端

`39.107.55.13` 访问 `192.168.41.246:8000/8889` 均不可达（已实测），云主机也没有开放
额外端口的权限（阿里云安全组白名单，实测任意端口 `18899` 被拦）。所以「服务端代理」在
正式环境不可用。

### 约束二：Chrome 禁止公网页面访问内网地址

Chromium 的 **Local Network Access**（PNA 继任者）实测拦截，`Network.loadingFailed`
的 `corsError=InsecureLocalNetwork`：

```text
页面 http://39.107.55.13:3100  ──fetch──▶  http://192.168.41.246:8000   ✗
页面 http://192.168.41.246:8080 ──fetch──▶  http://192.168.41.246:8000   ✓
```

即使后端把 `Access-Control-Allow-Origin` 配对（我们试过 `MissingAllowOriginHeader` 修好之后
仍然被拦）也不行——这是浏览器策略，不是 CORS 配置问题。而视频（WHEP + UDP 媒体）更是
必须浏览器直连板端 mediamtx，云端页面无论如何都拿不到。

### 落地形态：两个入口，一份前端

| 入口 | 用途 | 背包能力 |
| --- | --- | --- |
| `http://39.107.55.13:3100` | 常规后台（设备/任务/地图/告警） | 无（浏览器策略所限） |
| `http://192.168.41.246:8080` | **现场入口**：同一份 `dist` 托管在内网机器上 | 视频 + AI 全功能 |

现场入口的实现是 `deploy/site-console/serve.py`（标准库静态服务 + SPA 回落 +
`/runtime-config.js` 注入云端 API 地址），前端通过 `src/utils/runtime.ts` 读这份运行时配置，
**不需要为两处部署做两次构建**。

接法（`ROBODOG_BACKPACK_BROWSER_URL`）在后端被识别为 **直连模式**：

```text
现场入口页面 ──REST/SSE──▶ 板端 Agent :8000        （内网 → 内网，CORS 已放行）
             ──WHEP+UDP─▶ 板端 mediamtx :8889      （内网 → 内网）
             ──REST/SSE──▶ 云端业务平台 :3100/api   （内网 → 公网，反方向不受限）
```

板端为此补了 CORS 中间件（`AGENT_CORS_ORIGINS` 白名单 + 内网来源正则），并做成
`backpack-agent.service`（开机自启、崩溃重启、`OSD_FIFO_FPS=4`）。

### 线上验收（2026-11-14，无头 Chrome 实操）

| 步骤 | 结果 |
| --- | --- |
| 六路源 | 都能出图：四路 + 狗前置 1280×720@30，红外 256×192@25 |
| AI 开启（狗前置） | `识别中`，推理 ~118ms，实测 7~8fps |
| 运行中切「左后」 | 该路真实检出「灭火器 1处」：SVG 框 + 类别计数 + 时间线同步 |
| 画框方式 → 背包烧录 | 切到 `preview` 流，画面不中断，说明文案与状态一致 |
| 停止识别 | 3s 内回干净流、框清空、`preview` 消失 |
| 直接开云端 URL 看视频 | 被浏览器拦（预期），页面提示改用现场入口 |

### 已知运维点

- 板子 wifi IP 是 DHCP：`ROBODOG_BACKPACK_BROWSER_URL` 需随之更新（建议路由器静态租约）。
- Agent 无鉴权：不要把 `:8000` 映射到公网；`AGENT_CORS_ORIGINS` 仅放行云端控制台。
- 板端画框是纯 Python 渲染（~200ms/帧），`OSD_FIFO_FPS` 已降到 4，避免反压主视频。

### 后续（P1，若需要「不在现场也能看」）

两条路都需要额外基础设施，任选其一：

1. **WS 中继**（推荐，复用 `server/src/video.rs` 的 FLV 房间模型）：板端主动把 H264/FLV
   推到平台，浏览器同源播放。只需出站 443/3100，不受安全组与 PNA 限制，代价 1~2s 延时。
2. **HTTPS + TURN**：平台上 HTTPS（页面变安全上下文，可走 PNA 预检），mediamtx 配
   `webrtcICEServers2` 指向 443/TLS 的 TURN。需要证书、开放端口与带宽预算。

---

## 4. 平台现状与差距

`business/server/src/video.rs` 已经有一套成熟的**按需推流 + WS 中继 + FLV 补头**：

```
机器狗本体 RTSP ──ffmpeg──▶ 设备客户端 ──ws://平台/ws/device/video──▶ 平台 ──ws──▶ 浏览器(flv.js)
```

它解决的是「平台在公网、设备在内网」的问题：由设备侧主动推流上行。**背包 Demo 走的
是相反方向**（浏览器直连板端 mediamtx）。两者不能直接复用：

| | 平台现有中继 | 背包 Demo |
| --- | --- | --- |
| 通路 | 设备 → 平台（出站） | 浏览器 → 板端（局域网直连） |
| 协议 | HTTP-FLV over WS | WebRTC(WHEP) |
| 延时 | 1~2s（编码 GOP + MSE 缓冲） | ~200ms（WebRTC） |
| 多路 | 一路一个房间 | 6 路 path，任选一路播放 |

所以分阶段：

- **P0（本次实现）**：现场直连。浏览器与背包同局域网，平台后端只做 **REST/SSE 同源代理**，
  视频仍走浏览器 ⇄ mediamtx WHEP，不引入中继延时。
- **P1**：平台中继。背包 Agent 增加「向平台推流」（WHIP 或 FLV/WS，复用 `video.rs` 房间模型），
  公网/跨网段可用，代价 1~2s 延时。
- **P2**：遥测 / 控制 / SOS 告警 / AI 事件入 `alerts` 与设备详情。
- **P3**：多背包、设备-用户分配、录像与回放。

---

## 5. P0 设计（本次落地）

### 5.1 后端：`business/server/src/backpack.rs`

配置（环境变量 > `server.toml > 默认空`）：

| 键 | 环境变量 | 说明 |
| --- | --- | --- |
| `[backpack] base_url` | `ROBODOG_BACKPACK_URL` | 平台 → 背包 Agent，如 `http://192.168.234.200:8000` |
| `[backpack] webrtc_url` | `ROBODOG_BACKPACK_WEBRTC_URL` | **浏览器** → mediamtx，如 `http://192.168.234.200:8889` |

接口（全部走 `require_auth`）：

```text
GET  /api/backpack/status            聚合 cameras + relay + detect(tasks/status) → 前端一次拿全
GET  /api/backpack/events            agent SSE 透传（换行/心跳原样转发，附 agent 状态事件）
POST /api/backpack/detect/start      {task,labels,conf,cams,fps} → agent
POST /api/backpack/detect/stop
POST /api/backpack/detect/config
```

`status` 返回的 `sources` 是**已归一化**的 6 路：

```json
{ "key": "dogcam", "label": "机器狗前置", "stream": "dogcam", "kind": "rtsp-relay",
  "ai": true, "available": true, "hint": "" }
```

`available=false` 时前端把该源置灰并给出 `hint`（例如「板端未配置机器狗前置 RTSP 源」）。

### 5.2 前端：`/monitor` 单路实时视频

界面结构（桌面）：

```
[页面标题 实时视频 · 单路视频 + AI 识别]        [背包已连接] [刷新]
源切换条：狗前置 / 左前 / 右前 / 右后 / 左后 / 红外          更新于 hh:mm:ss
┌──────────────────────────────────────────────┬───────────────────────────┐
│  ┌────────────────────────────────────────┐  │ ① AI 识别面板             │
│  │ video 16:9：源名 / LIVE / AI 徽标       │  │   开关 · 场景 · 阈值 · fps │
│  │ 浏览器叠加时画 SVG 框（中文名+同类数量）│  │   类别多选                │
│  │ 底部：画框方式一句话 + 抓拍 / 全屏      │  │   画框方式（两选一）       │
│  └────────────────────────────────────────┘  │   状态：推理耗时/实测fps   │
│  四格事实条：当前通道 / AI / 画框方式 / 检出  │ ② 检出汇总（类别 chip+时间线）│
│                                              │ ③ 链路状态（Agent/relay/OSD）│
└──────────────────────────────────────────────┴───────────────────────────┘
```

实现位置：

| 层 | 文件 |
| --- | --- |
| 路由装配 | `server/src/app.rs`（`/api/backpack/*`，统一走 `require_auth`） |
| 代理 | `server/src/backpack.rs`（聚合 / SSE 透传 / AI 启停转发） |
| 配置 | `server/src/config.rs`（`[backpack] base_url` / `webrtc_url`） |
| 播放器 | `web/src/lib/whep.ts`（WHEP，非 trickle，指数退避重连） |
| 页面 | `web/src/views/monitor/index.vue` |
| 组件 | `web/src/components/backpack/{VideoStage,SourceRail,AiPanel}.vue` |
| 测试 | `server/tests/backpack_proxy.rs`（假 Agent：源归一化 / `?token=` SSE / 未配置与不可达） |

交互规则（状态机）：

1. **AI 关闭**：播放所选源的干净流；框开关不显示。
2. **AI 开启 + 浏览器叠加**：播放干净流，SSE `detect` 事件的框画在 SVG 上，
   坐标按 `object-fit: cover` 反算（与 demo 一致，`frame_w/frame_h` + 最大缩放比 + 居中偏移）。
3. **AI 开启 + 背包烧录**：等 `detect.status.osd.ready===true` 再把播放地址切到
   `preview`；6s 内无视频轨自动切回干净流并提示「已回退浏览器叠加」。
4. **切到红外**：AI 开关禁用（红外不支持），若 AI 正在跑则先 `stop` 并提示。
5. **切到未配置的 dogcam**：chip 置灰不可点，来源卡片保留并写明恢复方式。
6. 任何 REST/SSE 失败都在状态行给一句话原因 + 「重试」按钮，不弹黑框。

样式：沿用平台既有的设计令牌（`--brand #0284c7`、`--line`、`--r 14px`…），
视频舞台用深色 `#0b1220` 以保证画面与框的对比度，其余卡片保持浅色台面。

### 5.3 验收清单

- [ ] 6 个源都能单独出图，切源 ≤2s 恢复，切源不残留上一路画面
- [ ] 红外被选中时 AI 面板禁用并给出原因
- [ ] AI 开启：浏览器叠加模式下 SVG 框与画面同步（画面缩放/窗口变化不错位）
- [ ] AI 开启：背包烧录模式下播放 `preview`，框自带中文标签
- [ ] `preview` 起不来时自动回退浏览器叠加，状态行写明原因
- [ ] 停止识别后 3s 内恢复干净流并清空框
- [ ] 背包不可达时页面给出明确提示，不出现无限 loading
- [ ] 键盘可达：源 chip 可 Tab 聚焦、`aria-pressed` 正确；`prefers-reduced-motion` 下无动画
