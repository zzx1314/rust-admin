# backpack-sim：本地模拟 `.246` 背包机

`.246`（`192.168.41.246`）是现场那台背包机：mediamtx、`backpack-agent`（FastAPI :8000 +
RKNN）、视频中继、设备客户端、现场控制台。它一旦连不上，**背包页 / 航点 AI / 证据图 /
视频中继**这几条链就没法改、也没法验。

这个目录用仓库里的**真实实现** + 三个模拟件，在本机把 `.246` 复刻出来：

```text
                     本机（模拟「现场」）                           本机（模拟「平台」）
┌───────────────────────────────────────────────────────┐   ┌──────────────────────┐
│ fake_cams.py ──RTSP──▶ mediamtx :8554/:8889           │   │                      │
│    (testsrc2)           ▲            ▲                │   │  business/server     │
│                         │            └──WHEP── 浏览器  │   │  :3100               │
│ mock_agent.py :8000 ────┘（cam0~cam3/preview 由 Agent 推）│   │  ├ /api/backpack/*   │
│      ▲   ▲                                               │   │  └ /ws/device/video  │
│      │   └──SSE──┐                                       │   │                      │
│  relay_push.py ──┼──WS /ws/device/video（FLV 分片）──────┼──▶│  VideoHub 房间 bp:*  │
│  （仓库原版脚本） │                                       │   │                      │
│                  │                                       │   │                      │
│ robodog-device-client --robot mock ──WS /ws/device──────┼──▶│  设备台账 / 任务 / AI │
│      （仓库原版二进制）  │                               │   └──────────────────────┘
│                        └──HTTP 127.0.0.1:8000（agent_relay 隧道）
└───────────────────────────────────────────────────────┘
```

写这块之前先看 `deploy/backpack-246.md`（板子上到底有什么）、
`business/docs/backpack-video-integration.md` §2/§6（Agent 契约、中继参数、隧道）。

## 快速开始

```bash
cd deploy/backpack-sim

python3 run.py setup    # 一次性：下 mediamtx、建 venv(websockets)、编译设备客户端
python3 run.py up       # 起全套（前台；Ctrl-C 全停）
```

另开终端：

```bash
cd deploy/backpack-sim
python3 run.py status   # 端口 / 进程 / 连通性（端口从 pid 文件读，不用重复设 SIM_*）
python3 run.py smoke    # 冒烟自测：隧道 + 中继 + AI + 证据图
python3 run.py down     # 兜底收尸（up 崩了/被 kill 了）
```

前端（二选一）：

```bash
# A. 开发服务器（热更新）
cd business/web && pnpm dev      # → http://127.0.0.1:5273，/api 已代理到 :3100

# B. 用已构建产物（business/web/dist 存在时由平台直接托管）
#    → http://127.0.0.1:3100
```

登录 `admin / admin123`；设备号 `RD-MOCK-01`（`--robot mock`，任务会真的跑完）。

| 入口 | 地址 | 说明 |
| --- | --- | --- |
| 业务平台 | <http://127.0.0.1:5273> 或 <http://127.0.0.1:3100> | 设备/任务/告警/背包页 |
| 现场入口（可选 `--with-console`） | <http://127.0.0.1:8080> | `deploy/site-console/serve.py`，同网段控制台 |
| 模拟 Agent | <http://127.0.0.1:8000/api/detect/status> | 板端 Agent 的替身 |
| mediamtx WHEP | `http://127.0.0.1:8889/<stream>/whep` | 浏览器直连看流 |
| mediamtx RTSP | `rtsp://127.0.0.1:8554/<stream>` | 中继从这里拉 |

## 哪些是真的、哪些是假的

| `.246` 上的东西 | 本地 | 真实性 |
| --- | --- | --- |
| `backpack-relay.service`（`relay_push.py`） | **同一个文件**，`RELAY_PLATFORM` 指本机平台 | ✅ 档位/降档/积压看门狗/断线重连全都在跑 |
| `robodog-device-client.service` | **同一个二进制**，`--robot mock` | ✅ 注册/心跳/隧道/ai_patrol/假本体协议全真 |
| `site-console.service` | **同一个 `serve.py`** | ✅ |
| `business/server`（平台） | **同一个 crate** | ✅ 但库是新建的（`data/robodog.db`） |
| `backpack-agent`（FastAPI + RKNN） | 默认 `mock_agent.py`（纯标准库）；`--agent real` 用**仓库里**的 `business/backpack_dashboard/` | ⚠️ mock 契约一致、识别是假的；real 需板端依赖（fastapi/evdev/rknn） |
| mediamtx（7 条 path） | 官方二进制 + `mediamtx.yml` | ✅ RTSP/WHEP 行为一致，源全当 publisher |
| 4 路 USB 相机 / 本体前置 | `fake_cams.py`（ffmpeg `testsrc2`） | ⚠️ 只有彩条 |
| Wi-Fi / 4G 双出口 | 无 | ❌ `RELAY_ROUTE_POLL=0`，没有出口切换 |

