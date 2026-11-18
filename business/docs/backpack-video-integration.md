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

- 采集/编码（四路 USB）：`1280x720@30`、`H264 High`、`2Mbps`、`GOP30`、无 B 帧、无音轨
- dogcam 归一化（`relay.py`，源 1080p60 HL4.0 full-range）：默认 `1920x1080@30`、`4Mbps`，
  由板端环境变量 `DOGCAM_WIDTH/HEIGHT/FPS/BITRATE/GOP` 控制；**故意不复用 `CAPTURE_*`**
  （那组同时管 v4l2 采集尺寸，改了会连带影响四路 USB）。带宽吃紧时回退：
  `DOGCAM_WIDTH=1280 DOGCAM_HEIGHT=720 DOGCAM_BITRATE=2000k`。
- 平台中继（`deploy/backpack-relay/relay_push.py`）默认 `RELAY_PROFILE=light`：
  缩到 **720p15 / 1.0Mbps**（编码器自动挑 `h264_rkmpp` → `h264_v4l2m2m` → `libx264`）。
  原因见 §6.10：平台公网出口实测 ≈2.5Mbps，`-c:v copy` 的 1080p60（4~8Mbps）必卡。
  `RELAY_PROFILE=copy` 只在出口 ≥6Mbps 时用；页面上的分辨率由播放器实测上报
  （`VideoPlayer` / `VideoStage` 的 `meta` 事件），不写死规格。
- 背包烧录（`preview`）仍按 `CAPTURE_*` 缩放：`DOGCAM_*` 调到 1080p 时，
  带框那一路仍是 720p30（OSD 画布 `OSD_W/H` 是 640x360 放大），属预期。
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
| `http://39.107.55.13:3100` | 常规后台（设备/任务/地图/告警） | **视频**走平台中继、**AI**走设备客户端隧道（2026-11-17 起） |
| `http://192.168.41.246:8080` | **现场入口**：同一份 `dist` 托管在内网机器上 | 视频 + AI 全功能，延时最低 |

> 云端入口以前是「背包能力彻底没有」（浏览器 PNA 拦截 + 平台连不到板端）。现在两条独立通道
> 补上了：视频由板端推流到平台房间（§6），接口/AI 由设备客户端代调（§6.8）。浏览器与背包
> 是否同网段，只影响「走不走中继、延时多少」，不再影响「能不能用」。

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
  - ✅ 已落地（`deploy/backpack-relay/relay_push.py` + `/api/backpack/video/ws`），见 §6；
  - ✅ 跨网段的**接口/AI** 另走设备客户端隧道（§6.8），不依赖中继。
- **P2**：遥测 / 控制 / SOS 告警 / AI 事件入 `alerts` 与设备详情。
  - ✅ 已落地「**航点到点 AI 识别**」：巡检走到带 `AI 识别` 动作的航点时自动开识别窗口，
    检出的目标写进 `alerts` + `task_events`（`server/src/ai_patrol.rs`，
    状态见 `GET /api/backpack/ai-window`，决策与边界见
    [`official-app-parity.md`](official-app-parity.md) §5.6）；窗口时长跟着停留时间走
    （停留 + 8s，离开航点立即结束）；正式环境因平台在公网连不到板端，改由**设备客户端
    代调**同机 Agent（`[backpack] point_ai`，见同一节 B 段）；
  - ✅ 路线编辑器的「AI 识别」动作**复用本页的模型目录**（`/api/backpack/status` 的
    `detect.tasks`）：场景 + 识别类别直接勾选，存成动作的 `scene` + `labels`，
    到点开窗口时原样下发，不再靠中文关键词猜场景；
  - 遥测 / SOS / 其它事件的入库仍未接。
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

---

## 6. 推流中继（2026-11-16 上线）：让公网入口也能看

### 6.1 为什么必须做

§3.5 的两个入口方案有个绕不过去的副作用：**「实时视频」能不能看，取决于操作员那台
电脑**，而不是平台。现场实测到的三种翻车方式：

| 变化 | 结果 |
| --- | --- |
| 换一个 WiFi（`192.168.41.x` → `192.168.43.x`，同一条 `/22`） | REST/SSE 全通、WHEP 信令通，但媒体 ICE 一直 `deadline exceeded` → 黑屏 |
| 用公网地址 `39.107.55.13:3100` 打开 | Chrome LNA 拦掉页面 → `Failed to fetch`，除非本机手动「允许本地网络访问」 |
| 清浏览器数据 / 换台电脑 | 上面那条许可没了，又黑 |

浏览器那侧的原理（两闸门，缺一不可）：

1. **Local Network Access**：公网 origin 访问私有地址要许可（Chrome 141+ 灰度、157 实测强制）。
   按站点记忆，会因清数据/升级失效；无头浏览器默认拒绝。
2. **媒体通道**：WebRTC 走 UDP `8189`。同网段正常；跨 VLAN/代理/部分 AP 会被拦，
   症状是 mediamtx 日志 `deadline exceeded while waiting connection`。已在板端
   `mediamtx.yml` 打开 `webrtcLocalTCPAddress: ':8189'` 作为 ICE-TCP 回退（延时略高）。

这两条都不是我们能在服务端修的，所以换方向：**板子主动出公网推流**。

### 6.2 链路

```text
本地 mediamtx :8554/<stream>
  └─ ffmpeg -rtsp_transport tcp -use_wallclock_as_timestamps 1 \
            -vf scale=720p,fps=20 -b:v 1200k -f flv -   （默认 light；copy 见 §6.10）
       └─ ws://<平台>/ws/device/video       video_register(deviceId="bp:<stream>") + 二进制 FLV
            └─ 平台 VideoHub 房间 bp:<stream>  ──ws──► 浏览器 flv.js
                 GET /api/backpack/video/ws?stream=<stream>&token=…
```

- 服务端：`business/server/src/video.rs`（`BACKPACK_ROOM_PREFIX = "bp:"`、`backpack_viewer_ws`）
  + `app.rs` 路由 `/api/backpack/video/ws`（在 `require_auth` 层内，`?token=` 鉴权）。
- 板端：`deploy/backpack-relay/relay_push.py` + `backpack-relay.service`。
- 前端：`business/web/src/views/monitor/index.vue`（顶部「现场直连 / 平台中继」切换，
  直连不可达时自动切中继）；中继播放器就是 `VideoPlayer.vue`（现在只播背包中继）。

### 6.3 三条协议约定（改这块必须同时看）

1. **房间名**：`bp:<stream>`。带这个前缀的推流连接才会被挂上**控制通道**，
   观看者的 `video_start`/`video_stop` 直接写进它自己那条推流连接（不走设备网关）。
2. **`video_reset`**：板端每次起 ffmpeg 前先发一条，服务端据此清掉该房间的
   `init`/`pending`。常驻连接如果不清旧流头，后加入的观看者会拿到「上一轮的关键帧 +
   这一轮的中段字节」，flv.js 解析错位后卡 `readyState=0`（黑屏）。
3. **有流头就补，不分第几个观看者**：只要房间里存着**当前这一轮**的 `init`，任何观看者
   加入都先收它（FLV 魔数 + AVC 配置 + 首个关键帧）。
   早先的规则是「第一个观看者不补头」，前提是它的加入必然拉起一条新流；现在没人看时
   推流端不立刻被杀（见 6.4），重复的 `video_start` 也被推流端按地址去重了 ——
   不起新流却又不补头，浏览器只能收到中段字节，flv.js 解析错位后黑屏 → 重连 → 再黑，
   现场看到的就是「一闪一闪的连接中断」。
   流头不会串轮次：板端 relay 起 ffmpeg 前发 `video_reset`（第 2 条），推流端重连时
   整个房间被换掉（`VideoHub::publisher_attach`），两种情况都会清旧头。

### 6.4 按需与配额

- 常驻 6 条 WS（`cam0..cam3`、`dogcam`、`dog-ipcam`），空闲时每条 ~1KB；
- 第一个观看者 → `video_start` → 推流端起 ffmpeg 或 RTSP 拉流；
- 最后一个观看者离开 → **不立刻停**，只保留 `streaming` 标记；看门狗每 15s 兜一次
  「起过流但没人看」，超过 `STREAM_IDLE_GRACE`（60s）才下发 `video_stop`。宽限期内
  重新打开是**秒开**：流还在跑，服务端直接把流头补上，不用等 ffmpeg 重启；
