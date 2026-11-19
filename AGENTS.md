# AGENTS.md — rust-admin

Monorepo containing a Rust backend API, a Vue 3 frontend admin panel and the standalone `business/` robot-dog service (Rust backend + Vue console).

## Project Structure

```
./
├── backend/          # Rust REST API (axum + sea-orm + SQLite)
├── business/         # Standalone robot-dog service: parks/devices/tasks/routes + console
└── frontend/         # Vue 3 admin panel (vue-pure-admin-max)
```

**Each subproject has its own detailed AGENTS.md — always read those first for domain-specific guidance:**
- `backend/AGENTS.md` — Rust backend patterns, SeaORM conventions, test structure
- `frontend/AGENTS.md` — Vue/TypeScript frontend patterns, component conventions
- `business/README.md` — standalone robot-dog service (parks, devices, tasks, routes)
- `DOG.md` — middle-platform resource module (maps, models, media, config)

## Commands

### Backend (Rust)
```bash
cd backend
./run.sh                     # Run API server (port 3000); exports the required secrets first
cargo run                    # Same, but requires APP_JWT_SECRET to be exported
cargo test                   # Run all tests
cargo clippy -- -D warnings # Lint
```

### Frontend (Vue)
```bash
cd frontend
pnpm dev                     # Dev server
pnpm build                   # Production build
pnpm lint                    # Lint (required before commit)
```

### Running Both
No combined command — run backend and frontend in separate terminals:
```bash
# Terminal 1
cd backend && cargo run

# Terminal 2
cd frontend && pnpm dev
```

## API Integration

Frontend proxies `/api` → `http://127.0.0.1:3000` (configured in `frontend/vite.config.ts`).

OAuth2 token endpoint: `/api/token` with Bearer token in Authorization header.

Device onboarding, gateways and task/route management are owned by the
`business/` service (see `business/README.md`). The root backend exposes only
the resource domains listed in `DOG.md`.

## Secrets

`backend/config/config.toml` is committed and holds **no** secrets. Required
environment variable: `APP_JWT_SECRET` (≥32 bytes). Optional:
`APP_REDIS_PASSWORD`, `APP_CONFIG_FILE`, `APP_DOG__BUSINESS_API_TOKEN`
（中台设备页「同步地图 / 指令 / 任务」调用业务平台内部接口的共享令牌，需与业务平台
`ROBODOG_SERVICE_TOKEN` 一致）。
Any config key can be overridden with `APP_<SECTION>__<KEY>`.

业务平台部署在没有入站端口的内网时，中台无法主动连它，改由业务平台拨入中台
的反向控制信道（`GET /api/dogDevice/control-plane/ws`）。此时业务平台需额外配置
`ROBODOG_ZENTAI_WS_URL`（如 `ws://10.0.0.8:3000`）；不配置则沿用「中台直连
HTTP」模式。两者共用同一个服务令牌，见 `business/README.md`。

背包页（`business/` 的「实时视频」）的接口/AI 有三条通道，后端按可用性自动退让
（`business/server/src/backpack.rs`）：浏览器直连（`ROBODOG_BACKPACK_BROWSER_URL`）→
服务端直连（`ROBODOG_BACKPACK_URL`）→ **设备客户端隧道**（`agent_tunnel.rs` /
`client/src/agent_relay.rs`，走已有设备 WS，只放行 4 个只读接口 + 3 个 AI 动作）。
所以「浏览器和背包不在同一网段」不再等于「AI 不能用」；视频跨网段仍走平台中继
（`deploy/backpack-relay/`），想看到框要让板端烧录（`preview` 流）。
多背包部署配 `ROBODOG_BACKPACK_TUNNEL_DEVICE`。详见
`business/docs/backpack-video-integration.md` §6.8。
中继画质是**板端自适应**的（`deploy/backpack-relay/relay_push.py`）：5s 窗口内 `ws.send`
被反压且实测码率明显低于目标档位就就地换 `640x360@12 / 500k`（`video_reset` + 重启
ffmpeg，观看端不重连）；到期先确认链路真恢复才回配置档，反复升降时窗口翻倍。
**起流默认档位在平台「系统设置 → 视频中继」配**（`settings.video_relay_profile` /
`video_relay_auto_degrade`，默认 `degrade` 省流档 + 允许自动降档，现场主用 4G）：平台随
`video_start.payload.profile` 下发，保存设置时再广播 `video_config` 让正在看的流就地换档；
板端把它当起点（省流档还能再降到极省流档），关掉自动降档就锁死在该档。没有该字段时
（老板端 / 老平台）仍按板端 `RELAY_*` 环境变量走。
平台侧 `video.rs` 不做 ABR（观看端追不上就 `lagged` 断连重连），前端也没有掉档。
改这块先读 §6.10⑥⑩，回归：`python3 deploy/backpack-relay/test_relay_push.py`。
中继重启 ffmpeg（`video_reset`）时平台会**主动断开观看端**让它重连拿新流头 ——
否则 flv.js 收到倒退的时间轴会「画面定住但字节还在流」；前端用
`liveBufferLatencyChasing` 追直播边缘（否则越看越延迟），切换直连/中继期间显示
「最后一帧 + 起流中」。回归：`cargo test --test video_relay`（§6.10⑦⑧）。

