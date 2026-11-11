# RoboDog 设备客户端

连接 RoboDog OPS 业务服务的设备客户端，同时作为**智航 Pro 机器狗本体**的驱动层。

一条任务的完整链路：

```
OPS 业务服务 ──task_dispatch──► 本客户端 ──智航 Pro WS──► 机器狗本体
（园区/任务/路线）                （协议翻译 + 执行引擎）    （建图/导航/回充）
       ▲                                │
       └──────task_progress 上报────────┘
```

建图任务还多一段**地图回传链路**（A 方案，见下文「建图回传中台」）：

```
机器狗本体 ──4001 推送地图路径──► 本客户端
                                      │ ① HTTP GET 拉 .zip 包
                                      ▼
                                本体文件服务（额外开放）
                                      │ ② POST /api/map-versions/ingest
                                      ▼
                              OPS 业务服务 ──► 中台 dog_map（同时写 map_versions）
```

`--robot` 为空时退化为纯模拟模式（只回心跳，不驱动机器人），便于前端联调。

本体建链细节（`control` / `bulkdata` 双通道、`1201/1202/1203` 时序、心跳与重连）
见 [`../docs/client-connection-guide.md`](../docs/client-connection-guide.md)。

## 架构

```
┌──────────────────┐   ws://server:3100/ws/device   ┌────────────────────┐
│  本客户端         │ ◄════════════════════════════► │  OPS Server         │
│                  │                                │  gateway.rs         │
│  main.rs         │                                └────────────────────┘
│   ├ dispatch.rs  │ 业务载荷 → FlightPlan
│   └ zhpro/       │
│      ├ client    │ ──── ws://robot:10010/control ────┐
│      ├ protocol  │ ──── ws://robot:10010/bulkdata ───┤  智航 Pro 网关
│      ├ status    │                                     │  (机器狗本体)
│      ├ mapping   │  建图 / 地图管理                     │
│      └ route     │  航线执行引擎                        ┘
│   └ mapupload.rs │ 建图完成后拉包 + 回传中台
└──────────────────┘
```

## 快速开始

```bash
cd business/client

# 1) 纯模拟模式（不接本体，只跑业务协议）
cargo run -- --device-id RD-001

# 2) 接真实本体：驱动智航 Pro
cargo run -- --device-id RD-001 --robot 192.168.10.21

# 3) 指定地图根目录与建图类型
cargo run -- --device-id RD-001 --robot ws://192.168.10.21:10010 \
  --map-root /ota/alg_data/map --map-kind indoor

# 4) 只做一次建图，结束即退出（运维手工建档用）
cargo run -- --robot 192.168.10.21 --map-only --map-name "B区-20261110"

# 5) 建图完成后自动回传中台（A 方案）
#    园区归属优先用控制台绑定的园区（注册回执下发），也可用 --park-id 覆盖
cargo run -- --device-id RD-001 --robot 192.168.10.21 \
  --robot-file-base http://192.168.10.21:12000 \
  --api-username admin --api-password admin123

# 查看全部参数
cargo run -- --help

# 6) 现场部署：用配置文件固定参数（推荐）
cp client.toml.example client.toml   # client.toml 已被 .gitignore 忽略
cargo run -- --config client.toml
cargo run -- --print-config          # 打印脱敏后的生效配置
```

## 配置文件

优先级：**命令行 > 环境变量 > `client.toml` > 内置默认值**。

- 默认读取工作目录下的 `client.toml`；可用 `--config <path>` 或 `ROBODOG_CLIENT_CONFIG` 指定。
- 默认路径不存在时静默使用默认值；**显式指定**的路径不存在则启动失败。
- 密钥推荐走环境变量：`server.secret_env` / `api.password_env` 填写变量名，不要写明文。
- 未知字段会直接报错（防止 `secrect` 这类拼写错误被忽略），`--print-config` 可核对生效值与来源。

完整字段、默认值与 systemd 示例见
[`../docs/client-connection-guide.md`](../docs/client-connection-guide.md) §11。

## 命令行参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--server` | `ws://127.0.0.1:3100` | 业务服务地址 |
| `--device-id` | `RD-001` | 设备编号（须与 `devices.device_id` 一致） |
| `--secret` | 无 | 设备密钥（推荐 `ROBODOG_DEVICE_SECRET`） |
| `--heartbeat` | `10` | 心跳间隔（秒） |
| `--battery` | `86` | 初始电量 |
| `--signal` | `4` | 信号强度 |
| `--position` | `0.0, 0.0` | 初始地图坐标 |
| `--firmware` | `Hermes_M_v0.3.2` | 固件版本 |
| `--robot` | 空 | **智航 Pro 本体地址**。空 = 纯模拟模式 |
| `--map-root` | `/ota/alg_data/map` | 本体地图根目录 |
| `--map-only` | false | 只执行一次建图后退出 |
| `--map-name` | 时间戳 | `--map-only` 时的地图名 |
| `--map-kind` | `outdoor` | 建图类型：`outdoor` / `indoor` |