- 推流端按 stream 去重 `video_start`（`deploy/backpack-relay/relay_push.py`）：同一路
  已经在推就不重启 ffmpeg（重启 = 黑 3~7s + 换新 FLV 头 = 一次闪断）；
- 单路 2Mbps：六路同时看 = 12Mbps 上行，现场带宽要先确认。

### 6.4.1 排障：`GET /api/backpack/video/state`

返回每个房间的 `publisher / initBytes / pendingBytes / viewers / streaming / control`。
「这一路黑屏」九成能从这张表看出来：`publisher=false` → 板端 relay 没连上；
`initBytes=0` 且 `viewers>0` → 推流端起流失败或还没出关键帧；`viewers` 不掉 → 有僵尸观看者。

```bash
TOKEN=<控制台登录拿到的 token>
curl -s -H "Authorization: Bearer $TOKEN" http://39.107.55.13:3100/api/backpack/video/state | python3 -m json.tool
```

### 6.5 已知限制

- 延时 1~2s（WHEP 直连 ~200ms）；延迟敏感场景仍建议现场入口。
- **首次看某一路要 3~7s**（ffmpeg 启动 + 等下一个关键帧）；同一个通道 30s 内再切回来是
  秒开 —— 最后一个观看者离开时不立刻停流，交给看门狗在宽限（`STREAM_IDLE_GRACE`，60s）
  过后才停。
- 中继模式下**浏览器叠加用不了**（中继流是 FLV，没有与抽帧对齐的坐标关系）：想让框跟着画面走
  就切「背包烧录」，板端把中文标签烧进 `preview` 流后一起中继（`RELAY_STREAMS` 需含
  `preview`，默认已含）。AI 的启停 / 模型目录 / 运行态 / 检出数量走设备客户端隧道（§6.8），
  与是否走中继无关。
- 板端 relay 的推流连接是常驻的，**每次起流换一条新 FLV**，所以服务端必须靠 `video_reset`
  清 `init`，否则中途加入的观看者会拿到「上一轮关键帧 + 这一轮中段字节」而黑屏。
- `dog-ipcam` 源若为 H.265，`-c:v copy` 无法封装成 FLV，需 `RELAY_TRANSCODE=1`
  （重编码，板端 CPU 开销大，优先改摄像头出 H.264）。

### 6.6 为什么「设备管理」里的视频会闪（2026-11-16 修，旧链路已于 §6.9 删除）

> **历史记录**：这里说的是已删除的「设备客户端拉 `video_url` → 平台中继」链路。
> 结论（补流头 / 60s 宽限 / 观看端不闪）对现在的背包中继同样成立，只是推流端换成了
> 板端 `backpack-relay`；`business/client/src/video.rs` 已删除，别再照它改配置。

现场现象：设备管理 → 控制抽屉里的实时监控画面一闪一闪，角标在「LIVE」与
「连接中断」之间反复跳；而同一台狗的前置摄像头在「实时视频」页用平台中继看却很稳。
两条链路其实是同一个 `VideoPlayer.vue` + 同一个 `VideoHub` 房间机制，
差别全在**推流端参数**和**观看端重连策略**上，四个真因都改了：

| # | 真因 | 修法 |
| --- | --- | --- |
| 1 | 客户端 ffmpeg 写管道**带缓冲**，转码小帧被攒成突发（板端 relay 一直带 `-flush_packets 1`，所以它稳） | `business/client/src/video.rs`（已删除）输出加 `-flush_packets 1 -muxdelay 0 -muxpreload 0`，输入加 `-fflags +genpts` |
| 2 | 浏览器 WS 一抖，服务端就算「第一个观看者」，重发 `video_start` → 客户端**重启 ffmpeg** → 黑 3~7s + 新 FLV 头 | 客户端按地址去重；服务端最后一个观看者离开时留 15~30s 宽限（看门狗收尾） |
| 3 | 宽限生效后「第一个观看者不补流头」就变成致命 bug：浏览器只收到中段字节 → flv.js 解析错位 → 黑屏 → 重连 → 循环 | 服务端改成**有流头就补**（不串轮次靠 `video_reset` / 房间替换） |
| 4 | 观看端 `waiting` 就盖全屏错误层、重连先 `destroy` 清空画面、固定 3s 一次重试 | `VideoPlayer.vue`：缓冲只给角标；断流先抓末帧当 poster；250ms 起退避封顶 3s；3s「时间轴不前进」看门狗；flv.js `enableStashBuffer: false` |

排障顺序（按成本从低到高）：

1. `GET /api/backpack/video/state` 看 `viewers` 是否在 0/1 之间跳 —— 跳就是 2/3 类问题；
2. 设备客户端日志里是否反复出现 `ffmpeg 已启动` / `video publisher reconnected` —— 是就没去重成功；
3. 浏览器 Network → WS 帧时间轴：一秒钟一串小帧 = 正常；半秒一团再静默 = 推流端没 flush；
4. 还闪就看 `viewer of <room> lagged` 日志：观看端网络追不上，服务端现在会主动断开让它拿
   干净的新流（早先是从中间丢帧继续发，FLV 字节流错位后才一直黑屏重连）。

### 6.6.1 手动遥控视频「连接很慢」（2026-11-17 现场，旧链路已删除）

> 同样是历史记录：当时慢在设备客户端 ffmpeg；现在遥控页画面也走背包中继（§6.9），
> 板端 relay 侧如果起流慢，按 §6.7 的顺序查。

现象：遥控页视频长时间停在「连接中 / 重连中」、偶尔黑一下。实测（云端直连中继 WS +
板端 ffprobe/ffmpeg）：

- 板端 `ffmpeg` 被相机侧抖动打退：`ffmpeg 结束（已传 N 字节），3s 后重试` —— 每次黑 4~5s；
- 最后一个观看者离开后看门狗 15s 内就 `video_stop`，浏览器刷新 / 切页签（只断几秒）
  也要重新起 ffmpeg + 等关键帧；
- 前端 3s 没新帧就判「假死」重连，重连又逼服务端重新拉流，越连越慢；
- 相机侧卡住时 ffmpeg 不会失败：画面定格，连接仍显示在线；
- 本体 RTSP 是 1080p、SDP 标 60fps，实际投递明显低于标称（`ffmpeg -f null` 里
  `speed` < 1x）。

改动：

| 位置 | 改动 |
| --- | --- |
| `business/client/src/video.rs`（已删除） | RTSP 输入加 `-timeout 8000000`（8s socket 读超时，卡住就报错让外层重启）；`RETRY_DELAY` 3s → 1s |
| `business/server/src/video.rs` | `STREAM_IDLE_GRACE = 60s`：最后一个观看者离开后留 60s 宽限才停流，期间新观看者立刻拿到缓存流头 + 关键帧 |
| `business/web/src/components/VideoPlayer.vue` | `STALL_MS` 3s → 8s |

复用排查手法：

```bash
# 云端：量首字节延迟 / 速率 / 收包间隔（替代肉眼看浏览器）
python3 - <<'EOF'
import websocket, time
url = "ws://127.0.0.1:3100/api/backpack/video/ws?stream=dogcam&token=<JWT>"
ws = websocket.create_connection(url, timeout=30); t0=time.time(); n=0
while time.time()-t0 < 10:
    op, data = ws.recv_data()
    if op == websocket.ABNF.OPCODE_BINARY: n += 1
print("首帧 %.2fs，消息 %d，%.0f kbps" % (time.time()-t0, n, 0))
EOF

# 板端：源流到底跟不跟得上实时（speed < 1x 就是源侧问题，不是平台）
ffmpeg -rtsp_transport tcp -i "rtsp://<背包IP>:8554/dogcam" -an -t 5 -f null - 2>&1 | tail -2
```

### 6.7 WHEP `HTTP 404` 排查（2026-11-17 现场事故，已修）

**症状**：前置摄像头一直「WHEP 协商失败（HTTP 404），2 秒后重连」，四路 USB 与红外都正常。

**结论**：跟浏览器、网络、账号都没关系 —— **板端 mediamtx 上没有 `dogcam` 这条 path**。
介质层证据（都不用进浏览器）：

