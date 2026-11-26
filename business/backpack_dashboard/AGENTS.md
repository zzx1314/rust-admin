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
│                     （`/api/network` 把 WiFi SIGNAL / 4G CSQ 统一成 0-100，是
│                      设备客户端心跳 `signal` + `signalSource` 的来源）
├── control.py        LED 闪烁 / 补光开关
├── record.py         航点 / 航段录像（ffmpeg 从本机 mediamtx 拉流，可多路相机，落 mp4 + 边车）
├── speak.py          航点 / 航段语音播报（聆思 TTS：call 0/1 <内容> / call stop）
├── phone.py          模拟电话（板端 ⇄ 同网段 PC 的 UDP 双向对讲：phone <ip> <port> / phone stop）
├── photo.py          航点拍照（ffmpeg 从本机 mediamtx 抓 JPEG，客户端到点触发）
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

# 录像状态机自检（起停 / 幂等 / 清单 / 删除 / 超期清盘，改 record.py 后必跑；不需要相机）
python3 business/backpack_dashboard/backend/tools/record_selftest.py

# 语音播报自检（内容净化 / 幂等 / 循环看门狗 / 收尾，改 speak.py 后必跑；不需要聆思小板）
python3 business/backpack_dashboard/backend/tools/speak_selftest.py

# 模拟电话自检（IPv4/端口白名单含注入 / asr 门禁 / 先收播报 / 挂断幂等，改 phone.py 后必跑）
python3 business/backpack_dashboard/backend/tools/phone_selftest.py

# 航点拍照自检（抓拍 / 多帧 / 清单 / 删除 / 路径白名单 / 超期清盘，改 photo.py 后必跑）
python3 business/backpack_dashboard/backend/tools/photo_selftest.py

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

## 录像（航点 / 航段「录像采集」）

板端只负责「按客户端的指令起停 ffmpeg」和「落盘」，起停时机全在设备客户端
（5009 状态机）：`POST /api/record/start|stop`、`GET /api/record/list|status`、
`POST /api/record/delete`，实现在 `backend/record.py`，客户端在
`business/client/src/record.rs`（同机 `127.0.0.1:8000` 调用，**不经平台隧道**）。

| 环境变量 | 默认 | 说明 |
|---|---|---|
| `RECORD_DIR` | `/media/media-data/record` | 落盘根目录（SSD；不要指到 eMMC） |
| `RECORD_SOURCE_CAM` | `dogcam` | **默认**录像源（路线动作没配 `recordCameras` 时用），`dogcam` 按当前档位选 `dogcam`/`dogcam720` |
| `RECORD_MAX_CAMERAS` | `4` | 一次最多同时录几路（路线动作多选；超出只丢多的几路） |
| `RECORD_RTSP_HOST` | `127.0.0.1` | 本机 mediamtx |
| `RECORD_POINT_DEFAULT_SECS` / `RECORD_POINT_MAX_SECS` | `15` / `300` | 航点录像默认 / 上限（路线动作可覆盖） |
| `RECORD_SEGMENT_SECS` | `300` | 航段录像切片长度（一段路一个 mp4，逐段回传） |
| `RECORD_TASK_MAX_SECS` | `7200` | 航段 / 老全程录像的兜底上限（客户端被杀时防跑飞） |
| `RECORD_RETENTION_HOURS` | `72` | 没传走的录像保留多久 |
| `RECORD_MIN_FREE_MB` | `512` | 剩余空间低于它不开新录像（写满会连 AI 存证一起丢） |
| `RECORD_TRANSCODE` | `0` | `1` = 重编码（源不是 H264 时）；默认 `-c copy` 不抢 MPP |
| `RECORD_LOG` | `/tmp/record_ffmpeg.log` | ffmpeg 的 stderr（追加）。「录像没文件」先看它 |

约定：

- **多路相机**：`POST /api/record/start` 的 `cameras: ["dogcam","cam0",…]`（也认单个
  `camera`）——**每一路一个独立 ffmpeg + 一个独立 mp4**，job key 是
  `<plan>|<scope>|<point>|<cam>`；`stop` 仍按 plan+scope+point 收，所以离开航点会把
  这一点的所有相机一起停。可选相机与 `photo.py::allowed_cameras()` 同一份
  （`dogcam` / `cam0`~`cam3` / `dog-ipcam`，`dogcam` = 正前 = 机器狗前置）。
  不认识的名字只丢那一路（回执 `rejected`），上游没推流的那一路起不来也只丢它
  （回执 `errors`）——**绝不让一路的失败拖垮整条录像**。但**点了相机却一路都不可用时
  直接 `ok=false`，不拿 `RECORD_SOURCE_CAM` 顶替**：那会录成另一路画面还不报错
  （2026-11-25 现场“航段配右前 `cam1`、录下来是正前 `dogcam`”）。只有请求里
  一个相机都没点（历史路线 / 老客户端）才退到默认源。
