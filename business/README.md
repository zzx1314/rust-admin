# RoboDog OPS · 北京华致技术

`business/` 是独立于仓库根目录 `backend/`、`frontend/`、`client/` 的业务服务，
按 `designprototype/robodog-management-system.html` 原型实现，拥有自己的
Rust 后端、SQLite 数据库与 Vue 3 运营控制台。

设备台账同时同步到中台（根 `backend`）的 `dog_device`：中台管理员在「设备管理」
页面给设备分配用户，业务平台用同一个中台账号登录后只看到自己管辖的设备；
机器狗建图回传时，地图归属到采集设备当前被分配的中台用户名下，园区建图时
只列出该用户名下的地图。

## 目录结构

```text
business/
├── designprototype/                # 原始 HTML 交互原型（只读参考）
├── server/                        # 独立 Rust 服务（axum + sqlx + SQLite）
│   ├── src/
│   │   ├── auth.rs                # JWT 登录、鉴权中间件
│   │   ├── db.rs                  # 连接池、SQL 迁移
│   │   ├── seed.rs                # 默认管理员与演示数据
│   │   ├── geo.rs                 # 路线里程/耗时/耗电计算、顺序优化
│   │   ├── gateway.rs             # 设备 WebSocket 网关（注册/心跳/下发/控制）
│   │   ├── gateway_types.rs       # 网关消息协议类型
│   │   ├── dispatch.rs            # 任务下发载荷组装（地图绑定/速度/结束动作）
│   │   ├── handlers/              # 园区、区域、设备、任务、路线、告警、设置、驾驶舱
│   │   └── app.rs                 # 路由装配、CORS、静态资源
│   ├── migrations/                # 001 建表 / 002 地图基线 / 003 地图来源 / 004 本体回执
│   ├── seed/routes.json           # 演示路线（航点/航段/动作）
│   └── data/robodog.db            # 运行时生成，删除后自动重建并重新播种
├── client/                        # 设备客户端：业务协议 + 智航 Pro 本体驱动
│   ├── src/main.rs                # 双后端连接、任务分派、状态上报
│   ├── src/dispatch.rs            # 业务载荷 → FlightPlan
│   └── src/zhpro/                 # 智航 Pro 协议层、建图、地图管理、航线执行
└── web/                           # 独立 Vue 3 控制台
    ├── src/api/                   # 请求封装与类型
    ├── src/components/            # 图表、状态徽标、设备抽屉、园区态势图等
    ├── src/layout/                # 原型风格的侧边栏 + 顶栏
    ├── src/views/                 # 7 个业务页面
    └── src/styles/                # 原型设计令牌与页面样式
```

## 快速启动

### 1. 启动后端（端口 3100）

```bash
cd business/server
cargo run
```

首次启动会：

1. 执行 `migrations/001_schema.sql` 建表；
2. 创建默认管理员 `admin / admin123`；
3. 写入 3 个园区、8 个巡检区域、12 台设备、4 条路线、10 个任务、5 条告警等演示数据。

需要在中台设备页触发「同步地图」时，两个服务端要配置一致的共享令牌：

```bash
# business/server
export ROBODOG_SERVICE_TOKEN=dev-service-token

# backend（根目录）
export APP_DOG__BUSINESS_API_TOKEN=dev-service-token
```

### 2. 启动前端开发服务器（端口 5273）

```bash
cd business/web
pnpm install
pnpm dev
```

浏览器打开 <http://127.0.0.1:5273>，使用 `admin / admin123` 登录。
Vite 已将 `/api` 代理到 `http://127.0.0.1:3100`。

> 代理里配了 `ws: true`：实时监控走 WebSocket（`/api/devices/{id}/video/ws`），
> 不加这项时代理只转发普通 HTTP，视频一路连不上——**页面没画面、后端日志里
> 也一条请求都没有**，很容易误判成后端问题。改完 `vite.config.ts` 需要重启 `pnpm dev`。

### 3. 生产模式（单端口）

```bash
cd business/web && pnpm build
cd ../server && cargo run
```

后端检测到 `business/web/dist` 后会直接托管前端，访问 <http://127.0.0.1:3100>。

### 4. 启动设备客户端（部署在 192.168.41.246）

内置默认值就是正式环境值：业务服务 `ws://39.107.55.13:3100`（外网）、本体 `192.168.234.234:10010`；
设备编号不用配，客户端启动时自动读本体 SN 分区 `/dev/mmcblk0p5`。

```bash
cd business/client

# 现场：连业务服务 + 机器狗本体（无需参数）
cargo run --

# 纯模拟模式：不接本体，只跑业务协议（前端联调用）
cargo run -- --robot sim

# 装成 systemd 服务（开机自启 + 自动重拉）
sudo ./deploy/install.sh
```

详见 [`client/README.md`](client/README.md)。

## 页面与功能

| 页面 | 功能 |
| --- | --- |
| 总览驾驶舱 | KPI、园区态势（真实底图/设备坐标）、设备状态环图、实时告警、7 日趋势、事件流、导出日报、全屏 |
| 园区管理 | 园区卡片/表格、完成率、巡检区域清单、园区与区域增删改、跳转巡检计划 |
| 巡检路线 | 三栏编辑器：路线模板、SVG 画布拖拽加点/选段、航点属性、航段任务、动作库、撤销、顺序优化、校验、发布、模拟执行 |
| 设备管理 | 状态统计、筛选、电量/心跳、手动绑定园区、设备详情抽屉（实时监控 + **手动遥控页内嵌实时画面**）、远程指令、批量下发、CSV 导出 |
| 任务管理 | 看板拖拽改状态、列表视图、任务创建/详情、航点执行序列、开始/暂停 |
| 告警中心 | 级别/状态筛选、单条/全部处理、告警详情 |
| 系统设置 | 平台参数、通知开关、关于平台 |

### 驾驶舱的数据归属

总览驾驶舱按登录人收敛，不再展示平台全量数据：

- 设备：按中台设备分配关系过滤；园区实时态势只展示自己管辖的设备。
- 园区 / 区域 / 任务：园区包含「自己创建的」与「自己设备所在的」，区域/任务统计跟随可见园区和创建人；
- 告警 / 事件流：只保留可见园区或可见设备相关的记录；
- 设备状态分布、7 日任务趋势、最新动态：分别从 `devices`、`tasks`、
  `task_events` + `alerts` 实时聚合，不再使用 `daily_stats` 等预置统计；
- 园区实时态势：优先用园区绑定地图包的栅格底图，设备点位由 `devices.position`
  （地图米制坐标）换算；没有地图包时退回示意底图，并按设备坐标范围归一化摆放。
- 管理员和没有中台归属的本地账号仍保持全局视角。

## API 速查

所有接口统一返回 `{ code, msg, data }`，成功 `code = 10200`。
除 `POST /api/auth/login` 和 `GET /api/health` 外均需 `Authorization: Bearer <token>`。