```bash
# ① mediamtx 自己怎么说（404 的原文就是这句）
curl -s -X POST -H 'Content-Type: application/sdp' --data-binary 'v=0' \
  http://192.168.41.246:8889/dogcam/whep
# {"status":"error","error":"no stream is available on path 'dogcam'"}

# ② 同一个 path 的 RTSP 也一样 404（说明是 path 层面没流，不是 WHEP 的事）
ffprobe -v error rtsp://192.168.41.246:8554/dogcam          # 404 Not Found
ffprobe -v error rtsp://192.168.41.246:8554/cam0            # h264 1280x720 30（对照组）

# ③ 源本身是好的：3588 的 1080p60 直连能出流
ffprobe -v error "$(cat /root/261113/backpack_dashboard/backend/dog_rtsp_url)"
# h264,1920,1080,60/1

# ④ 活配置里根本没有 dogcam（生效的是 snap 的那份）
ssh root@192.168.41.246 'sed -n "/^paths:/,$p" /var/snap/mediamtx/common/mediamtx.yml'
# 事故现场只有：dog-front-cam（source 不带账号密码 → 每 5s 401 Unauthorized）
#             dog-ipcam、all_others
journalctl -u snap.mediamtx.mediamtx | grep 401     # [path dog-front-cam] bad status code: 401
```

链条：`1080p60` 档位下 relay **不参与**（`relay.py`：1080p60 = mediamtx 直拉 `dogcam`，
只有 `720p30` 才由 relay 推 `dogcam720`），所以前置能不能看完全取决于 `mediamtx.yml`
里有没有一条**能拉通**的 `dogcam`。事故现场被写成了 `dog-front-cam`（名字和前端/
`/api/cameras` 的 `stream` 对不上），而且 source 少了 3588 的 `user:…@`，于是：
path 名字没人读、拉流 401 循环、前端拿 `dogcam` 去要流 → 404。

**修法**（板端，改完 `snap restart mediamtx`；cam0~cam3 由 5s 看门狗重新推上来，实测秒级恢复）：

```yaml
# /var/snap/mediamtx/common/mediamtx.yml —— 名字必须是 dogcam，source 必须带凭据
paths:
  dogcam:
    source: 'rtsp://user:******@192.168.234.1:8554/test'   # 取 backend/dog_rtsp_url 第一行
    sourceOnDemand: no
```

- 这个文件现在含口令，`chmod 600`（快照里那份 `mediamtx.yml` 备份也一起注意）；
- 改完自检：`ffprobe rtsp://127.0.0.1:8554/dogcam` → `h264,1920,1080,60/1`；浏览器
  WHEP 真握手实测 `HTTP 201` + `ICE connected`（≈200 ms）。
- 不想在前置上写凭据时，另一条路是把档位切到 `720p30`（`POST /api/dogcam/profile`）：
  relay 用 `dog_rtsp_url` 里的凭据拉流、重编码推 `dogcam720`，只需再确认
  `dogcam720` 落在 `all_others`（publisher）里即可，不动 `mediamtx.yml`。代价是重编码 + 720p30。

**踩坑记录**：`GET /api/relay/status` 在 `1080p60` 档位把 `running` 硬编码成 `true`
（板端 `relay.py` 的既有实现，不在本仓库），平台据此把前置判为「可用、无 hint」，
于是错误只能从 WHEP 状态里暴露。前端因此把 404 翻成人话（`lib/whep.ts` 的
`whepFailure`）：「板端 mediamtx 上没有「dogcam」这条流（path 未登记 / 名字不一致 /
没有推流）」，省掉一轮猜。

### 6.8 跨网段的接口/AI：设备客户端隧道（2026-11-17）

**要解决的问题**：操作员电脑与背包不在同一网段时（换 WiFi、出差远程、走云端入口），
背包页会写「背包不可达，暂不能开启 AI」——模型目录、运行态、AI 启停全靠浏览器直连
`192.168.x.x:8000`，而 Chrome 的 Local Network Access 也不允许公网页面访问内网地址。
巡检到点的 AI 早就由设备客户端代调（`official-app-parity.md` §5.6 B 段），但**手动开的
识别**没有对应通道。

**做法**：设备客户端本来就跑在背包那块板子上（`127.0.0.1:8000` 一定通），而且有一条到
平台的常驻设备 WS。把背包页要的四件事挂到这条已有信道上，不新增端口、不依赖中继：

```text
浏览器 ──GET  /api/backpack/status────────▶ 平台 ──agent_request──▶ 客户端 ──HTTP──▶ Agent
浏览器 ──POST /api/backpack/detect/start──▶ 平台 ──agent_request──▶ 客户端 ──HTTP──▶ Agent
浏览器 ◀──SSE /api/backpack/events───────── 平台 ◀──agent_event──── 客户端 ◀──SSE── Agent
```

| 环节 | 代码 | 说明 |
| --- | --- | --- |
| 通道选择 | `server/src/backpack.rs::status` | `direct`（浏览器直连）→ `proxy`（服务端直连）→ `tunnel`（客户端代调）依次退让；`?transport=tunnel` 强制走隧道 |
| 服务端隧道 | `server/src/agent_tunnel.rs` | 设备登记（`agent_hello`）、请求表（`requestId` + oneshot，超时清理）、检出事件广播 |
| 消息收发 | `server/src/gateway.rs` | `agent_hello` / `agent_result` / `agent_event` 三类 |
| 客户端代调 | `client/src/agent_relay.rs` | 白名单转发 + 常驻订阅 Agent `/api/events`（只转 detect 四类） |
| 页面回退 | `web/src/views/monitor/index.vue` | 直连探测失败且 `tunnel.available` → 带 `transport=tunnel` 重拉 |

**白名单**：4 个只读接口（`/api/cameras`、`/api/relay/status`、`/api/detect/tasks`、
`/api/detect/status`）+ 3 个动作（`start` / `stop` / `config`）。板端 Agent 没有鉴权，
做成通用代理等于把现场内网的任意 HTTP 能力暴露给控制台账号，两端各自校验一遍。

**不缓存**：页面本来就 8s 轮询一次，服务端按需问一次客户端（4s 超时）。好处是「客户端掉线 /
Agent 挂了」立刻表现为 `reachable=false`，没有一份迟早过期的快照要维护。

**排障顺序**：

1. `GET /api/backpack/status` 看 `mode` 与 `tunnel.available`：
   - `mode=tunnel` + `reachable=true`：隧道正常；
   - `tunnel.available=true` 但 `reachable=false`：客户端在线、本机 Agent 不通 →
     在板子上 `curl 127.0.0.1:8000/api/cameras`，并看客户端日志有没有
     `板端 Agent 隧道已登记`（没有说明 `hello` 探测就失败了）；
   - `tunnel.available=false`：设备客户端没在线，或它 `[ai] enabled=false`。
2. 客户端日志搜 `agent_request` / `板端 Agent 事件流已连接`：
   前者是页面的请求到达了，后者是检出事件在转发（没有这行则页面收不到框）。
3. 只差框不动、数量也不变：看板端 Agent 是不是根本没在识别
   （`/api/detect/status` 的 `running`），隧道只负责搬数据。

**多背包**：隧道默认取「最近上报且在线」的那台。园区里有多台背包时必须配
`ROBODOG_BACKPACK_TUNNEL_DEVICE=<设备号>`，否则页面可能问到另一台狗上。

### 6.9 设备管理 / 计划管理的视频统一到前置 `dogcam`

**要解决的问题**：设备管理的控制抽屉、计划管理的「实时监控」原来播放的是
`devices.video_url`（设备客户端 ffmpeg 拉本体 RTSP → 平台中继），而同一个本体前置在
「实时视频」页走的是背包 relay 归一化的 `dogcam`。两套推流端、两套参数（§6.6 / §6.6.1 的
闪断、起流慢都出在 `video_url` 那条路上），现场就会出现「实时视频很稳、抽屉一直重连」。

**最终形态（2026-11-18）**：不保留兜底 —— 旧链路整条删除：
`/api/devices/{id}/video/ws`、`VideoSource::DeviceClient`、`gateway::send_video_control`、
`client/src/video.rs`（ffmpeg 拉流）、客户端 `[video]` 配置段、设备表暴露的
`videoUrl` / `videoType`（DB 列保留但代码不再读写）、设备编辑弹窗的两个视频字段。
没接背包的部署只看到说明卡，不再退到本体 RTSP。

