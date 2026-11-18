# 与官方 App（SmartNav 1.2.1）对齐记录

调查对象：`~/SmartNav_1.2.1.apk`（`com.jszr.smart_nav`，versionCode 17）。
它是 Flutter 应用，控制逻辑在 `lib/arm64-v8a/libapp.so`（Dart AOT），底层用厂商
`smart_sdk`；本文的结论来自 AOT 字符串、文件路径、日志文案和协议文档，不是官方源码。

## 一、官方 App 的控制链路（从 APK 提取）

```text
SmartSdk connect
  → 等 sdk_node / G20 SDK 初始化（SdkNodeAvailabilityHelper / 设备初始化中）
  → 1204 拿 RoamerX 控制权（RoamerXControlHelper: 1205 not ok code=）
  → 检查 ctrl_source（BodyCtrlSourceHelper: non-sdk ctrl_source=）
  → 检查姿态：请先站立，再切换至巡航模式
  → 1302 cmd=62 切导航模式（wait_for_nav_mode）
  → 选地图 / 路径（4012/4013 map_path_list、map_route_exist、path_id）
  → 创建巡检任务（PatrolTaskWriteRequest / createPatrolTask）
  → 5009 type=patrol：patrol.navigating / point_arrived / waiting /
    photographing / returning / recharging / completed
  → 到点作业（NavLogic: patrol point arrived index=）
  → 结束动作 end_task（none / 返回起点 / 返回充电）
  → 记录（record=true，5010/5011 查询，5020/5021 删除）
```

官方 App 明确具备、而我们此前缺失的门禁/操作：

| 能力 | APK 证据 | 说明 |
| --- | --- | --- |
| SDK 节点就绪 | `SdkNodeAvailabilityHelper: sdk_node unavailable, show loading`、`机器狗SDK node尚未连接` | `sdk_node=false` 时不进入可操作状态 |
| 控制权 | `RoamerXControlHelper`、`1205 not ok code=`、`获取控制权失败，请重试` | 1205 失败要重试，不能假定一直持有 |
| 控制源抢占 | `BodyCtrlSourceHelper: non-sdk ctrl_source=`、`当前狗的控制权被其他遥控器抢断` | `ctrl_source=3` 时暂停 APP 遥控 |
| 摇杆门禁 | `JoystickOutboundGate: pause/resume 1304 outbound (ready+RoamerX)` | ready + 控制权才发 1304 |
| 姿态/模式 | `请先站立，再切换至巡航模式`、`当前模式不允许下发导航模式`、`wait_for_nav_mode.dart` | 先站立、等导航模式 |
| 急停 | `急停已触发！...`、`emergencyRecover`、`紧急停止，功能退出` | 软/硬急停分开提示，急停后功能退出 |
| 故障码 | `alg_err_34c0`（13504 `内部节点异常`）、`AlgErrorCodeLevelFromCode`、banner | 本地故障码表 + 严重级别 |
| 断线恢复 | `GatewayDisconnectRecovery`（18s）、`ControlSessionRecovery`、`ControlService: 已重连，重新 start_control` | 重连后重新建立控制会话 |
| 巡检任务 | `PatrolTaskWriteRequest`、`createPatrolTask`、`patrolSpeedVx`、`dwell_sec`、`minimum_battery`、`end_task`、`record` | 一条 patrol 任务覆盖整条路线 |
| 路网 | `cruise_mode_road_network`、`map_route_exist`、`road_network_*` | 有 `map.geojson` 时可走路网 |
| 围栏 | `forbidden_areas`、`_forbiddenAreasToRobotJson`、`add_nav_path(... forbidden_zones)` | 路径/任务携带禁区 |

## 二、本仓库已完成的对齐（2026-11-12）

### 客户端（`business/client`）

- 记录 1207 `modules[]` 的 `sdk_node`，写入 `RobotState.sdk_node_available`；
- 心跳新增 `estopSoftware` / `sdkNodeAvailable`；
- `RobotState::motion_block_reason` 统一门禁：1315 超时、`sdk_node=false`、
  `estop.software=true`、`ctrl_source=3`、`lie_down`；
- 任务下发、`command`、`teleop` 入口按门禁拒绝，恢复类动作（急停/解除急停/
  避障开关）始终放行；