**积压是静默的**：平台给每个观看者的环形缓冲在溢出前不打任何日志（旧值 4MB ≈ 65 秒
@500k），上行抖一次之后板端 ffmpeg 会以 2~3 倍速把攒下的帧倒出来，观众端吃下十几秒就
一直「慢几秒」地看下去。所以 `bp:` 房间缓冲降到 128 片（1MB），并且按 FLV 视频 tag 时间戳
量「墙钟流逝 - 媒体流逝」，**超过 2s 就断开重连**（补的流头＝直播边缘），`video/state` 里
看 `viewerLagSecs`。推流端重连**不要**再 `重建房间`（会顺手踢掉正在看的人）。§6.10⑫。
平台流量默认被 `/32` 钉在 Wi-Fi 上（画质优先）。Wi-Fi 一天抖几十次、宁可牺牲画质也要
稳定时，把 `deploy/backpack-relay/50-platform-route.sh` 的 `PIN_IFACE` 改成 `none`
（钉死 4G）。钉死后 `enx…` 默认还会被判成「慢链路」一步压到极省流档 320x180@8/200k ——
**先量 4G 真实上行**（2026-11-19 现场实测 8.5Mbps）：够快就在 unit 里加
`RELAY_SLOW_IFACES=` 关掉压档，交给自适应。§6.10⑮。

航点 AI 识别的「谁去调板端 Agent」由 `ROBODOG_POINT_AI`（`[backpack] point_ai`，默认
`auto`）决定：配了 `ROBODOG_BACKPACK_URL` 就服务端直连，否则由现场设备客户端代调
（客户端与背包 Agent 同在 `.246`，走 `127.0.0.1:8000`；消息为 `ai_point` / `ai_start` /
`ai_detect` / `ai_alert`（数量核对不一致）/ `ai_end` / `ai_stop`）。航点动作的 `checkType`
分两类任务：`exist` 检测是否存在 / `count` 检测数量是否正确（后者带 `expectCount`，
不一致含「一个都没找到」都告警，且要「看满了」——客户端 `ai_end.expired`，或狗在这个点
停满 `dwell` 之后离开（平台先收到「已离开航点」关窗也算，窗口时长是 `dwell+8`，只认 `expired`
会让这类告警**永远报不出来**，2026-11-18 垃圾桶溢出漏报就是这么来的）。没有这个字段的历史数据按「有没有
`expectCount`」推断。`exist` 默认**只在检出时告警**（适合「不该出现」的目标）；
要「应该有却没有」，用 `count` + `expectCount`，或给 `exist` 勾上 `alertWhenMissing`
（看满停留时长且零检出 → 「AI 识别未检出」告警）。正式环境的云端
平台连不到内网 Agent，`auto` 走的就是客户端这条路；改这块先读
`business/docs/official-app-parity.md` §5.6。
**「现在站在哪个航点」只认 `task_progress.waypointIndex`（0 基下标），
`pointIndex` 是「已完成航点数」，混用会让 AI 窗口每 5s 被当成「到达下一个点」关掉**
（2026-11-17 现场事故，回归测试 `tests/ai_patrol.rs::interval_summary_does_not_kill_the_window`）。

「跑完要不要返航」是**任务属性**（`tasks.end_task` → 载荷 `endTaskMode`，
`none` / `return_to_start` / `return_to_charge`，不配 = 跟随路线），不是路线属性：
巡检 `5000 kind=4` 只把原始 `points` 写成本体路径，返程由本体按 `end_task` 自己规划。
`routeData.returnLeg` 只进画布显示与里程/耗时/耗电估算，改它不会改返航行为。
没有 `endTaskMode` 时客户端按 `loop_mode == "闭环"` 兜底返回起点（老行为），
所以新增 `endTaskMode` 时**不能**让服务端对所有任务默认下发 `none`，否则升级即全量停返航。

