# AGENTS.md — backpack_dashboard（板端数据采集 Agent）

板子 `.246` 上跑的那套「数据采集背包 Agent + 离线 Dashboard」。2026-11-19 起并入主仓库
（`git subtree` 导入，保留原有提交历史）。板端部署位置（2026-11-20 起）是
`/opt/xjpt/backpack_dashboard`，旧路径 `/root/261113/backpack_dashboard` **已不存在**
（迁移记录见 `deploy/backpack-246.md` §4.3）。

**这货不在开发机上真跑**：它依赖 RK3576 的 NPU（`librknnrt.so`）、4 路 USB 相机、GPIO、
4G 模块、聆思语音服务。改完要验接口链路，用 `deploy/backpack-sim/`（模拟 Agent + 假相机），
板端行为差异见那边的 README。

## 与其他子项目的关系

```text
business/backpack_dashboard（本目录，板端 .246）
   ▲ REST/SSE :8000
   │
   ├── 浏览器直连（现场入口 deploy/site-console/，同网段）
   ├── business/client（设备客户端，跑在同一块板子上）──agent_relay──▶ 平台隧道
   └── deploy/backpack-relay/relay_push.py（同一块板子，把 RTSP 转 FLV 推到平台）
          │
          ▼
business/server（平台 :3100）── /api/backpack/* 聚合、AI 启停、企业微信证据图
```

契约是谁在消费：`business/web/src/api/backpack.ts`（页面类型）、
`business/server/src/backpack.rs`（聚合/白名单）、`business/server/src/agent_tunnel.rs`
与 `business/client/src/agent_relay.rs`（隧道白名单：4 个只读 + `start/stop/config`）。
**改字段先改这三处 + `business/docs/backpack-video-integration.md` §2.1。**
航点 AI 的完整协议（`ai_point/ai_start/ai_detect/ai_alert/ai_end/ai_stop`、`plan_hit`、
`check_type`、证据图）见 `business/docs/official-app-parity.md` §5.6。

## 目录地图

```text
backend/
├── agent.py          FastAPI 主程序：摄像头清单、AI 启停、遥测/控制、SSE、WHEP 同源代理
├── config.py         全部可调项（环境变量 + 默认值），改参数只改这里
├── detect.py         AI 识别：拉 RTSP 旁路 → 帧 → 调 C++ detector（--serve）→ 框/事件/存证
├── evidence.py       证据图落盘：DETECT_JOBS_DIR/<plan|job>/pt<n>/*.jpg + result.json
├── osd.py            板端画框（ffmpeg 叠加，写 FIFO；中继「preview」流靠它带框）
├── relay.py          dogcam 归一化/转发（本体 RTSP → mediamtx）
├── stream.py         4 路 USB ffmpeg 推流编排（h264_rkmpp → libx264 回退）
├── telemetry.py      SHT4x/4G/WiFi/环境/SSD/风扇/聆思 采集
├── control.py        LED 闪烁 / 补光开关
├── buttons.py        SOS/STOP evdev 监听
├── cjk_font.py       画框中文字模（由 backend/tools/gen_cjk_font.py 生成）
├── start.sh          板端启动：camera-init → 复用 snap mediamtx → Agent
└── tools/            现场一次性工具（UVC 探测、GPIO 触发、四路时间轴、plan_count_selftest）

frontend/            零安装 Dashboard（原生 JS + mediamtx reader.js，WHEP 播放）
rknn/
├── librknnrt.so      自带 NPU runtime（start.sh 会加进 LD_LIBRARY_PATH）
├── code/code/        YOLOv8 C++ 推理（CMake，板端编译出 build/yolov8_detector）
└── models/           8 个 .rknn 模型包（每个 ~27MB，合计 ~211MB，已入库）：
                      fall / fire / fire-lane / firextinguisher / garbage_can /
                      overflow / plastic / smoke（每个目录 labels.txt + meta.json + model.json + model.rknn）
mediamtx/             自管 mediamtx 的配置样板（现场默认复用 snap 实例）
```

`README.md` 是原始现场文档（部署、摄像头触发、同步采集、画框排错），**细节以它为准**；
本文件只写「在本仓库里怎么改、改完要同步什么」。

## 常用命令

```bash
# 语法/静态检查（开发机即可，不 import 板端依赖）
python3 -m py_compile business/backpack_dashboard/backend/*.py
python3 -m py_compile business/backpack_dashboard/backend/tools/*.py

# 航点计划状态机自检（不需要 NPU / 相机，改 detect.py 的计划逻辑后必跑）
python3 business/backpack_dashboard/backend/tools/plan_count_selftest.py

# 计划灯光自检（夜间时间窗 / 白天跳过 / GPIO+LED / 看门狗，改 control.py 后必跑）
python3 business/backpack_dashboard/backend/tools/lights_selftest.py

# 本地接口链路（模拟 Agent，不需要板子）
cd deploy/backpack-sim && python3 run.py up && python3 run.py smoke

# 板端（.246）启动 / 重启
ssh root@192.168.41.246 'systemctl restart backpack-agent'   # 或 bash backend/start.sh

# 板端日志 / AI 运行态
ssh root@192.168.41.246 'journalctl -u backpack-agent -f'
curl -s 127.0.0.1:8000/api/detect/status
```