```text
POST   /api/auth/login
GET    /api/auth/me

GET    /api/parks                 POST   /api/parks
GET    /api/parks/{id}            PUT    /api/parks/{id}      DELETE /api/parks/{id}
GET    /api/zones                 POST   /api/zones
GET    /api/zones/{id}            PUT    /api/zones/{id}      DELETE /api/zones/{id}

GET    /api/devices               POST   /api/devices
GET    /api/devices/stats
POST   /api/devices/refresh
GET    /api/devices/{id}          PUT    /api/devices/{id}    DELETE /api/devices/{id}
POST   /api/devices/{id}/bind     DELETE /api/devices/{id}/bind
POST   /api/devices/{id}/command
POST   /api/devices/{id}/locate              # 载图 + 重定位（4030）
POST   /api/devices/{id}/action
POST   /api/devices/{id}/teleop              POST   /api/devices/{id}/teleop/take
POST   /api/devices/{id}/teleop/release      POST   /api/devices/{id}/teleop/ping

GET    /api/tasks                 POST   /api/tasks
GET    /api/tasks/stats
GET    /api/tasks/{id}            PUT    /api/tasks/{id}       DELETE /api/tasks/{id}
GET    /api/tasks/{id}/events      # 任务事件流（进度 / 定位异常 / 指令留痕）
PUT    /api/tasks/{id}/status

GET    /api/routes                POST   /api/routes
GET    /api/routes/{id}           PUT    /api/routes/{id}      DELETE /api/routes/{id}
POST   /api/routes/{id}/validate
POST   /api/routes/{id}/publish
POST   /api/routes/{id}/optimize

GET    /api/alerts                POST   /api/alerts
GET    /api/alerts/stats
POST   /api/alerts/handle-all
GET    /api/alerts/{id}           PUT    /api/alerts/{id}      DELETE /api/alerts/{id}
POST   /api/alerts/{id}/handle

GET    /api/settings              PUT    /api/settings
GET    /api/dashboard/summary

GET    /api/map-versions                  GET    /api/parks/{id}/maps
GET    /api/map-versions/{id}             GET    /api/map-versions/{id}/refs
GET    /api/map-versions/{id}/raster      GET    /api/map-versions/backend          POST   /api/map-versions/import-backend
GET    /api/map-versions/synced-sources   # 已同步的机器人地图路径（deviceId，兼容 parkId）
POST   /api/map-versions/{id}/publish     POST   /api/map-versions/{id}/new-version
POST   /api/map-versions/ingest          # 自动回传 / 手动同步共用入口

# 中台内部接口（共享服务令牌 x-service-token，不使用业务 JWT）
POST   /api/zentai/devices/{deviceId}/sync-maps   # 中台「同步地图」
POST   /api/zentai/devices/{deviceId}/command     # 中台下发指令
POST   /api/zentai/devices/{deviceId}/tasks       # 中台给设备下发任务
GET    /api/zentai/devices/{deviceId}/runtime     # 中台读取设备运行态（只读）
```

## 中台账号与设备分配

业务平台登录同时支持两类账号：

1. **中台账号（推荐）**：先用本地密码校验（兼容设备客户端服务账号），失败后再用
   **用户自己输入的中台账号密码**向 `ROBODOG_ZENTAI_URL` 登录（`POST /api/token`，
   密码 AES 加密后表单提交）；校验成功后自动在本地 `users` 表落地
   `zentai_user_id` / `zentai_username` / `zentai_roles`。
   这条路径**只需要配中台地址**，不需要在服务端配置中台账号。
2. **本地服务账号**：未配置中台或设备客户端使用，继续走本地 bcrypt 校验。

设备可见范围（`GET /api/devices`、`GET /api/devices/stats` 与所有单设备接口共用
`auth::allowed_device_codes`）：

- 中台管理员（角色 id = 1）在业务平台拥有全局视角；
- 普通中台账号只能看到 `dog_user_device` 中分配给自己的设备，**每次请求实时读中台
  分配关系**（中台改完分配，业务平台刷新页面即生效，无缓存）；
- 没有中台身份的本地账号不受限制（设备客户端 / 运维用途）；
- 只配了中台地址、既没有共享令牌也没有服务账号时，普通中台账号的请求会直接报
  「无法校验设备分配关系」，**不会**退化成返回全量设备。

业务平台的设备新增 / 修改 / 绑定园区 / 网关首次注册都会尽力同步到中台
`POST /api/dogDevice/upsert`（失败只记日志）；中台侧用「设备管理」页面批量
分配用户，业务平台在下一次请求时实时读取分配关系。中台侧只维护设备台账与
分配关系，运行状态、园区绑定仍以业务平台为准。

### 台账字段从哪来

中台「设备管理」的列全部由业务平台同步，**中台不再手工新增设备**；能自动填上的
字段就是设备实际能上报的那些：

| 中台 `dog_device` | 来源 | 何时写入 |
| --- | --- | --- |
| `device_id` | 客户端读本体 SN 分区（`/dev/mmcblk0p5`） | 注册 |
| `model` | 客户端注册/心跳里的 `model`（本体 `1201.type`，取不到则不报） | 注册、心跳变化时 |
| `firmware` | 客户端心跳里的 `firmware`（`--firmware` / `client.toml`） | 注册、心跳变化时 |
| `status` | 业务库 `devices.status`（待绑定 / 在线待命 / 执行任务 / 充电中 / 离线） | 注册、心跳变化时 |
| `device_ip` | 设备 WebSocket 接入的对端 IP（服务端 `ConnectInfo`） | 注册、心跳变化时 |
| `park_code` / `park_name` | 业务库园区（控制台绑定后） | 注册 / 绑定 / 解绑 |
| `device_name` / `remarks` | **设备侧无此数据**，只能在中台「修改」里人工填 | 人工维护（同步不会覆盖） |

心跳里的型号 / 固件 / 状态只在**变化时**才触发一次中台重同步，避免 10s 一次打接口；
运行态（电量、温度、里程、当前任务）只留在业务库，不进中台台账。

### 运行态哪些是真数据

心跳里除了运行态，还带 `bodyOnline` / `mode` 两个字段，业务平台据此区分真机与模拟：

| 页面字段 | 数据来源 | 说明 |
| --- | --- | --- |
| 电量 / 温度 / 里程 / 充电状态 / 工作模式 | 本体 `1315 kRobotStatus` 上报 | 客户端只取这个报文，见下表；`bodyOnline = true` 时才是真实值 |
| 运行状态 | 服务端派生 | 注册 → 待绑定/在线待命；`charging` → 充电中；任务进度 → 执行任务 |
| 最后心跳 | 服务端收到心跳的时间 | `POST /api/devices/refresh` 是**探活**（查活跃连接），不再改写它 |

`1315` 里我们实际用到的字段（现场抓包实测）：

| 1315 字段 | 用途 |
| --- | --- |
| `battery.power1` / `power2`（`present2` 为真时取较大值） | 电量 % |
| `battery.power_supply_status1/2`（1=充电中 2=充满）、`charging_pile_connect` | 充电状态 → `devices.charging` |
| `temp`（为 0 时退回 `battery.temperature1/2`） | 机身温度 |
| `mile_data` | 里程 |
| `mode`（协议里的 **MachineStatus**，字符串，实测 `remote`） | 工作模式 → `devices.robot_mode`，前端映射成 遥控/自动/手动… |

`1315` 里没有的：信号强度、型号、固件版本。型号取 `1201.type`，固件取客户端配置。
**「信号」这一列已删除**：协议 39 个字段里没有任何 rssi/csq/wifi/lte 指标，
`devices.signal` 变成历史列（客户端不再上报、接口不再返回、页面不再展示）。
充电状态以前是按「电量 ≤ 10% 就判充电中」猜的，现在以本体上报为准
（未绑定园区的设备始终是「待绑定」，执行中的设备不会被心跳改状态）。

两条兜底规则：

- **心跳看门狗**：`device_timeout_secs`（默认 90s，`ROBODOG_DEVICE_TIMEOUT_SECS`）内没有
  心跳的在线设备会被置为「离线」，清空接管态并写一条「心跳超时」告警。
  WebSocket 静默断开收不到 Close 帧，没有看门狗会一直显示「在线」。
- **模拟模式标出来**：客户端没连上本体时 `bodyOnline = false`、`mode = sim`，
  列表在状态下方标「模拟模式 / 本体离线」，电量和信号都显示 `—`。

设备客户端回传地图时，`mapInfo.deviceId` 会用于查询设备在中台的归属用户，
并把该用户写入中台 `dog_map.user_id`（同时把设备编号写入 `dog_map.device_code`，
中台地图列表可看出是哪儿台设备采的图）；园区选择地图时，非管理员只能看到
`user_id = 自己` 的中台地图。