`mock_agent.py` 实现的是平台/客户端真正会调的那一份契约：
`/api/cameras`、`/api/relay/status`、`/api/detect/tasks|status|start|stop|config`、
`/api/detect/plan/clear`、`/api/events`（SSE：`detect` / `plan_hit` / `detect_state`），
外加 CORS（直连模式浏览器要用）。字段定义见
`business/web/src/api/backpack.ts` 与 `business/docs/backpack-video-integration.md` §2.1。

## 两种接入模式

| 模式 | 命令 | 平台配置 | 页面通道 |
| --- | --- | --- | --- |
| `tunnel`（默认，= 线上正式环境） | `python3 run.py up` | 不配 `ROBODOG_BACKPACK_*_URL` | 平台 → 设备客户端隧道（`agent_tunnel` + `client/agent_relay`） |
| `direct`（现场入口） | `python3 run.py up --mode direct` | `ROBODOG_BACKPACK_BROWSER_URL=http://127.0.0.1:8000` | 浏览器直连 Agent（`/api/backpack/status` 返回 `mode=direct`） |

两种模式都设置了 `ROBODOG_BACKPACK_WEBRTC_URL=http://127.0.0.1:8889`，所以页面想直连看流
（WHEP）时不用改代码。视频本身始终是「中继」或「WHEP」两条路，与接口模式无关。

> `--platform <远端平台>` 时 `--mode` **不生效**：接入方式（direct / proxy / tunnel）取决于
> 远端平台自己的 `ROBODOG_BACKPACK_*_URL` 配置。远端平台要能看到本机，需双向网络可达；
> 正式环境（平台在公网、板端在内网）只能靠 tunnel，而 tunnel 需要本机设备客户端拨出去，
> 所以本机至少要能访问平台的 `ws://<platform>:3100/ws/device`。

## 常用开关

```bash
python3 run.py up --mode direct          # 浏览器直连 Agent
python3 run.py up --agent real           # 用仓库里的真 Agent（要 fastapi/evdev/rknn，一般只在板子上）
python3 run.py up --platform http://127.0.0.1:3100   # 只起板端，复用一个已有平台
python3 run.py up --no-platform          # 等价写法（假定 :3100 已有平台）
python3 run.py up --with-console         # 额外起 :8080 现场入口（要 business/web/dist）
python3 run.py up --no-cams --no-relay   # 只模拟 Agent / 客户端（不看视频）
python3 run.py up --device-id RD-TEST-02 # 换设备号（同时只连一台，别和上面混跑）

SIM_PUBLISH_STREAMS=cam0,cam1,cam2,cam3,dogcam,preview python3 run.py up   # 全 7 路
SIM_AGENT_PORT=8000 SIM_PLATFORM_PORT=3000 python3 run.py up              # 换端口
SIM_COUNT_RESULT=mismatch python3 run.py up     # 数量任务少 1 个 → count 告警 + 证据图
SIM_EXIST_FOUND=0 python3 run.py up             # 存在性任务一直不命中 → 「未检出」告警
SIM_LATE_EVIDENCE=1 python3 run.py up           # 图比卡片晚到 → ai_evidence 只补图
SIM_KEEP_WECOM=1 python3 run.py up              # 保留 ROBODOG_WECOM_*（会真的往群里推！）
```

`run.py` 会**主动剥掉**继承来的 `ROBODOG_*`（尤其是 `ROBODOG_WECOM_WEBHOOK`、
`ROBODOG_ZENTAI_*`）再启动平台和设备客户端，避免本机跑一次就往现场群推消息、或误连正式中台。
`--platform` 指向真实平台时**会注册一台新设备**（`RD-MOCK-01`），别对着生产环境随手跑。

想演练故障场景（背包 Agent 挂了 / 客户端掉线 / 中继断流），直接 `kill` 对应进程即可：
只有 `mediamtx` 与平台算关键组件（死了 `up` 会收队），其余退了只告警、其余组件继续跑；
`python3 run.py status` 能看到「Agent 不通」，Ctrl-C 后重跑 `up` 恢复。

## 模拟 Agent 的控制接口

真 Agent 没有这些，纯调试用：

```bash
curl -s 127.0.0.1:8000/api/sim/state
curl -s -XPOST 127.0.0.1:8000/api/sim/relay -d '{"enabled":false,"running":false}'  # 页面置灰/未推流
curl -s -XPOST 127.0.0.1:8000/api/sim/relay -d '{"running":true}'
curl -s -XPOST 127.0.0.1:8000/api/sim/emit  -d '{"type":"telemetry","battery":88}'  # 往 SSE 塞任意事件
curl -s -XPOST 127.0.0.1:8000/api/sim/flush                                        # 立刻收尾当前计划（落图+plan_hit）
```

