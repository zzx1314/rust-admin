# AGENTS.md — rust-admin

Monorepo containing a Rust backend API, a Vue 3 frontend admin panel and the standalone `business/` robot-dog service (Rust backend + Vue console).

## Project Structure

```
./
├── backend/          # Rust REST API (axum + sea-orm + SQLite)
├── business/         # Standalone robot-dog service: parks/devices/tasks/routes + console
│   └── backpack_dashboard/  # Board-side backpack Agent (Python/FastAPI + RKNN + dashboard)
└── frontend/         # Vue 3 admin panel (vue-pure-admin-max)
```

**Each subproject has its own detailed AGENTS.md — always read those first for domain-specific guidance:**
- `backend/AGENTS.md` — Rust backend patterns, SeaORM conventions, test structure
- `frontend/AGENTS.md` — Vue/TypeScript frontend patterns, component conventions
- `business/README.md` — standalone robot-dog service (parks, devices, tasks, routes)
- `business/backpack_dashboard/AGENTS.md` — board backpack Agent (Python + RKNN, `.246`)
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
**媒体只有一条路：WebRTC**（2026-11-19 起）。板端 relay 用 RTSP/TCP 把流转推到平台
mediamtx（`deploy/external-hz/mediamtx/`，TCP 8554 进 / UDP 8189 出）→ 浏览器 WHEP
（`/api/backpack/whep/*`，业务平台反代到只绑回环的 `:8889`），延时 ~300ms。
「FLV over WebSocket」那条老通路（含 `RELAY_OUTPUT`、`media=1`、平台环形缓冲扇出、
`RELAY_RTSP_FALLBACK` 退让）**已整体删除**，别再把它加回来——那一整套坑记在
`business/docs/backpack-video-integration.md` §6。
平台侧 `/api/backpack/video/ws` 仍然存在，但**只当在场心跳**：只计数 + 触发 `video_start`，
一个媒体字节都不转发。没有它按需推流和 30s 看门狗都不工作。板端起流时发
`video_transport` 上报通路，`GET /api/backpack/video/state` 的 `transport` 字段就是它：
现在只可能是 `rtsp`，**`ws`/null 说明那台板子的 relay 没升级**（前端据此直接报错，
而不是让人对着「起流中」干等）。**rtsp 通路的字节必须聚合后再写**
（`RELAY_RTSP_BATCH_*`，24KB/60ms）：ffmpeg 每包 write 的话上行 TCP 是
app-limited、cwnd 停在 10 个段，4G 的 96ms RTT 把吞吐压到 ~0.5Mbps（现场实测 0.55 →
1.03Mbps，drain 占用 70% → 1%）。改这块先读
`business/docs/backpack-video-integration.md` §7。

中继画质是**板端自适应**的（`deploy/backpack-relay/relay_push.py`）：5s 窗口内写平台
mediamtx 被反压且实测码率明显低于目标档位就就地换 `640x360@12 / 500k`（`video_reset` +
重启 ffmpeg，观看端不重连）；到期先确认链路真恢复才回配置档，反复升降时窗口翻倍。
**起流默认档位在平台「系统设置 → 视频中继」配**（`settings.video_relay_profile` /
`video_relay_auto_degrade`，默认 `degrade` 省流档 + 允许自动降档，现场主用 4G）：平台随
`video_start.payload.profile` 下发，保存设置时再广播 `video_config` 让正在看的流就地换档；
板端把它当起点（省流档还能再降到极省流档），关掉自动降档就锁死在该档。没有该字段时
（老板端 / 老平台）仍按板端 `RELAY_*` 环境变量走。
平台侧 `video.rs` 不做 ABR，前端也没有掉档 —— 积压判断全在板端（`_update_lag` 量 RTP
时间戳 vs 墙钟，超 `RELAY_MAX_LAG` 重启 ffmpeg）。改这块先读 §6.10⑥⑩、§7.5，回归：
`python3 deploy/backpack-relay/test_relay_push.py`。
`video_reset` 现在**只是一行日志**（平台没有流头要清、也不许踢观看端：媒体在 mediamtx 里，
重协商由 `lib/whep.ts` 自己按指数退避做，平台断了观看通道只会让计数掉到 0、看门狗跟着
`video_stop`）。切换直连/中继期间前端显示「最后一帧 + 起流中」。回归：
`cargo test --test video_relay`。
平台流量默认被 `/32` 钉在 Wi-Fi 上（画质优先）。Wi-Fi 一天抖几十次、宁可牺牲画质也要
稳定时，把 `deploy/backpack-relay/50-platform-route.sh` 的 `PIN_IFACE` 改成 `none`
（钉死 4G）。钉死后 `enx…` 默认还会被判成「慢链路」一步压到极省流档 320x180@8/200k ——
**先量 4G 真实上行**（2026-11-19 现场实测 8.5Mbps）：够快就在 unit 里加
`RELAY_SLOW_IFACES=` 关掉压档，交给自适应。§6.10⑮；现场实施记录（含 `.bak` dispatcher
抢路由那个坑）见 `deploy/backpack-4g-pin-2026-11-19.md`。