### 建图回传中台（A 方案）

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--robot-file-base` | 由 `--robot` 推导 | 本体文件服务基地址，如 `http://192.168.10.21:12000` |
| `--robot-file-port` | `12000` | 推导基地址时使用的端口（文档 §4 文件服务端口；`--robot-file-base` 显式给出时忽略） |
| `--no-upload-map` | false | 关闭建图后自动回传，只在本体落盘 |
| `--park-id` | 控制台绑定园区 | 地图归属园区：不填则用注册回执里下发的绑定园区；两者都没有时跳过回传。手动指定时优先生效 |
| `--ingest-map-name` | 用 4001 返回的目录名 | 入库到中台的地图名 |
| `--api-base` | 由 `--server` 推导 | OPS 业务服务 HTTP 地址，如 `http://127.0.0.1:3100` |
| `--api-username` | `admin` | 业务服务登录用户名 |
| `--api-password` | `admin123` | 业务服务登录密码 |

`--robot` 支持三种写法，等价：裸 IP、`ws://ip:10010`、`ws://ip:10010/control`。

园区归属优先级：`--park-id` > 服务端注册回执 / `park_binding` 推送（控制台设备管理里绑定）
> 两者都没有则跳过回传。控制台绑定 / 解绑会实时推送给在线设备，不需要重启客户端。

## 支持的任务类型

`task_dispatch` 的 `taskType` 决定分派路径（见 `dispatch.rs::TaskMode`）：

| taskType | 智航动作 | 说明 |
|----------|----------|------|
| `建图` / `地图构建` | 5000 `kind=1` | 建图任务，结束触发 4001 保存 |
| `巡检` / 其它 | 航线执行引擎 | 逐航点下发 5000 `kind=2`，客户端侧执行点位动作 |
| `返航` / `返回充电` | 5014 | 返回已标定的回充点 |
| `回充` / `充电` | 5000 `kind=3` | 对桩充电 |
| `出桩` | 5000 `kind=5` | 从充电桩驶出 |

> 智航 v1.0.3 的 `kind=4`（patrol 原生巡检）**尚未开放**，因此巡检拆成
> 「导航任务逐点 curl + 客户端侧作业」。这样不受 `tasks` 动作闭集
> （只有 `0=等待`、`1=拍照`）的限制，后续接拍照/云台/红外都可以在客户端扩展。

## 业务侧下发载荷

`task_dispatch.payload` 的关键字段：

```json
{
  "taskId": 12,
  "taskCode": "TK-1110-01",
  "taskName": "A区上午例行巡检",
  "taskType": "巡检",
  "executeMode": "返航充电",
  "route": { "routeId": 3, "routeCode": "RT-003", "routeName": "A区产线" },
  "routeData": {
    "schemaVersion": 2,
    "loop": "闭环",
    "points": [
      { "name": "起点", "x": 0.0, "y": 0.0, "dwell": 0, "heading": 0,
        "radius": 0.3, "must": true, "onFail": "跳过继续", "actions": [] },
      { "name": "配电柜", "x": 5.0, "y": 2.0, "dwell": 15, "heading": 90,
        "radius": 0.3, "must": true, "onFail": "重试 2 次",
        "actions": [{ "type": "可见光拍照", "param": "1080P", "retry": 1 }] }
    ],
    "legs": [{ "mode": "沿道路", "speed": 1.5, "avoid": "标准" }],
    "returnLeg": null
  },
  "map": {
    "mapId": "ROS-MAP-3@v1.0",
    "mapPath": "ROS-MAP-3_v1.0",
    "mapType": 1,
    "parkId": 1,
    "mapName": "ROS-MAP-3",
    "version": "v1.0"
  },
  "speedLimit": 1.5,
  "avoidLevel": 40,
  "endTask": "return_to_charge"
}
```

- `map` 由服务端组装：优先用路线冻结的 `routes.map_version_id`，回退到园区当前基线
  `parks.map_version_id`。都没有则不下发，客户端会拒绝执行并上报异常。
- `mapPath` 是本体侧目录名（`@` → `_`）。客户端会再按 `--map-root` 拼成
  `/ota/alg_data/map/history_map/<mapPath>`。