`deploy/backpack-agent.service` 是 systemd unit（`WorkingDirectory=/opt/xjpt/backpack_dashboard/backend`）。

板端依赖的厂商硬件程序不在本仓库，2026-11-20 起统一在 `/opt/xjpt/` 下
（`led/test_rgbrun`、`wenshidu/sht4x_app`、`lingsi/asr_usb_record`、`LED/`），
`config.py` 里 `LED_TEST_BIN` / `SHT4X_APP` / `LINGS_PLAY_WAV` 的默认值就指那里；
`/root/{led,LED,lingsi,wenshidu}` **已不存在**（搬走后就删了软链）。详见 `deploy/backpack-246.md` §4.4。

## 发布到板端

板端没有 Rust/Node 工具链，也不需要：改的是 Python/JS/模型时直接同步文件即可。

```bash
# 只改后端（最常见）
rsync -av business/backpack_dashboard/backend/ \
  root@192.168.41.246:/opt/xjpt/backpack_dashboard/backend/
ssh root@192.168.41.246 'systemctl restart backpack-agent'

# 改了 rknn（模型/so/C++ detector）后还要重启中继（preview 流的画框由 Agent 推）
rsync -av business/backpack_dashboard/rknn/ root@192.168.41.246:/opt/xjpt/backpack_dashboard/rknn/

# 改了画框字库（cjk_font.py 由 tools/gen_cjk_font.py 生成）→ 同步后重启 Agent
# 改了 detector C++（rknn/code/code）→ 板端 cmake 编译，不是在开发机
```

重启顺序：`camera-init` → `backpack-agent` → `backpack-relay`（见 `deploy/backpack-246.md` §7）。

## 代码约定

- **配置集中在 `config.py`**：新参数一律 `os.getenv("XXX", 默认值)` + 在 README「需要按现场
  核对的可调项」里补一行；不要在业务代码里散写魔法路径/端口。
- **Agent 没有鉴权**（板端内网、FastAPI 裸跑）：CORS 只允许白名单
  (`AGENT_CORS_ORIGINS`)，别写成 `*`；也不要新增「任意 URL 转发」这类接口——平台侧
  `agent_tunnel.rs` 的白名单是同样的理由。
- **SSE 事件是外部契约**：`detect` / `detect_clear` / `detect_state` / `detect_error` /
  `plan_hit` / `telemetry` / `button` / `lingsi`。客户端只转 detect 四类，平台 `/api/backpack/events`
  原样透传；新增字段可以，**改/删已有字段要同时改页面与两边测试**。
- **存证目录分两份，别合并**：
  1. `DETECT_JOBS_DIR/<plan_id>/pt<航点下标>/*.jpg` + `result.json` —— **临时存证**，
     客户端读它给企业微信配图（`ROBODOG_AI_JOBS_DIR`），推完图由 `ai_cleanup` 删；
     **任务终态 / 取消时客户端把整个 `<plan_id>/` 删掉**（只删推过图的航点清不干净，
     没命中的航点永远等不到 `ai_cleanup`），启动时还会兜底清一把超 3 天没动过的目录；
  2. `MEDIA_DATA_DIR/<plan_id>/pt<航点下标>/<cam>_<序号>_<时间戳>.jpg` + `result.json`
     —— **媒体归档**，给中台「媒体资源」用。命中时固定保存 `save_frames` 帧
     （取采样窗口里检出数最大的前 N 帧，**一律存原始帧、不画框**——它是训练数据，
     带框会污染标注；画框那份只在 `detect_jobs` 里），中台设备页点「同步媒体」
     后由客户端**按每包 8 张打 zip** 回传（中台解包后仍一张一条入库），
     **只删 `results[].ok` 的条目**。
  目录/帧数/开关的唯一真源是**中台「参数配置」**（`media_picture_dir` /
  `media_frames` / `media_capture_enabled`），随 `ai_start` 的 `media_dir` /
  `save_frames` / `captureEnabled` 下发；板端 `config.py` 的
  `MEDIA_DATA_DIR` / `DETECT_PLAN_SAVE_FRAMES` 只是没收到配置时的兜底。
  **采集默认关**（`media_capture_enabled=0`）：关着时 `_plan_write_media` 直接
  跳过——告警证据图、数量核对不受影响，只是不攒训练数据；请求里没带
  `capture_enabled`（老平台 / 自测工具）时按「开」跑，保持历史行为。
  改目录布局 = 改 `evidence.py`、`detect.py` 的 `_plan_write_media`、
  `client/src/{ai_patrol,media_sync}.rs`、`deploy/backpack-sim/mock_agent.py`。