航点 AI 识别的「谁去调板端 Agent」由 `ROBODOG_POINT_AI`（`[backpack] point_ai`，默认
`auto`）决定：配了 `ROBODOG_BACKPACK_URL` 就服务端直连，否则由现场设备客户端代调
（客户端与背包 Agent 同在 `.246`，走 `127.0.0.1:8000`；消息为 `ai_point` / `ai_start` /
`ai_detect` / `ai_alert`（数量核对不一致）/ `ai_count`（数量核对一致回执，见下）/ `ai_end` / `ai_stop`）。航点动作的 `checkType`
分两类任务：`exist` 检测是否存在 / `count` 检测数量是否正确（后者带 `expectCount`，
不一致含「一个都没找到」都告警，且要「看满了」——客户端 `ai_end.expired`，或狗在这个点
停满 `dwell` 之后离开（平台先收到「已离开航点」关窗也算，窗口时长是 `dwell+8`，只认 `expired`
会让这类告警**永远报不出来**，2026-11-18 垃圾桶溢出漏报就是这么来的）。没有这个字段的历史数据按「有没有
`expectCount`」推断。`exist` 默认**只在检出时告警**（适合「不该出现」的目标）；
要「应该有却没有」，用 `count` + `expectCount`，或给 `exist` 勾上 `alertWhenMissing`
（看满停留时长且零检出 → 「AI 识别未检出」告警）。**`count` 任务只有「数量核对不一致」
会建单推群**：`ai_detect` 的检出只写任务时间线（`record_hit` 对 `count` 跳过告警入库），
不然一个航点检出几类目标就是几张工单几条群消息，把真正的数量结论淹掉
（回归 `ai_client_mode::count_task_detections_stay_out_of_alerts_and_wecom`）。
**核对一致默认不出声**；要在群里留一条「这轮核对上了」的回执，给动作勾
`notifyWhenCountOk`：板端 `plan_hit.count_ok=true` → 客户端转 `ai_count`（或服务端直连
模式自己识别）→ `notify::count_matched` **只推企业微信群，不落告警、不建工单**
（回归 `ai_client_mode::matched_count_notifies_wecom_without_alert_or_work_order`）。正式环境的云端
平台连不到内网 Agent，`auto` 走的就是客户端这条路；改这块先读
`business/docs/official-app-parity.md` §5.6。
**「现在站在哪个航点」只认 `task_progress.waypointIndex`（0 基下标），
`pointIndex` 是「已完成航点数」，混用会让 AI 窗口每 5s 被当成「到达下一个点」关掉**
（2026-11-17 现场事故，回归测试 `tests/ai_patrol.rs::interval_summary_does_not_kill_the_window`）。

**关窗不等于立刻下结论**：平台收到「已离开航点」时只下发 `ai_stop`、把窗口标成 `closing`
留在 `active` 里，等客户端把本点的 `ai_detect` / `ai_alert` / `ai_end` 说完（最多
`CLOSE_GRACE` = 12 s）。客户端收尾天然比关窗晚（停识别 → 板端 `_plan_flush` 落盘 →
扫盘/读图 → 上报），以前当场 `settle_no_hit` 会把「板端后到的检出 1 个」写成「实际检出 0 个」
并把证据图一起丢掉（2026-11-20 13:38 `TK-1112-11` PT2 现场，板端 `result.json` 是
`found_count: 1`）。改这段先看 `ai_patrol::close_client` / `spawn_close_grace`，回归
`ai_client_mode::late_count_alert_after_leaving_point_is_not_recorded_as_zero`。