- 1205 失败、`ctrl_source` 变成 0/3 时清空 `control_owned`，下次操作重新发 1204；
- 航段「沿道路」且 4013 `map_route_exist=true` 时，5000 用 `map_type=3`；
- 支持 `routeData.fencePath` / `payload.fencePath` → 5000 `params.fence_path`；
- `charge_point_exist=false` 且结束动作是返充时，提前失败，不再跑完整条路线才收 0x3672。

### 服务端（`business/server`）

- 迁移 `016_device_health.sql`：`devices.estop_software` / `sdk_node_available`；
- 心跳落库这两个字段；`body_online=false` 时设备状态落「故障」，恢复后回「在线待命」；
- 任务下发、`/teleop`、`/action`、`/command`、接管统一健康门禁；
- 4021 故障按 `severity` 映射告警级别：`>=2` 记「严重」，13504 不再被记成普通警告；
- 地图下发 `map_type` 根据 `map_versions.road_count` 选择 1/3；
- 设备没有 WS 连接时保留原「未送达」语义，不把“没连上”误报成“本体不健康”。

### 控制台（`business/web`）

- `Device` 类型新增 `estopSoftware` / `sdkNodeAvailable`；
- 设备抽屉增加健康检查提示，禁止在 SDK 未就绪/软件急停/本体离线时接管或开始巡检；
- 急停/解除急停/避障按钮不受接管态限制；
- 接管按钮在 `sdk_node=false`、急停、本体离线时禁用。

## 三、尚未对齐（需要下一步）

1. ~~**巡检任务模型（5000 kind=4）**~~ —— **已抓到真实报文（2026-11-14 13:36）并已实现**
   （客户端 `--task-mode patrol` 为默认，见 `business/client/README.md`「航线执行流程 A」）

   官方 App（0.8 手机 `192.168.234.8`）下发的完整请求（`wsniff.py` 抓包原文）：

   ```json
   {
     "kind": 4,
     "name": "2026_11_14_13_35_50",
     "schedule": 0,
     "execute_at": "2026-11-14 13:36:29",
     "priority": 0,
     "record": true,
     "params": {
       "map_id": "2026_11_10_16_15_06",
       "map_type": 0,
       "end_task": 1,
       "min_recharge_battery": 25.0,
       "speed_limit": { "vx": 0.5, "vy": 0.0, "vz": 1.5 },
       "path_id": "自定义路径1"
     }
   }
   ```

   → `5001 {"code":0,"msg":"ok","task_id":"68bd7394-0a3d-4f79-a4c1-047fe33fcd02"}`

   **与 APK 里的 AOT 字符串对应关系（重要：线上字段名和内部模型名不一样）**

   | 内部模型（APK 字符串） | 线上字段 | 说明 |
   | --- | --- | --- |
   | `task_name` | `name` | 官方用时间戳命名：`2026_11_14_13_35_50` |
   | `task_type` | `kind = 4` | patrol |
   | `map_id` | `params.map_id` | 本体**地图目录名**（不是业务地图编号） |
   | `path_id` | `params.path_id` | 本体上 `nav_paths/<名字>`；**航点与动作都在路径文件里，请求不带坐标** |
   | `dwell_sec` / `points` | —（不在请求里） | 停留/作业由路径点 `properties.tasks` 决定：`0`=等待（固定 2 s）、`1`=拍照。**平台侧 `dwell` 秒数用重复 `0` 表达（`ceil(dwell/2)` 个），粒度 2 s，单点封顶 300 s**（`business/client/src/zhpro/route.rs` 的 `wait_task_count`） |
   | `patrolSpeedVx` | `params.speed_limit.vx` | 实测 `0.5`（我们一直发 `1.2`） |
   | `minimum_battery` | `params.min_recharge_battery` | 浮点，实测 `25.0` |
   | `end_task` | `params.end_task` | **数字枚举**：`0`=none / `1`=return_to_start / `2`=return_to_charge（与记录里 `properties.end_task` 的字符串一一对应，已实测 0 和 1）。平台侧已提升为**任务属性**：`tasks.end_task` / 载荷 `endTaskMode`，不配则按 `loop_mode == "闭环"` 兜底返回起点（见 `../README.md`「跑完要不要返航」） |
   | `schedule_type` | `schedule` + `execute_at` | `schedule=0`（立即）；**`execute_at` 是本地时间字符串 `"YYYY-MM-DD HH:MM:SS"`，不是毫秒时间戳** |
   | `repeat` / `periodic` | 预约任务的字段 | 立即执行时不下发（记录里 `periodic=false`） |
   | `record` | `record` | **`true`**（写 5010/5011 巡检记录；我们目前发 `false`） |
   | `map_type` | `params.map_type` | **`0` = 2D 栅格 / `1` = 路网**（patrol 枚举，别和 kind=2 的 1/3 混，见下） |

   **`map_type` 三套枚举（别混）**：建图 kind=1 是 `0=室外/2=室内`；导航 kind=2 是
   `1=2D 栅格/2=3D(预留)/3=Route 路网`；**巡检 kind=4 是 `0=2D 栅格/1=路网`**
   （SDK 的 `MapType::kGrid=0 / kRoadNetwork=1`）。

   **服务端调用序列（ALG 日志实测）**：`1204` 取控制权 → `1302 cmd=1` 解除急停 →
   `4012/4013` 取地图列表 → `4022/4023` 按地图取路径 → `5000 kind=4` → `5001` →
   `5009` 心跳（`patrol.*`）→（可选）`5004/5006` 暂停/恢复 → `5012/5013` 返起点 →
   结束帧 `1302 cmd=0`（App 退出时补一帧急停）。

   **`5009` 的 patrol 进度序列（实测，1 个作业点）**：

   ```text
   patrol.preparing(running,0 命中) → patrol.capturing_origin → patrol.navigating
   → patrol.point_arrived  last_event.data = {"index":0,"total":1,"kind":1,"name":"1"}
   → patrol.photographing → patrol.point_completed → patrol.completed
   → patrol.returning（end_task=1 时）→ 终态 success / progress=100
   ```

   `index` 从 **0** 开始（与导航的 `last_arrived_goal.index` 从 0 开始的约定一致），
   `kind` 是该点的 `properties.type`（1=作业点）。