「返回起点」回的是**起步时本体 `capture_origin_pose` 采的起点位姿**，不是地图原点。载图
定位（`4030 op=1`）的 `initial_pose` **缺省必须发零四元数**（`Pose::unspecified()`）：协议
§2.4.1 里只有四元数全零才表示「读上一份位姿文件做完整重定位」，而 `Pose::default()` 的单位
四元数是一个**合法位姿**（原点 + 0°），会让起点位姿被采成地图原点、返航跑回原点
（回归用例 `load_map_without_pose_sends_zero_quaternion`）。派单时的定位是**自动**的：
`ensure_localized()` 判「这条路线绑的图 + loc 100」，缺一项就自己 4030 重定位、不收敛再
重试一次，不依赖人工点「重新定位」；`state.active_map` 只在定位真的成功后更新，不要按计划里
的 `mapId` 乐观写入（否则守卫会误判「已载好」而跳过自动重定位）。

企业微信告警推送：群机器人 webhook **在平台里配**（系统设置 → 企业微信群机器人，
存 `settings.wecom_webhook`，`value_type='secret'`，保存即生效、接口只回掩码、
不进 `server.toml`）；环境变量 `ROBODOG_WECOM_WEBHOOK` 保留为部署级兜底，平台为空
时才用。另可选 `ROBODOG_PUBLIC_URL` 拼消息里的「查看详情」链接。用法见 `business/README.md`「企业微信告警推送」，**设计与扩展点见
`business/docs/wecom-alert-push.md`（改这块之前必读）**：架构、代码地图、三层开关、
去重/限流参数、触发点清单、新增告警源的做法、踩坑记录。

责任人库（`staff`）可绑定平台账号（`staff.user_id`，迁移 025，一个账号最多一个责任人）：
绑定后「只看我负责的」（`GET /api/workorders?mine=true`）按它过滤，且系统派单的处置权限
收敛到该责任人 + 管理员；**没绑账号的责任人，其系统单仍是公共待办**（否则派了单没人能动）。
账号没绑责任人时保持老口径（自己建的 + 全部系统派单）。改动这块注意
`handlers/workorders.rs` 的 `append_visible_filter` / `ensure_visible` 与
`staff.user_id` 的唯一索引，回归用例：`tests/workorder_scope.rs`。

告警自动派单（`settings.alert_auto_workorder`，默认开）的**去重键是结构化的**：
建告警时写 `alerts.dedup_key`（`ai:<设备>:<任务>:<航点>:<类别|count|missing>` /
`fault:<设备>:<故障码>` / `hb:<设备>` / `ctrl:<设备>`），`alert_flow::find_open_order` 按
「同一设备 + 同一键 + 24h 内在办」复用老单。**新增告警源请带 key**——不要靠标题：
标题是「设备号 + 内容前 60 字」，内容里一有置信度/证据路径就会每轮新建一张工单。

「哪些告警才建单 + 推群」由 `settings.alert_only_in_task`（默认开）决定：只有**执行计划
期间**的告警（该告警绑定的设备当时有 `执行中` 的任务，`alert_flow::in_task_execution`）
才自动建工单、推企业微信；掉线（`hb:`）/ 控制权抢占（`ctrl:`）/ 页面手动新建 一律**只落库**
（告警中心照常能看，但不建单、不进群、不回写 `assignee` / `work_order_id`）。
所以新加告警源时先想清楚它算不算「执行计划期间」；想绕过就先看 `notify::alert_created`
的第一道门，不要在各调用点各自判断。回归用例：`business/server/tests/alert_in_task.rs`。

告警**证据图**（AI 识别**检出** / **数量核对不一致**时的板端抓拍）走企业微信
`msgtype=image`，作为 markdown 卡片之后的**第二条**消息：板端图在
`detect_jobs/<plan>/pt<n>/*.jpg`，平台读不到，所以由客户端（与 Agent 同机）读盘后上报 ——
数量核对附在 `ai_alert.evidenceImage`（板端 `plan_hit.evidence` 给路径），实时检出附在
`ai_detect.evidenceImage`。
⚠️ 检出的图**不是检出那一刻就有的**：板端 `_plan_frame` 要先连采 3 帧命中、画框存证，
才发带 `evidence` 的 `plan_hit`（实测比第一帧检出晚 **4.6s**）。所以客户端把命中**先挂起**、
拿到 `plan_hit.evidence` 再上报（`ai_detect` 晚几秒，换来卡片后面必有图；8s 等不到就先发
卡片），读不到路径才退回扫 `<jobs_dir>/<plan>/pt<n>/` 取本轮最新一张（`jobs_dir` 见
`[ai] jobs_dir` / `ROBODOG_AI_JOBS_DIR`）。命中帧稀疏时（板上只命中 1 帧，采样连采 3 帧
跑不完）`plan_hit` 要到**窗口收尾** `_plan_flush` 才发，图往往比卡片晚几十秒：客户端
`finish()` 会**先停识别再扫盘**，扫到图就单独报一条 `ai_evidence`，服务端
`notify::alert_evidence` 只补一张 `msgtype=image`（`Notice.image_only`，不重发卡片、
不重复落告警）。⚠️ 这条 `ai_evidence` 几乎**总是**在窗口关闭之后才到（平台收到「已离开
航点」就先摘窗口并下发 `ai_stop`，客户端正是收到它才开始扫盘），所以 `on_client_evidence`
不能只查 `active` 窗口——关掉的窗口要在 `AiPatrol.recent` 里留 30 分钟痕迹（2026-11-18 21:34
pt1「Trash over flow」卡片发了、图没发就是踩在这里）。服务端 `notify::evidence_image`
校验（base64 / ≤2MB / JPG-PNG / md5 算**base64 之前**的字节）再发。⛔ 不要改成往 markdown
里嵌 `![]()`（群机器人不支持），也不要存多 MB 的 base64 到 `notifications.payload`。
开关 `settings.wecom_alert_image`（默认开），回归用例 `tests/alert_image.rs`、
`tests/ai_client_mode.rs`（检出带图整条 WS 链路 / `ai_evidence` 只补图 /
`late_evidence_after_window_closed_is_still_pushed` 窗口关了才补图）、
`client ai_patrol::agent_flow::evidence_landing_seconds_late_still_rides_with_the_alert`、
`client ai_patrol::agent_flow::card_sent_without_image_is_followed_by_ai_evidence`。