- **不重编码**：源已经是 H264，`-c copy` 封装成 mp4，不与实时视频抢 MPP；
  `-movflags +faststart` 让浏览器边下边播。多路也是 `-c copy`，只多几路拉流 + 封装。
- ⚠️ **必须关掉 edit list**（`use_editlist=0`，航点单文件 `-use_editlist 0`；航段切片走
  `-segment_format_options "movflags=+faststart:use_editlist=0"`，多个选项用 `:` 分隔）：
  ffmpeg 从 RTSP 拉流时第一帧的 DTS 不是 0（要等源的下一个关键帧：`dogcam` ~2s、
  `cam0`~`cam3` **8~13s**），mov 封装器会把这段偏移写成**空 edit**，于是
  `mvhd.duration = 画面 + 偏移`。现场 `leg3-cam1-20261126-144815-000.mp4` 的 `mvhd`
  65.35s 而真正能解的只有 52.4s：播放器按 `mvhd` 画进度条、按第一帧开播，**最后那段
  偏移只有进度条没有画面**（现场原话「后面几秒不能播放」，而 Chrome 反而会在片头空
  十几秒——同一个空 edit）。别用 `-copyinkf` 硬补片头：关键帧之前的 P 帧解不出来。
  边车的 `durationSec` 也改成读文件自己的 `mvhd`（`_mp4_duration`）：job 墙钟会把等
  关键帧那十几秒算进去，平台就写成 65.4s 而实际只有 52.4s。
- **范围（`scope`）**：`point` = 航点录像（`-t seconds` 一个 mp4）、`leg` = 航段录像
  （起步开、到下一个航点停，`-f segment` 按 `segmentSeconds` 切；`pointIndex` 是
  **起点航点下标**）、`task` = 旧版「全程录像」（整条任务一条流，只为兼容老客户端保留）。
  `stop` 带 `scope` / `pointIndex` 时只停那一段，不带就停该计划全部。
- **文件名 + 边车**：`RECORD_DIR/<planId>/<pt<n>|leg<n>|task>-<相机>-<时间戳>[-<片段>].mp4`，停下时
  写同名 `.json`（planId / scope / pointIndex / pointName / camera / 起止时间 / 时长）。客户端按
  边车拼回传元数据（含 `camera`），传成功才调 `/api/record/delete` 删文件。
  老文件（没有相机段的文件名 / 边车没 `camera`）回执里按 `RECORD_SOURCE_CAM` 兑底。
- ⚠️ **切片模板必须带 `%03d`**（`leg0-cam1-20261126-110421-000.mp4`…）：只靠开始时间戳
  时 segment 每片都写同一个文件、后一片把前一片**覆盖**掉（2026-11-26 现场：航段
  8 分钟只剩最后几秒）。也**别再加 `-strftime 1`**：时间戳已经在模板里，它还会把
  `%03d` 当日期格式吃掉（glibc 给 `026`）。边车里的 `durationSec` 取文件自己的
  `mvhd`（读不到才退回 `-segment_list` 的 csv / job 时长），所以**每一片**写的是它
  自己的真实时长，不是整个 job 的。
- **`list` 不返回正在录的文件**：客户端可能传半截 mp4；`stop` 会等 ffmpeg 写完
  边车再返回（`job.finished`），所以「停 → list」是安全的。
- **`delete` 只认 `<planId>/<文件名>.mp4`**：路径穿越、`..`、非 mp4 一律拒绝。
- 录像源 URL 可能含账号密码（`DOG_RTSP_URL`），**不进接口、不进日志**。

## 语音播报（航点「语音播报」/ 航段「语音播报」）

板端只负责「按客户端的指令说话」：`POST /api/speak`、`POST /api/speak/call`、
`POST /api/speak/stop`、`GET /api/speak/status`，实现在 `backend/speak.py`，客户端在
`business/client/src/speak.rs`（同机 `127.0.0.1:8000` 调用，**不经平台隧道**）。
起停时机同样在 5009（航点播报 = 到点 / 离开，航段播报 = 起步 / 到达下一个航点），
平台只下发路线动作——板端不要自己按时序猜。

另有一个**人工「呼叫」**（任务详情页喊话）：平台 `POST /tasks/{id}/call` → 设备指令 `呼叫`
→ 客户端 `manual_call` → 这里 `POST /api/speak/call`，**不走 5009**、也不靠计划。

| 环境变量 | 默认 | 说明 |
|---|---|---|
| `SPEAK_MAX_CHARS` | `30` | 中文汉字上限（板端固件限制，同时也是 FIFO 注入护栏） |
| `SPEAK_MAX_LEN` | `40` | 总字符上限 |
| `SPEAK_MAX_SECS` | `600` | 循环播报看门狗：客户端掉线/被杀时到点自动 `call stop` |