2. **机器人侧路径管理（4022 / 4024 / 4026 / 4028）** —— 已接入 `4024/4028`（客户端写路径）
   官方 App 的 patrol 任务引用 `path_id`，路径由 4024 写入本体，并携带
   `forbidden_zones`。我们目前把路线放在业务库、逐点发 kind=2，没有 `path_id`，
   所以无法真正复用官方的路网/巡检模型。

3. **任务记录（5010/5011/5020/5021）**
   官方 App `record=true` 并查询/删除巡检记录；我们所有 5000 仍是 `record:false`。

4. **最低电量 / 预约 / 重复任务**
   `minimum_battery`、`schedule_type`、`repeat` 还未接入；当前一律立即执行。

5. **围栏编辑器**
   客户端已支持 `fence_path` 透传，但业务路线编辑器还没有围栏数据模型；
   目前只能由接口/手工在 `routeData.fencePath` 里给本体侧围栏文件路径。

6. **导航模式等待**
   官方有 `wait_for_nav_mode.dart`；我们的 1315 只看到 `remote/unknown`，
   还没确认可靠的“已进入导航模式”信号，暂未做硬等待。

## 四、建议的验证顺序

1. 用官方 App 连同一台狗，抓一次「正常巡检」的 WS 报文，重点看：
   `1204/1205`、`1302 cmd=62`、`4012/4013`、`4022/4024`、`5000 kind=4`、
   `5009 patrol.*`、`5002/5004/5006`、`5010/5011`。
2. 先在测试设备上按真实报文发一条 kind=4 巡检（不接生产任务）；
3. 确认 `patrol.point_arrived`、`end_task`、`record`、`minimum_battery` 行为后，
   再把 `TaskMode::Route` 切到 patrol；
4. 最后补围栏编辑器 / 路径同步（4024 + forbidden_zones）。

## 五、航点停留时长（dwell）与「用暂停当停留」的取舍（2026-11-15）

### 5.1 协议事实：停留时长不可配，只有「固定 2 s 的等待」原子动作

| 出处 | 原文 / 结论 |
| --- | --- |
| §2.2.3.7 路径点结构（行 864） | `tasks` = `0` → **等待** → 「在该点**停留一段固定时长（当前实现 2 秒）**」 |
| 同上（行 874） | 「同一动作可重复出现（如 `[0, 1, 0]` = 等待→拍照→等待）」 |
| 同上（行 876） | `tasks` 取值**闭集 `{0, 1}`**，「其它数字一律视为非法并丢弃」 |
| 全文检索 `停留 / dwell / 时长 / 等待` | 只有以上三处；**不存在 `dwell_sec` / `wait_seconds` / 停留秒数之类字段** |
| `5000 kind=4` | 文档连参数表都没有（kind 枚举标注「暂不对外开放」），抓包里也没有任何 `dwell*` 字段 |
| APK 字符串 | 官方 App 内部有 `dwell_sec`（见 §三.1 表），但**不在线上报文里** → App 也是把它翻成 `tasks` |