- `endTask`：`return_to_charge` / `return_to_start` / `none`。
  缺省时客户端退化为按 `executeMode` 文案推断。
- `avoidLevel`：10 关闭 / 20 仅急停 / 30 感知 / 40 全避障。任务级生效，
  进入 STANDBY 后本体自动恢复 40。

## 上报载荷

`task_progress.payload`：

```json
{
  "taskCode": "TK-1110-01",
  "status": "执行中",
  "progress": 60,
  "pointIndex": 3,
  "totalPoints": 5,
  "message": "已完成 3/5 个航点",
  "stage": "navigation.path_running",
  "robotTaskId": "task-8f3a1c"
}
```

- `stage` 直接透传智航 5009 快照里的阶段串，服务端落到 `tasks.robot_stage`。
- `robotTaskId` 是 5001 返回的本体任务 ID，落 `tasks.robot_task_id`，用于对账机器人日志。
- 上报节奏：5009 推送是 500ms，客户端合流成 **5s 一次**，避免刷爆业务库。

`task_control` 支持 `pause` / `resume` / `cancel`，客户端收到后：
- 航线任务 → `RouteRunner::control`，暂停时同步给本体 5004，恢复 5006，取消 5002；
- 建图任务 → `cancel` 会触发 5002 并等待 4001 保存结果。

## 目标字段使用说明

`goals[]` 逐点下发（`MapManager` 前置校验通过后）：

| 业务字段 | 智航字段 | 说明 |
|----------|----------|------|
| `x` / `y` | `goals[].pose.position` | 地图坐标系（米） |
| `heading` | `goals[].pose.orientation` | 度 → 四元数 |
| `radius` | `goal_tolerance.x/y` | 到达判定半径，默认 0.25 |
| `dwell` | 客户端 sleep | 停留秒数 |
| `must` + `onFail` | 客户端重试策略 | `must=true` 不可跳过 |
| `actions[].type` | 客户端作业 | `可见光拍照` → `PointAction::Photo` |

## 智航 Pro 协议对接要点

```
1. control 连上 → 收 1201（client_id / session_id / bulkdata_bind_token）
2. 带 token 建 bulkdata → 首帧 1202 → 收 1203 才算可用
3. 1207 模块可用性：sdk_node 3s 内无 1525 心跳即判不可用
4. 1299 网关传输层错误、1300/1301 网关心跳
5. 4000~5999 自由数据区间需先 1204 取得 RoamerX 控制权
```

实现文件：

| 文件 | 职责 |
|------|------|
| `zhpro/protocol.rs` | 全部消息类型常量、枚举、`Envelope` 信封 |
| `zhpro/client.rs` | 双通道建链、单写循环、请求-响应关联、网关心跳 |
| `zhpro/status.rs` | 5009 V2 快照解析、`TaskMonitor` 去重与终态判定 |
| `zhpro/mapping.rs` | 建图会话（5000/4035/4001）与地图管理（4012~4045） |
| `zhpro/route.rs` | 航线执行引擎 `RouteRunner`、`FlightPlan`、`Waypoint` |
| `dispatch.rs` | 业务载荷 `routeData` → `FlightPlan` |

## 建图流程

```
--map-only 或 taskType=建图
  → 5000 kind=1, params.map_type=0(室外)/2(室内)
  → 4035 实时位姿与剩余里程（日志）
  → 结束原因：Ctrl+C / task_control cancel
  → 5002 取消建图
  → 等 4001 保存结果（含 map_path）
  → 上报 task_progress 90，stage=mapping.saved
  → 地图回传中台（见下节）
  → 上报 task_progress 终态（已完成/异常）
```

### 建图回传中台（A 方案）

智航 Pro 协议 40xx 全是操作类消息（列表/改名/删除/设回充点/载图定位），
**没有下载地图文件的接口**，所以地图包不能通过 WS 取回。A 方案的做法是：
本体额外开放一个 HTTP 文件服务，客户端在建图完成后自行拉包并上传。

```
4001 成功 ──► 取得地图目录名（见下方「注意」）
           ──► GET {base}/.../history_map/{dir}.zip   ← 依次探测 6 种 URL，校验 PK 魔数
           ──► POST {api_base}/api/map-versions/ingest  （multipart: file + mapInfo）
           ──► 业务服务转推中台 dog_map，同时写 map_versions
```

文档 §4「文件服务」明确：**HTTP 端口 12000**，L2 文件根 `/userdata/alg_data`、
M1 文件根 `/ota/alg_data`；文档 §2.2.4.1「4013 地图列表」明确单张地图路径为
`<map_dir_prefix>/history_map/<目录名>`，其中 `map_dir_prefix = /ota/alg_data/map`。
据此探测的 6 种 URL 依次为（命中即停，全部要求返回 `PK` 开头的 zip）：

