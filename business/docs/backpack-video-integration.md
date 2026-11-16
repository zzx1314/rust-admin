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
- 平台中继（`deploy/backpack-relay/relay_push.py`）是 `-c:v copy` 只换容器，
  浏览器看到的分辨率就是 dogcam path 的分辨率；页面上的「1920×1080」由播放器
  实测上报（`VideoPlayer` / `VideoStage` 的 `meta` 事件），不再写死规格。
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
  └─ ffmpeg -rtsp_transport tcp -use_wallclock_as_timestamps 1 -c:v copy -f flv -
       └─ ws://<平台>/ws/device/video       video_register(deviceId="bp:<stream>") + 二进制 FLV
            └─ 平台 VideoHub 房间 bp:<stream>  ──ws──► 浏览器 flv.js
                 GET /api/backpack/video/ws?stream=<stream>&token=…
```

- 服务端：`business/server/src/video.rs`（`BACKPACK_ROOM_PREFIX = "bp:"`、`backpack_viewer_ws`）
  + `app.rs` 路由 `/api/backpack/video/ws`（在 `require_auth` 层内，`?token=` 鉴权）。
- 板端：`deploy/backpack-relay/relay_push.py` + `backpack-relay.service`。
- 前端：`business/web/src/views/monitor/index.vue`（顶部「现场直连 / 平台中继」切换，
  直连不可达时自动切中继）；播放器复用 `VideoPlayer.vue` 的 `type="relay"` 分支。

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
   流头不会串轮次：板端 relay 起 ffmpeg 前发 `video_reset`（第 2 条），设备客户端重连
   推流时整个房间被换掉（`VideoHub::publisher_attach`），两条路径都会清旧头。

### 6.4 按需与配额

- 常驻 6 条 WS（`cam0..cam3`、`dogcam`、`dog-ipcam`），空闲时每条 ~1KB；
- 第一个观看者 → `video_start` → 推流端起 ffmpeg 或 RTSP 拉流；
- 最后一个观看者离开 → **不立刻停**，只保留 `streaming` 标记；看门狗每 15s 兜一次
  「起过流但没人看」，在 ~15~30s 内下发 `video_stop`（板端 relay 与设备客户端同一套
  宽限，见 `VideoSource::stop`）。宽限期内重新打开是**秒开**：流还在跑，服务端直接把
  流头补上，不用等 ffmpeg 重启；
- 推流端按地址去重 `video_start`（`business/client/src/video.rs`）：同一地址已经在推就
  不重启 ffmpeg（重启 = 黑 3~7s + 换新 FLV 头 = 一次闪断）；
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
  秒开 —— 最后一个观看者离开时不立刻停流，交给看门狗在 15~30s 内停（见 `VideoSource::stop`）。
- 中继模式下**没有 AI 叠加**：板端在本机内网，公网页面拿不到 SSE `detect` 事件；
  AI 仍要现场入口。
- 板端 relay 的推流连接是常驻的，**每次起流换一条新 FLV**，所以服务端必须靠 `video_reset`
  清 `init`，否则中途加入的观看者会拿到「上一轮关键帧 + 这一轮中段字节」而黑屏。
- `dog-ipcam` 源若为 H.265，`-c:v copy` 无法封装成 FLV，需 `RELAY_TRANSCODE=1`
  （重编码，板端 CPU 开销大，优先改摄像头出 H.264）。

### 6.6 为什么「设备管理」里的视频会闪（已修）

现场现象：设备管理 → 控制抽屉里的实时监控画面一闪一闪，角标在「LIVE」与
「连接中断」之间反复跳；而同一台狗的前置摄像头在「实时视频」页用平台中继看却很稳。
两条链路其实是同一个 `VideoPlayer.vue` + 同一个 `VideoHub` 房间机制，
差别全在**推流端参数**和**观看端重连策略**上，四个真因都改了：

| # | 真因 | 修法 |
| --- | --- | --- |
| 1 | 客户端 ffmpeg 写管道**带缓冲**，转码小帧被攒成突发（板端 relay 一直带 `-flush_packets 1`，所以它稳） | `business/client/src/video.rs` 输出加 `-flush_packets 1 -muxdelay 0 -muxpreload 0`，输入加 `-fflags +genpts` |
| 2 | 浏览器 WS 一抖，服务端就算「第一个观看者」，重发 `video_start` → 客户端**重启 ffmpeg** → 黑 3~7s + 新 FLV 头 | 客户端按地址去重；服务端最后一个观看者离开时留 15~30s 宽限（看门狗收尾） |
| 3 | 宽限生效后「第一个观看者不补流头」就变成致命 bug：浏览器只收到中段字节 → flv.js 解析错位 → 黑屏 → 重连 → 循环 | 服务端改成**有流头就补**（不串轮次靠 `video_reset` / 房间替换） |
| 4 | 观看端 `waiting` 就盖全屏错误层、重连先 `destroy` 清空画面、固定 3s 一次重试 | `VideoPlayer.vue`：缓冲只给角标；断流先抓末帧当 poster；250ms 起退避封顶 3s；3s「时间轴不前进」看门狗；flv.js `enableStashBuffer: false` |

排障顺序（按成本从低到高）：

1. `GET /api/backpack/video/state` 看 `viewers` 是否在 0/1 之间跳 —— 跳就是 2/3 类问题；
2. 设备客户端日志里是否反复出现 `ffmpeg 已启动` / `video publisher reconnected` —— 是就没去重成功；
3. 浏览器 Network → WS 帧时间轴：一秒钟一串小帧 = 正常；半秒一团再静默 = 推流端没 flush；
4. 还闪就看 `viewer of <room> lagged` 日志：观看端网络追不上，服务端现在会主动断开让它拿
   干净的新流（早先是从中间丢帧继续发，FLV 字节流错位后才一直黑屏重连）。