**结论：秒数只能靠「重复 `0`」累加表达，粒度 2 s。**

### 5.2 我方实现（2026-11-15 落地）

`business/client/src/zhpro/route.rs`：

```rust
pub const WAIT_TASK_SECONDS: i64 = 2;   // 本体每个「等待」动作的时长
pub const MAX_WAIT_TASKS: i64 = 150;    // 单点封顶 300 s（填更大值会 WARN 后截断）
fn wait_task_count(dwell: i64) -> usize // ceil(dwell / 2)，dwell<=0 → 0
```

映射关系（`point_path_tasks`）：

| 平台 `dwell` | 下发 `tasks` | 实际停留 |
| --- | --- | --- |
| 0 | `[]`（只配拍照则 `[1]`） | 0 s，拍完就走 |
| 1~2 | `[0]` | 2 s |
| 3~4 | `[0,0]` | 4 s |
| 5~6 | `[0,0,0]` | 6 s |
| 6 + 拍照 | `[0,0,0,1]` | 6 s 后拍照 |
| > 300 | 150 个 `0` | 300 s（WARN 截断） |

向上取整（填 5 实际停 6 s），单测 `dwell_seconds_expand_to_repeated_waits` 覆盖。
控制台「停留时长」输入框已改为 `step=2` 并提示「2 s 粒度」。

### 5.3 仍待真机验证：连续多个 `0` 会不会被合并

文档只说「可重复」，**没说连续 `0` 是否累加**。必须实测一次（`dwell=6` 的点），三选一：

1. `5009` 的 `patrol.waiting` 与 `patrol.photographing` 两帧时间差应 ≈6 s；
2. 本体巡检记录 `/ota/alg_data/patrol_record/<task_id>.json` 里 `phase` 中 `waiting` 的 `duration`；
3. `5010/5011` 查记录（`phase[].started_at / ended_at / duration`，客户端尚未接）。

- ≈6 s → 方案成立，可全量推广；
- ≈2 s → 本体把连续 `0` 合并了，只能改回单个 `0`，并向厂商要 `tasks` 的时间参数。

### 5.4 不要用 `5004/5006`（暂停/恢复）当停留定时器

结论：**不用**。它把一个「本体自治」的问题退回成「客户端实时抢控制」，收益（精度）与风险不成比例。

| 风险 | 具体原因 |
| --- | --- |
| 停不准 | `5009` 是 500 ms 周期快照 + 变化补帧；`point_arrived` → 客户端收到 → 发 `5004` → 本体减速收敛，量级 0.3~1 s+。等你停住，狗可能已离点 1 m，**停在半路** |
| 语义未定义 | 文档只写「受理」，没写停在哪、恢复是否重算、**恢复后会不会对当前点重发 `patrol.point_arrived`**（会的话定时逻辑自我循环） |
| 污染状态机 | 暂停期间 5009 `lifecycle=paused`（§2.6.5），客户端会把平台任务同步成「暂停」，看板/进度守卫/定时任务全部误判 |
| 通道冲突 | 操作员在页面点「暂停」走的也是 `5004/5006`（`route.rs` 的 `Control::Pause/Resume`），内部定时暂停与人工暂停无法区分，谁恢复谁说不清 |
| 能力占用 | 暂停时导航能力仍被占用；此时取消/断线会留下挂起任务 → `13897 capability busy`，现场恢复手段是重启本体 `dog_task`（见 `deploy/robodog-body.md` §10） |
| 精度收益有限 | 真要「稳定 30 s 测温」还得读 `1401` 等底盘收敛、加超时与断线 watchdog，复杂度远超「重复 `0`」 |

**唯一适合用暂停的场景：人工接管 / 作业时长不可预测**（操作员看画面确认、手动对准、临时遥控）。
若使用，必须同时做到：