| 接口 | 命令 | 语义 |
|---|---|---|
| `/api/speak` `{text, loop, seconds}` | `call 0 <内容>` / `call 1 <内容>` | 单次 / 循环；同一 `(planId, scope, pointIndex)` + 同文本重复请求幂等 |
| `/api/speak/call` `{text, loop, seconds}` | 同上 | 人工呼叫（`manual=true`）：同一句话可重复喊（**不幂等**），循环缺省 30s |
| `/api/speak/stop` `{reason}` | `call stop` | 停止并回到 voice 模式（没有在播时什么都不写） |

约定：

- **TTS 在云端**：`start` 会先 `telemetry.ensure_asr()`；断网 / asr 起不来回
  `ok=false + error`，**绝不影响巡检**（客户端只打日志）。这也是为什么播报失败
  不能做成 5xx 重试风暴 —— 5009 每 500ms 一帧。
- **内容白名单**（`normalize`）：去所有空白 → 只留中文汉字 / 中文标点 / 数字 →
  按 `SPEAK_MAX_CHARS` / `SPEAK_MAX_LEN` 截断。白名单之外的字符**丢掉并计数**
  （日志「内容已净化：丢弃 N 个不支持的字符」），不报错 —— 现场配歪了也不能让任务停。
  ⚠️ 这份白名单同时是**命令注入护栏**（`_write_lingsi_cmd` 走 shell `echo`）。
- **说了就要收**：单次播报按 `0.4 × 字数 + 3s` 估时后主动 `call stop` 回到 voice；
  循环播报由客户端在离开航点 / 终态发 stop，看门狗兜底。**不要**让板子长期停在
  call 模式（唤醒词会失效）。
- **一次只播一条**：FIFO 单通道，`start` 里会先 `call stop` 掉上一条再起新的；
  新的 `start` 会让上一条的定时器作废（`_STATE["token"]`）。
- 手工验：`curl -XPOST 127.0.0.1:8000/api/speak -d '{"text":"请避让","loop":false}'`；
  用 `deploy/backpack-sim` 时假 Agent 同样实现这三个接口（`/api/sim/state` 的 `speak`）。
- 完整链路（平台编辑器 → 客户端 → 板端）见 `business/docs/plan-speech.md`。

## 电话（对讲）

`phone.py`：把「和现场的人说话」落地为聆思 FIFO 的两条命令，音频是**局域网 UDP 直连**，
不经平台也不经中继：

| 接口 | 命令 | 语义 |
|---|---|---|
| `POST /api/phone` `{peer, peerPort}` | `phone <对端IP> <端口>` | 发起通话（先进 `speak.stop` 收播报，再 `ensure_asr()`） |
| `POST /api/phone/stop` `{reason}` | `phone stop` | 挂断回 voice 模式（不在通话中时幂等） |
| `GET /api/phone/status` | — | 通话态 + 板端 IPv4 网卡（页面据此拼 PC 端命令） |

- **平台侧入口只有「网页直连」**：设备客户端在本机 `bind 127.0.0.1:0` 当那个对端
  （`phone 127.0.0.1:<port>`），音频经设备 WS 转到浏览器（见 `business/docs/phone-intercom.md`）。
  平台**不再**提供「拨给现场某台 PC」的入口；下面的 PC 脚本只留给现场调试。
- **板端固定监听 5000**（`phone.LOCAL_PORT`）：`phone <ip> <端口>` 里的端口是对端 PC 的，
  PC 端命令是 `python voice_call_pc.py --peer <板端IP> --peer-port 5000 --local-port <同一端口>`
  （脚本在 `backend/pc_tools/`，`GET /api/lingsi/voice_call_pc.py` 可下载）。
- ⚠️ **对端地址必须白名单校验**（`normalize_peer`：点分十进制 IPv4 + 1~65535）：
  这条命令同样进 shell `echo`，对端地址来自界面输入。平台 / 客户端 / 板端三层都校验。
- **FIFO 单通道**：进电话模式会打断播报，所以 `start` 先 `speak.stop()`，
  否则客户端以为循环播报还在响。
- 完整链路 / 现场排查见 `business/docs/phone-intercom.md`。

## 航点拍照（航点动作「可见光拍照」）

板端只负责「按客户端的指令抓一张图 + 落盘」：`POST /api/photo`、`GET /api/photo/list`、
`POST /api/photo/delete`、`GET /api/photo/status`，实现在 `backend/photo.py`，
客户端在 `business/client/src/photo.rs`（同机 `127.0.0.1:8000` 调用，**不经平台隧道**）。
起停时机在 5009（到点抓拍、任务终态回传），平台只下发路线动作。