**做法**：新增 `web/src/components/FrontCameraVideo.vue` + `web/src/composables/useFrontCamera.ts`，
三处（设备抽屉「实时监控」、抽屉「手动遥控」内嵌画面、任务实时监控）**同一份组件、同一路流**：

| 判定 | 播放 | 说明 |
| --- | --- | --- |
| 板端 relay 在线（`/api/backpack/video/state` 里 `bp:dogcam` 的 `publisher=true`） | `VideoPlayer`（中继 `bp:dogcam`） | **默认**：设备管理 / 计划管理固定中继优先，与实时视频的「平台中继」同一条通路 |
| 中继推流端不在，且页面在内网 / 直连探通 | `VideoStage`（WHEP `dogcam`） | 兜底：现场入口还能看，不至于直接黑屏 |
| 中继推流端不在，且页面在公网 | 仍按中继（等板端 relay 恢复） | — |
| `status.configured=false`（部署没接背包） | 只给「怎么配背包」的说明卡，不兜底、不拉流 | — |

要点：

- **中继优先**：`frontCameraPath()` 只看「板端 relay 是否在线」（`bp:dogcam` 的 publisher），
  在线就走中继；不在线才退回 WHEP（现场入口/内网）。实时视频页仍保留手动「现场直连 /
  平台中继」切换，判定逻辑不共用（2026-11-18 现场要求设备/任务页默认中继）。
- **status 共享**：`/api/backpack/status` 要聚合板端 4 个接口（含 `detect/tasks`），
  抽屉/弹窗可能同时存在，所以 `useFrontCamera` 做进程内缓存（15s）+ 单飞合并，
  挂载期间 20s 低频刷新（页面隐藏时不刷）。
- **不抢 AI / 不拉 preview**：这里永远放干净流 `dogcam`；背包烧录（`preview`）与浏览器叠加
  只属于「实时视频」页，避免两个页面互相覆盖板端画框会话。
- **延时**：直连 ≈200ms，中继 1~2s，与「实时视频」页同一量级；中继首次起流 3~7s
  （板端 relay 已常驻，通常秒开）。

**排障**：抽屉/任务页黑屏时先看「实时视频」页的同一路是否正常 —— 同源，正常则问题在
这一页的 `status`（登录态、`?transport=tunnel`、页面是否公网打开）；两边都黑就是 §6.7 的
`dogcam` path 问题，按 `ffprobe rtsp://<背包IP>:8554/dogcam` 往下查。

### 6.10 「平台中继很卡」排查与修复（2026-11-18 上线验证）

现场反馈：实时视频切「平台中继」很卡（甚至每 20 多秒断一次），切「现场直连」反而流畅；
只是背包不是总能直连。2026-11-18 在正式环境逐项实测，**三个独立原因**，都已修：

#### ① 每 23s 精确断一次：`init` 残留导致不发 `video_start`

- 现象：平台日志 `last viewer of bp:dogcam left` 的时间戳精确间隔 23.0s；
  观看端先「正在连接平台中继」，20s 连接超时（`CONNECT_TIMEOUT_MS`）后重连。
- 根因：最后一个人看完 → 看门狗 `video_stop` → 板端杀掉 ffmpeg，**但房间里的
  `init`（FLV 头 + 关键帧）还留着**。旧判断是
  「`first && (!has_init || !has_publisher)` 才发 `video_start`」，
  于是新观看者看到「有 init + publisher 在线」→ 永远不重新起流 → 只能收到 40KB 旧流头。
  `GET /api/backpack/video/state` 可直接看到 `initBytes: 41361` + `publisher: true` 但没有新字节。
- 修复（`server/src/video.rs`）：**每个「第一个观看者」都发 `video_start`**
  （板端 `StreamSession.start()` 幂等，已在推的流不会被重启）；看门狗停流时
  `reset_stream()` 清掉上一轮 init。
- 回归：`server/tests/video_relay.rs::stale_stream_header_does_not_block_video_start`。

#### ② 码率只有 400kbps / 7fps：板端出公网走的是 4G dongle

- 现象：`measure_bitrate.py` 量到 `dogcam: 397 kbps, 7 fps`（light 档位却是 1200k/20fps），
  板端 `推流结束` 的字节数也对得上 ~0.5Mbps。
- 根因：板子的默认路由是 **4G dongle**（`10.128.102.x`，`enx020c29a39b6d`，metric 100），
  实测上行只有 **0.56Mbps**；而 Wi-Fi `HuaZhi`（`wlp4s0`）上行 **25Mbps**。
  `ws.send` 被上行带宽反压 → ffmpeg 输出阻塞 → 实际帧率/码率被压到链路上限。
- 修复：**只给平台 IP 加一条 /32 路由走 Wi-Fi**（不动默认路由，避免影响其它服务）：
  `deploy/backpack-relay/50-platform-route.sh` → 安装到
  `/etc/NetworkManager/dispatcher.d/50-backpack-relay-route`（Wi-Fi up / dhcp4-change 时自动补）。
  ```bash
  ip route replace 39.107.55.13/32 via 192.168.40.254 dev wlp4s0   # 手工等价命令
  ```
  改完必须 `systemctl restart backpack-relay`：已有 WS 连接走的是旧路由（conntrack），不重启不生效。

#### ③ 平台公网出口 2.5Mbps < 1080p60 copy 的 4~8Mbps

- 平台 `speedtest-cli --simple`：Download 10.57 Mbps / **Upload 2.53 Mbps**（出口就是它）。
- 中继原默认 `-c:v copy` 把 1080p60 原样推出去（4~8Mbps）→ 浏览器侧 `sink.send` 反压 →
  广播缓冲溢出 → `lagged` 断开重连（24h 内 11 次）。
- 修复：`relay_push.py` 默认 `RELAY_PROFILE=light`（720p20 / 1.2Mbps / GOP 2s，
  编码器试编挑 `h264_rkmpp` → `h264_v4l2m2m` → `libx264`）。带宽紧再压
  `RELAY_BITRATE=800k RELAY_FPS=15`；出口 ≥6Mbps 才考虑 `copy`。

#### 验证（2026-11-18 10:10 发布后，同一环境）

```text
# 平台机器上（loopback，不占公网出口）
dogcam: 1146 kbps, 19 fps, 收到 2.74 MB / 20.1s
# 等 75s 让看门狗 video_stop（旧代码这里会残留 init 导致起不来）
dogcam: 1156 kbps, 19 fps, 收到 2.07 MB / 15.0s
# 平台日志
last viewer of bp:dogcam left after 20.0s / 2806 KB（平均 1147 kbps）
# 板端日志
[dogcam] 开始拉流（profile=light，1280x720@20 1200k enc=h264_rkmpp）
```

`lagged` 不再出现；`last viewer ... left` 不再按 23s 周期出现。

#### ④ 板端重启 / 本体 RTSP 未就绪时的 WHEP 404（2026-11-18）

现场报「板端 mediamtx 上没有「dogcam」这条流（path 未登记 / 名字不一致 / 没有推流）」，
但 `mediamtx.yml` 里 `dogcam` 明明是登记好的。查下来是**上游没就绪**：

```text
# 板端 mediamtx 日志
ERR [path dogcam] [RTSP source] dial tcp 192.168.234.1:8554: connect: connection refused
ERR [path dogcam] [RTSP source] bad status code: 404 (Not Found)
INF [path dogcam] stream is available and online, 1 track (H264)     # 几秒后又好了

# 板端启动记录：09:12 / 10:32 / 10:39 三次「重启」，都没有干净的 shutdown 日志（硬复位/掉电）
$ last -x reboot
```

板端（`.246`）会随本体一起断电/复位，mediamtx 起来时本体 RK3588 的
`media_server`（`192.168.234.1:8554/test`）还没就绪，`dogcam` 就没有流 → 直连页面的
WHEP 拿到 404。处理（已按此配置）：