1. 触发点选 `patrol.waiting`（不是 `point_arrived`），去重键 `task_id + point_index`；
2. 与人工暂停互斥（内部暂停期间禁用页面按钮，或加 `internal_pause` 标志）；
3. 定时恢复要有 watchdog：客户端重启/断线后主动 `5006` 或 `5002` 收尾，别把本体挂在暂停态；
4. 恢复后观察该点是否重发 `point_arrived`，若重发按「同一点第二次作业」跳过；
5. 把 `lifecycle=paused` 映射成「执行中（内部等待）」，不要显示成平台「暂停」。

### 5.5 到点做平台侧作业（拍照 / AI 识别）的正确姿势

分工：**采集归本体，识别归平台。**

| 事情 | 谁做 | 怎么落 |
| --- | --- | --- |
| 到点停住 | 本体 | `tasks` 里的 `0`（2 s 粒度，可重复累加，见 5.2） |
| 拍照/存图 | 本体 | `tasks` 里的 `1` → ALG 调 `/capture_image` → 落巡检记录 `5011` 的 `phase[].image_path` |
| 拉图、AI 推理、告警入库 | 平台 | 任务结束后从 `5010/5011` 取 `image_path` → 拉图 → 推理 → `alerts` |

- **识别不要卡在巡检执行里**：模型挂了不影响巡检跑完，图有落盘路径可复核/重跑；
- 需要「到点实时出结果」时，客户端只做**只读**参与：收到 `patrol.waiting` → 抓帧上传 → 平台推理，
  窗口不够就把该点 `dwell` 调大（`[0,0]`=4 s、`[0,0,0]`=6 s），**不要发 `5004`**；
- 已知缺口：背包 Agent 只有「常开 detect」（`/api/detect/start|stop|config`）与浏览器 canvas 抓拍，
  **没有「传一张图返回检测结果」的入口**。所以到点 AI 目前用「开识别窗口」实现（见 5.6），
  单图推理接口一旦有了，5.6 的窗口可以直接换成「拍一张 → 推一张」。

### 5.6 航点到点 AI 识别（已落地，2026-11-15；2026-11-16 加客户端代调）

决策逻辑在 `business/server/src/ai_patrol.rs`，**调用方有两种**（`[backpack] point_ai`
/ `ROBODOG_POINT_AI` = `auto`(默认) / `client` / `server`）：

```text
A. server（服务端与板端同网段，配了 ROBODOG_BACKPACK_URL）
客户端 task_progress（stage = patrol.point_arrived / patrol.waiting，带 waypointIndex）
  → 服务端按 taskCode 找到任务 → 路线 route_data.points[index].actions[]
  → 命中「AI 识别」动作（动作名含 `AI` 或 `识别`）
  → POST {agent}/api/detect/start（已在跑则 /config 热更新）
      body 带 plan_id(=taskCode) / point_index / expect_count
  → 读 {agent}/api/events 的 SSE，只取 type=detect 的 boxes
  → 框的 conf ≥ 阈值 → 写 alerts + task_events（同类目标窗口内只报一次）
  → type=plan_hit 且 count_alert=true → 写「数量核对不一致」告警 + 任务事件 + 企微
  → 窗口到期 / 任务终态 / 被下一个点顶替 → POST /api/detect/stop
     （只是「保留操作员已开的识别」时改发 /api/detect/plan/clear，把计划摘掉）

B. client（正式环境：平台在公网、Agent 只在内网，服务端连不到板端）
客户端到点 → GET 127.0.0.1:8000/api/detect/{tasks,status}
  → 上报 ai_point（动作参数 + 板端模型目录）→ 服务端用**同一套**规则算场景/类别/阈值/窗口
  → 下发 ai_start（含 planId / pointIndex / expectCount）
  → 客户端 POST /api/detect/start|config → 读本机 SSE
  → 按阈值 + 类别去重回传 ai_detect（带命中帧：等 plan_hit.evidence，读不到再扫
    detect_jobs/<plan>/pt<n>/ 取本轮最新一张）
    → 服务端写 alerts + task_events + 企微（卡片后面跟一张证据图）
  → 数量任务（checkType=count）与 expectCount 不符（SSE plan_hit.count_alert）
    → 转发 `ai_alert`；到期一个都没找到也由服务端按 ai_end.expired 判成数量不一致
  → 图晚于卡片（命中帧稀疏）→ 停识别扫盘后补 ai_evidence（平台只补一张图）
  → 客户端收尾 ai_end（带 expired）；任务终态/取消时服务端下发 ai_stop
```