**航点 AI 存证图 → 中台「媒体资源」**（2026-11-20 起）：板端命中时除了写临时存证
`detect_jobs/<planId>/pt<n>/`（企业微信推完即删），还固定保存 N 帧到媒体归档目录
（默认 `/media/media-data/picture/<planId>/pt<n>/`，**原始帧、不画框**——中台当训练
数据用；带框的那份只在临时存证里。与临时存证**分开**，不参与 `ai_cleanup`）。目录与帧数在**中台「参数配置」**里配（`media_picture_dir` /
`media_frames`，迁移 31 预置），链路是：中台读配置 → `ai_start` 下发 `mediaDir` /
`mediaFrames` → 业务平台 → 设备客户端 → 板端 Agent 写入；中台设备页「同步媒体」
按钮再沿同一条设备 WS 下发 `pictureDir`/`frames`，客户端扫目录**按每包 8 张/8MB
打 zip**（Stored，JPEG 不重复压缩）→ `POST /api/media/ingest` → 业务平台转发中台
`POST /api/dogInternal/media/upload`（`x-service-token`）→ 中台**解包后仍一张一条**
写入 `dog_media` + 落盘 `<upload_dir>/media/<设备>/<计划>/pt<n>/`，回 `results[]`；
客户端**只删 `ok` 的条目**（中台按图片 sha256 去重，`created=false` 也算成功），
单张模式保留给老客户端。这些图是**训练数据**：媒体页以下载为主（单张
`GET /api/dogMedia/{id}/file`，批量 `GET /api/dogMedia/export`——勾选 ids 或按
设备/计划/来源/时间筛选，服务端打 zip + `manifest.json`，上限 200 张 / 100MB），
同时保留单张预览 `GET /api/dogMedia/{id}/image`（JSON + base64，排查画框用）。
中台配置是唯一真源（60s 缓存），不要再在板端/客户端各配一份；改这条链路先读
`business/backpack_dashboard/AGENTS.md` 的「存证目录」一节与
`DOG.md` §2/§3。

**航点采集默认是关的**：`media_capture_enabled`（迁移 34 预置 `0`）是自动采集的
总开关，关着时板端只写 `detect_jobs/…` 告警证据图（企业微信配图、数量核对照旧），
不再往媒体归档目录攒训练图；开关随 `ai_start` 的 `captureEnabled` 下发（客户端
直连模式与板端直连模式都下发，老平台没这个字段时板端按「开」兜底）。要攒训练
数据就在中台「参数配置」把它改成 `1`，最多 1 分钟生效。新增媒体相关配置时
**不要**默认打开采集——现场主用 4G + 小板子，默默攒图会吃带宽和磁盘。

「同步媒体」的**回传是「清空目录」而不是「每航点最多 N 张」**：同一个 `pt<n>/`
每跑一轮就多 `media_frames` 张（文件名带时间戳），按帧数截断会让板端永远传不干净
（2026-11-20 现场：回执「删除板端文件 6」但目录里还躺着 6 张，就是每航点只传走 3 张）。
`media_frames` 只管板端写几张；客户端把 `pt<n>/` 里的图全部回传（单轮仍限 200 张，
超了回执「板端仍有 N 张未回传」），跑完连「只剩 `result.json`」的空目录一起收掉。

**「同步媒体」的状态要看得到**：它是长任务，中台 `POST /api/dogDevice/{id}/command`
只回「已下发」，审计**停在 `pending`**（`awaitingDevice=true`，同一 `requestId` 在跑的
10 分钟内不重复下发）。真实进度/结论由客户端 `command_progress`（每传完一包：已上传
8/24）/ `command_result`（扫几张、删几张、目录是否清空）经业务平台回写
`POST /api/dogInternal/media/sync-status`（`x-service-token`，只认 `同步媒体` 那一条
审计 + 设备编号要对得上），设备页轮询 `GET /api/dogDevice/{id}/mediaSyncStatus`；
`pending` 超过 10 分钟没有回报显示「无回报」。两个 2026-11-20 发布联调时踩到的坑：
① `requestId` 在设备侧是 **`params.params.requestId`**（业务平台 `push_command` 会再包一层），
`client main::command_request_id` 少认这一层就会「审计一直停在已下发」（回归
`command_result_tests::reads_request_id_when_the_platform_wraps_params_twice`）；
② 进度与结论是两条**各自 spawn 的异步上报**，晚到的进度会把「已完成：扫描 9 张…」
盖成「同步中：已上传 9/9 张」——所以 `audit::update_from_device` 里进度只允许改
`pending` 行（回归 `dog_command_control_tests::media_sync_dispatch_stays_pending_until_the_device_reports`
最后一段）。回归：
`dog_command_control_tests::media_sync_dispatch_stays_pending_until_the_device_reports`、
`dog_internal_tests::media_sync_status_report_updates_only_its_own_audit`、
`client media_sync::tests::collect_files_drains_every_frame_of_the_point`。