```yaml
# /var/snap/mediamtx/common/mediamtx.yml（snap 的活配置）
  dogcam:
    source: 'rtsp://user:…@192.168.234.1:8554/test'
    sourceOnDemand: yes            # 有人看才拉流：板子刚启动时观看者被挂住等，而不是立刻 404
    sourceOnDemandStartTimeout: 15s
    rtspTransport: tcp             # 1080p60 走 UDP 会丢包（RTP packets lost），固定 TCP
  # 已删除遗留的 dog-front-cam（source 没带凭据 → 每 5s 401 重试，纯噪声）
```

- 前端 404 文案也改成了能指导现场的话（「本体 RTSP 未就绪 / 正在重连，10~30 秒会自动恢复」）；
- 没人看时不再常驻拉 1080p60，本体 `media_server` 的负载也小一截；
- 中继（`backpack-relay` 拉本地 RTSP）同样受益：它是个 reader，会触发按需起流。

**仍要现场关注**：板端为什么反复硬复位（供电 / 看门狗 / 与本体急停、重启同源）。
只要板端或本体在重启，直连视频一定会短暂中断；现在最多等 15s 自动恢复，不会再立刻报 404。

#### ⑤ Wi-Fi 掉线 / 出口切走以后画面冻住（2026-11-18，板端 relay 已补）

现场反馈：「平台中继好像有一部分还是走狗体的 Wi-Fi，Wi-Fi 连不上以后画面就卡住了」。
检查下来不是浏览器的问题，而是**板端到平台这条 WS 在链路死掉以后不会自己恢复**，
三个叠加点：

| # | 位置 | 原来会怎样 |
| --- | --- | --- |
| 1 | `relay_push.py` 的 `ws.send` | 没有超时。Wi-Fi 一断，TCP 是**半开**状态（不 FIN 也不 RST，`send` 既不报错也不返回），整条管道被反压到停住；`ffmpeg` 还活着，`StreamSession.running` 仍是 `True`。 |
| 2 | 平台 + relay 的 `video_start` | 新观看者重连后平台会再发 `video_start`，但 relay 用 `if self.running: return` 幂等吃掉 —— 永不重启，只能 `systemctl restart backpack-relay`。平台侧则要 60s 没消息才判推流端死。 |
| 3 | 出口路由 | 平台 /32 路由压在 Wi-Fi 上；Wi-Fi 断开后内核撤掉这条路由、流量回落默认路由（4G dongle ~0.5Mbps 或狗体内网 eth0），但**已建立的 TCP 不会自己换**。Wi-Fi 恢复后旧连接也还挂在慢链路上。 |

修法（板端 `backpack-relay` 升级 + 一行路由脚本，平台侧只收紧判定）：

| 位置 | 改动 |
| --- | --- |
| `relay_push.py` `_pump` | 每个分片的 `ws.send` 加 `RELAY_SEND_TIMEOUT`（默认 10s）。超时 = 上行已死：标记拥塞 → 关掉连接 → 外层 1~15s 退避重连，`ffmpeg` 随连接一起被杀。 |
| `relay_push.py` `_watch_stall` | 新增卡住看门狗：起流后 `RELAY_STALL_TIMEOUT`（默认 15s）一个字节都没推出去（RTSP 半死 / 相机卡住）就重启 `ffmpeg`（保留 WS，先发 `video_reset` 清平台旧流头），观看端不用重连。 |
| `relay_push.py` `route_watcher` | 每 `RELAY_ROUTE_POLL`（默认 5s）`ip route get <平台IP>`，**出口网卡一变就断开全部推流连接**。Wi-Fi 断开回落 4G、Wi-Fi 恢复切回快链路，都靠它收敛（日志：`平台出口网卡 wlp4s0 → enx…`）。 |
| `relay_push.py` 自适应档位 | 按 `RELAY_CONGEST_WINDOW`（5s）窗口统计 `ws.send` 占用与实测码率：**占用高 + 码率明显低于目标档位**（或单个分片 `send` 超时）即判「上行吃不下」→ 立刻发 `video_reset` + 重启 `ffmpeg` 换 `640x360@12 / 500k`（`RELAY_DEGRADE_*`），**观看端不用重连**。旧逻辑要等 10s 超时 → 断连 → 重连才降得下来，正好错过最卡的那十几秒。 |
| `relay_push.py` 回档 | 降档后 `RELAY_DEGRADE_TTL`（默认 300s）到期**不盲目回配置档**：还要 `send` 占用低于 `RELAY_RECOVER_DUTY` 并持续 `RELAY_RECOVER_STABLE`（20s）才升回；链路仍拥塞就继续省流。刚回档又拥塞（`RELAY_FLAP_WINDOW` 内）= 链路没真恢复，窗口翻倍（上限 `RELAY_DEGRADE_TTL_MAX`），避免反复升降。 |
| `relay_push.py` RTSP 输入 | 加 `-timeout 8000000`（8s socket 读超时）：源侧不吐数据时让 `ffmpeg` 报错退出，配合上面的看门狗。 |
| `50-platform-route.sh` | 平台 /32 路由**真的变了**才 `systemctl try-restart backpack-relay`（不变不动，避免 dhcp4-change 抖动断流）。Wi-Fi 回来立即切回快链路。 |
| `server/src/video.rs` | 推流端静默判定 60s → `PUBLISHER_SILENCE` 30s（板端 10s 一次心跳，3 次没到即判死）；`/api/backpack/video/state` 新增 `lastFrameAgeSecs`。 |

**排障顺序**（画面又冻住时）：

1. `GET /api/backpack/video/state`：`lastFrameAgeSecs` 一直涨 → 板端没新字节（Wi-Fi / 源侧）；
   一直是 `null` + `initBytes=0` → 起流就没成功；
2. 板端 `journalctl -u backpack-relay -f`，认这几行：
   - `上行阻塞超过 10s…断开重连` → 上行真死了（看 `ip route get 39.107.55.13`）；
   - `15s 没有推出任何字节…重启拉流` → 源侧卡住（`ffprobe rtsp://127.0.0.1:8554/<stream>`）；
   - `平台出口网卡 A → B` → 出口切换过（B 是 `eth0` 就说明走了狗体内网，见 `deploy/README.md` 三）；
   - `开始拉流（profile=省流，640x360@12 500k…）` → 上行还在拥塞窗口里；
   - `5s 内实测 400kbps（目标 1200kbps，send 占用 95%）→ 改用省流档…` → 降档触发（见 ⑥）；
   - `上行已恢复（send 占用 8%），回到配置档…` → 链路真好了、已回高画质（见 ⑥）。
3. 浏览器端不需要动：播放器 8s 没新帧自己重连、连续失败也按 3s 上限持续重试，
   链路恢复后自动出图（不会停在错误层）。

验收（拔 Wi-Fi 再插回）：

```bash
# 板端：拔掉 wlp4s0 后 10~20s 内应看到这两行，然后按退避重连
ssh root@192.168.41.246 'journalctl -u backpack-relay -f | grep -E "上行阻塞|出口网卡"'
# 平台：推流端断开（最多 30s）→ 观看端收到 Closed 自动重连；Wi-Fi 回来后恢复出图
ssh root@39.107.55.13 'journalctl -u rust-admin-robodog -f | grep -E "video publisher|backpack stream"'
# 码率仍按 §6.10 量（拥塞窗口内应该是 ~500kbps）
python3 deploy/backpack-relay/measure_bitrate.py --stream dogcam --seconds 20
```

#### ⑥ 画质不会自己变：加自适应档位（2026-11-19）

现场问题：「平台中继会因为卡顿自动降画质吗？」—— 原来**不会**。三个环节全是
「要么全速推、要么断线重连」，没有降档：

| 环节 | 原来的行为 |
| --- | --- |
| 板端 `relay_push.py` | 档位只在起流时算一次；唯一的降档路径是 `ws.send` **超时**（10s 一个分片都写不出去）→ 断连 → 重连，重连后才用省流档 |
| 平台 `server/src/video.rs` | 房间只有一路流（推流端给多少就转多少），没有多码率层/`quality` 参数；观看端追不上就 `lagged` → **关连接**重连（FLV 不能从中途丢帧，注释里有原因） |
| 浏览器 `VideoPlayer.vue` | flv.js 单码率，没有 ABR（无 buffer 感知掉档） |