> 为什么要有 B：正式环境的业务平台在 `39.107.55.13`（公网），背包 Agent 在
> `192.168.41.246:8000`（现场内网），`curl` 实测超时。2026-11-16 线上计划
> `TK-1112-11` 首点配了 AI 识别，日志里 5 条
> `AI 识别：本部署的服务端访问不到背包 Agent，跳过航点…`，告警表 0 条。
> 设备客户端本来就跑在同一块板子上（`.246`，见 `deploy/README.md`），由它代调即可。
> `auto` 会按「有没有配 `ROBODOG_BACKPACK_URL`」自动选，配错也不会两条链路各写一条告警。

**「哪个航点」只认 `waypointIndex`，不要用 `pointIndex`**（2026-11-17 现场事故）：

| 字段 | 语义 | 谁填 |
| --- | --- | --- |
| `waypointIndex` | **0 基航点下标**（= 5009 的 `detail_index`），查 `route_data.points[]` 用它 | 到点载荷与 5s 摘要都填 |
| `pointIndex` | **已完成航点数**（1 基计数），前端「进度 n/N」用它；5s 摘要里它等于「当前点 + 1」 | 两处语义不同，别混用 |

2026-11-17 18:26 `TK-1112-11`（路线 `RT-005`：0 号点 AI 识别灭火器、1 号点可见光拍照）
用 0 号点开窗后，客户端 5s 摘要送的是 `pointIndex=1`（已完成 1 个），服务端拿它查到
**1 号点**（无 AI 动作）→ 立刻「到达无 AI 动作的航点」关窗 + 下发 `ai_stop`；客户端收到
`ai_stop` 清掉去重标记，下一个 5009 帧又重新 `ai_point`……于是**每 5s 开关一次窗口**：
日志里 3 次「开始」（18:26:38 / 18:26:41 / 18:26:46，配置窗口 18s 实际只活 3~5s）、
板端 `osd.session=3`、零 `ai_detect`、零告警。修复后到点判定只读 `waypointIndex`
（老客户端不发这个字段时才退回 `pointIndex`，即保留差一位的老行为），回归测试见
`business/server/tests/ai_patrol.rs::interval_summary_does_not_kill_the_window`。

**动作参数怎么填**（路线编辑器「该点执行动作」→ `AI 识别`）：

| 字段 | 用途 | 例 |
| --- | --- | --- |
| `param` | **可读的场景/类别描述**，展示与排查用；历史数据（无 `scene`/`labels`）仍按 `/ 、 , 空格` 切词匹配 Agent 的模型名/中文名/类别名，一个都不命中时退回第一个可用模型并打 WARN | `烟火检测 · 火焰 / 烟雾` |
| `scene` | **模型包 id**，优先于 `param` 关键词：精确匹配 Agent `task`（模型包下线时才退回关键词匹配）。由路线编辑器从「实时视频 → AI 识别」的模型目录里选 | `fire` |
| `labels` | **识别类别**（Agent 的 label `name`，与实时视频下发的 `labels` 同一套）；为空 = 该场景全部类别；板上不存在的类别会被忽略并打 WARN | `["flame", "smoke"]` |
| `threshold` | 置信度阈值：`0.85`、`> 0.85`、`85%` 都认；`> 65℃` 这类温度阈值不会被误当成置信度 | `置信度 ≥ 0.40` |
| `checkType` | **任务类型**：`exist` 检测是否存在 / `count` 检测数量是否正确。老数据没有这个字段时后端按「有没有 `expectCount`」推断（给了数量就是数量任务） | `count` |
| `expectCount` | **检查目标数量**：仅 `count` 任务必填；由板端把 AI 检出的目标数与它比对，不一致上报数量告警（`exist` 任务即使误传也不判数量） | `3` |
| `alertWhenMissing` | **未检出也告警**：仅 `exist` 任务；窗口正常到期却一个目标都没找到 → 「AI 识别未检出」告警（含自动工单）。适合「必须有」的东西（灭火器、气体钢瓶），不适合消防通道占用这类「不该出现」的目标 | `true` |

**两类任务怎么落地**：

| 类型 | 命中 | 窗口结束仍未命中 |
| --- | --- | --- |
| `exist` 检测是否存在 | 存证 + `ai_detect` 上报（不判数量） | 默认：只算「没找到」；勾了 `alertWhenMissing` → 「AI 识别未检出」告警（同样只认窗口**正常到期**） |
| `count` 检测数量是否正确 | 存证 + `ai_detect`；`found_count ≠ expectCount` → 数量告警 | 0 ≠ N 也算数量不一致 → 数量告警（仅窗口**正常到期**时，被打断不记账） |