```
{base}/ota/alg_data/map/history_map/{dir}.zip       ← 文档 M1 绝对路径
{base}/userdata/alg_data/map/history_map/{dir}.zip  ← 文档 L2 绝对路径
{base}/history_map/{dir}.zip                        ← base 已含 map 根目录
{base}/{dir}.zip
{base}/history_map/{dir}?format=zip
{base}/{dir}?format=zip
```

`{base}` 由 `--robot-file-base` 显式给出，否则从 `--robot` 推导成
`http://<host>:12000`。探测失败时错误信息会带上最后一次尝试的 URL 与失败原因
（HTTP 状态码 / 非 zip 内容），便于对着本体路由排查。

> **注意：地图路径来源尚未与文档对齐**
> 文档 §2.2.1.1 的 4001 **只返回 `data.code` 和 `data.msg`，不含地图路径**。
> 当前 `MappingSaved` 里的 `map_path` / `map_name` 提取属于兼容性兜底：若本体固件
> 额外回了路径就能用，按标准 v1.0.3 则取不到，拉包会失败并把任务记「异常」。
> 权威来源是 **4013 的 `map_path_list[].path`**（绝对路径）。真机联调前需在
> 4001 之后主动发一次 4012，按目录名/最新时间从列表里取到新地图路径再拉包。
> `zhpro/mapping.rs::RobotMap` 已经解析好 `path`，可直接复用。

回传结果直接决定任务终态：

| 结果 | 任务状态 | progress | 说明 |
|------|----------|----------|------|
| 上传成功 | 已完成 | 100 | 消息里带中台 `mapId` |
| 跳过（`--no-upload-map` / 设备未绑定园区且未给 `--park-id`） | 已完成 | 100 | 消息注明跳过原因 |
| 拉包或上传失败 | **异常** | 90 | 地图已在本体落盘，但未入库中台 |

之所以失败记「异常」而不是「已完成」：地块已建但没入库，后续无法绑定路线，
按成功上报会掩盖问题。包在业务服务 `data/map_ingest/` 留档，可凭日志路径重放。

建好的地图落盘在本体 `--map-root` 下。若走 A 方案自动回传，中台 `dog_map` 与业务库
`map_versions` 都会写入，无需再手工导入；否则业务侧需要用
`POST /api/map-versions/import-backend` 或在前端「地图基线」页做导入后才能绑定路线。

## 航线执行流程

```
1. 解析 routeData → FlightPlan
2. 前置校验：4013 地图列表里必须存在该 map_dir
3. 若 endTask=return_to_charge，校验 map_route_exist / charge_point_exist
4. 4044 设置任务级避障等级
5. 逐航点 5000 kind=2：
     need_locate 只在第 1 个点 true（避免每点重载地图）
     等 5009 生命周期终态
     失败按 on_fail 重试 / 跳过（must=true 不跳过）
6. 结束后按 endTask：
     return_to_charge → 5014
     return_to_start  → 再下发一次首点导航
     none             → 原地待命
```

## 业务协议速查

统一信封：`{ msgType, ts, payload }`

### 设备 → 服务端

| msgType | 说明 |
|---------|------|
| `register` | 设备注册 |
| `heartbeat` | 心跳（10s） |
| `task_progress` | 任务进度（5s） |
| `event` | 设备事件 / 告警 |
| `pong` | 心跳响应 |

### 服务端 → 设备

| msgType | 说明 |
|---------|------|
| `register_ack` | 注册确认 |
| `heartbeat_ack` | 心跳确认 |
| `task_dispatch` | 任务下发 |
| `task_control` | 暂停 / 恢复 / 取消 |
| `command` | 远程指令 |
| `error` | 错误信息 |

### 远程指令

`command.payload.command` 取值：

| 指令 | 客户端动作 |
|------|-----------|
| `开始巡检` | 1204 取得 RoamerX 控制权 |
| `暂停任务` | 5004 暂停当前本体任务 |
| `召回充电` | 5014 返回回充点 |

## 与 REST API 的联动

| REST API | WebSocket 行为 |
|----------|---------------|
| `POST /api/tasks`（带 `deviceId`） | → 推送 `task_dispatch` |
| `POST /api/devices/{id}/command` | → 推送 `command` |
| `PUT /api/tasks/{id}/status`（`已暂停`/`执行中`/`已完成`） | → 推送 `task_control` |
| 设备注册 | ← 补发所有 `待执行` 任务 |

## 校验

```bash
cd business/client
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
cargo test
```