所以「卡但还能出图」时只有 TCP 反压：上行吃不下 → `ws.send` 阻塞 → `ffmpeg`
输出阻塞 → 帧率/码率被压到链路上限（§6.10② 实测 397kbps/7fps）。表现是糊+卡，
不是稳定换成低画质。

现在（**只改了板端 relay**，平台/浏览器不用动）：

```
每 RELAY_CONGEST_WINDOW(5s) 记一次：duty = send 占用时间比例，bps = 实测码率
  duty ≥ RELAY_CONGEST_DUTY(0.5) 且 bps ≤ 目标 × RELAY_CONGEST_BPS_RATIO(0.9)
        （`RELAY_PROFILE=copy` 没有目标码率，只判 duty）
        └─► mark_congested()：video_reset + 重启 ffmpeg 换 640x360@12/500k
              （WS 不动 → 观看端不重连、不黑屏；已在省流档只续窗口）

每 RELAY_RECOVER_POLL(2s) 看一次窗口是否到期
  _congested_until 到期 且 有人在看 且 send 占用 ≤ RELAY_RECOVER_DUTY(0.2) 持续 20s
  且 当前档实测码率打得到本档目标
        ├─ 是 ─► step_up_profile()：**只升一档**（2→1→0）+ 45s 试用期
        │        积压超过 RELAY_PROBATION_LAG(0.6s) → relapse() 退回上一档
        │        并把那一档封顶（hold_seconds() 内不再试）
        └─ 否 ─► 继续省流（再续一个窗口）

刚升档 RELAY_FLAP_WINDOW(600s) 内又拥塞 = 这一档撑不住 → 窗口 ×2（上限 1800s）+ 封顶
```

设计要点（改之前先看这几条，都是踩过的）：

- **只看 `send` 耗时不够**：链路恰好跑满时每个 8KB 分片也会阻塞几十 ms、但画面是流畅的。
  必须「占用高 **且** 实测码率低于目标」才判拥塞，否则正常跑满也会被误降档。
- **降档不能 `return` 出 `_pump`**：`_pump` 的 `finally` 在没有 `_stopping` 时会关掉 WS，
  那就变成「断线重连」而不是就地换档。降档后继续泵，重启任务会在锁里取消它并置
  `_stopping=True`。
- **`degraded()` 是独立布尔值**，不能由 `_congested_until > now` 推断：窗口到期只是
  「可以开始评估」，链路没恢复还得继续待在省流档。
- **升档要看得见链路**：`send` 占用的 EWMA 持续低于 `RELAY_RECOVER_DUTY`，而且
  当前档自己得跑满本档目标。没人在看时**不升档**（`send 占用 0%` 什么都证明不了，
  以前就是在这里把极省流档的 0% 当成「链路恢复了」，见⑨）。
- **回档只升一档 + 试用期**：跳回配置档会让「板子出帧跟不上」的那 5 分钟没人管；
  一档一档升、积压一涨就退，代价只有一次就地换档（观看端不重连）。
- 回归：`python3 deploy/backpack-relay/test_relay_push.py`（纯状态机，不需要 websockets /
  ffmpeg / 网络；22 个用例覆盖降档、两级降档、窗口统计、延迟降档、延迟换算、
  一次只升一档、试用期退回、封顶、抖动翻倍、硬超时、空跑不重启）。
- 仍是**单档**（配置档 ⇄ 省流档），不做多码率 ABR：要真正的「按 buffer 连续掉档」得给
  平台房间加多路/多码率层（`video.rs` 的房间模型 + `relay_push.py` 的起停协议都要改）。

#### ⑦ 延迟大 / 降档不生效 / 越看越卡：`_stop_locked` 无限等 + 没有延迟看门狗（2026-11-18）

现场现象：日志里已经打了 `降档：重启拉流换档位 → 640x360@12 500k`，但画面一直很糊、
而且**延迟越来越大**；`video_stop` 也进不来（平台看门狗发停流没反应）。

根因（`relay_push.py` 两个问题叠在一起）：

| # | 位置 | 原来会怎样 |
| --- | --- | --- |
| 1 | `_stop_locked` 的 `await asyncio.gather(*tasks)` | 没有超时。上行拥塞时 `_pump` 卡在 `ws.send()`（transport 写不出去），`cancel()` 只是打标记、协程停不下来 → gather 永远不返回 → **锁一直被占着**，session 变僵尸：降档永远不生效（还在用旧档位推）、`video_stop` 进不来、ffmpeg 还被反压着继续跑 |
| 2 | 没有任何「延迟」指标 | 只能看到「码率/帧率」，看到延迟只能靠肉眼。积压发生在 ffmpeg 内部，码率看着正常也无从发现 |

事故实证（14:05:16）：

```
5s 内实测 336kbps（目标 1200kbps，send 占用 100%）→ 改用省流档 640x360@12 500k
降档：重启拉流换档位 → 640x360@12 500k
降档，停止拉流（已推 74771886 字节）     ← 之后既没有 `推流结束` 也没有 `开始拉流`
# 8 分钟后仍在跑的仍是降档前的 light ffmpeg，send 队列积压，观看端越看越晚
```

修法：

| 位置 | 改动 |
| --- | --- |
| `_stop_locked` | `asyncio.wait(tasks, timeout=RELAY_STOP_GRACE)`；超时就 `close_quietly(ws)` **把连接掐掉**（只有掐链路才解得了卡住的 `send`），置 `_conn_broken`，然后照常 terminate/kill ffmpeg |
| `_restart` | 连接被掐断时不往死 WS 上贴流，交给外层 `stream_worker` 重连，重连后按新档位起流 |
| `FlvScanner` + `_update_lag` | 解析 FLV 视频 tag 时间戳（跨分片，只吃 tag 头），`lag = 现在墙钟 - (ffmpeg 起流时刻 + tag_ts)` 就是「帧在管道里躺了多久」；`≥ RELAY_LAG_LOG`（1s）每 30s 记一行 `画面延迟 x.xs`，`≥ RELAY_MAX_LAG`（3s）重拉 ffmpeg 重新对齐（防抖 `RELAY_LAG_RESTART_MIN` 20s） |
| `ffmpeg_args` | 补上 `-muxdelay 0 -muxpreload 0`（文档里一直写着有、代码里其实漏了）→ 白丢的 0.5~1.2s mux 缓冲 |

实测（2026-11-18 14:13 重部署后）：

| 指标 | 结果 |
| --- | --- |
| 板端积压延迟 | 起流后 2s 是 `1.2s`（含 RTSP 握手 + 等首个关键帧），3 分钟后回落至 **<1s** 且不再打日志 |
| 平台观看端积压（60s 探针） | 增量 `-0.02s`（不累积；`fps` 滤镜的 CFR 时间轴让积压只会被排空） |
| 无观看者时 | 不起 ffmpeg，零开销 |

排障要点：

- 用户说「延迟大」时，先在板端 `journalctl -u backpack-relay | grep 延迟` 看板端给到的数字：
  **板端 <1s 而人看到好几秒** → 延迟在源侧（本体 `dogcam` 1080p60 RTSP / mediamtx
  `sourceOnDemand`）或浏览器，不在中继；
- 刚打开页面/刚发生重同步时要等关键帧（GOP 2s，`RELAY_GOP=20` 可降到 1s）；
- 探针（平台侧跑，触发一次观看并算积压增量）：`python3 deploy/backpack-relay/latency_probe.py`。
- 回归：`python3 deploy/backpack-relay/test_relay_push.py`（15 个用例，含 FLV 分片扫描、
  延迟换算、`-muxdelay 0`）。

#### ⑧ 直连 ⇄ 中继切换时「卡住」、越看越延迟（2026-11-18 二次修复）

现场反馈：「在**现场直连**和**平台中继**之间切换的时候卡住了，而且延迟感觉很大」。查下来是三个叠加：