> 选哪个：**「不该出现的东西」（消防通道占用 / 明火 / 闯入人员）选 `exist`**——检出即告警；
> **「必须有 N 个」（灭火器、钢瓶）选 `count` + `expectCount`**；
> **「必须有、但个数不重要」选 `exist` + `alertWhenMissing`**。
> 只配 `exist` 跑在灭火器上 = 灭火器好好在那儿也每轮告警并自动建工单（语义反了）。

**计划 id 与存证**：开窗同时把 `planId`（= 业务任务号 `taskCode`）与 `pointIndex`
下发给板端，命中帧与 `result.json` 落到板端
`detect_jobs/<planId>/pt<pointIndex>/`（图已画中文框），方便按计划回溯。2026-11-17 起板端还会额外写一份 **`result_<YYYYmmdd_HHMMSS>.json`**：
`result.json` 只保留最新一次，同一航点每轮都覆盖，历史结论靠这些副本翻
（板端改动在 `/root/261113/backpack_dashboard/backend/detect.py::_finish_job`，
不在本仓库；工单详情的「关联告警」里也能看到历次告警正文与证据图路径）。
数量判定在板端做（它知道本帧到底画了几个框），结论文案形如
`AI 识别数量核对不一致：任务「…」航点 0「…」检查 3 个，实际检出 2 个（证据：…/dogcam_x.jpg）`，
客户端转发 `ai_alert`、服务端直连模式读 SSE 的 `plan_hit`/`plan_miss`，两条路最终都进
同一个 `record_count_alert`（告警中心 + 任务时间线 + 企微），每个窗口只落一条。
数量核对取「命中后连续几帧的检出最大值」，单帧遮挡少画一个框不会误报。
「未命中 = 数量不一致」只认客户端的 `ai_end.expired=true`（窗口正常到期）；
任务结束 / 被新航点顶替时 `expired=false`，服务端不记这笔账。

**检出告警的证据图**（2026-11-18 加，同日 18:54 修复「只有卡片没图」）：企业微信那条卡片后面跟一张命中帧。
**图不是检出那一刻就有的**：板端 `_plan_frame` 要连采 `DETECT_PLAN_COUNT_SAMPLE`（默认 3）帧命中、
画框存证，才发带 `evidence` 的 `plan_hit`（现场实测比第一帧检出晚 **4.6s**）。所以客户端把命中
**先挂起**，等 `plan_hit` 送图来一起上报（`ai_detect` 晚几秒，但卡片后面必有图）；旧实现在命中后
只扫盘等 2.4s，晚了就只有卡片。
图优先读 `plan_hit.evidence` 给的路径，读不到再扫 `<jobs_dir>/<planId>/pt<航点下标>/` 取**窗口开始之后**
写下的最新一张（mtime 卡 5s 容错）；一直等不到（老板端 / 画框失败）就在 8s 兜底时先发卡片。
窗口结束时还在等图的，由 `finish()` **先停识别再扫盘**（板端 `_plan_flush` 正是停的时候才把采样中的
命中落盘）后补发。
命中帧稀疏时（板上整个窗口只命中 1 帧，板端「连采 3 帧」的采样跑不完）`plan_hit` 要到**窗口收尾**
才发，图往往比卡片晚几十秒：8s 兜底先发的卡片照旧，停识别扫到图后客户端单独报一条 `ai_evidence`，
服务端 `notify::alert_evidence` 只补一张 `msgtype=image`（不重发卡片、不重复落告警）。
服务端 `record_hit` 校验后发 `msgtype=image`，路径同时写进正文「（证据：…）」。
`jobs_dir` 优先级：`[ai] jobs_dir` / `ROBODOG_AI_JOBS_DIR`（客户端配置）> 从板端
`plan_hit.evidence` 反推出的真实根 > 内置候选（`/root/backpack_dashboard/backend/detect_jobs`、
`/root/261113/backpack_dashboard/backend/detect_jobs`）。板子路径与这两个都不同
（或客户端不在板端）时不带图，告警照发——与 `count` 那条的退让一致。
设计与回归见 `business/docs/wecom-alert-push.md`「证据图」一节。