## 设备与园区绑定

设备首次通过 WebSocket 注册时如果没有建档，服务会自动创建一条「待绑定」设备
（`devices.park_id IS NULL`、`status = '待绑定'`）；也可以在控制台先行建档
（表单默认不选园区）。绑定规则：

- `POST /api/devices/{id}/bind`（`{ parkId, zoneId? }`）手动绑定园区；`zoneId` 必须属于该园区。
- `DELETE /api/devices/{id}/bind` 解绑，设备回到「待绑定」，其待执行任务会被清空分配。
- 未绑定园区的设备**不参与任务下发**：`POST /api/tasks` / `PUT /api/tasks/{id}` 会拒绝
  未绑定设备，派发时会跳过 `task.park_id` 与设备园区不一致的任务。
- 注册回执 `register_ack` 会带上 `parkId / parkCode / parkName`，设备客户端据此确定建图回传的归属园区。
- 绑定园区后若设备在线，服务会立即补发该园区的待执行任务。
- 地图同步 / 建图回传**不要求绑园区**：地图先跟设备走，见下一节。

## 地图归属：先设备，再园区

地图只与设备有关系（`map_versions.device_code`、`map_versions.park_id` 可空）：

1. 机器人建图回传 / 中台「同步地图」→ 地图入库为**设备地图**（`park_id IS NULL`，
   `owner_user_id` 取中台里这台设备的绑定人）。设备没绑园区也照常入库、中台 `dog_map`
   的 `parkCode` / `parkName` 留空；
2. 设备绑定人登录业务控制台 →「新增园区」→ 地图基线下拉里选中自己的设备地图，
   保存时该地图被认领到新园区（`map_versions.park_id` 写入园区、园区拿到当前基线）；
3. 已被其他园区认领的地图不能再绑到第二个园区；没绑园区的地图也能建路线——
   新建路线时选中它同样会认领到该路线的园区。

`GET /api/map-versions` 对非管理员按「中台设备绑定人」过滤（`owner_user_id`），
所以每个人只会看到自己设备采的图；支持 `deviceCode` / `unbound=true` / `ownerUserId` 过滤。

## 地图同步：自动与手动

同一张机器人地图只入库一次：业务库 `map_versions.source_ref` 保存机器人本体
地图绝对路径，ingest 写库前按 `deviceCode + sourceRef` 判重（升级前入库的历史行
没有 `device_code`，一并算已同步），重复请求直接返回 `{ skipped: true }`。
**地图同步不要求设备绑定园区。**

1. **自动触发**：建图收到 4001 保存结果 → 客户端从文件服务下载地图包 →
   `POST /api/map-versions/ingest`（`sourceRef` = 4001 的 `map_path`）。
2. **手动触发**：中台「设备管理」列表点「同步地图」→ 中台调用本服务内部接口
   `POST /api/zentai/devices/{deviceId}/sync-maps`（共享服务令牌）→ 服务通过
   设备 WebSocket 下发 `同步地图` 指令 → 客户端用 4012 拉地图列表，先查
   `GET /api/map-versions/synced-sources?deviceId=...`，只下载尚未同步的地图。

因此自动回传与手动同步交叉执行也不会重复上传/入库。

判重不能只看业务库：中台才是地图主库。某张图在中台被删掉后，业务库里的历史行
不能算「已同步」，否则设备端永远跳过、中台列表里也永远看不到这张图。所以：

- `GET /api/map-versions/synced-sources` 返回前会调中台
  `POST /api/dogInternal/maps/exists`（只传 `map_versions.source_id`）复核，
  中台已删除（`is_deleted = 1`）的路径会重新交给设备下载上传；
- ingest 侧同样复核：中台已删就走正常上传并把业务行指向新的 `dog_map.id`，
  但**业务地图 id 不变**——路线 / 任务通过 `map_version_id` 引用它；
- 中台不可达时退回旧行为（按业务库记录判「已同步」），不把一次抖动放大成
  每台设备都重新下载几十兆地图。

所以「中台地图管理里删了某张图 → 再到设备管理点同步地图」是一条可恢复路径：
地图会重新上传到中台，业务库里原有的园区归属与路线引用不受影响。

### 「设备未连接」但控制台显示「在线待命」

设备在线状态以**服务进程内的连接池**（`device_id → 广播通道`）为唯一真相，
`devices.status` 只是给控制台和中台看的展示态：

- 注册时把本连接的通道登记进池子；断连清理**只摘掉自己那一条**（按通道身份
  `same_channel` 比对）。客户端重连时旧 TCP 往往还没死透（拔网线 / 4G 掉线 /
  进程被 kill 都收不到 Close 帧），新连接会顶替池里的登记；旧连接超时退出时若
  无条件 `remove(device_id)`，摘掉的就是活着的新连接 —— 台账仍是「在线待命」，
  中台点「同步地图」却报「设备未连接」，而且要等设备重连才恢复；
- 心跳到达时发现池里登记的不是本连接（进程重启 / 历史残留），会补登记自愈；
- 下发失败时把残留的「在线待命 / 充电中 / 执行任务」纠正为「离线」，错误提示
  同时给出「台账状态」与「已将其标记为离线」，比原来的「当前未连接（在线待命）」
  更能指向问题。

排查时先看 `devices.last_heartbeat`（与当前时间差超过 `device_timeout_secs`
就是掉线，心跳看门狗每 15s 扫一次）和 `device_ip`，再看设备客户端日志里
「业务连接断开 …N s 后重试」。

## 中台控制面：指令 / 任务 / 运行态

中台不持有设备连接，它只有控制面入口；**所有指令与任务最终都由本服务的设备
WebSocket 下发**。请求有两条链路，执行逻辑完全一致：

```text
① 反向信道（推荐，内网部署 / 无入站端口）
   本服务 ──dial(出站)──▶ 中台 GET /api/dogDevice/control-plane/ws
        ◀── request 帧（command / task / runtime / sync-maps）
        ──── result 帧（ok / error）────▶

② 直连 HTTP（同网部署）
   中台 ──▶ POST /api/zentai/devices/{deviceId}/command（或 /tasks / runtime）
```