| # | 位置 | 现象 / 原因 | 修法 |
| --- | --- | --- | --- |
| 1 | 平台 `video.rs::reset_stream` | 中继重启 ffmpeg 时只清房间流头，**不踢已经在看的浏览器**。同一条推流连接上接着发新一轮 FLV（时间戳从 0 重来、降档后分辨率还从 720p 变 640x360），flv.js 收到时间轴倒退的字节流不会自愈 —— **画面定住、字节却还在流**（`pendingBytes` 一直是 0，服务端看不出异常），只能等前端 8s 假死看门狗 | `Room` 加 `reset: Arc<Notify>`，`video_reset` 时 `notify_waiters()`；观看端 `select!` 到它就断开，前端 250ms 起重连 → 拿到**新流头**。回归：`video_relay::video_reset_disconnects_attached_viewers_so_they_refetch_the_header` |
| 2 | 前端 `VideoPlayer.vue` | 没有追直播边缘：MSE 里攒下的那截不会自己消化 → **越看越延迟**，最后定在某一帧 | 打开 flv.js `liveBufferLatencyChasing`（落后 >1.2s 直接跳到直播边缘，`liveBufferLatencyMinRemain=0.5`） |
| 3 | 前端切流 | 切到中继要板端**重新起 ffmpeg**（1~7s，含等关键帧），这期间是老的黑屏，现场一律说「卡住了」 | 新增 `composables/useVideoPoster.ts`：直连/中继/换通道都把**最后一帧**存成 320px 小图，新播放器没出画前先顶着；起流超过 2.5s 角标变「起流中」并提示「板端正在起流（约 1~7s）」，不再让人以为卡死 |

另外板端把关键帧间隔从 2s 收到 1s（`backpack-relay.service` 里 `RELAY_GOP=15`）：起流 / 重同步 / 切通道都要等一个关键帧，这一项直接砍掉一半等待。

**越看越延迟的真正来源（2026-11-18 15:06 线上实证）**：板子出公网的上行实测只有
**~1.15Mbps**，而编码按 1.2Mbps 跑 —— 链路永远差一点点，ffmpeg 又不会丢帧，于是帧一路
攒着，延迟从 1.1s 涨到 4.0s（232s 内涨 2.9s ≈ 1.3%/s），攒到 `RELAY_MAX_LAG` 就被延迟看门狗
重启一次，**每 4 分钟断你一次**：

```
15:03:04 画面延迟 1.1s
15:06:47 画面延迟 1.3s
15:06:55 延迟 4.0s 超过上限 3s，重启 ffmpeg 重新对齐时间轴
15:07:00 1 个协程 5s 没停下（多半卡在 ws.send），掐断连接      ← 观看端被断开
```

修法：**留余量** —— `RELAY_BITRATE=1000k` + `RELAY_FPS=15`（720p15 / 1.0Mbps，比链路低
~15%）。改完实测：60s 积压增量 `-0.03s`（此前 40s 涨 1.6s），板端起流后不再刷「画面延迟」，
不再每 4 分钟重同步一次。

> ⛔ 别用 `-fps_mode drop` 去"让 ffmpeg 丢帧"：这块板子的 ffmpeg（2023 构建）上它会把
> 时间轴一起丢掉 —— 实测 20s 只出 273 帧、`tag_ts` 只走了 387ms，整路作废。

**第三档「极省流」320x180@8 / 200k（2026-11-18）**：给 4G / 弱 Wi-Fi 用。

现场 15:54 实证：板子出口掉到 4G dongle（`enx020c29a39b6d`），实测上行 **~250kbps**，而省流档
目标就是 500kbps —— **链路比目标还小一半**，`send` 永远 100% 占用、延迟涨到 5.9s、每 40 秒被
延迟看门狗重启一次（每次重启都掐连接、把人踢下线）。现在降档是两级的：

```
配置档 720p15/1000k  →(5s 窗口实测 <90% 目标)→  省流档 640x360@12/500k
                     →(省流档也吃不下)→        极省流档 320x180@8/200k
```

- 升级判据：`_current_level_is_workable()` —— 最近一个窗口实测码率 < 本档目标 ×90% 就再降一档；
  已经是极省流档就只续窗口、不重启；
- 升档：窗口到期 + `send` 占用 ≤20% 持续 20s + 当前档跑得满 → **只升一档**并进 45s
  试用期（积压超 0.6s 就退回），不再是「直接回配置档」，见⑨；
- 实测（板端真机，`ffmpeg` 直出 20s 稳态）：`320x180@8/200k` → **~2.2 fps、~170 kbps**，
  `640x360@12/500k` → ~5 fps，`720p15/1000k` → 13.9 fps。

> ⚠️ 为什么极省流档只有 2~3fps：**rkmpp 在低 bps 下靠丢帧保码率**（`mpp_enc: mode cbr`）。
> 已试过没用/更糟的做法：`-rc_mode avbr`（0.7fps）、`vbr`（≈不变）、`-rc_mode 2 -qp_init`
> （0.3fps）；唯一有效的是拉长 GOP：`-g 200` 能到 3.2fps，但关键帧间隔 25s 意味着重连要
> 等很久才出图，不划算，所以维持 `-g fps*2`。真要 8fps 的 320x180，大约需要 600~800kbps。
> 结论：这一档的定位是「**慢刷新幻灯片，但不断线**」，不是「流畅」。
> 回归：`python3 deploy/backpack-relay/test_relay_push.py`（22 个用例，含两级降档 +
> 一次只升一档 / 试用期退回 / 封顶）。

判定口诀（现场照着念）：

- 页面底部写「**平台中继 · 机器狗前置 · 延时 1~2s**」→ 走中继，中继本身 1~2s 是设计值，
  比直连（WHEP ~200ms）慢是**正常**的；要低延迟就切直连（需同内网 + 浏览器许可）；
- 切过去先黑 1~7s 再出画 = 正常起流（角标会显示「起流中」）；一直不出画才是问题；
- 画面定住但平台日志 `lastFrameAgeSecs` 还是 0（字节在流）= 第 1 条那个 bug，
  现在会被平台主动断开重连，日志：`推流端开启新一轮流（video_reset），断开让前端重连拿新流头`；
- 越看越晚 = 第 2 条，前端追边缘后不会再累积。

#### ⑨ 每 10 分钟「越看越卡 → 掉线重连」：回档判据是假的（2026-11-18 三次修复）

现场反馈：「平台中继看着看着就卡住了」。两边日志对齐后是一条**固定周期**：

```text
16:38:24 板端 配置档实测 846kbps（目标 1000k，send 占用 53%）→ 降省流档
16:38:57 板端 省流档实测 418kbps（目标 500k，send 占用 79%） → 降极省流档
16:39:16 极省流 320x180 开跑；平台侧这段 283.5s / 6.6MB / 平均 192kbps
16:43:58 板端「上行已恢复（send 占用 0%），回到配置档」          ← 问题在这
16:43:59 配置档 720p15/1000k 开跑；平台侧这 144s 平均 909kbps
16:49:29 板端「画面延迟 1.7s」
16:49:33 板端「延迟 5.3s 超过上限 3s，重启 ffmpeg」+ 再降省流档
16:49:33 平台「stream reset（新一轮 ffmpeg）」→ 主动踢观看端
16:49:41 平台「video publisher detached」← relay 自己「1 个协程 5s 没停下」掐了 WS
```

三个叠在一起的问题：

| # | 位置 | 原来会怎样 |
| --- | --- | --- |
| 1 | `profile_check` 的「链路已恢复」判据 | 只看 `send` 占用 ≤20%。**极省流 200k 档上它天然是 0%**（根本没东西可发），等于「用最低档的闲」证明「1Mbps 带宽恢复了」→ 直接跳回配置档。
| 2 | 跳回是 `_degrade_level = 0` | 极省流 → 配置档一步到位，跳过了省流档的验证；配置档在这块板子上撑不住（出帧只有目标帧率的 ~91%，rkmpp 的 CBR 靠丢帧保码率）→ tag 时间戳比墙钟走得慢 → **积压以 ~1.6%/s 涨**，5 分钟涨到 5.3s，中间这段时间观众看到的就是「越来越卡」。
| 3 | 没有「延迟」类降档信号 | 窗口判据是「`send` 占用高 **且** 码率低于目标×0.9」。配置档实测 909kbps / 1000k = 0.909，刚好在阈值之上，`duty` 也不高 —— 完全看不出问题，只能等 3s 的硬重同步（顺带把连接掰了）。

修法（只改板端 `deploy/backpack-relay/relay_push.py`，平台与前端不用动）：