**怎么配**：路线编辑器「该点执行动作」→ 动作类型选 `AI 识别`，场景与识别类别直接勾选
「实时视频 → AI 识别」面板上的同一份模型目录（数据来自 `GET /api/backpack/status` 的
`detect.tasks`），服务端不需要再猜。目录读不到（规划时不在园区内网）时可手填关键词，
后端仍有关键词兜底。

**窗口时长**：`max(dwell, 4) + 8` 秒，下限 10 s、上限 `[backpack] ai_window_max_secs`
（默认 300 s，与路线停留上限一致）。**停多久就识别多久**——`dwell=100` 的点会识别 108 s，
不再被截断成 30 s（2026-11-16 改：以前 30 s 上限让长停留只剩前 30 s 在识别）。

防误报不靠截断时长，靠**阶段收口**：`stage` 一旦不再是 `patrol.point_arrived` /
`patrol.waiting` / `patrol.photographing` / `patrol.point_completed`（狗开始走向下一个点、
返航或进入别的任务阶段），窗口立刻关闭（客户端代调模式由客户端停 Agent，服务端模式由
服务端停；两边同一判据 `is_point_stage`）。窗口秒数在
`GET /api/backpack/ai-window` 的 `windowSecs` 里能直接看到。

**与操作员手动识别的关系**：Agent 是单路单任务。

- 操作员已在「实时视频」页开着识别 → 巡检窗口只做 `/api/detect/config` 热更新，
  **窗口结束不会把操作员的识别停掉**；
- 巡检窗口开着时操作员再点「开始识别」会被 Agent 拒绝（「识别已在运行」），属预期。

**排查入口**：`GET /api/backpack/ai-window` 返回当前窗口（是否在跑、`source` = `server`/`client`、
任务/航点、解析到的场景 `scene`、实际下发的类别 `labels`、阈值、相机、窗口内已上报类别）或
`{"active": false}`。现场「配了 AI 识别但没告警」按这个顺序看：

0. `source` 与部署对不对得上 → `point_ai=auto` 但 `ROBODOG_BACKPACK_URL` 没配时是按 `client`
   跑的，此时服务端**不该**出现「访问不到背包 Agent」的 WARN；客户端日志里要能看到
   `航点 AI 识别：请求开窗 …` 与 `航点 AI 识别开始：…`；

1. 窗口有没有开 → 没开说明 `stage`/`waypointIndex` 没上报，或动作类型不含 `AI`/`识别`；
   窗口「开了又秒关」看关闭原因里的航点是不是下一个点（那就是 `pointIndex` 混用，见上表）；
2. 场景对不对 → `scene` 是否解析到了期望的模型（不对就回路线编辑器重选场景，或改动作 `param`）；
3. 类别对不对 → `labels` 是否只含勾选的那些（勾错了或板上已无该类别时会是空 = 全部类别）；
4. 阈值 → `threshold` 是否过高（0.95 很难触发）；
5. `hits` → 有 hits 说明识别到了，只是告警被去重（同类只报一次）或写在别的任务上。

**部署前提（很容易踩）**：这条链路是**服务端**去调 Agent，所以业务服务必须能 HTTP 到板端：

- ✅ 服务端在园区内网（或与板端同网段）：配 `ROBODOG_BACKPACK_URL=http://<板端>:8000`；
- ❌ 只配了 `ROBODOG_BACKPACK_BROWSER_URL`（浏览器直连模式）：浏览器能看视频/AI，
  但**服务端连不到 Agent，航点 AI 永远起不来**——日志里会出现
  「本部署的服务端访问不到背包 Agent，跳过航点…」，`/api/backpack/ai-window` 始终 `active:false`；
- 外网部署（`39.107.55.13`）要跑航点 AI，只能把业务服务下沉到现场，或给 Agent 一条
  服务端可达的地址（反向控制信道目前只覆盖中台，未覆盖背包）。

**测试**：`business/server/tests/ai_patrol.rs`（假 Agent 跑完整链路：开窗口 → 阈值过滤 →
去重 → 告警/事件落库 → 终态停识别；以及「无 AI 动作不碰 Agent」「未配 Agent 不留脏窗口」
「5s 摘要不能关窗」）、`business/server/tests/ai_client_mode.rs`（客户端代调：`ai_point` →
`ai_start` → `ai_detect`/`ai_alert`/`ai_end`，含 `exist + alertWhenMissing` 的未检出告警、
被打断的窗口不记账、同一问题跨轮巡检只挂一张工单）。
