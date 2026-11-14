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
   | `dwell_sec` / `points` | —（不在请求里） | 停留/作业由路径点 `properties.tasks` 决定：`0`=等待（固定 2 s）、`1`=拍照 |
   | `patrolSpeedVx` | `params.speed_limit.vx` | 实测 `0.5`（我们一直发 `1.2`） |
   | `minimum_battery` | `params.min_recharge_battery` | 浮点，实测 `25.0` |
   | `end_task` | `params.end_task` | **数字枚举**：`0`=none / `1`=return_to_start / `2`=return_to_charge（与记录里 `properties.end_task` 的字符串一一对应，已实测 0 和 1） |
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

2. **机器人侧路径管理（4022 / 4024 / 4026 / 4028）**
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