| 改动 | 说明 |
| --- | --- |
| **回档只升一档** | `step_up_profile()`：2→1→0，不再跳级。每一档都要单独验证过才允许再往上。 |
| **升档试用期** | 升上去进 `RELAY_PROBATION`（45s）试用期，期间积压超过 `RELAY_PROBATION_LAG`（0.6s）就 `relapse()` **立刻退回上一档** + 就地重启 ffmpeg（观看端不重连）。代价从「5 分钟越来越卡 + 被踢」变成「几秒内闪一下」。 |
| **封顶 `_ceiling_level`** | 试用期失败 / 刚升档就拥塞的那一档记下来，`hold_seconds()` 内不再尝试（比它更好的档位一起挡）。同一个坑不会每 10 分钟踩一次。 |
| **延迟增速当降档信号** | `_check_lag_growth()`：积压 ≥ `RELAY_LAG_DEGRADE`（1.2s）并持续 `_HOLD`（10s）就降一档 —— 这比 `RELAY_MAX_LAG`（3s）的硬重启早得多，也不会掰连接。 |
| **没人在看不升档** | 原来「没人在看 → 到期直接清掉拥塞状态」（= 回配置档）。可 `send 占用 0%` 在没有流的时候什么都没证明。现在保持当前档位，等真有人看、且真跑满本档目标时才评估。 |
| **升档还要「本档自己跑得满」** | `profile_check` 多一道 `_current_level_is_workable()`：当前档实测码率打不到本档目标×0.9，就别往上试。 |

新增/改动日志（认这几行就知道状态机在干什么）：

```text
上行看起来恢复了（send 占用 8%），升一档到省流档 640x360@12 500k，45s 试用期（积压超过 0.6s 就退回）
省流档 试用期通过（45s 内没有积压，640x360@12 500k）
[dogcam] 试用期延迟 1.1s（这一档出帧跟不上） → 退回省流档 640x360@12 500k（600s 内不再试配置档）
省流窗口到期，但配置档已知撑不住，继续待在省流档（600s 后再试）
[dogcam] 延迟 1.9s 持续上涨（1280x720@15 1000k 出帧跟不上） → 改用省流档 640x360@12 500k（至少保持 300s）
没人看：继续待在省流档，有人看时再评估要不要升档
```

新增环境变量（都有默认值，一般不配）：`RELAY_PROBATION`（45）、`RELAY_PROBATION_LAG`（0.6）、
`RELAY_LAG_DEGRADE`（1.2）、`RELAY_LAG_DEGRADE_HOLD`（10）、`RELAY_LAG_DEGRADE_DEBOUNCE`（30）。

回归：`python3 deploy/backpack-relay/test_relay_push.py`（22 个用例，含「一次只升一档」
「试用期失败退回并封顶」「积压持续上涨先降档而不是硬重启」「没人看保持档位」）；
纯状态机，不需要 websockets / ffmpeg / 网络。

> ⚠️ 如果**修完还在降档**，说明这块板子的链路 / 出帧能力就是撑不住配置档：
> 直接把配置档降到它能吃下的规格（例如 `RELAY_BITRATE=900k RELAY_FPS=12`，
> 或 `RELAY_WIDTH=1024 RELAY_HEIGHT=576`），别靠自适应去救 —— 自适应只能保证「体感可接受」，
> 不能凭空变出带宽。

#### ⑩ 档位改成页面可配置（默认省流档，4G 主用）

原来的默认档位只能在**板端**改 `RELAY_PROFILE` / `RELAY_*` 环境变量再重启 `backpack-relay`；
现场主要走 4G 模块，希望「不登板子就能把起流画质定下来」。现在：

- 页面：**系统设置 → 视频中继**
  - 默认档位：`light` 高清 720p15/1000k / `degrade` 省流 640x360@12/500k（默认，4G 推荐）/
    `degrade2` 极省流 320x180@8/200k；
  - 按上行带宽自动降档：开（默认）/ 关（锁死选定档位，只保留上行彻底断掉时的重连保护）。
- 存储：`settings.video_relay_profile` / `settings.video_relay_auto_degrade`
  （`seed.rs` 写入默认值 `degrade` + `true`，老库启动时 `INSERT OR IGNORE` 补上）。
- 平台 → 板端协议（都在推流 WS 的文本帧里）：
  - `video_start.payload.profile = {"tier": "degrade", "auto": true}`：第一个观看者进来时下发；
  - `video_config.payload.profile = …`：设置页保存后**广播**给 7 条在线推流连接，正在看的流
    就地换档（`video_reset` + 重启 ffmpeg，观看端不重连），不用等下一次起流。
- 板端（`relay_push.py`）：平台下发的 `tier` 成为**起点档位**（`base_level()`），
  `_degrade_level` 改为绝对档位（起点 1 = 省流档时还能再降到 2 = 极省流档，但不会再往上升）；
  `auto=false` 时 `mark_congested()` 只续窗口、不降档（日志每分钟一条「平台已锁定…」）。
  没有 `profile` 字段时（老平台 / 手工运行）完全按环境变量走，行为与以前一致。

两条兼容性约定（改协议时别破坏）：

- **老板端 + 新平台**：老 `relay_push.py` 忽略 `payload.profile`，继续用板端环境变量；
- **新板端 + 老平台**：没字段 → `base_level()` 返回 0（= `RELAY_PROFILE`），与原来一致。

回归：

- 板端状态机：`python3 deploy/backpack-relay/test_relay_push.py`（27 个用例，含
  「平台起点省流档」「起点极省流档不再降」「锁定档位不降档」「换档后旧拥塞状态作废」
  「老平台不带 profile」）；
- 平台：`cargo test --test video_relay`（`relay_profile_defaults_and_updates_reach_the_board`）、
  `cargo test --test api_smoke relay_profile_setting_is_validated`（枚举校验 + 默认值可读）。

> ⚠️ 页面只选**档位**；档位背后的分辨率 / 帧率 / 码率仍由板端 `RELAY_*` 决定（本节的
> 默认值就是 §6.10⑥⑧ 调出来的那套）。要现场微调又不改页面，就改板端环境变量后重启
> `backpack-relay`（平台下发的档位名不变，参数跟着变）。

#### 排查工具与清单

```bash
# 1) 中继真实码率 / 帧率（在平台机器上，loopback）
python3 deploy/backpack-relay/measure_bitrate.py --stream dogcam --seconds 20
# 2) 平台出口上限
ssh root@39.107.55.13 'speedtest-cli --simple'      # 看 Upload
# 3) 板端到平台走哪条路由 / 上行多少
ssh root@<背包IP> 'ip route get 39.107.55.13'
ssh root@<背包IP> 'python3 /tmp/bw.py'              # 见 §6.10② 的量法（WS 推 2MB 计速）
```

| 现象 | 先查 | 结论 |
| --- | --- | --- |
| 画面突然定格、重开页面也不动，重启 relay 才好 | `lastFrameAgeSecs`、板端 `上行阻塞` / `没有推出任何字节` | ⑤（2026-11-18 已修，板端要升级 relay） |
| Wi-Fi 断了再连上，画面还是卡 / 很糊 | 板端 `平台出口网卡 A → B`、`profile=省流` | ⑤ 出口切到慢链路（4G / 狗体内网）；看 `ip route get`。现在到期还会先确认链路已恢复才回配置档（⑥），仍糊就说明链路确实只有这么多 |
| 每 23s 左右断一次、20s 连接超时 | `/api/backpack/video/state` 的 `initBytes` + 服务端是否 `stream reset` | ①（已修） |
| 有画面但卡、`measure_bitrate` 远低于配置 | 板端 `ip route get 39.107.55.13`、`bw.py` | ② 路由走了慢速 dongle |
| 反复 `lagged`、`left after ... kbps` 高于出口 | `speedtest-cli` 的 Upload vs 平均 kbps | ③ 降档位 / 升带宽 |
| 看着看着越来越卡（延迟涨），然后掉一下重连 | 板端 `画面延迟 x.xs` / `延迟 x.xs 超过上限` / `回到配置档`；平台 `stream reset` + `publisher detached` | ⑨（2026-11-18 已修）。修完还有：看 `试用期延迟` / `已知道撑不住` 两行，如果反复在降档，就把配置档降到链路能吃下的规格，别靠自适应救 |
| 同时开多个视频页面更卡 | 每个观看者 = 一份出口带宽 | 出口按 1.5Mbps/路 规划 |