反向信道由「中台 WS 地址」+「共享服务令牌」两项开启，任一为空则不建立
（继续用 HTTP 直连模式）。两项既可以走环境变量，也可以写进 `server.toml`
（见[配置文件](#配置文件-servertoml)）：

```bash
# 内网部署（业务平台只能出站）：环境变量方式
export ROBODOG_SERVICE_TOKEN=dev-service-token
export ROBODOG_ZENTAI_WS_URL=ws://192.168.41.10:3000
```

```toml
# server.toml 方式（环境变量仍优先）
[zentai]
ws_url        = "ws://192.168.41.10:3000"
service_token = "dev-service-token"
```

握手请求头带 `x-service-token`，连接断开后按 1s→30s 指数退避重连；
中台每 30s 发一次 `ping`，本服务回 `pong`。

另外，业务平台 → 中台的出站调用（设备台账同步、分配关系查询、地图回传）
全部走内部接口 `/api/dogInternal/*`，同样只用共享服务令牌，**不需要中台账号**。
`[zentai] username/password` 仅在没配令牌的兼容模式下才需要。

HTTP 链路（②）的三个约定：

| 请求头 / 帧字段 | 作用 |
| --- | --- |
| `x-service-token` | 共享令牌，与中台 `APP_DOG__BUSINESS_API_TOKEN` 一致 |
| `x-request-id` | 幂等键，重试原样带回；命中 `zentai_requests` 直接回放上次响应 |
| `x-operator-id` / `x-operator-admin` | 中台账号身份：二次校验设备分配关系，并写进 `task_events` 留痕 |

- **指令**：走 `devices::push_command`（与网页端同一实现），白名单
  `teleop::BUSINESS_COMMANDS`，未绑定园区的设备拒绝「开始巡检 / 继续任务」。
- **任务**：走 `tasks::create_task_core`（与网页端同一实现）；中台只给路线 ID 与
  计划时间，航点/航段/动作、地图绑定、限速与结束动作由 `dispatch.rs` 组装。
- **未送达即失败**：设备不在线时返回 4xx（`指令未送达`），中台会把审计记成
  `failed`，不会显示成「已下发」。
- **运行态**：只读聚合（在线、状态、电量、当前任务与进度、接管人），不写库。

## 地图基线

### 业务地图编号 vs 中台地图编号

- 业务库 `map_versions.id` 形如 `ROS-MAP-<首次入库时的中台 dog_map.id>@<版本>`，
  创建后不可变（路线/任务用 `map_version_id` 引用）。中台把某张图删掉后重新上传时会
  **复用业务编号**，中台侧却换成了新的 `dog_map.id` —— 所以编号里的数字可能跟中台当前编号对不上。
- 业务库 `map_versions.source_id` 是**当前**中台 `dog_map.id`，两库关联以它为准。
- 控制台「地图基线详情」直接列出「中台地图编号 / 地图名称 / 采集设备」，并提供
  「在中台地图管理查看」直达链接（`/dogPark/map/index?mapName=…&deviceCode=…`）；
  中台地图列表也有「地图编号」列，两边按编号或名称对照即可。
- 迁移 015 之前入库的历史地图没有 `device_code`，服务启动时按
  `source_id = dog_map.id` 从只读中台库回填一次（幂等）。

地图版本保存在 `map_versions` 表，`id` 形如 `MAP-ZJ-01@v2.1`，发布后不可变。
园区通过 `parks.map_version_id` 持有「当前生效基线」指针，路线创建时把
`routes.map_version_id` 冻结为当时的版本，之后地图发新版不会改写历史路线坐标。

- `GET /api/map-versions` 支持 `parkId`、`deviceCode`、`unbound`、`status` 过滤，前端用它渲染基线徽章与版本下拉；
  非管理员只返回自己设备采的地图（`owner_user_id`，管理员可看全部 / 用 `ownerUserId` 指定他人）。
- `GET /api/map-versions/backend` 读取仓库根目录 `backend` 的 `dog_map` 地图库，列出已上传的 ROS map_server 地图包；
  支持 `parkCode` 过滤，业务平台可据此只查看某园区的地图。
- `POST /api/map-versions/import-backend` 把 `{ backendMapId, parkId?, bind }` 指定的地图导入本业务库；
  `parkId` 缺省时按中台地图携带的 `parkCode` 反查业务园区，园区编码未登记会报错。
  导入时会从 `.zip` 内解析 `model.yaml` 的 `resolution` / `origin` 和 `model.pgm` 的宽高，并可选绑定为园区当前基线。
- `GET /api/map-versions/{id}/raster` 解析地图包里的 `model.pgm`，转成 PNG data URL，返回分辨率、原点与栅格宽高；路线编辑画布可切换「真实地图」底图。
- `GET /api/map-versions/{id}/refs` 返回引用该版本的路线、任务与园区设备数，用于影响分析。
- `POST /api/map-versions/{id}/publish` 发布版本并切换园区基线指针，同园区旧版本归档。
- `POST /api/map-versions/{id}/new-version` 派生下一版本并发布，历史路线保持旧版引用。
- `POST /api/map-versions/ingest` 一次性完成「推中台 + 写业务库」，是设备客户端建图回传的入口（见下节）。

### 本体地图目录名：`mapId` ≠ 目录名

智航只认**本体上的目录名**（4013 `map_path_list[].path` 的最后一段，如
`2026_11_10_16_15_06`，由建图会话决定）。业务的 `map_versions.id`
（`ROS-MAP-1794446794789@v1.0`）是平台自造的，把 `@` 换成 `_` 得到的目录名本体上
并不存在：5000/4030 会指到空目录，本体只回
`13898 map not found or invalid (missing map.yaml)`——从字面很难看出是「目录名对不上」。

- 目录名的唯一可靠来源是 `map_versions.source_ref`：设备「同步地图」回传时写入
  `/ota/alg_data/map/history_map/<目录名>`。
- `import-backend` 从中台重新导入同一份包时，会按「同名 zip」认回已登记的目录名
  （`mapdir::source_ref_for_file_name`）；历史行由启动时的
  `mapdir::backfill_source_refs` 幂等补齐。
- 下发（`dispatch.rs`）把真实目录名同时放进 `map.mapDir` 与 `map.mapPath`；没有
  `source_ref` 时退回合成名，由客户端再用 4013 列表核对。
- 客户端（`MapManager::resolve`）按 `mapDir` → `mapPath` → 合成名在 4013 列表里匹配，
  命中就用本体上报的绝对 `path`（L2/M1 地图根不同，不能靠平台拼）；全都不命中就
  **拒绝下发**，任务事件里直接列出本体现有目录。「重新定位」（4030）与返充
  （5014 的 `map_id` 也是目录名）走同一套解析。
- 注意：这里只解决「目录名」。本体目录里必须是 `map.yaml` / `map.pgm` /
  `map.geojson` / `map.pcd` 这套智航命名，ROS map_server 的 `model.*` 包即使目录名
  对了也载不进去——需要推图到本体时得先把 `model.*` 改名成 `map.*`。

### 路线坐标：地图米制（不是画布像素）

智航 Pro `5000 kind=2` 的 `goals[].position` 是**地图坐标系下的米**（协议
§2.5.1.4，坐标系由 `map_path` 那张图的 `frame_id` 决定）。业务平台的约定：

- `routes.route_data.points[].x/y` **一律存地图米**，并带 `"coordFrame": "map"`
  （`schemaVersion: 3`）；`legs`/`returnLeg` 只有速度与动作，不含坐标。
- 编辑器画布是 800×460 的 viewBox，点击/拖拽只在画布与米制之间换算，
  换算公式集中在 `server/src/coords.rs` 与 `web/src/utils/coords.ts`（两份必须一致）：

```text
scale = min(800 / width, 460 / height)      // 底图等比铺进画布
rectX = (800 - width * scale) / 2,  rectY = (460 - height * scale) / 2

画布 → 地图：x = ox + (cx - rectX) / (width  * scale) * width  * res
             y = oy + (1 - (cy - rectY) / (height * scale)) * height * res   // y 轴翻转
地图 → 画布：cx = rectX + (x - ox) / (width  * res) * width  * scale
             cy = rectY + (1 - (y - oy) / (height * res)) * height * scale
```

- `resolution` / `origin` / `map_width` / `map_height` 全部来自冻结的地图版本；
  没有真实 ROS 地图包（演示基线、未绑定基线）时退回**示意坐标系**
  （0.1 m/px、原点 (0,0)、画布自身 800×460），公式仍可逆。
- 里程/耗时/耗电按米直接计算（`geo.rs`），不再乘任何「像素→米」经验系数。
- **示意坐标系与未迁移的像素路线一律拒绝下发**：
  `POST /api/tasks` 与 `POST /api/tasks/{id}/dispatch` 会返回 400 并说明原因；
  发布路线、路线校验也会给出同样的检查项。
- 历史路线在启动时由 `coords::migrate_route_coordinates` 一次性换算
  （`coordFrame == "map"` 即跳过，可重复执行），并重算里程/耗时/耗电。

`server/seed.rs` 会在 `map_versions` 为空且已有园区时自动补齐演示基线，并为
RT-001 ~ RT-004 写入对应的冻结版本引用。

## 建图回传：`POST /api/map-versions/ingest`

设备客户端在本体建图完成后，把 `.zip` 地图包直接 POST 到这个接口，一次完成两件事：
**写中台 `dog_map`** + **写业务库 `map_versions`**。区别于 `import-backend`
（那是从根 backend 的 SQLite 反向读取，要求地图已经在中台里）。

请求：`multipart/form-data`

| 字段 | 必填 | 说明 |
|------|------|------|
| `file` | ✅ | 地图 `.zip`（须含 `model.pgm` + `model.yaml`），≤ 256MB |
| `mapInfo` | ✅ | JSON 字符串：`{ mapName, parkId, deviceId, bind, mapType, remarks, source, sourceRef }`；`parkId` 可省略（设备未绑定园区时地图先入库为设备地图）；`deviceId` 用于确定地图归属用户、作为中台 `dog_map.device_code`，也是同步判重的主键 |

响应：

```json
{ "code": 10200, "msg": "ok", "data": {
    "mapId": "ROS-MAP-3@v1.0", "mapName": "B区-20261110", "parkId": null,
    "zentaiMapId": 12, "bound": false,
    "stashPath": "data/map_ingest/B区-20261110_20261110153000.zip",
    "map": { } } }
```

处理流程：

```
1. 校验 zip 魔数 PK、大小；parkId 存在时校验园区存在（缺省则入库为设备地图）
2. stash_package → data/map_ingest/ 留档（中台不可用时凭此路径重放）
3. parse_local_meta → 本地解析 model.pgm 宽高 / model.yaml resolution+origin
4. push_to_zentai → 共享令牌 POST /api/dogInternal/maps/upload（无令牌时退回账号登录 + /api/dogMap/upload）
   mapInfo 会带上上报设备 deviceCode、设备归属用户 ownerUserId 与（有则带）园区 parkCode / parkName
5. 写 map_versions（ON CONFLICT(id) DO UPDATE；重传不会冲掉已有园区归属）
6. bind=true 且设备已绑园区时切换园区当前基线
```

中台地图带上园区后，业务平台从中台拉取地图时（`/api/map-versions/backend`）
可以按 `parkCode` 过滤或直接导入，`import-backend` 未显式指定园区时会自动按园区编码回填。

> 园区归属用 `parks.park_code` 做跨服务关联，请尽量不要修改已绑定地图的园区编码；
> 确有变更时，中台历史地图仍指向旧编码，需重新导入或手工修正 `dog_map.park_code`。

中台账号只配在**服务端**（`ROBODOG_ZENTAI_*`），不随设备客户端分发。
中台凭证未配置时接口返回内部错误，`GET /api/health` 的 `mapIngest.ready` 会暴露该状态，
可用于上线前自检。

上传体积：真实机器人地图包常见 10~30MB（`map.pcd` 27MB、`map.pgm` 1MB 等），
`/api/map-versions/ingest` 单独把 body 上限放到 **256MB**（`app.rs` 的
`MAP_UPLOAD_LIMIT`）；axum 默认 2MB，不放宽会在真实回传时直接
`400 Error parsing multipart/form-data`。

## 路线数据模型

路线保存在 `routes.route_data`（JSON），结构为：

```json
{
  "schemaVersion": 2,
  "loop": "闭环",
  "points": [
    { "name": "配电柜测温点", "x": 212, "y": 294, "dwell": 15, "radius": 0.3,
      "must": true, "heading": 90, "onFail": "重试 2 次",
      "actions": [{ "type": "红外测温", "param": "云台 P90", "trigger": "停留后执行",
                    "threshold": "> 65℃ 生成告警", "retry": 2 }] }
  ],
  "legs": [
    { "mode": "沿道路", "speed": 1.2, "avoid": "标准",
      "actions": [{ "type": "全程录像", "param": "1080P / 25fps / 连续" }] }
  ],
  "returnLeg": { "mode": "沿道路", "speed": 1.5, "avoid": "标准", "actions": [] }
}
```

后端在保存时重新计算 `point_count`、`distance_m`、`duration_s`、`battery_pct`，
校验与发布接口直接读取该 JSON。

## 配置项

### 配置文件 `server.toml`

除了环境变量，也可以用配置文件；优先级 **环境变量 > `server.toml` > 内置默认值**。

```bash
cp server.toml.example server.toml            # 或放到 /etc/robodog/server.toml
export ROBODOG_SERVER_CONFIG=/etc/robodog/server.toml   # 可选：显式指定路径
```

```toml
port = 3100

[zentai]
# 中台地址：建图回传、设备台账同步用
base_url      = "http://192.168.41.10:3000"
# 反向控制信道（业务平台主动拨出）
ws_url        = "ws://192.168.41.10:3000"
# 共享服务令牌：上面两条链路都用它，不需要中台账号
service_token = "change-me"
```

- 文件里的相对路径按**配置文件所在目录**解析，与启动时的工作目录无关。
- 未知字段直接报错（拼错的键不会静默失效）。
- 显式用 `ROBODOG_SERVER_CONFIG` 指定但文件不存在 → 启动失败；默认路径不存在 → 静默跳过。
- 密钥类（`jwt_secret` / `zentai.password` / `service_token`）建议只走环境变量，
  文件里放地址类配置。

### 环境变量

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `ROBODOG_PORT` | `3100` | 后端端口 |
| `ROBODOG_DB` | `server/data/robodog.db` | SQLite 文件路径 |
| `ROBODOG_JWT_SECRET` | 开发用默认值 | JWT 密钥，生产必须覆盖 |
| `ROBODOG_JWT_TTL_HOURS` | `24` | Token 有效期 |
| `ROBODOG_WEB_DIST` | `../web/dist` | 前端构建产物目录 |
| `ROBODOG_BACKEND_DB` | `../../backend/data/users.db` | 仓库根目录 backend 的 SQLite 文件，用于导入 `dog_map` 中的 ROS 地图 |
| `ROBODOG_ZENTAI_URL` | 空 | 中台地址，如 `http://127.0.0.1:3000`。配合共享令牌（或兼容模式的账号）即启用建图回传 / 设备台账同步 |
| `ROBODOG_ZENTAI_USER` | 空 | **兼容模式**用：没有共享令牌时才需要中台账号（登录换 JWT）。只配令牌时可删 |
| `ROBODOG_ZENTAI_PASSWORD` | 空 | 兼容模式用的中台账号密码（明文，服务端 AES 加密后提交；建议改用共享令牌） |
| `ROBODOG_INGEST_DIR` | `server/data/map_ingest` | 回传地图包的本地留档目录 |
| `ROBODOG_SERVICE_TOKEN` | 空 | 与中台之间的共享服务令牌：调中台内部接口 `/api/dogInternal/*`、反向控制信道握手。需与中台 `APP_DOG__BUSINESS_API_TOKEN` 一致 |
| `ROBODOG_ZENTAI_WS_URL` | 空 | 反向控制信道的中台地址，如 `ws://10.0.0.8:3000`。与 `ROBODOG_SERVICE_TOKEN` 同时具备才建立信道；为空时走「中台直连 HTTP」 |
| `ROBODOG_SERVER_CONFIG` | 空 | 指定配置文件路径；不设置时读取当前目录的 `server.toml`（不存在则只用环境变量） |

## 任务下发与本体对接

任务通过 `devices.device_id` 关联设备，设备上线后由
`server/src/gateway.rs::dispatch_pending_tasks` 补发所有 `待执行` 任务；
`POST /api/tasks`（带 `deviceId`）会立即推送。

`PUT /api/tasks/{id}`（控制台「编辑」）只允许修改 `待执行` 的任务，执行中 / 已完成 /
异常的任务需先在看板里拖回 `待执行`；字段缺省表示保持原值，`routeId: null` 表示显式
解除路线关联（改为临时路径）。修改会写入 `task_events`（`event_type = 'updated'`）。

下发载荷由 `server/src/dispatch.rs::build_dispatch_payload` 统一组装：

| 字段 | 来源 | 说明 |
| --- | --- | --- |
| `routeData` | `routes.route_data` | 前端三栏编辑器的 `points/legs/returnLeg`；`points[].x/y` 是地图米制（`coordFrame: "map"`） |
| `map.mapId` | `routes.map_version_id` → 回退 `parks.map_version_id` | 冻结版本优先 |
| `map.mapPath` | 由 `mapId` 派生（`@` → `_`） | 本体侧目录名 |
| `map.mapType` | 固定 `1`（2D 栅格） | `3` 预留给 Route 路网 |
| `speedLimit` | `routes.default_speed` | m/s |
| `avoidLevel` | 固定 `40`（全避障） | 10 关闭 / 20 仅急停 / 30 感知 / 40 全避障 |
| `endTask` | 由 `executeMode` 推断 | `return_to_charge` / `return_to_start` / `none` |

客户端（`client/`）把载荷翻译成智航 Pro 指令驱动机器狗本体，详见
[`client/README.md`](client/README.md)。

### 定位与「机器狗不在图上」

任务能不能跑，取决于机器人**在地图坐标系里知不知道自己在哪**。链路是：

1. `POST /api/tasks` / 重连补发 → 客户端发 `5000 kind=2`，首航点带
   `params.map_path` + `need_locate: true`（协议 §2.5.1：载图并等定位收敛，安全默认）；
2. 初始位姿由 SLAM 读「上一份位姿文件」做完整重定位（协议 §2.4.1：`initial_pose`
   为零四元数时的行为）；
3. 客户端在发 5000 **之前**加了定位守卫：`loc_state.status` 已是 100 直接起步，否则先
   下发 `4030 op=1` 载图重定位并等收敛（上限 60s）；收敛后首航点用 `need_locate=false`
   复用这份定位，不再重载一次地图。60s 内不收敛（或 `203/210`）就**不派发**，任务直接
   置「异常」，进度消息写明下一步该做什么；任务详情弹窗的「任务事件」能直接看到这条原因。

恢复手段（按代价从低到高）：

| 场景 | 动作 |
| --- | --- |
| 只是没载图 / 刚上电 | 什么都不用做，守卫会自动 4030 重定位 |
| 人把设备挪了位置，仍在地图内 | `POST /api/devices/{id}/locate` + `{ "x":.., "y":.., "heading":90 }` 指定初始位姿 |
| 定位丢失（`122`）/ 被搬到地图外 | 先「远程接管」把设备开回地图覆盖范围，再重定位；实在开不回来只能现场搬回 |
| 老任务已被判异常 | 看板把任务拖回「待执行」→ 重新下发（待执行任务在设备重连时会自动补发） |

接口：

```bash
# 全量重定位（地图与位姿都走默认：机器人当前地图 / 上次位姿文件）
curl -X POST /api/devices/12/locate -H 'Content-Type: application/json' -d '{}'

# 换地图 / 指定初始位姿（heading 为度）
curl -X POST /api/devices/12/locate -H 'Content-Type: application/json' \
  -d '{"mapId":"ROS-MAP-3@v1.0","x":1.5,"y":-2.0,"heading":90}'
```

地图解析顺序：请求 `mapId` → 设备上报的 `activeMapId` → 园区 `map_version_id` 基线；
都解析不到直接 400，避免按错的图重定位。设备离线返回 400「未送达」，不会记成已下发。
定位状态（`locStatus`）与当前地图（`activeMapId`）由设备心跳上报并落库，控制台设备列表
与详情抽屉可见。

### 进度与阶段回传

`task_progress` 携带 `stage`（智航 5009 阶段串）与 `robotTaskId`（5001 返回的本体任务 ID），
落库到 `tasks.robot_stage` / `tasks.robot_task_id` / `tasks.robot_updated_at`（迁移
`004_robot_task_binding.sql`），前端任务列表可直接展示执行阶段。

客户端 5s 一报，服务端落 `task_events` 时按「标题 + 描述」去重：内容没变就不再插新行。
以前暂停/卡住时一小时能刷出上千条同样的「任务已暂停」，把「导航指令非法」
「无有效绕障轨迹」这类真正要看的故障全冲掉了。

本体侧的健康变化也会单独落事件（由客户端 1315 变化触发）：运动状态（`walk` /
`lie_down` / `unknown` …）、机器模式（`remote` / …）、控制源（0 未知 / 1 APP /
2 SDK / 3 遥控器）。「它什么时候自己趴下的」「谁把控制权拿走了」直接看事件流即可。
`4021` 故障事件的文案里带上了原始 `code` 与 `severity`，便于对着本体文档排查。

### 任务控制

`PUT /api/tasks/{id}/status` 映射到客户端的 `task_control`：

| task_status | action | 客户端行为 |
| --- | --- | --- |
| `已暂停` | `pause` | 5004 暂停本体任务（真实 `task_id`） |
| `执行中` | `resume` | 5006 恢复（真实 `task_id`） |
| `已完成` / `异常` | `cancel` | 5002 取消；建图任务会等 4001 保存结果 |

控制台任务卡片上的「取消」按钮走的就是 `已完成` / `异常` → `cancel` 这条链路，用于把
本体上挂着的任务真正收尾（只暂停不收尾会让本体一直占着导航能力）。

> 下发任务前请先**释放远程接管**：接管态下客户端会把本体切到 1302 `cmd=60` 通用
> （手动）模式，导航能力被判给手动控制，本体对每个 `5000` 都回
> `13897 resource conflict (capability busy)`。服务端 `dispatch_task` 现在会直接
> 返回 400 并提示「正在被 xxx 远程接管」，不再让它拒五次。

同理，重新下发任务时客户端会先用 `5002` 取消上一轮残留的本体任务（并把 `nav_task_id`
保留到取消完成），否则「暂停后再下发」永远起不来。

`reason` 字段会透传给客户端，便于在机器人日志中定位中断原因。

## 完全接管：摇杆「走不动」的三种原因

现场常见现象：平台点「接管」后能下发站立/卧倒，但摇杆前进后退没反应，而物理遥控器
照样能开。原因只有三类，平台现在都能看见并处理：

| 原因 | 判据 | 处理 |
| --- | --- | --- |
| 只拿了控制权，本体不在运动模式 | 1304 `lx/ly` 在**原地模式**下按协议「无作用」 | 客户端接管时自动补一条 `1302 cmd=60`（通用模式）；也可在设备抽屉手动切「通用模式」 |
| 硬件遥控器抢控制权 | `1315 ctrl_source = 3`（other），平台发 1204 也拿不回来 | 协议里没有禁用遥控器的消息：关掉遥控器电源 / 按它的「释放控制」键，再释放并重新接管 |
| 摇杆帧太稀 | 浏览器 ~10Hz 的离散帧，本体侧看着像点动 | 客户端按协议建议 50Hz 重发（`teleop::spawn_republisher`），松手立即停车 |
| 本体卧倒 / 锁定 | `1315 motion_status = lie_down / locked` | 先点「站立」，等 `motion_status` 变成 `stand_up` 再遥控 |

因此「接管」的完整语义是：`1204` 拿到控制权 → `1302 cmd=60` 切通用模式 → 摇杆按 50Hz
持续下发 → 用 `1315 ctrl_source / motion_status` 自证「真的接管到位」。设备抽屉的遥控页
有一条诊断条直接显示这三项，接不通时会写明该做什么。

本体动作白名单（`POST /api/devices/{id}/action`）与之配套：
`stand_up` / `lie_down` / `emergency_stop` / `light_on` / `light_off` /
`generic_mode` / `in_place_mode` / `navigation_mode`；除急停外都要求先接管。

## 实时视频与手动遥控

### 视频：协议里没有，只能外挂

协议原文已归档在 `business/docs/zhihang-pro-api-v1.0.3.pdf`（84 页），
同目录 `.txt` 是便于检索的纯文本版；设备客户端与机器狗本体的连接方式（双通道建链、
心跳、重连、配置项）见 [`docs/client-connection-guide.md`](docs/client-connection-guide.md)。

智航 Pro v1.0.3 接口文档全文（84 页）**没有视频传输相关描述**：`视频` / `码流` /
`摄像头` / `图传` / `RTSP` / `HLS` / `WebRTC` / `video` / `camera` 均 0 次出现，
API Type 总表 60 个消息里也没有图像或流媒体类型。`bulkdata` 通道的大负载是
1401/1403/1405/1407 的位姿与路径数据，不是视频帧。

文档里与「影像」沾边的只有两处，都**不是视频流**：

- 巡检点动作 `tasks=1 拍照`，调用上装 `/capture_image`，单张静态图；
  任务记录里用 `data.items[].phase[].image_path` 返回图片落盘路径。
- §4「文件服务」HTTP 端口 `12000`，L2 根 `/userdata/alg_data`、M1 根
  `/ota/alg_data`，用于下载地图包 / 图片等文件。

因此视频由**本体自带的流服务**提供，设备档案里只存地址：
设备编辑弹窗填 `videoUrl` + `videoType`（`none`/`hls`/`flv`/`mp4`/`webrtc`/`rtsp`），
未显式选类型时按地址后缀自动推断。现场本体对外是 RTSP，例如：

```
rtsp://user:Abcd%232027%21@192.168.234.1:8554/test
        └── 用户名 └── 密码（URL 编码，解码后 Abcd#2027!）
```

| 类型 | 播放方式 |
| --- | --- |
| `hls` | hls.js；Safari 走原生 `canPlayType` |
| `flv` | flv.js（MSE） |
| `mp4` | 原生 `<video>` |
| `rtsp` | **经设备客户端回传**（见下节），浏览器用 flv.js 播中继流 |
| `webrtc` | 需要信令网关，当前未接入，前端只给提示 |
| `none` / 无地址 | 显示占位说明，不拉流 |

前端 `VideoPlayer.vue` 里 hls.js / flv.js 按需动态导入，只有真正要播放时才加载。

### RTSP：客户端拉流转 FLV，经业务平台中继给浏览器

浏览器不能直连 RTSP，能访问本体网段（`192.168.234.x`）的是设备客户端。
所以整条链路是「客户端拉流 → 平台中继 → 浏览器 MSE 播放」：

```
机器狗本体 RTSP ──ffmpeg──► 设备客户端 ──ws://平台/ws/device/video──► 业务平台 ──ws──► 浏览器(flv.js)
                         转封装 H.264/FLV        二进制帧中继                 /api/devices/{id}/video/ws
```

- **按需推流**：控制台打开「实时监控」才下发 `video_start`（携带设备档案里的
  `video_url`），最后一个观看者离开立刻 `video_stop`。没人看时机器狗侧不占 CPU / 带宽。
- **拉流地址只有一处**：就是设备管理里的 `video_url`，改地址不用动客户端配置。
- **补流头**：中途加入的浏览器会先收到「FLV 头 + SPS/PPS + 首个关键帧」再进直播，
  否则 flv.js 拿不到魔数与 AVC 配置会黑屏（`server/src/video.rs` 的 `flv_init_len`）。
- **墙钟时间戳**：现场相机的 SDP 标 60fps、PTS 也按 1/60 递增，但实际只投递 ~25 帧/秒。
  若沿用相机时间轴，媒体时间只走 0.42x 实时，浏览器 MSE 会反复饿死
  （播放器在「连接中 / LIVE」之间来回跳、画面慢放）。客户端拉流时固定加
  `-use_wallclock_as_timestamps 1`，把 PTS 换成到达时间，时间轴与真实时间 1:1。
- **鉴权**：浏览器 WS 握手不能带请求头，中继端点用 `?token=<JWT>`，
  `require_auth` 为此放开了查询串令牌（普通接口仍只认 `Authorization`）。
- **空转兜底**：推流重连会丢弃旧房间（新流的新 FLV 头不能混进老连接），
  浏览器侧自动重连；另有 15s 看门狗把「有人在推、没人看」的流停掉，
  防止浏览器崩溃（没有 Close 帧）时机器狗一直在编码。

现场前置条件：

| 项 | 要求 |
| --- | --- |
| 客户端所在机器 | 能访问本体 RTSP 地址（现场就是机器狗自身 / 同网段） |
| `ffmpeg` | 客户端机器上必须可执行，默认取 `PATH` 里的 `ffmpeg`，可用 `[video].ffmpeg` 指定 |
| 编码器 | 默认 `[video].encoder = "auto"`：现场机器狗的 ffmpeg 没有 libx264，会选中瑞芯微硬件编码器 `h264_rkmpp` |
| 客户端配置 | `[video].enabled = true`（默认开）；`fps` / `bitrate` / `width` 按带宽调 |
| 平台 | 无需额外媒体服务器；`/ws/device/video` 与 `/api/devices/{id}/video/ws` 已内置 |

延迟量级：编码 2 秒一个关键帧 + 网络 + MSE 缓冲，实测在 1~2 秒；带宽按
`[video].bitrate`（默认 1200k）计，一台设备一路流。

### 遥控：1302 本体命令 + 1304 摇杆

文档 §3 把 `1302`（本体命令）、`1304`（遥控摇杆）、`1310`（充电桩）归为控制类请求，
响应看 `1303` / `1311` 的 `status_code`（现场固件只对 `1310` 回 `1311`）。

```
前端 DeviceDrawer → POST /api/devices/{id}/teleop/take  → 落库接管态
                  → POST /api/devices/{id}/teleop        → WS msgType=teleop
                  → 客户端 zhpro/teleop.rs → 智航 1304
                  → POST /api/devices/{id}/action        → WS msgType=command
                  → 客户端 zhpro/teleop.rs → 智航 1302
```

抽屉布局：进入「手动遥控」页时抽屉自动加宽到 780px，**左侧实时画面 + 电量/模式/位置/接管
徽标，右侧摇杆 + 速度档 + 动作 + 急停**，一屏操作不必来回切页签；视口窄于 860px 时
自动上下堆叠。画面同样是按需起流：只在打开抽屉、进入该页签时下发 `video_start`。

协议对齐（v1.0.3 原文）：

| 项 | 文档要求 | 客户端实现 |
| --- | --- | --- |
| `1302 data.cmd` | **uint32 枚举**：`0` 紧急停止、`1` 解除急停、`10` 站立、`11` 匍匐、`12` 卧倒、`100~105` 补光灯… | `BodyCommand::code()` 按码值下发（以前发的是字符串 `stand_up`，本体不认） |
| `1303` 受理结果 | 文档写 `data.status_code` + `data.message` | **现场固件实测不发 `1303`**（合法 `cmd=102` 与非法 `cmd=999` 都无应答），因此 `1302` 与 `1304` 一样单向下发；执行效果以 `1315` 的 `motion_status` / `fill_light` / `estop` 变化为准（客户端会把这些变化打成日志） |
| `1303` 关联 | 文档示例 head 不带 `msg_id` | `call_control()` 按「响应类型 = 请求类型 + 1」关联（留给真有应答的 `1310`/`1311`） |
| `1304 data` | `lx / ly / rx / ry / turn / high_low`，全部 `[-1, 1]` | 按此字段名下发；`lx` 正数左移、`rx` 正数左旋，业务侧轴值取反后发送 |
| `1304` 响应 | 协议里不存在 `1305` | 摇杆走单向下发（`send()`），不等应答 |

安全约束：

- **接管态闸门**：`devices.teleop_holder` / `teleop_at` 落库，未接管时摇杆和动作
  一律拒绝（前端按钮置灰）；同一台设备同一时刻只允许一个接管者。
- **急停常驻**：`/action` 里的 `emergency_stop` 不受接管态约束，任何状态都能发。
- **两级看门狗**：前端松手立即发全 0 帧；客户端 1 秒无新摇杆帧自动补停车；
  服务端接管 120s 空闲自动失效（前端每 45s 发一次 `/teleop/ping` 保活）。
- 摇杆三轴 `forward`/`strafe`/`turn` ∈ [-1, 1] 直接映射到本体的归一化摇杆值
  （本体侧自行换算速度），`speed` 档位 0.1~1.0 在客户端二次缩放。
- 所有接管、释放、动作都写 `task_events`（`event_type='teleop'`）留审计。

可用动作：`stand_up` 站立（`cmd=10`）、`lie_down` 卧倒（`12`）、
`emergency_stop` 急停（`0`）、`light_on` / `light_off` 前补光灯（`102` / `104`）。
危险动作前端二次确认。**「位姿复位」已下线**：协议 1302 的枚举里没有对应码值，
以前下发的字符串本体根本不认。

> `1302` 不在 `4000~5999` 区间，不受 RoamerX 控制权约束，但本平台仍要求先
> 「远程接管」才能发动作（与业务侧闸门保持一致）；接管失败会在客户端日志里
> 留下 `xyz 需要先接管`。动作本身没有受理回执（固件不回 `1303`），真正执行与否
> 看 `1315`：`motion_status`（`stand_up` / `balance_stand` / `lie_down`）、
> `fill_light.front`、`estop.software`。
>
> **急停闸**：`1315 estop.software = true` 时本体运动层会拒掉动作（`mode` /
> `motion_status` 会一直是 `unknown`），此时先解除急停（`1302 cmd=1` 或现场
> 急停按钮复位）再下发站立。客户端检测到这个状态会打一条 WARN。

## 当前边界

- 设备状态、电量、心跳来自本服务数据库；演示数据由启动播种生成。
- 设备网关（`server/src/gateway.rs`）已接入：注册、心跳、任务下发、任务控制、
  远程指令、进度与事件回传。客户端在 `client/`，接入真实本体需传 `--robot`。
- 智航 v1.0.3 的 `kind=4`（原生巡检）尚未开放，巡检采用
  「逐点导航任务 + 客户端侧点位动作」实现，见 `client/README.md`。
- 视频：`hls`(m3u8) / `flv` / `mp4` 浏览器直连播放；`rtsp` 由设备客户端拉流转 FLV、
  经业务平台中继（`server/src/video.rs` + `client/src/video.rs`），支持多观看者与按需起停。
  `webrtc` 仍需要额外信令网关，前端只给提示；中继没有做录制 / 回放，也不做多路混流。
- 手动遥控（接管 / 摇杆 / 本体动作 / 急停）前后端与客户端全链路已打通，接管态落库并按
  120s 空闲超时自动失效；但 holder 目前是设备维度标识（中间件没有把用户名透传到 handler），
  多个账号同时操作同一台设备时仍是「谁先接管谁控制」，不是按登录人隔离。
- 告警目前以数据库记录与手动处置为主，自动规则（温度/离线/低电量）预留后续扩展。

## 重置演示数据

停止后端后删除数据库文件，再次启动即可重新建表与播种：

```bash
rm -f business/server/data/robodog.db*
cd business/server && cargo run
```

## 测试约定

测试分两层，避免「集成测试写进业务代码」：

| 层级 | 位置 | 内容 |
|------|------|------|
| 单元测试 | `src/**/mod.rs` 末尾 `#[cfg(test)] mod tests` | 纯函数、解析、状态机。`cfg(test)` 在 release 构建中整体剔除，不进入产物 |
| 集成测试 | `tests/*.rs` | 走真实路由 / 真实迁移 / 真实临时库的黑盒用例 |

- `server` 与 `client` 都提供 `src/lib.rs` 作为库入口，二进制 `src/main.rs` 只做进程装配，
  这样集成测试可以像外部调用方一样使用 `robodog_ops_server::*` / `robodog_device_client::*`；
- 需要数据库、HTTP 路由、文件 IO 的用例一律放 `tests/`，不要塞进 `src/`；
- 新增业务逻辑尽量先抽成纯函数，顺带补内联单测；临时资源（数据库、配置文件）用
  `std::env::temp_dir()` + 进程号 + 原子计数命名，并用 `Drop` 清理，保证并行安全；
- 线上事故修复要补回归用例，例如驾驶舱 `video_url` 缺列 → `server/tests/api_smoke.rs`。

当前覆盖：

| 包 | 单测 | 集成测试 |
|------|------|------|
| `server` | 26（`dispatch` / `geo` / `ingest` / `teleop` / `routes` / `devices`） | 3（`tests/api_smoke.rs`：健康检查、鉴权、驾驶舱设备字段） |
| `client` | 33（`dispatch` / `protocol` / `client` / `mapping` / `route` / `status` / `teleop` / `mapupload`） | 15（`tests/config_tests.rs`：配置优先级、密钥引用、sn/现场默认值、校验与脱敏） |

## 检查命令

```bash
cd business/server && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt -- --check
cd business/client && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt -- --check
cd business/web && pnpm typecheck && pnpm build
```

> Server 与 client 均已做到零告警，可用 `-D warnings`；新增代码请保持零新增告警。
> 协议预留字段（`register.secret`、`TaskProgressPayload.task_id`、`DeviceEventPayload.data`、
> `DeviceSession`、`get_online_devices`、`is_device_online`）以 `#[allow(dead_code)]` 显式标注。

## 重新部署（业务服务 + Web + 客户端）

现场拓扑：业务服务跑在 **192.168.41.227**（开发机，`./target/debug/robodog-ops-server`
`setsid nohup` 启动，日志 `/tmp/robodog-server.log`，端口 3100）；设备客户端跑在
**192.168.41.246**（aarch64 Ubuntu 22.04，无 Rust 工具链、无外网，连外网业务平台
`ws://39.107.55.13:3100`）。

| 改动了什么 | 产物 | 怎么上线 |
| --- | --- | --- |
| `business/server` | `target/debug/robodog-ops-server`（x86_64） | `cargo build` → 重启进程（`kill <pid>` 后 `setsid nohup ./target/debug/robodog-ops-server >> /tmp/robodog-server.log 2>&1`）→ `curl /api/health` 核对 |
| `business/web` | `web/dist` | 本机 `pnpm build` 即可；后端用 `ServeDir` 读盘托管 `../web/dist`，**不用重启服务**，浏览器强刷即可 |
| `business/client` | `robodog-device-client`（aarch64） | `./deploy/build-aarch64.sh` → `scp` 到 `/opt/robodog/client/` → `systemctl restart robodog-device-client` → `journalctl` 看「设备注册成功」 |

注意两点：

1. 本机 glibc 比目标机新时，普通交叉编译会要求 `GLIBC_2.39`（目标机只有 2.35）；
   `deploy/build-aarch64.sh` 已优先用 `cargo zigbuild --target aarch64-unknown-linux-gnu.2.35`，
   并在结尾校验符号上限。若业务服务也要上 aarch64 机器：
   `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc cargo build --release --target aarch64-unknown-linux-gnu`
   （同样要用 zigbuild 才能压在 2.35 以内）。
2. 客户端替换前先留一份现场二进制（`cp -a ... .bak-$(date +%Y%m%d-%H%M%S)`），
   出问题 30 秒内能回滚。