**计划的「发布状态」不是执行状态**：`tasks.publish_status`（`草稿` / `已发布`，迁移 029）
决定自动调度，`task_status` 只管执行进度。**只有已发布的计划才会自动执行定时任务**；
草稿在建计划（`create_task_core`）、设备上线补发（`gateway::dispatch_pending_tasks`）、
后台扫描（`schedule::sweep`）三条自动路径上全部被 `schedule::DueState::Draft` 挡住，
所以草稿也不报「窗口未执行」；人工 `POST /tasks/{id}/dispatch` 不受限（现场要能直接跑一次）。
**默认值必须是 `已发布`**（历史数据 / 老客户端行为不变），改成 `草稿` 等于升级当天全量停跑。
中台一键下发（`/zentai/devices/{id}/tasks`）建的任务固定 `已发布`。
发布 = `POST /tasks/{id}/publish`（先校验时间窗 + 路线坐标），撤回 = `unpublish`。
回归：`schedule::tests::draft_plans_are_never_auto_dispatched`、
`tests/schedule_window.rs::draft_plan_is_not_dispatched_until_published`、
`tests/api_smoke.rs::task_publish_status_gates_auto_scheduling`。

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

**航点灯光（补光灯 / LED）= 航点动作 + 计划夜间开关**：路线编辑器的航点动作多了
`lightFill`（开补光灯）/ `lightLed`（开 LED 灯带），计划（`tasks.light_night_only`，
迁移 030，表单上叫「仅夜间自动开灯」）决定它们是不是只在夜里开。客户端与 Agent
同机，到点（5009 `patrol.point_arrived` / `patrol.waiting`）调板端 `POST /api/lights`，
离开航点 / 到达无灯光动作的点 / 任务终态 / 取消时调 `/api/lights/off`。
**夜间判定只在板端**（`control.is_night`，`LIGHT_NIGHT_START` / `LIGHT_NIGHT_END`
默认 18:00-06:00 本地时间、允许跨零点、起止相同 = 关闭）：客户端只把 `nightOnly`
转下去，白天到达板端直接 `skipped=daytime` 什么都不做（不写 GPIO、不拉进程、
不记「已开灯」——否则离开航点会顺手关掉操作员手动开的灯）。**别把判夜搬到平台/客户端**：
两地时区或时钟不一致时，白天开灯现场无法解释。LED 只有厂商的 `test_rgbrun`（循环
闪烁，无常亮接口），开关 = 拉起/按进程组杀掉，手动按钮与计划**共用同一个进程句柄**。
计划灯有看门狗 `LIGHT_MAX_ON_SECS`（默认 900s），手动开的灯不看门狗。
改这块先读 `business/docs/plan-lights.md`；回归
`backend/tools/lights_selftest.py`、`client lights::tests`、
`api_smoke::task_light_night_only_reaches_dispatch_payload`。

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

**告警白名单（`settings.alert_ignore_fault_codes`，默认 `12608,13440`）是落库前的一道门**：
命中白名单的**本体故障码**（`code=12608` RS/lidar 时间不同步、`code=13440` 导航指令非法）
**不落 `alerts`、不自动建单、不推企业微信**，只在 `task_events` 事件流留一行。
过滤点在 `gateway::handle_device_event`（不是 `notify`）：放到推送层会让告警中心照样一秒一条。
只认码值（解析复用 `gateway::fault_code`，与自动工单去重键同一份），取不到 `code=` 的事件
一律不过滤；白名单留空 = 不过滤。改这块回归 `tests/alert_whitelist.rs`。

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

**推完即删板端存证**：图片那条企业微信回 `errcode 0` 之后，服务端在同一条设备 WS 上下发
`ai_cleanup {planId, pointIndex}`，客户端删掉 `detect_jobs/<计划ID>/pt<航点>/`
（画框 JPG + `result.json`，计划目录空了连它一起删）——板端每命中一次写一张图，一天几十轮
巡检很快把板子写满，而图进群之后没有第二次用途。三条线不能松：**只在推送成功后删**
（没图 / 图被拒 / 设备不在线都不删）、**窗口还开着先记账**（`AiPatrol::pending_cleanup`，
收尾扫完盘再删，否则会把还没推出去的那张图毁掉）、**只删安全路径**（`planId` 只认字母数字
汉字 `-` `_`，航点下标 ≥ 0）。要清哪一轮由告警的 `dedup_key` 反推
（`notify::evidence_cleanup`，`ai:<设备>:<任务>:<航点下标>:<what>`），非 AI 告警没有存证可删；
`count` 任务的**检出**不落告警，所以那些航点不会被清（结论告警带图时照样清）。回归用例：
`tests/ai_client_mode.rs::pushed_evidence_image_triggers_board_cleanup`（推送成功 → 收到
`ai_cleanup`）、`alert_without_image_keeps_the_board_evidence` /
`rejected_evidence_image_keeps_the_board_evidence`（不删）、
`notify::tests::evidence_cleanup_parses_only_ai_keys`、
`client ai_patrol::tests::ai_cleanup_removes_the_point_dir_after_the_image_is_pushed` /
`ai_cleanup_while_the_window_is_open_is_deferred` / `evidence_paths_refuse_unsafe_plan_ids`。