| 环境变量 | 默认 | 说明 |
|---|---|---|
| `PHOTO_DIR` | `/media/media-data/photo` | 落盘根目录（SSD；不要指到 eMMC） |
| `PHOTO_SOURCE_CAM` | `dogcam` | 路线动作没配 `photoCamera` 时的默认源 |
| `PHOTO_RTSP_HOST` | `127.0.0.1` | 本机 mediamtx（与录像 / AI 抽帧一致，不碰 v4l2） |
| `PHOTO_MAX_FRAMES` | `5` | 单次最多抓几张（路线动作的张数被夹到这个上限） |
| `PHOTO_FPS` | `2` | 多帧时的采样帧率（连着抓 N 帧是同一张图） |
| `PHOTO_QUALITY` | `2` | ffmpeg `-q:v`（2 最好，1080P 约 300~600KB） |
| `PHOTO_TIMEOUT_SECS` | `15` | 抓拍超时（等 RTSP 起流 + 解码首帧），超时即失败 |
| `PHOTO_MIN_FREE_MB` | `128` | 剩余空间低于它就不抓 |
| `PHOTO_RETENTION_HOURS` | `72` | 没传走的照片保留多久 |
| `PHOTO_LOG` | `/tmp/photo_ffmpeg.log` | ffmpeg 的 stderr（追加）。「拍出来是黑的」先看它 |

约定：

- **相机白名单**：`allowed_cameras()` = `config.CAMERAS` 的 key（cam0~3 / dogcam）
  ∪ `dogcam720` / `dog-ipcam`。配错相机直接 400 + 可选项，别让现场以为在拍。
- **文件名即元数据**：`<时间戳>-<相机>[-序号].jpg`，`list_files` 从文件名反解相机；
  航点名记在 `<计划>/points.json`（客户端重启后回传也能带上名字）。
- **抓拍失败绝不影响巡检**：流没在推 / ffmpeg 缺失 / 磁盘不足都回 `ok=false + error`；
  客户端只打日志、任务照跑（5009 每 500ms 一帧，不能重试风暴）。
- **不碰 AI 存证目录**：`detect_jobs/` + `MEDIA_DATA_DIR` 是训练图 / 告警证据，
  这里是巡检结果照片（`PHOTO_DIR`），留存与消费方都不一样。
- **`delete` 只认 `<planId>/pt<n>/<文件名>.jpg`**：路径穿越、非 jpg 一律拒绝；
  文件删空后连 `points.json` 与计划目录一起收掉。
- 完整链路见 `business/docs/waypoint-photo.md`。

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
- **计划灯光（补光灯 / LED）只有一份状态**：`control.py` 的补光灯 GPIO + LED 开关
  被**手动按钮**（`/api/filllight`、`/api/led/blink`）和**计划**（`/api/lights`、
  `/api/lights/off`）共用。
  **LED 走 systemd**：`systemctl start/stop alarmlight`（`LED_SERVICE`，板端 unit =
  `/opt/xjpt/led/alarmlight`，72 颗灯珠红蓝 100ms 爆闪，**不要 enable**）。
  为什么不 Popen 二进制：厂商程序只在 **SIGINT** 里写「全 0 + reset」熄灯，老逻辑发
  SIGTERM 是默认终止 —— 进程没了、灯珠停在最后一帧亮着；unit 里 `KillSignal=SIGINT` +
  `TimeoutStopSec=10` 才是真正的「关灯」，而且进程归 systemd 管、Agent 重启不会留下
  继续抢 SPI 的孤儿。没有 unit 的板子自动退回「直接跑 `LED_TEST_BIN`」（`LED_CONTROL`
  = `auto`/`systemd`/`proc` 可强制），兜底路径关灯也是**先 SIGINT** 再退 SIGTERM/SIGKILL。
  状态里的 `led` 是问 `systemctl is-active` 得来的真实现状（`led_control` /
  `led_service` / `led_service_state` 一并回报），不是乐观写入。
  计划级「自动开灯」（`tasks.light_fill` / `light_led`）由客户端在任务一开始就调一次
  `/api/lights`（`pointIndex=-1`），全程不关，终态 / 取消才 `/api/lights/off`；计划没配
  才走航点动作的「到点开、离开关」。`systemctl start/stop` 天然幂等，客户端 300s
  一次的重发 / 看门狗重入都不会出问题。
  「仅夜间」的夜间判定**只在板端**（`control.is_night`，默认窗 `LIGHT_NIGHT_START` /
  `LIGHT_NIGHT_END` = 18:00-06:00；计划里自定义的 `nightStart`/`nightEnd` 优先，
  允许跨零点，起止相同 = 关闭）：白天返回
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