计划模式（`detect/start` 带 `plan_id`）下，Agent 会按真板子的节拍走：先出 `detect`，
`SIM_PLAN_HIT_DELAY`（默认 4.5s，真机实测 4.6s）后把命中帧画框落盘到
`data/detect_jobs/<plan_id>/pt<idx>/`，再发 `plan_hit`（带 `evidence` / `found_count` /
`expect_count` / `count_alert`）。停识别时如果命中还在「采样中」，会像板端 `_plan_flush`
一样先落盘再返回——这就是客户端「图晚到、`ai_evidence` 只补图」那条链路的复现口。

## 冒烟自测覆盖了什么

`python3 run.py smoke` 会真的跑一遍（不是看健康检查）：

1. 模拟 Agent 的 4 个只读接口；
2. 平台聚合 `/api/backpack/status` + 隧道 `agent_hello` 登记；
3. **视频中继**：用浏览器同款方式连 `/api/backpack/video/ws`，收到 FLV 分片才算过
   （这一步才会触发 `video_start` → relay 起 ffmpeg）；只连上但没二进制分片＝没通；
4. **航点 AI**：`detect/start(plan_id)` → SSE `plan_hit` → 证据图落盘且是 JPG/PNG ≤2MB；
5. **AI 启停经平台隧道**下发，并回读 Agent 运行态确认真改了。

## 能验 / 不能验

能验：

- 背包页三种通道（direct / proxy / tunnel）的成败与回退；
- 中继按需起流、FLV 分片、档位下发（平台「系统设置 → 视频中继」改档会就地生效）；
- 航点 AI 的计划事件、证据图落盘、`ai_detect` / `ai_alert` / `ai_evidence` 的服务端处理
  （配合 `business/server/tests/*` 或看告警中心/日志）；
- 设备客户端的注册、心跳、任务执行（假本体）、agent_relay 白名单。

不能验（必须真板子）：

- RKNN 模型加载、NPU 忙（`13897 capability busy`）、画框字体缺失（`missing_glyphs`）；
- 相机没插 / 本体 RTSP 没就绪 / 红外黑屏；
- Wi-Fi↔4G 出口切换、`RELAY_SLOW_IFACES` 按网卡名压档、真实上行带宽与积压；
- 企业微信群真推（默认被剥掉）、中台（:3000）联动。

另外**任务派单到「航点 AI」的完整链路**还差一块：派单要求路线绑定的地图有真实 ROS 地图包
（`business/server/src/coords.rs`、`dispatch.rs::check_route_data`），本模拟不造地图包，
所以 `up` 起来后**手动**在路线编辑器里给地图基线发布一张真实地图、再给航点加「AI 识别」动作，
才能看到平台真的下发 `ai_start`。只想验 Agent 侧，直接用上面的
`detect/start(plan_id)` 或 `run.py smoke`。

## 文件

```text
deploy/backpack-sim/
├── run.py           进程编排 + 依赖检查（setup / up / down / status / smoke）
├── mock_agent.py    模拟 Agent（:8000）
├── fake_cams.py     ffmpeg 假相机 → mediamtx
├── smoke.py         冒烟自测（run.py smoke 会调它）
├── mediamtx.yml     7 条 path，全部 source: publisher
├── bin/             mediamtx 二进制（setup 下载，gitignore）
├── runtime/venv/    relay 的 websockets（setup 创建，gitignore）
├── data/            robodog.db + detect_jobs/（gitignore）
└── logs/            每个进程一份日志（gitignore）
```

## 排障

| 现象 | 原因 / 处理 |
| --- | --- |
| `setup` 下载 mediamtx 卡住 | GitHub release 需要代理：`export https_proxy=http://127.0.0.1:7890` 后再跑 |
| `端口 8000 已被占用` | 本机已经有别的 Agent（比如以前的临时联调脚本）：`SIM_AGENT_PORT=18000 python3 run.py up`，后面 `status/smoke` 不用再设（读 pid 文件） |
| `mediamtx` 起不来 | 8554/8889 被占：`ss -lntp \| grep -E '8554\|8889'`，先停掉旧进程 |
| 中继日志 `[dogcam] 连接平台失败` | 平台没起 / `--no-platform` 但 :3100 没服务：先 `status` 看平台 |
| 页面「背包不可达」 | `status` 看 Agent；直连模式下还要看浏览器能不能访问 `127.0.0.1:8000` 的 CORS |
| 视频黑屏但中继有日志 | 看 `logs/relay.log` 是否有「开始拉流」；`logs/cams.log` 是否在重连（mediamtx 没起会一直退避） |
| 证据图没发到群里 | 本模拟默认剥掉 `ROBODOG_WECOM_*`（见 `SIM_KEEP_WECOM`）；另外 `settings.alert_only_in_task` 只推执行计划期间的告警 |
| `run.py up` 被 Ctrl-C 后端口还被占 | `python3 run.py down`（按 pid 文件杀进程组） |