**计划跑完就删整个计划目录**（2026-11-21 起）：`ai_cleanup` 只覆盖「推过图的那个航点」，
数量任务的检出、一个都没命中的航点、图被拒 / 设备当时不在线的都等不到它，
`detect_jobs/<计划>/` 就一直堆在板子上。所以客户端在**任务终态 / 取消**时调
`AiPatrol::finish_plan`（本地 `RouteRunner` 收尾同样调），整个删 `detect_jobs/<计划>/`：
窗口还在收尾就记账（`AiPatrol::ending_plans`）等它跑完再删；新一轮同名计划开始就把标记撤销
（`AiPatrol::start`），别删新存证；客户端启动时 `AiPatrol::sweep_stale_plans` 再兜一把
「收尾没跑到」的残留（`STALE_PLAN_AGE` = 3 天没动过才删）。回归用例：
`client ai_patrol::agent_flow::finish_plan_removes_the_whole_plan_dir` /
`finish_plan_while_the_window_is_open_defers_to_the_reaper` /
`sweep_removes_only_stale_plan_dirs`。

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
中台 <http://39.107.55.13:3007>（3000 被本机 frps 占用）、业务平台/设备网关
<http://39.107.55.13:3008>。一键发布：

```bash
./deploy/external-hz/push.sh          # 打包 + 上传 + 安装重启
```

注意事项与目标机约束（CentOS 7 / glibc 2.17 / systemd 219、musl 静态构建、
systemd unit 写法、安全组端口）见 `deploy/external-hz/README.md`。

背包机 `.246` 连不上（拔电 / 不在现场）时，用 `deploy/backpack-sim/` 在本机复刻它：
mediamtx + ffmpeg 假相机 + 模拟 Agent（`:8000`）+ 仓库原版 relay / 设备客户端 /
site-console，可连本地 `business/server` 或已有平台。

```bash
cd deploy/backpack-sim && python3 run.py setup && python3 run.py up
# 另开终端：python3 run.py status  /  python3 run.py smoke
```

RKNN、真相机、Wi-Fi/4G 出口切换不能模拟（要真机）；细节与「能验 / 不能验」见
`deploy/backpack-sim/README.md`。

现场机器人本体（背包 `.246` → Orin NX `192.168.234.234` + RK3588 `192.168.234.1`）的
服务树、日志路径、`sdk_node=false` / `13897 capability busy` / `ctrl_source=3` / 急停卡死
（`estop.software` 恒 true、点「解除急停」无效）/ 「任务第 1 点秒失败」的排查顺序与
2026-11-13 / 11-14 事故复盘见 `deploy/robodog-body.md`（急停卡死见 §10，
结论是重启 `.1` 的 `dog_task`，不是重启 `.234`）。2026-11-14 那批问题的完整复盘
（含 `.234`/`.1` 的 `robots_dog_msgs` 版本不一致导致 `dog_task` 崩溃、
以及**用官方 APK 可以正常执行任务的临时方案**）见 `deploy/robodog-issues-2026-11-14.md`。
部署与现场发布流程见 `deploy/README.md`。
背包机 `.246` 本身（8 个 unit、`/opt/backpack-relay`、`/opt/robodog/client`、
`/opt/xjpt/backpack_dashboard`、mediamtx 的 7 条 path、`dispatcher.d` 路由脚本、
`/etc/robodog/client.toml` 等）的位置与环境变量见 `deploy/backpack-246.md`。
人不在现场同网段时，走它上面那条 frpc 反向隧道：`ssh -p 3006 root@39.107.55.13`
（公网 frps `:8090` → `.246:22`，出网走 4G；仓库 `deploy/backpack-246-frpc/`）。

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
| Backpack board Agent (Python, RKNN, cameras) | `business/backpack_dashboard/AGENTS.md` |
| Architecture questions | Both sub-AGENTS.md |
| Running/building | This file |