- **计划状态机分两类任务**：`check_type=exist`（是否检出）/ `count`（数量是否等于
  `expect_count`）。数量不一致、以及「窗口到期一个都没找到」都要 `count_alert=true`；
  `_plan_flush` 要在停识别时把采样中的命中落盘（客户端靠「先停再扫」补图）。
  改这块先读 `official-app-parity.md` §5.6，并跑 `plan_count_selftest.py`
  （里面有「采集关闭：不写媒体归档、证据图照写」与「不传开关默认开」两条）。
- **计划灯光（补光灯 / LED）只有一份状态**：`control.py` 的补光灯 GPIO + LED 进程句柄
  被**手动按钮**（`/api/filllight`、`/api/led/blink`）和**计划**（`/api/lights`、
  `/api/lights/off`）共用。`LED_TEST_BIN` 只有「循环闪烁」没有常亮接口，所以「LED 开」
  = 拉起进程、「关」= 按进程组杀掉（`start_new_session=True`，厂商程序可能 fork）；
  手动与计划必须共用 `_LED_PROC`，否则计划结束时杀不掉手动拉起的那一个。
  「仅夜间自动开灯」的夜间判定**只在板端**（`control.is_night`，`LIGHT_NIGHT_START` /
  `LIGHT_NIGHT_END` 默认 18:00-06:00，允许跨零点，起止相同 = 关闭）：白天到达返回
  `skipped=daytime` 且**什么都不做**（不写 GPIO、不拉进程、不记「已开灯」——否则离开
  航点时会把操作员手动开的灯顺手关掉）。计划灯有看门狗（`LIGHT_MAX_ON_SECS` 默认
  900s），客户端掉线 / 被 kill 时兜底关灯；手动开的灯不进看门狗。
  改这块先读 `business/docs/plan-lights.md`，并跑 `lights_selftest.py`。
- **`relay.py` 的 dogcam720 要硬解**：它常驻把本体 1080p60 转成 720p，软解一条就吃
  一个核的 71%（`.246` 实测）。`_probe_decoder()` 会先真解 2 帧确认 `h264_rkmpp` 可用
  （写死 `-c:v` 有风险：解不出来时 ffmpeg 不报错、只是永远等不到帧），失败退回软解；
  状态看 `GET /api/relay/status` 的 `decoder`。`scale_rkrga`（RGA 硬缩放）在这台机器的
  ffmpeg 上用不了（`Error reinitializing filters`），别费劲。
- **CMA 只有 16MB，是这台板子的硬瓶颈**：`journalctl -b | grep -c cma` 几十条就是
  快满了，会连带 `scale_rkrga` 失败、甚至无日志硬重启。减 CMA 首选「少一路 1080p 转码」
  （前置切回 `1080p60` 档直拉），详见 `deploy/backpack-246.md` §4.1.1。
- **OSD 会反压主视频**：`OSD_FIFO_FPS` 默认 4，别为了「画框更顺」拉到 8+——板端实测会拖慢
  AI 那一路（`deploy/README.md` 有现场记录）。
- **降帧率只能走 `CAPTURE_OUT_FPS`**：现场四路 USB 相机 MJPG 只报 **30fps 的离散帧间隔**
  （`v4l2-ctl --list-formats-ext` 实测），把 `CAPTURE_FPS` 改成 15 会让 v4l2 直接 EINVAL、
  四路全黑。`stream.py` 在编码前加 `-vf fps=N` 丢帧；`CAPTURE_FPS` 保持 30（采集侧），
  `relay.py` 的 dogcam720 用 `CAPTURE_NOUT_FPS`。带框预览的 `OSD_FPS` 要同步降，
  否则源 15fps 会被 `fps=30` 补重复帧白耗编码。2026-11-19 线上已设 15，见
  `deploy/backpack-246.md` §4.1。
- **NPU 单路独占**：`DETECT_FPS_MAX=12` 是上限；`/api/detect/start` 只接受一路 camera
  （`single_cam=true`），不要放开多路并发。
- 代码风格沿用现有：4 空格、中文注释/文档字符串、模块顶部写「为什么」；不要引入
  pandas/numpy 这类板端没有的依赖（离线包在 `backend/package/`，已在 `.gitignore`）。

## 仓库化后的注意事项

- 模型与 `librknnrt.so` 已入库（`rknn/models/` 8 个包约 211MB）。**不要**再往仓库里塞新的
  `.rknn`/so；确实要换模型先跟维护者确认（必要时迁 Git LFS）。板端 `/opt/xjpt/backpack_dashboard/backend`
  下的模型是现场真源，换代时用 rsync 同步（见上文「发布到板端」）。
- `backend/detect_jobs/`、`backend/package/`、`__pycache__` 是运行期/离线包产物，已 ignore。
  `backend/detect_events.jsonl` 是事件样例（默认落盘路径），改动它等于改动默认输出格式，谨慎。
- 部分文件历史上是 CRLF（`.vscode/settings.json`、`tools/uvc_ctrl_probe.py`、
  `detect_events.jsonl`）。编辑时保持原样或用 LF 都行，但**不要整文件重排**，否则 diff 全是噪音。