`backend/run.sh` exports the required secret (and probes Redis for
`APP_REDIS_PASSWORD`) before running `cargo run`. Set `APP_REDIS_PASSWORD`
when Redis enforces `requirepass` (the compose one uses `123456`), otherwise
login fails with `Redis set failed: NOAUTH: Authentication required`.

没有 Redis 的机器可以用内存令牌存储：`APP_TOKEN_STORE=memory ./run.sh`
（`[redis]` 段可删；进程重启后需重新登录，仅单实例可用）。默认仍是 Redis。

### 单端口部署（不再需要 nginx）

`[server] static_dir` / `APP_SERVER__STATIC_DIR` 指向前端构建产物后，后端会自己
托管前端：SPA 路由回落 `index.html`、`/static/*` 长缓存 + gzip，`/api/*` 保持
JSON 404。生产环境只需要跑一个后端进程：

```bash
cd frontend && pnpm build
cd ../backend && APP_SERVER__STATIC_DIR=../frontend/dist ./run.sh
```

## Deployment

外网正式环境：`root@39.107.55.13`，安装目录 `/opt/hz/rust-admin`，
中台 <http://39.107.55.13:3005>（3000 被本机 frps 占用）、业务平台/设备网关
<http://39.107.55.13:3100>。一键发布：

```bash
./deploy/external-hz/push.sh          # 打包 + 上传 + 安装重启
```

注意事项与目标机约束（CentOS 7 / glibc 2.17 / systemd 219、musl 静态构建、
systemd unit 写法、安全组端口）见 `deploy/external-hz/README.md`。

现场机器人本体（背包 `.246` → Orin NX `192.168.234.234` + RK3588 `192.168.234.1`）的
服务树、日志路径、`sdk_node=false` / `13897 capability busy` / `ctrl_source=3` / 急停卡死
（`estop.software` 恒 true、点「解除急停」无效）/ 「任务第 1 点秒失败」的排查顺序与
2026-11-13 / 11-14 事故复盘见 `deploy/robodog-body.md`（急停卡死见 §10，
结论是重启 `.1` 的 `dog_task`，不是重启 `.234`）。2026-11-14 那批问题的完整复盘
（含 `.234`/`.1` 的 `robots_dog_msgs` 版本不一致导致 `dog_task` 崩溃、
以及**用官方 APK 可以正常执行任务的临时方案**）见 `deploy/robodog-issues-2026-11-14.md`。
部署与现场发布流程见 `deploy/README.md`。

## Important Notes

- **Backend**: SQLite database at `backend/data/users.db`; session storage is
  Redis by default, or process memory with `APP_TOKEN_STORE=memory`
- **Frontend**: Requires Node 18+/pnpm 9+, 4GB RAM for dev, 8GB for build
- Both projects have pre-commit hooks — run lint before committing
- Backend tests use UUID-based temporary database files
- Map uploads land in `[dog] upload_dir` (default `backend/data/device_uploads/maps`)

## When to Use Which AGENTS.md

| Task | Read |
|------|------|
| Backend code changes | `backend/AGENTS.md` |
| Frontend code changes | `frontend/AGENTS.md` |
| Robot-dog park/device/task/route changes | `business/README.md` |
| Architecture questions | Both sub-AGENTS.md |
| Running/building | This file |
