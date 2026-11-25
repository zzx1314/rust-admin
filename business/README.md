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
├── docs/                          # 接入与设计文档（背包视频、客户端协议、企微告警）
├── server/                        # 独立 Rust 服务（axum + sqlx + SQLite）
│   ├── src/
│   │   ├── auth.rs                # JWT 登录、鉴权中间件
│   │   ├── db.rs                  # 连接池、SQL 迁移
│   │   ├── seed.rs                # 默认管理员与演示数据
│   │   ├── geo.rs                 # 路线里程/耗时/耗电计算、顺序优化
│   │   ├── gateway.rs             # 设备 WebSocket 网关（注册/心跳/下发/控制）
│   │   ├── gateway_types.rs       # 网关消息协议类型
│   │   ├── dispatch.rs            # 任务下发载荷组装（地图绑定/速度/结束动作）
│   │   ├── ai_patrol.rs           # 航点到点 AI 识别（算场景/阈值 → 直连或经客户端调 Agent → 写告警）
│   │   ├── notify.rs              # 企业微信告警推送（去重/限流/截断/重试）
│   │   ├── handlers/              # 园区、区域、设备、任务、路线、排班、工单、告警、设置、驾驶舱
│   │   └── app.rs                 # 路由装配、CORS、静态资源
│   ├── migrations/                # 001 建表 / 002 地图基线 / … / 020 排班与工单 / 023 自定义班次
│   ├── seed/routes.json           # 演示路线（航点/航段/动作）
│   └── data/robodog.db            # 运行时生成，删除后自动重建并重新播种
├── client/                        # 设备客户端：业务协议 + 智航 Pro 本体驱动
│   ├── src/main.rs                # 只做进程装配（配置 → 日志 → 连本体 → 连业务）
│   ├── src/session.rs             # 业务连接主循环：注册/心跳/进度/事件定时器
│   ├── src/body.rs                # 本体链路：建链、断线原地重连、一次性建图
│   ├── src/pump.rs                # 本体推送泵：5009/1315/4001 分发与状态帧处理
│   ├── src/handlers/              # 下行消息：mod.rs 分发 + task/control/command 三个业务分支
│   ├── src/wire.rs                # 业务 WS 线协议（信封 / 心跳 / 进度 / 事件 / 发送）
│   ├── src/state.rs               # 运行时状态与运动门禁（RobotState / ActiveTask）
│   ├── src/maps.rs                # 地图推送/回读、增量同步、中台上传
│   ├── src/locate.rs              # 定位门禁：载图定位、初始位姿、地图匹配
│   ├── src/report.rs              # 上报合流与限频（事件去重 / 进度降频）
│   ├── src/ai_patrol/             # 航点 AI 识别：mod / window / evidence / agent（+ tests）
│   ├── src/dispatch.rs            # 业务载荷 → FlightPlan
│   └── src/zhpro/                 # 智航 Pro 协议层、建图、地图管理、航线执行
└── web/                           # 独立 Vue 3 控制台
    ├── src/api/                   # 请求封装与类型
    ├── src/components/            # 图表、状态徽标、设备抽屉、园区态势图等
    ├── src/layout/                # 原型风格的侧边栏 + 顶栏
    ├── src/views/                 # 9 个业务页面
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

责任人库 / 排班 / 工单**默认留空**（正式环境由运营自己建人排班）。需要带示例人员与班次的
演示库时显式设置 `ROBODOG_SEED_DEMO=1` 再启动，只在责任人库为空时写入一次。

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

> 代理里配了 `ws: true`：观看通道是 WebSocket（`/api/backpack/video/ws`，只做在场计数），
> 不加这项时代理只转发普通 HTTP，视频一路连不上——**页面没画面、后端日志里
> 也一条请求都没有**，很容易误判成后端问题。改完 `vite.config.ts` 需要重启 `pnpm dev`。

### 3. 生产模式（单端口）

```bash
cd business/web && pnpm build
cd ../server && cargo run
```

后端检测到 `business/web/dist` 后会直接托管前端，访问 <http://127.0.0.1:3100>。

### 4. 启动设备客户端（部署在 192.168.41.246）

内置默认值就是正式环境值：业务服务 `ws://39.107.55.13:3008`（外网）、本体 `192.168.234.234:10010`；
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
| 巡检任务 | 三栏编辑器：路线模板、SVG 画布拖拽加点/选段（**滚轮缩放 + 拖动平移**，右上角 `+ / − / 适配`，真实栅格与示意图通用）、航点属性、航段任务、动作库、撤销、顺序优化、校验、发布、模拟执行 |
| 设备管理 | 状态统计、筛选、电量/心跳、手动绑定园区、设备详情抽屉（实时监控 + **手动遥控页内嵌实时画面**）、远程指令、批量下发、CSV 导出 |
| 计划管理 | 看板拖拽改状态、列表视图、任务创建/**编辑**（`PUT /tasks/{id}`，仅「待执行」可改，其余状态先回到待执行）、详情、航点执行序列、开始/暂停、**计划发布状态（草稿 / 已发布：只有已发布的计划才会自动执行定时任务，`POST /tasks/{id}/publish` / `unpublish`）**、**执行实时监控（本体视频 + 地图上的实时位置）** |
| 排班管理 | 周视图（班次 × 一周七天）与列表视图、**班次自定义（默认早/中/夜班，可增删改「白班」「晚高峰班」等，支持跨零点时段、停用历史班次）**、新建排班（责任人/园区/区域/关联计划）、同人同时段冲突拦截、**周视图拖拽复制（把卡片拖到别的日期 / 班次格子即复制一条，不用重填表单）**、**整段复制上一周 / 上个月（预览 + 跳过/覆盖策略）**、责任人负载条、**责任人库增删改（角色/班组/电话/技能/在岗状态，改名同步历史排班与工单）**、**责任人库 Excel 批量导入（下载模板 → 填好上传，逐行回报新增/跳过/失败，重名与工号重复幂等跳过）**、排班详情可直接**修改**（日期 / 班次 / 责任人 / 园区 / 区域 / 关联计划 / 状态 / 备注，改动同样做冲突校验）或「据此派发工单」 |
| 工单管理 | 看板拖拽流转 / 列表视图、优先级与时限、工单详情处理轨迹时间线、追加处理记录、告警与排班转工单（来源保留 AL-/TK- 编号）、CSV 导出 |
| 告警中心 | 级别/状态筛选、单条/全部处理、告警详情；**自动派单**（按当天排班找到当班责任人 → 自动建工单 → 企业微信 @ 到人，见下文「告警自动派单」）、已派单的告警直接「查看工单」 |
| 系统设置 | 平台参数、通知开关、**视频中继默认档位（4G 主用省流档）/ 自动降档开关**、关于平台 |

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
GET    /api/tasks/{id}/recordings  # 任务录像（航点录像 / 全程录像切片）
POST   /api/recordings/ingest      # 设备客户端回传 mp4（multipart：file + meta）
GET    /api/recordings/{id}/play   # 换一个只对这一条有效的 `?pt=` 播放令牌（10 分钟）
GET    /api/recordings/{id}/file   # 播放 / 下载（支持 Range；Bearer 或 `?pt=` 二选一）
DELETE /api/recordings/{id}        # 删除录像（文件 + 记录）
PUT    /api/tasks/{id}/status
POST   /api/tasks/{id}/dispatch     # 立即下发 / 重新下发（人工覆盖，不受发布状态限制）
POST   /api/tasks/{id}/publish      # 发布计划（此后才到点自动执行）
POST   /api/tasks/{id}/unpublish    # 撤回为草稿（停止自动下发）

GET    /api/routes                POST   /api/routes
GET    /api/routes/{id}           PUT    /api/routes/{id}      DELETE /api/routes/{id}
                                  # 只删没人引用的模板：被巡检计划引用 → 400，提示先换绑
POST   /api/routes/{id}/validate
POST   /api/routes/{id}/publish
POST   /api/routes/{id}/optimize

GET    /api/alerts                POST   /api/alerts
GET    /api/alerts/stats
POST   /api/alerts/handle-all
GET    /api/alerts/{id}           PUT    /api/alerts/{id}      DELETE /api/alerts/{id}
POST   /api/alerts/{id}/handle

GET    /api/staff                 POST   /api/staff
PUT    /api/staff/{id}            DELETE /api/staff/{id}
GET    /api/staff/import-template # 责任人库导入模板（.xlsx，含「填写说明」页）
POST   /api/staff/import          # multipart 字段 file：逐行导入，返回新增/跳过/失败明细

GET    /api/shift-slots           POST   /api/shift-slots      # 班次设置：默认早/中/夜班可改，可自定义
PUT    /api/shift-slots/{id}      DELETE /api/shift-slots/{id}  # 有排班在用的班次只能停用，不能删

GET    /api/shifts                POST   /api/shifts
GET    /api/shifts/stats
POST   /api/shifts/copy           # 整段复制：上一周 / 上个月 → 目标区间（按星期 / 日号对齐）
POST   /api/shifts/{id}/duplicate # 单条复制到另一天 / 班次（周视图拖拽用；冲突返回 400 + 原因）
GET    /api/shifts/{id}           PUT    /api/shifts/{id}       DELETE /api/shifts/{id}

GET    /api/workorders            POST   /api/workorders
GET    /api/workorders/stats
GET    /api/workorders/{id}       PUT    /api/workorders/{id}   DELETE /api/workorders/{id}
PUT    /api/workorders/{id}/status   # 状态流转：待接单 → 处理中 → 待验收 → 已完成
POST   /api/workorders/{id}/logs     # 追加处理记录（详情页时间线）

GET    /api/settings              PUT    /api/settings
GET    /api/dashboard/summary

GET    /api/map-versions                  GET    /api/parks/{id}/maps
GET    /api/map-versions/{id}             GET    /api/map-versions/{id}/refs
GET    /api/map-versions/{id}/raster      GET    /api/map-versions/backend          POST   /api/map-versions/import-backend
GET    /api/map-versions/{id}/package     # 地图包原文（设备「下发地图」拉最新包）
GET    /api/map-versions/synced-sources   # 已同步的机器人地图路径（deviceId，兼容 parkId）
POST   /api/map-versions/{id}/publish     POST   /api/map-versions/{id}/new-version
POST   /api/map-versions/ingest          # 自动回传 / 手动同步共用入口

# 中台内部接口（共享服务令牌 x-service-token，不使用业务 JWT）
POST   /api/zentai/devices/{deviceId}/sync-maps   # 中台「同步地图」
POST   /api/zentai/devices/{deviceId}/command     # 中台下发指令
POST   /api/zentai/devices/{deviceId}/tasks       # 中台给设备下发任务
POST   /api/zentai/devices/{deviceId}/push-map    # 中台编辑地图后写回狗体
GET    /api/zentai/devices/{deviceId}/runtime     # 中台读取设备运行态（只读）
```

## 地图写回狗体（中台编辑 → 本体）

中台地图页的橡皮擦只改中台的地图包，机器人本体上的地图文件不会自己更新
（保存后前端会询问「是否立即下发」，点「稍后」就不下发）。要让任务用上新地图，中台调用
`POST /api/dogMap/{id}/push`（反向信道 `push-map` 或 HTTP 直连
`/api/zentai/devices/{deviceId}/push-map`），本服务负责：

1. 按 `dog_map.id` 读中台 `dog_map`（中台是地图主库），拿到最新包；
2. 业务库按 `map_versions.source_id = dog_map.id` 找到对应行（可能有多条园区副本，
   优先设备所属园区那条）——**没导入过业务库就直接 400**，不会凭空造地图；
3. 把最新包暂存到 `[ingest] dir`，刷新这些行的 `file_path` / `file_name` /
   `file_size` / `map_hash` / 宽高 / 原点 / 分辨率，`/api/map-versions/{id}/raster`
   与设备拉包因此都拿到新内容；
4. 向设备下发 `下发地图` 指令（`mapId` / `mapDir` / `sourceRef` / `mapHash` /
   `packagePath`），由设备客户端拉包、写本体文件服务并重新载图（4030）。

前置条件：设备在业务平台在线；地图行上有本体目录名 `source_ref`（设备回传地图时
写入）；`ROBODOG_SERVICE_TOKEN` 与中台一致。设备离线时返回 400 并把台账标记离线，
与「同步地图 / 指令」的语义保持一致。

本体回传的包同时带根目录 `map.pgm`（建图原始地图）与 `map_edit/map.pgm`
（遥控器编辑后的地图）。平台展示、尺寸解析与下发都以 `map_edit/` 为准；
设备侧检测到本体存在 `map_edit/` 时只覆盖 `map_edit/map.pgm` 与
`map_edit/map.yaml` 并读回校验，**根目录的原始地图保持不动**；没有
`map_edit/` 的老固件才退回根目录。`map.pcd` / `map.geojson` / `nav_paths`
始终原样保留；客户端在任务执行 / 建图进行中会拒绝下发。

「下发地图」不是「帧送进设备连接」就算成功：业务平台会先登记 `requestId`
等待客户端执行完的 `command_result`（写本体 + 4030 重新载图），最长 60s。
成功返回设备侧 message；急停 / 任务占用 / 写文件失败 / 超时都会返回 400，
中台审计记 `failed`，前端直接显示原因。

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
| 信号（WiFi / 4G） | **背包板子自己** | 客户端读板端 Agent `GET /api/network`（WiFi 优先，没连 WiFi 用 4G 的 `AT+CSQ` 折算，都是 0-100）→ 心跳 `signal` + `signalSource`；两者都拿不到就不带该字段，页面显示 `—` |
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
**信号不再来自本体，但也不是编的**：协议 39 个字段里没有任何 rssi/csq/wifi/lte 指标，
所以 `devices.signal` 的历史列改由「设备客户端读背包板子的联网信号」填充 ——
客户端调同机 Agent 的 `GET /api/network`（**WiFi 优先，没连 WiFi 时退回 4G**：
WiFi 是 `nmcli` 的 SIGNAL，4G 是 `AT+CSQ` 按 −113~−51 dBm 线性折算，都统一成 0-100），
把 `signal` + `signalSource`（`wifi` / `cellular`）随心跳上报（见
`business/client/src/signal.rs`），页面按 80/60/40 分 优/良/中/差 并标明来源。
**没连 WiFi 也没拿到 4G IP、Agent 没起、老客户端不带该字段时保留库里原值，显示 `—`**：
宁可空着，也不要拿假数字误导「是不是信号差导致掉线」的排查。
充电状态以前是按「电量 ≤ 10% 就判充电中」猜的，现在以本体上报为准
（未绑定园区的设备始终是「待绑定」，执行中的设备不会被心跳改状态）。

两条兜底规则：

- **心跳看门狗**：`device_timeout_secs`（默认 90s，`ROBODOG_DEVICE_TIMEOUT_SECS`）内没有
  心跳的在线设备会被置为「离线」，清空接管态并写一条「心跳超时」告警。
  WebSocket 静默断开收不到 Close 帧，没有看门狗会一直显示「在线」。
- **任务看门狗**：`执行中` 的任务两种情况下会被置为「异常」并记一条事件：
  `task_timeout_secs`（默认 120s，`ROBODOG_TASK_TIMEOUT_SECS`）内没有任何进度上报；
  或者进度数值在 `task_stuck_secs`（默认 600s，`ROBODOG_TASK_STUCK_SECS`）内没有推进
  （客户端还在周期性上报「执行中 / 0%」，只靠「有没有上报」判不出来）。
  另外，进度上报有状态机保护：已完成 / 异常的任务不会被后续「执行中」的上报拉回去。
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
3. **不同园区可以绑定同一张地图基线**：绑定的唯一真相是 `parks.map_version_id`，
   `map_versions.park_id` 只记采集 / 主归属园区。已被别的园区认领的设备地图会以
   「我的设备地图 · 其他园区在用（可共用同一张，不改归属）」列在下拉里，选中保存只写本
   园区的基线，原园区不受影响；授权看「采集设备是否在当前账号管辖范围内」
   （管理员 / 上传人本人 / 中台把该设备分配给当前账号），没有 `device_code`、
   没有上传人的历史 / 演示地图只有管理员能用。新建路线同样可以选共用的基线
   （不指定版本时优先用园区当前基线）；
4. 没绑园区的地图也能建路线——新建路线时选中它同样会认领到该路线的园区。

`GET /api/map-versions` 对非管理员按「中台设备绑定人」过滤（`owner_user_id`），
所以每个人只会看到自己设备采的图；支持 `deviceCode` / `unbound=true` / `ownerUserId` 过滤。

**地图状态跟着中台走**：`map_versions.status` 是入库时的快照（机器人回传一律
「部分标定」），中台「发布」后是「已发布」。`handlers/maps/mod.rs::sync_map_statuses`
按时把 `dog_map.map_status` 拉回来（启动一次、`schedule::sweep` 每 30s、
`GET /api/maps` 顺手拉一次但 10s 节流），园区 `map_status` 一起收敛
（已发布 → 已标定 / 部分标定 → 部分标定 / 其余 → 待标定）。同步只改状态，
不动 `park_id` / `owner_user_id`。

## 地图同步：自动、手动与定时轮询

业务库 `map_versions.source_ref` 保存机器人本体地图绝对路径，ingest 写库前按
`deviceCode + sourceRef` 判重（升级前入库的历史行没有 `device_code`，一并算已同步）。
**地图同步不要求设备绑定园区。**

| 触发 | `mapInfo.mode` | 语义 |
|------|----------------|------|
| 建图 4001 自动回传 | `incremental` | 命中 `sourceRef` 就 `{ skipped: true }`，只新增 |
| 设备空闲时定时轮询（客户端 `--map-sync-interval`，默认 600s） | `incremental` | 先查 `/synced-sources`，只补平台**没有**的地图，绝不覆盖 |
| 中台「设备管理」点「同步地图」 | `refresh` | 全量比对：内容一致跳过，变了就**原地覆盖中台同一条 `dog_map`** |

三条路径都走 `POST /api/map-versions/ingest`；手动同步经内部接口
`POST /api/zentai/devices/{deviceId}/sync-maps`（共享服务令牌）→ 设备 WebSocket
下发 `同步地图` 指令 → 客户端 4012 拉列表后再回传。

### `mode=refresh` 的细节

- **不新建副本**：`mapInfo.id` 带中台 `dog_map.id`，中台按 id 覆盖地图包并重算
  `map_hash` / 宽高，地图列表里不会出现同名多份；
- **内容判据是包内文件指纹**，不是整包哈希：客户端现场重新打包时 zip 时间戳每次都不同，
  用整包哈希会把「没变」误判成「变了」。指纹只看条目名 + 内容，重打包多少次都一样；
- **不可变版本不覆盖**：中台状态为「已发布 / 部分标定 / 已归档」的地图会拒绝覆盖
  （机器人回传的图默认是「部分标定」），同步报告里记为失败并带上中台原话，
  需要先在中台派生新版本再同步；
- **业务地图 id 不变**：刷新复用业务库原 `map_versions.id`，路线 / 任务通过
  `map_version_id` 引用它不会断；
- 中台那张图已被删（`is_deleted = 1`）时不传 `mapId`，按新建恢复，与增量路径一致。

因此自动回传、定时轮询与手动同步交叉执行也不会重复上传/入库。

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
        ◀── request 帧（command / task / runtime / sync-maps / push-map）
        ──── result 帧（ok / error）────▶

② 直连 HTTP（同网部署）
   中台 ──▶ POST /api/zentai/devices/{deviceId}/command（或 /tasks / runtime / push-map）
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
  非管理员只返回自己设备采的地图（`owner_user_id` = 自己，或 `device_code` 属于中台分配给自己的设备，
  管理员可看全部 / 用 `ownerUserId` 指定他人）。同一台狗绑定多个中台账号时，每个账号都能看到它的地图。
- `GET /api/map-versions/backend` 读取仓库根目录 `backend` 的 `dog_map` 地图库，列出已上传的 ROS map_server 地图包；
  支持 `parkCode` 过滤，业务平台可据此只查看某园区的地图。非管理员除了自己名下的地图，
  还能看到**自己管辖设备（中台 `dog_user_device`）上报的地图**，以及中台**分享给「分配给自己的设备」**的地图
  （`dog_map_share`，中台地图页「更多 → 分享设备」），因此同一张图可以直接给多台设备用，不用重新建图。
  根库还没有该表时退回原行为。
- `POST /api/map-versions/import-backend` 把 `{ backendMapId, parkId?, bind }` 指定的地图导入本业务库；
  `parkId` 缺省时按中台地图携带的 `parkCode` 反查业务园区，园区编码未登记会报错。
  导入权限与上面的列表同一口径：自己名下的地图、自己管辖设备上报的地图，或分享给自己管辖设备的中台地图。
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
- 编辑器画布支持**缩放 / 平移**（滚轮缩放、Shift+左键或中键拖动平移，选择模式下
  空白处直接拖；右上角 `+ / − / 适配`）：它们只是一个 `<g transform>`，**不改变落库的
  米制坐标**——所有「屏幕 → 画布」的换算在 `svgCoords` 里先还原这个变换再套下面的
  公式。给新交互加换算时漏还原，放大后落点就会偏。
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
| `mapInfo` | ✅ | JSON 字符串：`{ mapName, parkId, deviceId, bind, mapType, remarks, source, sourceRef, mode }`；`parkId` 可省略（设备未绑定园区时地图先入库为设备地图）；`deviceId` 用于确定地图归属用户、作为中台 `dog_map.device_code`，也是同步判重的主键；`mode` = `incremental`（默认，命中 `sourceRef` 跳过）/ `refresh`（内容有变化才原地覆盖中台同一条 `dog_map`） |

响应：

```json
{ "code": 10200, "msg": "ok", "data": {
    "mapId": "ROS-MAP-3@v1.0", "mapName": "B区-20261110", "parkId": null,
    "zentaiMapId": 12, "bound": false, "mode": "incremental", "replaced": false,
    "stashPath": "data/map_ingest/B区-20261110_20261110153000.zip",
    "map": { } } }
```

`mode=refresh` 且包内容与平台副本一致时返回 `{ "skipped": true,
"reason": "地图包内容未变化，平台副本已是最新" }`，不会落盘也不会推中台。

处理流程：

```
1. 校验 zip 魔数 PK、大小；parkId 存在时校验园区存在（缺省则入库为设备地图）
2. stash_package → data/map_ingest/ 留档（中台不可用时凭此路径重放）
3. parse_local_meta → 本地解析 model.pgm 宽高 / model.yaml resolution+origin
4. push_to_zentai → 共享令牌 POST /api/dogInternal/maps/upload（`mode=refresh` 时 mapInfo 带 `id`，中台原地覆盖；无令牌时退回账号登录 + /api/dogMap/upload）
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

## 航点 AI 存证图回传（「同步媒体」）

板端每一轮航点 AI 识别会固定保存 N 帧到 `/media/media-data/picture/<计划>/pt<航点>/`
（**原始帧、不画框**，中台当训练数据用；临时存证 `detect_jobs/` 是另一份带框的告警
证据，企业微信推完即删）。让图进中台「媒体资源」：

```text
中台设备页「同步媒体」
  → 业务平台 push_command（白名单 BUSINESS_COMMANDS，原样透传 params + requestId）
    → 设备 WS：command=同步媒体（不需要本体在线：客户端只读本地磁盘 + 上行）
      → 客户端 media_sync 扫 <pictureDir>/<plan>/pt<n>/*.jpg（**整个目录全传**），
        每包 8 张 / 8MB 打成 zip
        → POST /api/media/ingest（multipart：file=zip + mediaInfo.entries）
          → 转发中台 POST /api/dogInternal/media/upload（x-service-token）
            → 中台解包，**一条一条** dog_media + <upload_dir>/media/<设备>/<计划>/pt<n>/<sha256>.<ext>
              → 回 results[]，客户端只删 ok 的条目
                → 中台媒体资源页（下载：GET /api/dogMedia/{id}/file，批量 /export）

设备 → 平台的状态回报（设备页「媒体同步状态」看的就是这条）：
      command_progress（每传完一包：已上传 8/24 张）
        → POST /api/dogInternal/media/sync-status（status=running，仍 pending）
      command_result（最终：扫了几张、删了几张、目录是否清空）
        → 同上 status=succeeded / failed + detail{drained,remaining,…}
          → 中台写 dog_command_audit（requestId 与下发时一致）
            → GET /api/dogDevice/{id}/mediaSyncStatus 供页面轮询
```

压缩包规则：`Stored`（JPEG 已压缩，再 deflate 只烧板子 CPU）；包内名是纯 ASCII
`0001.jpg…`（中文名在不同 unzip 下会乱码），原名放 `entries[].remarks`；
中台只处理 `entries` 里**声明过**的条目（白名单，顺便挡掉 zip 里的路径穿越），
单张上限 8MB、整包 64MB、一次最多 100 张。业务平台不解包，只验形态/大小并转发。

要点：

- **目录 / 帧数 / 采集开关只在中台「参数配置」配**（`media_picture_dir` /
  `media_frames` / `media_capture_enabled`）。
  业务平台在组装 `ai_start` 时通过 `GET /api/dogInternal/media/config` 取一次
  （60s 缓存，中台不可达就沿用上次值/默认值，**不能**让 AI 停摆），随指令下发
  `mediaDir` / `mediaFrames` / `captureEnabled`；「同步媒体」指令再带同一份目录与帧数。
  两边目录必须一致，否则会传空。
- **采集默认关**（`media_capture_enabled=0`，迁移 34）：关着时板端只写告警证据图
  （`detect_jobs/…`，企业微信配图与数量核对照旧），不往媒体归档目录攒训练图，
  「同步媒体」自然扫不到新图。要攒训练数据就在中台把它改成 `1`（最多 1 分钟生效）。
  老中台没有这个字段 / 取配置失败时按关处理（`MediaCaptureConfig::default().enabled
  == false`）；老平台没下发 `captureEnabled` 时板端按开兜底（历史行为不变）。
- **回传以「清空目录」为准，不按每航点帧数截断**：同一个 `pt<n>/` 每跑一轮就多
  `media_frames` 张（文件名带时间戳），按张数截断会让板端永远传不干净
  （2026-11-20 现场：点完「同步媒体」审计说删了 6 张，目录里还躺着 6 张）。
  单轮仍受 200 张上限约束，超了会回执「板端仍有 N 张未回传」，再点一次即可。
- **按条目删板端文件**：中台按图片 sha256 去重，`created=false`（重复图）也算成功；
  只有 `results[].ok=true` 的那几张会被删，失败的留到下次。连续失败 3 张中止本轮。
  跑完还会清掉「只剩 `result.json`」的空航点目录与空计划目录。
- 体量上限：单张 8MB、压缩包 64MB（`MEDIA_UPLOAD_LIMIT`，axum 默认 2MB 会直接 413/400），
  只收 JPG/PNG 且校验文件头；业务平台两种形态（zip / 单张）都接受。
- 「同步媒体」是长任务：`push_command` 只保证「已下发」，中台审计**停在 pending**
  （`awaitingDevice=true`），真实进度/结论由客户端 `command_progress` /
  `command_result` 经业务平台回写 `POST /api/dogInternal/media/sync-status`。
  设备页通过 `GET /api/dogDevice/{id}/mediaSyncStatus` 轮询；pending 超过 10 分钟
  没有回报就显示「无回报」，允许重新下发（同一 `requestId` 在跑时不会重复下发）。

## 巡检录像（航点「录像采集」/ 航段「全程录像」）

路线编辑器里有两类录像动作：

| 动作 | 挂在哪 | 参数 | 行为 |
|---|---|---|---|
| 录像采集 | 航点动作 | `recordSeconds`（3~300，默认 15）+ `recordCameras` | 到点开录，**离开航点就停**（不足时长也提前收尾） |
| 全程录像 | 行进任务（`legs[].actions`） | `recordSegmentSeconds`（30~1800，默认 300）+ `recordCameras` | 任务一开始就录，跨航点一直录到终态 / 取消，按切片长度分段 |

**录哪一路可以多选**（`recordCameras: ["dogcam","cam0"]`，2026-11-25 起）：
`dogcam` = 正前（机器狗前置）、`cam0`~`cam3` = 背包四路 USB（左前 / 右前 / 右后 /
左后）、`dog-ipcam` = 红外；**选几路就同时录几路**（每路一个 ffmpeg + 一个 mp4）。
不配 / 老数据 = 板端默认源（`RECORD_SOURCE_CAM`，默认 dogcam）；超过
`RECORD_MAX_CAMERAS`（默认 4）只录前几路并在回执里说明。相机名与航点拍照同一份，
板端不认识的名字只丢那一路，**不会整条录像失败**。

链路（录像本身在板端，平台只管存与回放）：

```text
路线动作 → task_dispatch 的 routeData（原样下发）
  → 设备客户端在 5009 状态机上起停（`client/src/record.rs`）
    → 板端 Agent POST /api/record/start|stop（`backpack_dashboard/backend/record.py`）
      → 每选一路一个 ffmpeg，从本机 mediamtx 对应流拉流（-c copy，不重编码）
        → RECORD_DIR/<计划>/<pt?-相机-时间戳>.mp4（文件名带相机名）
        （**就攒在这里，任务终态 / 客户端启动都不自动回传**）
        → 平台下发指令「回传录像」：GET /api/record/list → POST /api/recordings/ingest（multipart）
          → 平台落盘 <recording_dir>/<设备>/<任务号>/…mp4 + 一条 task_recordings（带 camera）
            → 上传成功才回调 /api/record/delete 删板端文件
              → 任务详情「任务录像」播放 / 下载（`<video src>` 直连 Range 流）
```

播放为什么不走 Blob：`<video src>` 带不上 `Authorization`，所以前端先调
`GET /api/recordings/{id}/play` 换一个**只对这一条有效、10 分钟过期**的 `?pt=` 令牌，
再把这个地址交给 `<video>`（后端 Range 流，秒开、可拖进度条）。以前是「整段 37~64MB
先取成 Blob 再 `createObjectURL`」：弱网下几十秒到几分钟只有一行「正在取回录像…」，
axios 还有 120s 硬超时，现场表现就是「点回放没反应」（2026-11-24 修）。

要点：

- **起停时机跟 5009**：航点录像在 `patrol.point_arrived` / `patrol.waiting` 起，
  离开航点（`patrol.navigating` 等）停；全程录像在这条任务的第一个非终态帧起，
  终态 / 取消 / 本地收尾停。和航点 AI、灯光同一套收口规则（见 `AGENTS.md`）。
- **多路相机**：`recordCameras` 里每一路是独立 job（key 带相机名），**停按时按
  plan+scope+point 收**，所以离开航点会把这一点的所有相机一起停；某一路上游没推流
  只丢那一路（回执 `errors` / `rejected`），其余照录。任务详情「任务录像」按
  `camera` 区分（老数据默认 `dogcam`）。
- **幂等**：5009 每 500ms 一帧，客户端按「(任务, 航点) 正在录 / 全程已在录」去重；
  Agent 侧同一 `planId+scope+pointIndex` 重复 start 返回同一个任务。
- **失败不影响巡检**：板端没升级（404）/ 磁盘不足 / ffmpeg 起不来都只打日志，
  客户端记为 `unsupported`，本轮不再尝试；到点、上报、AI、灯光全部照旧。
- **全程录像切片**：4G 上一次传几百 MB 必然断，所以按 `RECORD_SEGMENT_SECS`（板端）
  切 mp4，客户端逐段上传；任何一段失败只影响那一段（失败的文件留 72h 等下一次）。
- **回传是平台按需的**（2026-11-25 起）：任务终态 / 取消 / 客户端启动都**不再自动上传**，
  mp4 就攒在板端 `RECORD_DIR/<计划>/`，等设备指令「回传录像」再传（中台设备页
  「下发指令 → 回传录像」，进度 / 结论看「录像回传状态」）。理由：现场主用 4G，
  一场巡检几百 MB，终态就传会和下一轮的地图 / 状态上报抢带宽，而那时多半没人看录像。
  同一时间只跑一轮（重复下发回「上一轮还在跑」）；**上传成功才删**板端文件，失败的留到
  下一次（计划已被删掉的那种会一直失败，只能手工清板端目录）。
  ⚠️ 板端只保留 `RECORD_RETENTION_HOURS`（默认 **72 小时**）：超过就由 `sweep()` 清掉，
  所以「想看再传」要在 72 小时内点（要放宽就改板端这个环境变量）。
- **谁读板端磁盘**：客户端与 Agent 同机，`/api/record/list` 回绝对路径，客户端直接读文件
  上传；Agent 不对外提供 mp4 下载（公网平台够不到板端）。
- **平台侧留存**：`recording_keep_days`（默认 30，`0` = 不清理）由 `schedule::sweep`
  每分钟顺手清超期的行 + 文件；平台只存**相对路径**，接口从不回绝对目录。
- **计划删了，已录的片子还在**：`task_recordings` 不跟 `tasks` 级联删，所以删计划后
  那几行会变成「孤儿」。可见性判定先按任务归属，任务已不存在就退到录像自己的园区
  （`ensure_recording_visible`），列表 / 播放 / 删除都还能用——否则它们会变成
  谁都打不开也删不掉的 404（2026-11-24 `TK-1124-17` 现场，6 段全锁死）。
  超期后两条路都会把它们清掉（`sweep_expired`）。
- **为什么不进中台「媒体资源」**：那边是 AI 存证**训练图**（JPG/PNG、按 sha256 去重、
  打包导出），视频是任务执行结果，体积 / 留存 / 使用方式都不同。
- 板端目录 / 开关：`[backpack]` 之外的 `RECORD_*` 环境变量见
  `business/backpack_dashboard/AGENTS.md`「录像」一节；客户端可用 `--no-record`
  或 `ROBODOG_RECORD_ENABLED=0` 整体关掉。

## 航点拍照（航点动作「可见光拍照」）

路线编辑器里给航点配「可见光拍照」，跑到该点由**板端相机**抓一张（或多张）JPEG，
任务结束后回传到平台，任务详情「任务照片」里看 / 下载 / 删除。动作字段：

| 字段 | 默认 | 说明 |
|---|---|---|
| `photoCamera` | `dogcam` | 抓拍源：`dogcam` 机器狗前置可见光 / `cam0`~`cam3` 背包四路 USB / `dog-ipcam` 红外 |
| `photoFrames` | `1` | 抓几张（1~5）；多帧按板端 2fps 拉开，避开运动模糊 |

链路：

```text
到点（5009 patrol.point_arrived / waiting）→ 客户端 POST 板端 /api/photo
  → 板端 ffmpeg 从本机 mediamtx 抓 JPEG → PHOTO_DIR/<计划>/pt<n>/
    → 任务终态 / 取消 / 客户端启动：GET /api/photo/list
      → POST 平台 /api/photos/ingest（multipart，sha256 去重）
        → 落 <photo_dir>/<设备>/<任务>/pt<n>/ + 一条 task_photos
          → 上传成功才 POST /api/photo/delete 删板端文件
            → 任务详情「任务照片」（缩略图按需加载 / 大图 / 下载）
```

要点：

- **为什么不用本体的相机**：本体 `tasks=[1]` 走的是上装 `/capture_image`，服务没起就
  `14005` 直接失败，而且本体没实现 5010/5011 记录查询（拿不到 `image_path`）。
  背包相机本来就在跑（实时视频 / AI 抽帧同一路），抓图不依赖任何新协议；
  本体侧行为保持不变。
- **到点同步抓**：停留只有几秒，晚一拍拍到的是下一段路；单次抓拍实测 0.5~2s，
  客户端 12s 超时兜底，同一航点只拍一次（5009 每 500ms 一帧）。
- **失败不影响巡检**：流没在推 / ffmpeg 缺失 / 磁盘不足只记日志；
  老版本 Agent（404）本轮不再试。
- **回传成功才删板端文件**：失败留在板端，下一轮任务 / 重启接着传
  （`--no-photo` / `ROBODOG_PHOTO_ENABLED=0` 可整体关掉）。
- **留存**：平台 `photo_keep_days`（默认 90 天，`0` = 不清理）；板端
  `PHOTO_RETENTION_HOURS`（默认 72h）兜没传走的。
- **不进中台「媒体资源」**：那边是 AI 存证的训练图（sha256 去重、批量导出），
  巡检照片是任务结果（与录像同一理由）。
- 设计 / 代码地图 / 排查：`business/docs/waypoint-photo.md`；
  板端参数见 `business/backpack_dashboard/AGENTS.md`「航点拍照」一节。

## 巡检语音播报（航点动作 / 行进任务）

路线编辑器里可以配「语音播报」：航点动作 = **到点播报**，行进任务 = **进入该航段时播报**
（离开航段 / 终态 / 取消停）。话术与时长都在动作上（`speakText` / `speakSeconds`，
`0` = 只说一次，`>0` = 循环播这么久）。链路：

```text
路线动作 speakText / speakSeconds → task_dispatch 的 routeData（原样下发）
  → 设备客户端在 5009 状态机上起停（`client/src/speak.rs`）
    → 板端 Agent POST /api/speak|/api/speak/stop（`backend/speak.py`）
      → 聆思 TTS：call 0 <内容> 单次 / call 1 <内容> 循环 / call stop 回到 voice
```

要点：

- **TTS 在云端**：需要背包联网 + 板端 `asr` 服务（Agent 会先 `ensure_asr()`）。
  断网 / asr 起不来的播报一律跳过，只打一行日志，**巡检继续**。
- **内容限制来自板端固件**：不能有空格，只认中文汉字 + 中文标点 + 数字，
  中文 ≤30 字、总长 ≤40。编辑器保存时硬拦，板端运行期只做「丢非法字符 + 截断」。
- **循环播报一定要收尾**：客户端在离开航点 / 离开航段 / 终态 / 取消发 `stop`；
  客户端掉线时由板端看门狗（`SPEAK_MAX_SECS`，默认 600s）兜底。
- **一次只播一条**：板端 FIFO 单通道，新播报会打断旧的（Agent 每次先 `stop` 再起）。
  同一个点既有航点播报又有航段播报时，**航点优先**。
- 开关：客户端 `--no-speak` / `ROBODOG_SPEAK_ENABLED=0`；板端参数
  `SPEAK_MAX_CHARS` / `SPEAK_MAX_LEN` / `SPEAK_MAX_SECS`。
- 设计、代码地图与排查见 `business/docs/plan-speech.md`；板端行为见
  `business/backpack_dashboard/AGENTS.md`「语音播报」一节。

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
                    "threshold": "> 65℃ 生成告警", "retry": 2 },
                  { "type": "录像采集", "param": "1080P / 25fps / 15s",
                    "recordSeconds": 15 }] }
  ],
  "legs": [
    { "mode": "沿道路", "speed": 1.2, "avoid": "标准",
      "actions": [{ "type": "全程录像", "param": "1080P / 25fps / 连续",
                    "recordSegmentSeconds": 300 }] }
  ],
  "returnLeg": { "mode": "沿道路", "speed": 1.5, "avoid": "标准", "actions": [] }
}
```

后端在保存时重新计算 `point_count`、`distance_m`、`duration_s`、`battery_pct`，
校验与发布接口直接读取该 JSON。

录像动作的两个参数字段（`recordSeconds` / `recordSegmentSeconds`）只是**记录配的时长**，
真正起停由客户端按 5009 状态机调板端 Agent，见上文「巡检录像」。老数据没有这两个字段时
客户端从 `param` 文本里读秒数（默认航点 15s / 切片 300s）。

控制台「路线编排 → 属性配置」里，已配置的航点动作和行进任务卡片上都有**修改**按钮：
点开复用同一个动作表单（类型 / 执行参数 / 场景与类别 / 检测类型与数量 / 触发时机 /
阈值 / 重试次数），改完**原地覆盖那一条**，不重排顺序、不产生重复记录，撤销栈里只留
一条快照。要换类型或删掉重配也行，改完记得点「保存」把草稿写回路线。

## 航点净空与「原地待命」的停车位

`endTaskMode = none`（原地待命）时，**最后一个航点就是停车位姿**（本体
`5000 kind=4` 的 `params.end_task = 0` 就是「停在最后一点」）。本体规划器把
目标判进占据/膨胀层时**会正确地拒绝出轨迹**（`13377 无有效绕障轨迹` /
`13446 目标被障碍物占据`），5009 于是停在「前往 P{n}」，既不终态也不报错：
平台一直「执行中」、灯不灭、下一个调度窗口被占着。

我们这边三处缺口已补：

| 位置 | 行为 |
| --- | --- |
| `server/src/handlers/maps/clearance.rs` | 从地图包的 `map.pgm` 算欧氏净空距离场（只认占据格，未知格不算障碍）；按 `map_id\|file_path` 缓存，图与 `map.yaml` 几何对不上就放弃 |
| `POST /routes/{id}/validate` | 新增「终点停靠净空」「航点净空」两项，建议 ≥0.5 m（现场实测阈值） |
| `POST /routes/{id}/publish` | 任一点 <0.15 m → 拒绝发布；终点 <0.5 m → 允许但在 `task_events` 留痕 |
| 任务下发（`handlers/tasks.rs`） | 结束动作真的会停在最后一点（显式原地待命 / 跟随路线 + 往返单向）且终点 <0.5 m → 时间线先留一行提醒 |
| `client/src/zhpro/route.rs` | 只剩最后一个点 + 150 s 内 3 次规划类故障（`13377/13446/13440`）→ 提前报「终点进不去」并给出改法，不再干等 `180 s/点` |

厂商侧目前**没有**巡检（kind=4）的到点容差（`goal_tolerance` 只在 `kind=2` 文档里，
`4024` 路径点只有 `type/tasks`），所以「差 0.3 m 就收尾」做不到，长期解是向厂商要
巡检容差。设计、判据、下一步（停车点与巡检点解耦）见
**`docs/waypoint-clearance.md`**。

## 责任人库 Excel 导入

排班管理页 →「责任人库」右上角两个按钮：**下载模板** / **Excel 导入**。
正式环境一次上线几十个人时，不要一个个手填。

### 代码地图

| 文件 | 职责 |
| --- | --- |
| `server/src/xlsx.rs` | 极简 xlsx 读写（写模板 / 读上传件），只依赖已有的 `zip` crate |
| `server/src/staff_import.rs` | 模板列定义 + 表头别名识别 + 行级校验 |
| `server/src/handlers/schedules.rs` | `import_staff`：查重（库里 + 文件内）、自动编号、逐行落库 |
| `web/src/views/schedules/index.vue` | 导入弹窗：下载模板 → 选文件 → 逐行结果 |

### 模板与填写规则

模板两张表：`责任人`（表头 + 列宽 + 冻结首行，由用户填）与 `填写说明`。
列固定为 **姓名** / 工号 / 角色 / 班组 / 联系电话 / 在岗状态 / 技能：

- **姓名必填**，其它选填；`在岗状态` 只能是 `在岗 / 休息 / 请假`（留空按在岗）；
- 工号留空时按 `EMP-<MAX(id)+1>` 自动生成（与手动新增一致），被手填占用则顺延；
- 导入按**表头名**认列（`姓名/责任人/负责人`、`电话/手机号` 等别名都认），列顺序可调、多余列忽略；
- 只读**第一张工作表**，空行自动忽略，一次最多 500 行、文件 ≤ 4MB（`staff_import::MAX_ROWS` / `MAX_FILE_BYTES`）。

### 导入语义

逐行处理，坏行不拖累好行，结果按 Excel 行号回报：

| 结果 | 触发条件 |
| --- | --- |
| 新增 | 通过校验且姓名、工号都不冲突 |
| 跳过（`skipped`） | 文件内重名 / 库里已有同名 / 工号重复——**不算错**，重复导入同一份文件是幂等的 |
| 失败（`failed`） | 姓名为空、在岗状态不认识、字段超长（姓名 32 / 其它 64 字） |

新责任人按固定调色板轮换头像底色。导入后会自动刷新责任人库与周视图（负载条要算新人）。

> 为什么不引 `calamine` / `rust_xlsxwriter`：场景固定——模板由平台发、列结构不变、
> 只用到「表头 + 字符串单元格」，而 xlsx 就是 zip + 几个 XML。读的时候兼容
> `sharedStrings` / `inlineStr` / 数字（Excel 把 `13800000000` 存成 `1.38E10`）四种形态，
> 单测里有一份 openpyxl 生成的真文件（`server/tests/fixtures/`）做回归。

## 企业微信告警推送

**执行计划期间**（设备有执行中的任务）的告警——本体故障/障碍、AI 识别检出/数量核对、
以及任务被判「异常」——推到企业微信群机器人。掉线、控制权被抢、页面手动新建的告警
**只落库不进群**（告警中心照常可见）。实现见 `server/src/notify.rs`，记录落 `notifications` 表。

> **本节是使用手册。改这个功能之前先读
> [`docs/wecom-alert-push.md`](docs/wecom-alert-push.md)** —— 那里有架构图、
> 代码地图、去重/限流参数、触发点清单、扩展点（多群路由 / 恢复通知 / 中台代发）、
> 踩坑记录与部署清单。

### 配置

webhook 在企业微信**客户端**里创建（官方没有建机器人的 API）：

> 群聊 → 右上角「···」→ 添加群机器人 → 新建 → 复制 Webhook 地址

拿到的地址形如 `https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=xxx`。

**推荐做法：直接在平台上填。** 控制台「系统设置 → 企业微信群机器人」粘贴地址 →
「保存地址」→ 点「发送测试消息」验证。保存**立即生效**，不用重启服务；
页面只回显掩码（`…?key=94fdb35e…`），完整地址不会回到浏览器。

```text
GET  /api/notifications/webhook        # 状态：configured / source(db|env) / masked / enabled
GET  /api/notifications/webhook?reveal=true   # 管理员取一次原文（页面「显示原文」）
PUT  /api/notifications/webhook        # {"webhook":"https://…?key=…"}，传空串 = 清除
POST /api/notifications/test           # 同步发一条，失败直接回 errcode
```

页面上的「显示原文」= 带 `?reveal=true` 的读取：默认只有掩码，**想找回上次配的完整
地址**才点它（仅管理员可用，日志里记「谁看过」但不记地址）。输入框留空不会清除配置，
只有粘贴新地址并保存才会覆盖。

地址存 `settings.wecom_webhook`（`value_type = secret`，所以**不出现在**通用
`GET /api/settings` 列表里，`PUT /api/settings` 也不接受它）。

**部署时的环境变量仍然有效**，优先级是 **平台配置 > 环境变量**：

```bash
# 想「锁死」不让页面改，就只放环境变量；不要写进 server.toml（文件会进 git）
export ROBODOG_WECOM_WEBHOOK='https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=xxx'
export ROBODOG_PUBLIC_URL='http://39.107.55.13:3008'   # 可选：消息里的链接
cargo run
```

> 地址就是全部凭据（没有签名校验，谁拿到谁能往群里发消息）：接口只回掩码、
> 日志只打 key 末 6 位；数据库文件 / 备份同样要按凭据对待。

失败时页面直接显示企业微信的 `errcode`（如 `93000 invalid webhook url`）。
不点按钮也可以先 `curl` 验证：

```bash
curl -sS "<webhook 地址>" -H 'Content-Type: application/json' \
  -d '{"msgtype":"text","text":{"content":"测试"}}'
```

### 三个层级都得放行

| 层 | 位置 | 作用 |
| --- | --- | --- |
| 部署 | `wecom.enabled` + webhook（平台设置页或 `ROBODOG_WECOM_WEBHOOK`） | 这套部署有没有企业微信通道 |
| 范围 | 设置页 `alert_only_in_task`（默认开） | **只有执行计划期间的告警**才建单 + 推群 |
| 运营 | 设置页 `alert_push` / `task_timeout_notice` | 站内提示总开关 |
| 渠道 | 设置页 `wecom_alert_push` / `wecom_task_abnormal` | 这一类事件推不推企业微信 |
| 跟单 | 设置页 `wecom_alert_repeat_push`（默认开） | 同一问题已挂在在办工单上时，再次告警还推不推 |
| 证据图 | 设置页 `wecom_alert_image`（默认开） | 带证据图的告警是否多发一条 `msgtype=image` |
| 白名单 | 设置页 `alert_ignore_fault_codes`（默认 `12608,13440`） | 命中的本体故障码**根本不落告警**（也不建单、不推群） |

全部都要为真才发。改设置页上的开关立即生效（每次入队都会读库），不用重启。

### 会推什么

| 触发 | 是否执行计划期间 | 级别 |
| --- | --- | --- |
| 设备上报 `error` / `fault` / `obstacle`（`handle_device_event`） | 仅任务跑着时推 | `error` → 严重；其余看 `severity` |
| AI 识别检出 / 数量核对 / 未检出（`ai_patrol`） | 推 | 警告 |
| ↳ `count` 任务（数量核对）的检出 | **只进任务时间线**（不落告警、不推群） | — |
| ↳ 数量核对不一致的**证据图**（板端抓拍） | 推（卡片之后的图片消息） | 警告 |
| ↳ 数量核对**一致**（动作勾了 `notifyWhenCountOk`） | 推（**只发群，不落告警、不建工单**） | 提示 |
| 客户端上报任务 `异常` | 推 | 警告 |
| 任务看门狗判异常（`sweep_stuck_tasks`） | 推 | 设备离线时**严重**，否则警告 |
| 心跳超时被看门狗置离线（`sweep_stale_devices`） | **只落库** | — |
| `1315 ctrl_source=3` 控制权被抢（`open_control_preempt_alert`） | **只落库** | — |
| 页面上手动新建告警 | **只落库**（告警中心里可人工转工单） | — |

「只落库」= 告警中心看得见、能筛选、能转工单，但**不自动建单、不进群**，
也不回写 `assignee` / `work_order_id`。

急停解除、控制权收回这类「状态反转」**不发**消息（避免刷屏），只在告警中心自动结案。

### 告警白名单（忽略的故障码）

有些本体故障码是纯噪音：报出来既不值得派人处理，也不该进群。设置页
「通知策略 → 告警白名单」填的就是这类码（`notification.alert_ignore_fault_codes`，
逗号分隔，直接填数字）：

| 故障码 | 现场表现 | 为什么压掉 |
| --- | --- | --- |
| `12608` RS 与 lidar 时间不同步（级别 1） | 机器人每 ~90s 报一次 | 一直是这个码，处理的是根因工单，不是它本身 |
| `13440` 导航指令非法（级别 2） | 一定跟着 `13897 capability busy` | 看根因那条（控制权 / 能力占用），它是派生现象 |

命中白名单的故障**不落 `alerts`、不自动建工单、不推企业微信**，
只在 `task_events` 事件流里留一行（排查时还能证明「本体确实报过」）。

> 事件流那行不能一起省：`ctrl_source=3` 的文案判断（`gateway::recent_capability_busy`）
> 正是翻 `task_events` 里的 `13440` / `13897` 才能区分「本体能力被占」和「遥控器被接管」。

- 默认值就是上表两条；**留空 = 全部告警照旧**（不会因为「没配」就静默）。
- 只认码值，不认文案：取不到 `code=` 的事件（人工事件 / 翻译文案）一律不过滤。
- 改完点「保存配置」立即生效（每次事件读一次库），不用重启。
- 回归用例：`server/tests/alert_whitelist.rs`。

### 企业微信的硬约束（代码已处理）

| 约束 | 处理 |
| --- | --- |
| 20 条/分钟/机器人 | 本地令牌桶限流 18 条/分钟，多余的排队（不丢） |
| markdown 正文 ≤ 4096 **字节**（中文 3 字节/字） | 按字节截断并补「…（已截断）」，不切半个汉字 |
| markdown 不支持 @提及 | `mention_all` 时**另发一条 text** 带 `mentioned_list: ["@all"]` |
| 图片消息：原图 ≤2MB、仅 JPG/PNG、`base64` + **原图** md5 | `notify::evidence_image()` 统一校验（客户端先按文件头/大小筛一次），超限就只发卡片 |
| 只能发不能收 | 消息里带「查看详情」链接，处理动作仍在控制台 |
| 同一件事会被反复"发现"（看门狗 15s 一轮、客户端 5s 一报） | 5 分钟去重窗口，键为 `alert_code` / `task_code` |
| 外网 HTTP 可能超时 10s | 入队是同步的 `try_send`，**绝不在 WS handler / 看门狗里 await 外网** |
| 队列积压 | 60s 汇总一条「另有 N 条未逐条推送」，不静默丢 |

### 排查

```sql
-- 最近推送：失败行 last_error 带企业微信的 errcode + errmsg
SELECT created_at, status, level, ref_type, ref_id, attempts, last_error
  FROM notifications ORDER BY id DESC LIMIT 20;
```

控制台接口：`GET /api/notifications?limit=20&status=failed`、`POST /api/notifications/test`、
`GET/PUT /api/notifications/webhook`（webhook 状态与配置，见上文「配置」）。

常见错误码：`93000` key/地址无效（机器人被删或复制不全）、`45009` 超频、
`40058` 内容超长（理论上已被截断兜住）、`connection refused / timeout` 服务器出不了公网。

## 告警自动派单（责任人 → 企业微信 → 工单）

计划（`tasks`）本身没有「谁负责」，责任人只存在于计划层的排班里
（`shifts.task_id / park_id / zone_id → staff_id`）。所以告警落库后按**当天**排班找
最匹配的那一条，再建单、再推群，链路只在一处：`server/src/alert_flow.rs`
（由 `notify::alert_created` 统一调用）。

### 只有「执行计划期间」的告警才建单 + 推群

`notify::alert_created` 的第一道门是 `alert_flow::should_dispatch`：判据是**告警绑定的
设备当时有 `执行中` 的任务**（`alert_flow::in_task_execution`）。

| 告警来源 | 执行中 | 结果 |
| --- | --- | --- |
| 巡检中本体故障 / 障碍 / 急停（`AL-WS-*`，`fault:`） | 有 | 建单 + 推群 |
| 航点 AI 识别检出 / 数量核对 / 未检出（`ai:`） | 有 | 建单 + 推群 |
| 设备心跳超时掉线（`AL-HB-*`，`hb:`） | 无 | **只落库** |
| 控制权被遥控器抢占（`AL-CTRL-*`，`ctrl:`） | 无 | **只落库** |
| 页面手动新建（`POST /api/alerts`，未绑定设备也算） | 无 | **只落库** |

「只落库」= 告警中心照常能看到、能筛选、能人工转工单，但**不自动建单、不进企业微信群**
（`assignee` / `work_order_id` 也不回写）。这样夜里设备掉线或遥控器抢占不会往群里刷，
值班的人只需要盯「巡检时发现的问题」。

开关：设置页「只为执行计划期间的告警建单 + 推群」（`notification.alert_only_in_task`，
**缺行视为开启**）。关掉就退回老口径（所有告警都建单 + 推群）。
回归用例：`server/tests/alert_in_task.rs`。

### 派单规则（打分取最高）

| 匹配 | 加分 | 说明 |
| --- | --- | --- |
| 同一条计划 | +40 | 告警设备上正在执行（`执行中` 优先于 `已暂停`）的任务 == 排班的 `task_id` |
| 同一区域 | +20 | 告警设备所在区域 == 排班的 `zone_id` |
| 同一园区 | +10 | 告警园区（`alerts.park_id`，缺省取设备园区）== 排班的 `park_id` |
| 已发布 | +4 | 排班状态，草案不参与匹配 |
| 关联责任人库 | +2 | 排班只有姓名（责任人被删）时次之 |
| 在岗 | +1 | `staff.status = 在岗` |

分数为 0 视为没匹配上，按 **园区负责人（`parks.owner_name`）→ `未指派`** 兜底；
兜底也会建单，让值班长在看板上改派，而不是把告警闷掉。

### 建单与推送

- 工单：`source = 告警自动派单`、`sourceRef = 告警编号`、`alertId` 回指告警，
  优先级 `严重→紧急 / 警告→高 / 提示→中`，时限沿用默认 2 小时；
  一条告警只派一单（`alerts.work_order_id` 守卫，页面再点「转工单」会被告知已有单号）。
- 告警行回写 `staff_id / assignee / assignee_note / work_order_id`，告警中心与详情
  直接显示「当班责任人 / 派单依据 / 工单号」。
- 群消息在原有字段上多三行「关联计划 / 当班责任人 / 工单」，并附一段处置建议；
  责任人库里有手机号时**另发一条 text** 用 `mentioned_mobile_list` @ 到人
  （群机器人 markdown 不支持提及）。
- **重复告警不重复建单、不重复吵群**：同一设备、同一件**结构化去重键**（`alerts.dedup_key`：
  AI 用 `ai:<设备>:<任务>:<航点>:<类别>`，本体故障用 `fault:<设备>:<故障码>`，
  心跳/控制权用 `hb:` / `ctrl:`）、24 小时内已有在办（未「已完成」）的自动派单工单时，
  新告警直接挂到那张单上（`alerts.work_order_id` 指向老单），**不再建新单**。
  没带键的来源（人工告警）退回「标题相等」的旧行为。
  现场实测过：机器人每 ~90s 报一次 `12608 RS 与 lidar 时间不同步`，
  不去重的话 5 分钟就是 2 张工单 + 2 条群消息。AI 告警尤其需要这个：内容里带置信度
  （0.41 → 0.77）和证据图路径，按标题比等于没去重，每轮巡检都会攒一张新单。
- **跟单告警照样推群**（默认）：挂回在办工单的告警在群里会再出现一条
  「设备异常告警（跟单：同一问题第 N 次）」，字段里写明「工单 WO-…（原工单，未新建）」，
  **不再 @ 人**。理由：一次任务执行就是一次告警，周期计划第二天再跑同一航点
  也要在群里看得到（以前 24 小时内只推第一条，现场以为「巡检没跑 / 没告警」）。
  只有 5 分钟窗口内的连刷会合并成一条（`alert-repeat:<工单>` 去重键），
  机器人重复报 `12608` 时不会把群刷爆。嫌吵可关掉设置页
  `wecom_alert_repeat_push`，退回老口径。
  **AI 命中不吃这个窗口**（`ai:` 键改用结构化键 + 1 分钟，每轮一条）：一轮巡检就是
  一次结论，同一航点下一轮又命中是新信息；2026-11-19 取消后 4 分钟重跑，三条 AI 告警
  被上一轮的同工单推送合并掉，群里一条没有，看着就是「重跑后 AI 没上报」。
- **数量变化的跟单**会更明确：**数量核对的检出数变了**
  （如「检查 3 个，实际检出 1 个」→ 下一轮 2 个），推的那条写明
  「跟单：数值有变化 / 上一次 1 个 → 本次 2 个 / 原工单，未新建」；
  置信度抖动不带这类说明。实现见 `alert_flow::count_change` + `Assignment.follow_up`。
- **工单详情带「关联告警」**：自动派单建的单，同类告警都挂在同一张单上
  （`alerts.work_order_id`），详情接口回 `alerts[]`（编号/级别/正文/时间/状态），
  页面在「处理轨迹」上方列出——板端 `result.json` 每轮覆盖，历次结论和证据图路径
  从这里回看。
- 开关：设置页「告警自动派单（生成工单）」（`notification.alert_auto_workorder`，
  默认开）。关掉只停建单，**责任人解析与推送照旧**；
  「只为执行计划期间的告警建单 + 推群」（`notification.alert_only_in_task`，默认开）
  则决定**哪些告警**参与这一整套（建单 + 推群）。

### 责任人 ↔ 平台账号（「我的工单」）

派单只认责任人库（`staff`），但「登录的是谁」在账号表（`users`）里，两者以前没有关联，
于是做不到「只看派给我的」，也没法把处置权限定给责任人本人。现在：

- 「排班管理 → 责任人库」编辑责任人时可**关联平台账号**（`staff.user_id`，可空，
  一个账号最多绑一个责任人；中台 SSO 首次登录会 upsert 本地账号，所以两边都能选）；
- 工单管理多一个 **「只看我负责的」** 过滤（`GET /api/workorders?mine=true`）；
- 可见性/处置权限：
  | 场景 | 谁能看到 / 处置 |
  | --- | --- |
  | 人工建的工单 | 创建人 + 管理员 |
  | 系统派单，派给**已绑账号**的责任人 | 该责任人 + 管理员（其他人看不到，不是「工单不存在」的错） |
  | 系统派单，派给**未绑账号**的责任人 | 公共待办：所有登录用户都能看能处置（外包/临时工没账号时不会卡死） |
  | 删除 | 只有管理员或创建人（系统单是告警的存证链，不给现场删） |
- 账号没绑责任人时权限保持老口径（自己建的 + 全部系统派单），升级不会把人锁在门外。

> 提醒：绑定是有代价的——一旦把人绑上账号，**派给他的单就只有他本人能处置**；
> 如果现场习惯「谁在班上谁处理」，可以先不绑，等排班/账号体系理顺再绑。

### 与推送开关的关系

派单 + 建单跑在 `notify::alert_created` 最前面，**不看企业微信配置**：
现场没配 `ROBODOG_WECOM_WEBHOOK` 也必须有人管这条告警。推送才是旁路。

> 设计细节、扩展点与踩坑见 [`docs/wecom-alert-push.md`](docs/wecom-alert-push.md)。

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
| `ROBODOG_RECORDING_DIR` | `server/data/recordings` | 巡检录像（航点 / 全程）落盘目录。视频比图大得多，正式机建议指到大盘 |
| `ROBODOG_RECORDING_KEEP_DAYS` | `30` | 录像保留天数（`0` = 不自动清理）；超期的行 + 文件由计划扫描每分钟收掉 |
| `ROBODOG_SERVICE_TOKEN` | 空 | 与中台之间的共享服务令牌：调中台内部接口 `/api/dogInternal/*`、反向控制信道握手。需与中台 `APP_DOG__BUSINESS_API_TOKEN` 一致 |
| `ROBODOG_ZENTAI_WS_URL` | 空 | 反向控制信道的中台地址，如 `ws://10.0.0.8:3000`。与 `ROBODOG_SERVICE_TOKEN` 同时具备才建立信道；为空时走「中台直连 HTTP」 |
| `ROBODOG_SERVER_CONFIG` | 空 | 指定配置文件路径；不设置时读取当前目录的 `server.toml`（不存在则只用环境变量） |
| `ROBODOG_WECOM_WEBHOOK` | 空 | 企业微信群机器人地址的**部署级兜底**（平台设置页为空时才用）。见上文「企业微信告警推送」。**地址即凭据**，别写进 `server.toml` |
| `ROBODOG_WECOM_ENABLED` | `true` | 企业微信推送的部署级总开关 |
| `ROBODOG_WECOM_MIN_LEVEL` | `警告` | 推送级别下限（`严重` / `警告` / `提示`） |
| `ROBODOG_WECOM_MENTION_ALL` | `false` | 严重级别是否额外 @所有人（会占双倍限流额度） |
| `ROBODOG_PUBLIC_URL` | 空 | 控制台对外地址，如 `http://39.107.55.13:3008`，用于告警消息里的「查看详情」链接 |

## 任务下发与本体对接

任务通过 `devices.device_id` 关联设备，设备上线后由
`server/src/gateway.rs::dispatch_pending_tasks` 补发**已到窗口**的 `待执行` 任务；
`POST /api/tasks`（带 `deviceId`）也只在窗口内才会立即推送（见下一节）。

### 计划发布状态：只有已发布的计划才会自动执行（迁移 029）

`task_status` 是**执行**状态（待执行 / 执行中 / 已暂停 / 已完成 / 异常），回答「现在跑到哪了」；
它表达不了「这条计划还没定稿、先别自动跑」。所以另加一列 `publish_status`（`草稿` / `已发布`）：

| 值 | 行为 |
| --- | --- |
| `已发布`（**默认值**，历史数据全是它） | 按时间窗自动下发：建计划时窗口内立即下发、设备上线补发、后台扫描 `schedule::sweep` 全按老行为走 |
| `草稿` | 只保存，**不参与任何自动下发**：`POST /api/tasks` 不在窗口内顺手下发，`gateway::dispatch_pending_tasks` 跳过，`schedule::sweep` 跳过（也不会报「窗口未执行」）；事件流写一条「计划已存为草稿」说明原因 |

- 判定只有一个口子：`schedule::is_published` → `DueState::Draft`，三条自动下发路径都用 `due_state`
  做门禁，所以不会出现「某条路忘了判」的漏洞。
- **人工「开始执行 / 重新下发」（`POST /tasks/{id}/dispatch`）不受发布状态限制**：现场要能直接跑一次。
- 发布走 `POST /tasks/{id}/publish`：发布前校验时间窗与路线坐标（跑不起来的计划当场报错，
  而不是发布完在「待执行」里干等），并在事件流留痕；`published_at` 记时间，撤回不清空。
- 撤回走 `POST /tasks/{id}/unpublish`：执行中 / 已暂停的要先暂停并取消（否则运维以为已经停了，
  狗还在跑）；已经下发的那一次不受影响，只是不再自动排下一轮。
- 中台控制面 `POST /zentai/devices/{id}/tasks`（一键下发）建的任务固定为 `已发布`：它就是一次
  人工派单，存成草稿会让设备永远等不到这条任务。
- **默认值不能改成 `草稿`**：升级当天所有历史计划会一起停跑（与「新增 endTask 不能默认下发 none」
  是同一类事故）。
- 回归：`src/schedule.rs::tests::draft_plans_are_never_auto_dispatched`、
  `tests/schedule_window.rs::draft_plan_is_not_dispatched_until_published`、
  `tests/api_smoke.rs::task_publish_status_gates_auto_scheduling`。

### 定时巡检按计划时间窗执行

`plan_start` / `plan_end` 是**每天的执行窗口**（`HH:MM`，允许跨零点，
如 `23:30-00:30`），`schedule_days` 是执行日（`1,2,3,4,5`，1=周一 … 7=周日，
空 = 每天），`last_dispatched_on` 记最近一次到点下发的日期。判定与扫描都在
`server/src/schedule.rs`：

| 场景 | 行为 |
| --- | --- |
| 计划是 `草稿`（未发布） | 什么都不做：建计划不下发、扫描器跳过、设备上线也不补发（人工「开始执行」仍可跑）。见上一节「计划发布状态」 |
| `execute_mode = 周期任务`，现在在窗口内且今天还没跑过 | 到点下发（建任务、设备上线补发、后台扫描三条路径共用同一套门禁） |
| 窗口还没到 / 今天不是执行日 / 今天已经跑过 | 保持 `待执行`，`task_events` 里写一条 `schedule` 说明排到什么时候 |
| 设备离线 / 本体未就绪 | 窗口内每 30s 重试一次（`schedule::sweep`）；任务事件流里会有一条 `schedule_wait`「到点未下发」，写明原因 / 已重试次数 / 窗口剩余时间，每 5 分钟刷新描述（不会刷屏）——以前这里只有 debug 日志，线上 `RUST_LOG=info` 看不到，控制台表现就是「到点了却什么都不发生」 |
| 该条到点下发报错（路线坐标不是米制 / 地图缺 ROS 包 / 航点越界） | 只把**这一条**记成「下发失败 + 原因」继续扫下一批，**不打断整轮**（以前 `?` 出去会让排在它后面的计划全部停摆，日志里只有一句「计划调度扫描失败」）；回归 `tests/schedule_window.rs::one_undispatchable_plan_does_not_stop_the_whole_sweep` |
| 窗口过了仍没跑成 | 记一次 `schedule_miss` 事件 + 企微告警（同一天只报一次），任务**保持待执行**，下一个执行日同一时段重试 |
| 跑完变 `已完成` | 下一个执行日自动回到 `待执行`，等下一个窗口继续跑 |
| 跑完变 `异常` | **不自动重跑**：异常要人看一眼（本体故障 / 路线被拒），确认后手动拖回 `待执行` 即恢复排期 |
| `execute_mode = 立即执行 / 临时下发`，或中台 `task_dispatch` 不带 `executeMode` | 不看时间窗，建好就下发（保持原有语义） |
| 看板 / 详情里手动点「开始执行」 | 随时可跑（人工覆盖），并计入当天那次：同一天不会被自动再跑一遍 |

时间与执行日在前端只能用选择器填（`el-time-select` + 周几开关片），`周期任务` 缺时间
或起止相同会被后端 `schedule::validate_window` 拒掉——手填的 `9点` 这类值以前会让这条
计划永远不跑。

> 周期任务复用同一条 `tasks` 行（不按次拆实例），所以「已完成」的历史会被下一次
> 执行覆盖：执行过程留在 `task_events` 里，需要逐次审计时看事件流。

`PUT /api/tasks/{id}`（控制台「编辑」）只允许修改 `待执行` 的任务，执行中 / 已完成 /
异常的任务需先在看板里拖回 `待执行`；字段缺省表示保持原值，`routeId: null` 表示显式
解除路线关联（改为临时路径）。修改会写入 `task_events`（`event_type = 'updated'`）。

下发载荷由 `server/src/dispatch.rs::build_dispatch_payload` 统一组装：

| 字段 | 来源 | 说明 |
| --- | --- | --- |
| `routeData` | `routes.route_data` | 前端三栏编辑器的 `points/legs/returnLeg`；`points[].x/y` 是地图米制（`coordFrame: "map"`） |
| `map.mapId` | `routes.map_version_id` → 回退 `parks.map_version_id` | 冻结版本优先 |
| `map.mapPath` | 由 `mapId` 派生（`@` → `_`） | 本体侧目录名 |
| `map.mapType` | `map_versions.road_count > 0 ? 3 : 1` | 有路网段用 `3`（Route），否则 `1`（2D 栅格）；客户端还会用 `4013 map_route_exist` 再核对一次 |
| `speedLimit` | `routes.default_speed` | m/s |
| `avoidLevel` | 固定 `40`（全避障） | 10 关闭 / 20 仅急停 / 30 感知 / 40 全避障 |
| `endTask` | `tasks.end_task` → 回退 `executeMode` 推断 | `return_to_charge` / `return_to_start` / `none`；老客端只认这个字段 |
| `endTaskMode` | `tasks.end_task`（**仅显式配置时才带**） | 同上取值，包括显式 `none`；新客端用它区分「显式原地」与「跟随路线」 |

### 跑完要不要返航（`endTask`，2026-11-18）

以前「执行完去哪」**没有参数可配**，链路是：

```text
控制台执行模式只有 周期任务/临时下发/立即执行 → resolve_end_task 一律推断成 none
  → 客户端把 endTask="none" 当成「没指定」
  → patrol_end_task 兜底：loop_mode == "闭环" ? 返回起点 : 原地
```

而新建路线的闭环方式默认就是「闭环」，所以**每条路线跑完都必然返航**，想不返航只能把路线形态改成
「往返/单向」——那会清掉 `returnLeg`、连带改里程/耗时/耗电和校验文案，而且同一路线无法按任务区分。

现在把它提升为**任务属性**（`POST/PUT /api/tasks` 的 `endTask`，迁移 028）：

| 值 | 含义 | 下发 |
| --- | --- | --- |
| 不传 / `null` | 跟随路线（老行为） | 不带 `endTaskMode`，闭环仍回起点 |
| `none` | 原地待命（不返航） | `endTaskMode="none"` → 巡检 `end_task=0` |
| `return_to_start` | 返回起点 | `end_task=1` |
| `return_to_charge` | 返回充电点（地图需有回充点） | `end_task=2` |

校验在 `handlers/tasks.rs::normalize_task_end_task`：认不出来的值直接 400，不会静默当成「跟随路线」。
控制台「新建/编辑任务」有「跑完去哪（返航开关）」下拉；中台「下发任务」弹窗同名下拉（
`endTask` 透传到业务平台）。**兼容性**：只有显式配置才下发 `endTaskMode`——
老客端忽略它继续按 `endTask` + 闭环兜底跑，不会因为升级服务端就集体停止返航。

选「原地待命」时狗会停在**最后一个航点**（巡检终态仍是 `success`，AI 窗口按终态关闭）；
选「返回充电点」时客户端会先核对 `charge_point_exist`，没有就提前判异常而不是跑完才失败。

> `routeData.returnLeg`（返航段）**只参与画布显示和里程/耗时/耗电估算**，不参与执行：
> 巡检 `5000 kind=4` 只把原始 `points` 写成本体路径，返程由本体 ALG 按 `end_task` 自己规划。
> 改返航段的速度/动作不会改变返航路径。

客户端（`client/`）把载荷翻译成智航 Pro 指令驱动机器狗本体，详见
[`client/README.md`](client/README.md)。

### 本体健康门禁（对齐官方 App）

官方 App 在 `sdk_node` 未就绪时显示“设备初始化中”，在急停未解除 / 控制权被外部
遥控器抢占 / 本体 1315 超时时不允许运动。客户端和服务端现在使用同一套门禁：

| 字段 | 来源 | false / true 时的行为 |
| --- | --- | --- |
| `body_online` | 客户端心跳（1315 新鲜度） | false：设备状态落「故障」，任务下发 / 遥控 / 接管一律拒绝 |
| `sdk_node_available` | 1207 `modules[]` 的 `sdk_node` | false：拒绝任务、遥控、站立/模式切换；提示重启本体算法节点 |
| `estop_software` | 1315 `estop.software` | true：拒绝运动/任务；「解除急停」（1302 cmd=1）不受接管态限制 |
| `ctrl_source` | 1315 `ctrl_source` | 3（其他控制源：遥控器 / 第三方 App / 本体内部残留任务）：拒绝遥控/任务，并让本地 1204 标记失效；可用「强制接管」越过 |

**本体掉线时健康字段一律清空**：客户端在重建本体链路 / 1315 超时期间心跳里带
`bodyOnline=false`，服务端此时把 `sdk_node_available` / `estop_software` /
`motion_status` 置 NULL、`ctrl_source` 归 0。否则页面会拿着断链前的
`sdk_node=false` / `estop.software=true` 继续拦遥控与任务，现场表现是「本体已经恢复了，
页面还提示本体 SDK 节点未就绪」。

客户端在 `RobotState` 里保存这些状态，任务下发前、`command`/`teleop` 入口统一检查；
服务端把同样的原因返回给控制台，不再出现“接口 200、本体实际拒收”。1205 失败、
`ctrl_source` 变成 0/3 时客户端会清掉本地控制权标记，下一次接管会重新发 1204。

#### 强制接管（遥控器关了但状态还是 3）

现场最常见的交接动作是「遥控器直接关电源」，本体可能还要一会儿、甚至一直把
`ctrl_source` 报成 3。这时普通接管会被 400 拒掉，运维只能干等。为此接管接口支持
显式跳过**这一条**门禁：

```http
POST /api/devices/{id}/teleop/take
{ "force": true }
```

- 落库 `devices.teleop_force = 1`，响应里返回 `"forced": true` 与当前 `ctrlSource`；
- 只跳过 `ctrl_source`：离线 / `sdk_node=false` / 软件急停 / 卧倒姿态照旧拦；
- 该标记随**心跳回执**（`heartbeat_ack.teleopForce`）下发，客户端重启后也能继续遥控；
- 释放接管、接管超时、设备掉线都会清零，不会让设备永久免疫遥控器抢占检测；
- 控制台在 `ctrl_source = 3` 时把按钮换成「强制接管」，并二次确认「遥控器已断电」。

客户端侧同样带 `force`（`command` 的 `payload.force`），否则客户端自己的门禁会把
1304 摇杆拦下——服务端放行也没用。

#### 控制权不在平台（`ctrl_source = 3`）的提示

`ctrl_source` 由 1/2 变成 3 时（心跳里带，1Hz），服务端做两件事，不需要客户端配合：

| 落点 | 内容 |
| --- | --- |
| `alerts` | `AL-CTRL-<设备号>-<毫秒时间戳>`，级别「警告」，内容写明设备号 + 可能原因 + 下一步 |
| `task_events` | `控制权被其他控制源接管` / `控制权已收回`（只有进出 3 才记，避免心跳刷屏） |

**`3` 不等于「遥控器」**：协议里它是「其他控制源」，现场除了物理遥控器 / 第三方 App，
还有更常见的一种——本体内残留了没结束的任务（或算法节点卡住），导航能力被判 busy，
固件这时也报 3。判据是**近 5 分钟内是否出现过 `13897 capability busy` /
`13440 导航指令非法`**（落在 `task_events`）：

- 有 → 告警与门禁文案指向「本体内残留任务 / 算法节点占用」，提示去本体取消任务或重启算法服务；
- 没有 → 才提示「可能是物理遥控器 / 第三方 App」，并给出「强制接管」这条路。

不这么分，现场就会变成「每次下发任务都弹遥控器被接管，运维去关一个本来就关着的遥控器」。

- 同一台设备已有一条「未处理」的 `AL-CTRL-*` 时不重复插入，遥控器时断时续不会刷屏；
- 控制权回到 1/2 时自动把该告警置为「已处理」并写 `handled_at`，告警中心不会长期挂着已恢复的条目；
- 业务控制台（`web/src/layout/OpsLayout.vue`）每 10s 轮询设备列表，发现某台设备
  从非 3 变成 3 就弹一条**常驻**的 `ElNotification`（点击跳转设备列表），
  回到 1/2 时自动关闭；刚打开控制台时也会提示当前正处于 3 的设备，
  之后只在跳变时提示，不刷屏；
- 设备列表在「状态」下方加一枚红色「控制权被占用」徽标，抽屉标题栏加「控制权被占用 / 强制接管」标签，
  遥控页签的诊断条会给出可操作建议（确认遥控器已断电 / 本体取消残留任务 → 重新接管 / 强制接管）。

#### 急停 / 解除急停的回读校验

1302 在固件上**没有 `1303` 回执**，`clear_emergency_stop` 只能确认「命令进了设备连接」。
现场踩过：平台回 `wsDelivered=true`，本体一直报 `estop.software=true`，页面继续拦遥控与
任务，看起来像按钮没反应。所以 `POST /api/devices/{id}/action` 对这两个动作会：

1. 响应里带 `verify: { field: "estopSoftware", expect: <bool>, timeoutSec: 20 }`，
   控制台据此提示「已下发，20 秒内自动校验本体回读」而不是直报成功；
2. 后台每秒回读一次 `devices.estop_software`，20 秒内没变成期望值就落一条
   `AL-ESTOP-<设备号>-<时间戳>` 告警 + 一条「本体动作未生效」设备事件，
   文案直接给结论：`1302` 没被执行，请重启本体算法服务或整机重启；
3. 急停真的解除（下一次心跳 `estop.software=false`）时，未处理的 `AL-ESTOP-*` 自动结案。

**回读没生效时先分清是哪一侧**（2026-11-14 现场）：

| 现场特征 | 结论 | 去处 |
| --- | --- | --- |
| `sdk_node=false` 或 `body_online=false` | 1302 根本没到本体（链路断） | 本体运维手册 §4 |
| `sdk_node=1`、1315 一直刷，但 `estop_software` 恒为 true、`robot_mode`/`motion_status` 都是 `unknown` | 急停位锁在 `.1` 的 `dog_task` 状态机里（当日多次 `bad_alloc` / `/navigation_state` 反序列化崩溃） | `.1` `robot-launch restart dog_task`，详见本体运维手册 §10 |

两种都是**本体侧**问题，平台只需按告警/事件如实报导；“重启算法服务”并不总是正解
（重启 `.234` 清不了 `dog_task` 里锁住的急停位）。

#### 重连本体链路（运维手段）

`POST /api/devices/{id}/command` 的 `{"command": "本体重连"}`：让**设备客户端进程重启**，
由 systemd（`Restart=always` / `RestartSec=5`）重新建链（`1200`→`1201`→`1202`→`1203`）。
用于本体重启后旧会话没被踢、`bulkdata` 绑定丢失、gateway 的 RoamerX owner 卡住等
「接口通、本体不理」的半死状态。

- 它**不改本体**：不重启算法节点、不影响任务、动不了遥控器；只重建客户端到本体的连接；
- 代价是平台会短暂显示设备离线（约 5~15 秒），接管态与订阅会重建；
- 因此服务端在「有任务执行 / 接管中 / 模拟模式 / 客户端离线」时直接 400，
  客户端收到后还会再自检一次（任务 / 建图 / 接管）才执行；
- 客户端执行前会补一帧零摇杆、清接管态，然后上报一条 `warn` 事件再退出。

`5000` 的 `params.map_type` / `goal_type` 由客户端按 `--road-network` 决定：
默认 **`1`（2D 栅格）**，与协议文档的导航任务示例一致；`auto` 时航段写「沿道路」且本体
地图存在 `map.geojson`（`4013 map_route_exist=true`）才用 `3`（路网）；`on` 强制 `3`。
`routeData.fencePath`
（或 `payload.fencePath`）存在时，客户端会在 5000 `params.fence_path` 里带上围栏路径；
空串/空白不会下发，避免固件回「导航指令非法」。

### 定位与「机器狗不在图上」

任务能不能跑，取决于机器人**在地图坐标系里知不知道自己在哪**。

巡检（`kind=4`，现在是唯一会跑航线的任务）走**自动定位**，不需要人工点「重新定位」：派单时
客户端判「这条路线绑的图 + `loc_state.status == 100`」，缺一项就自己发 `4030` 载图重定位
（`initial_pose` 缺省发**零四元数** = 读上一份位姿文件做完整重定位），不收敛再自动重试一次；
两次都不成才把任务置「异常」并写清原因。细节见 `client/README.md`「巡检起步前的定位守卫」。

「返回起点」（`endTaskMode=return_to_start` → 巡检 `end_task=1`）回到的是**起步时本体
`capture_origin_pose` 采的起点位姿**，不是地图原点。所以若载图时误发了单位四元数 + 原点
（协议里那是一个**合法位姿**，不是「没给」），起点位姿会被采成原点，表现就是「返航回地图
原点」——2026-11 已改成缺省发零四元数，回归用例
`business/client/src/zhpro/mapping.rs::tests::load_map_without_pose_sends_zero_quaternion`。

历史 `kind=2` 导航链路的细节（客户端已不再使用）保留如下：

链路是：

1. `POST /api/tasks` / 重连补发 → 客户端发 `5000 kind=2`，首航点带
   `params.map_path` + `need_locate: true`（协议 §2.5.1：载图并等定位收敛，安全默认）；
2. 初始位姿由 SLAM 读「上一份位姿文件」做完整重定位（协议 §2.4.1：`initial_pose`
   为零四元数时的行为）；
3. 客户端在发 5000 **之前**加了定位守卫：先 `1204` 取控制权 + `1302 cmd=62` 切导航
   模式（官方链路顺序：先切模式再选图定位，避免切模式把已收敛的定位上下文重置掉），
   然后看 `loc_state.status`：已是 100 直接起步，否则下发 `4030 op=1` 载图重定位并等收敛
   （上限 60s）；**只有本次真的载过这张图**（走了 4030），首航点才用 `need_locate=false`
   复用定位；若上来就是 100（没动过地图），首航点仍按协议默认 `need_locate=true` 让本体
   重新载图定位——「已经定位」不等于「载的是这条路线绑的那张图」。60s 内不收敛
   （或 `203/210`）就**不派发**，任务直接置「异常」，进度消息写明下一步该做什么；
   任务详情弹窗的「任务事件」能直接看到这条原因。

`need_locate` 的取值规则集中在 `route.rs` 的 `locate_decision()`：

- 只有**首航点**可能 `true`；同一条航线内其余点一律 `false`；
- 首点复用预检定位的前提是「**同一张图** 且预检在 **60s 内**」
  （`PRE_LOCATE_REUSE_WINDOW`）——中途被换图、算法节点重启、久等超窗都会自动退回 `true`；
- 结束动作「返回起点」明确走 `false`：那时地图肯定还加载在本体上，再载一次只是白花
  10~30s，还会把位姿重置回「上一份位姿文件」；
- 复用定位失败（`13898 map not found` / 起步守卫「位未就绪」/ 含「定位」的错误）会**自动
  升回 `true` 重试一次**；仍失败则**中止整条航线**，不再按航点「跳过继续」，否则剩下的点
  会全部秒失败、把航线静默跑成 `0/N` 却回报「巡检完成」。

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

### 日志：怎么看「下发了什么 / 本体回了什么」

默认 `info` 级别就能看到完整报文，不用改任何配置：

| 行 | 含什么 |
| --- | --- |
| 服务端 `task_dispatch 下发载荷` | 业务拼好的整份 JSON（`routeData.points` / `map` / `speedLimit` / `initialPose`） |
| 客户端 `task_dispatch 载荷: …` | 客户端实际收到的业务载荷（核对是否被中继修改） |
| 客户端 `→ 智航完整帧（含 head）` | 发给本体的 `5000/5002/5004/5006/5014/4030/4044` 原始帧（`goals/map_path/map_type/need_locate/fence_path`） |
| 客户端 `← 智航完整帧（含 head）` | 本体应答 `5001/5003/5005/5007/5015/4031/4045` 原始帧（`task_id` / `code` / `message`） |
| 客户端 `5009 任务(...)` | 本体状态机变化（`lifecycle` / `stage` / `error` / `stop_reason` / `task_state` / `loc`），按快照去重，不刷屏 |

要看**每一帧**（含 `1315/1317/5009` 周期推送）或要长期取证，开帧落盘：
`ROBODOG_FRAME_DUMP=/var/log/robodog/frames.jsonl`（写在 systemd 的
`/etc/robodog/client.env`，一行一帧 `{"ts","dir","frame"}`，与日志级别无关）。
细节与命令见 `docs/client-connection-guide.md` §11.5。

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
| 控制权不在平台 | `1315 ctrl_source = 3`（other），平台发 1204 也拿不回来 | 三种来源：物理遥控器、第三方 App、**本体内部残留任务占着导航能力**。先看告警文案（平台会结合近 5 分钟是否出现 `13897/13440` 区分）；前者关遥控器电源/按「释放控制」，后者去官方 App 取消任务或重启算法服务；**确认遥控器已断电** 可直接点「强制接管」 |
| 摇杆帧太稀 | 浏览器 ~10Hz 的离散帧，本体侧看着像点动 | 客户端按协议建议 50Hz 重发（`teleop::spawn_republisher`），松手立即停车 |
| 本体卧倒 / 锁定 | `1315 motion_status = lie_down / locked` | 先点「站立」，等 `motion_status` 变成 `stand_up` 再遥控 |
| SDK 节点未就绪 | `1207 sdk_node=false` | 本体侧问题，不是等一等就好：先看 `.234` 的 `robot_sdk_node.stdout` 是否在刷 `Model/transport not ready`，再看 `.1` 的 `dog_task` 与 `df -h /dev/shm`；排查与修法见 [`../deploy/robodog-body.md`](../deploy/robodog-body.md) §4 |
| 软件急停未解除 | `1315 estop.software=true` | 先点「解除急停」（1302 cmd=1）；急停不受接管态限制，任何状态都能点 |

因此「接管」的完整语义是：`1204` 拿到控制权 → `1302 cmd=60` 切通用模式 → 摇杆按 50Hz
持续下发 → 用 `1315 ctrl_source / motion_status` 自证「真的接管到位」。设备抽屉的遥控页
有一条诊断条直接显示这三项，接不通时会写明该做什么。

本体动作白名单（`POST /api/devices/{id}/action`）与之配套：
`stand_up` / `lie_down` / `emergency_stop` / `light_on` / `light_off` /
`generic_mode` / `in_place_mode` / `navigation_mode`；除急停外都要求先接管。

## 实时视频与手动遥控

### 数据采集背包：单路视频 + AI 识别（`/monitor`）

背包（`business/backpack_dashboard/`，板端部署在 `/opt/xjpt/backpack_dashboard`）自带 4 路 USB 相机 + 机器狗前置 RTSP + 红外，
并在板端用 RKNN 做单路 AI 识别。控制台「实时视频」页只展示**一路**，可随时切换六路源；
AI 可对除红外外的五路开启，画框方式二选一：

| 画框方式 | 链路 | 特点 |
| --- | --- | --- |
| 浏览器叠加 | 干净流 + SSE 识别坐标 → 前端 SVG | 延时最低，切源即生效 |
| 背包烧录 | 板端把框烧进视频，推 `preview` 流 | 抓拍 / 录屏 / 转发自带中文标签 |

```text
接口（REST+SSE）三条通道，按可用性自动退让：
  ① 浏览器直连 ──▶ 板端 Agent :8000            （同网段：延时最低）
  ② 平台直连   ──▶ 背包 Agent :8000            （平台与背包同网段）
  ③ 平台隧道   ──▶ 设备客户端 ──▶ 127.0.0.1:8000（哪边都不同网段时的兜底）
视频：浏览器 ⇄ 板端 mediamtx（WHEP，直连）或 板端推流 → 平台房间 → 浏览器（中继）
```

- **接法由后端给结论**：`GET /api/backpack/status` 的 `mode` = `direct` / `proxy` / `tunnel`。
  直连模式下浏览器先自己探测 Agent；探不通（换了 WiFi、出差远程、公网入口、Chrome 的
  Local Network Access 拦截）且 `tunnel.available` 为真时，页面自动带
  `?transport=tunnel` 重试，改走设备客户端代调——不再要求回现场入口。
- 隧道怎么来的：设备客户端本来就跑在背包那块板子上（`127.0.0.1:8000` 一定通），
  注册成功后发一条 `agent_hello`，平台据此把 `/api/backpack/*` 的请求挂到这条设备 WS 上
  （服务端 `agent_tunnel.rs`，客户端 `agent_relay.rs`）。只放行 4 个只读接口
  （`cameras` / `relay/status` / `detect/tasks` / `detect/status`）+ 3 个 AI 动作，
  检出事件只转 `detect` 四类；多背包部署用 `ROBODOG_BACKPACK_TUNNEL_DEVICE` 指定设备号。
- **视频跨网段走平台中继**（「平台中继」按钮，或用云端入口时自动选）：
  延时 1~2 s，延时敏感场景仍建议现场入口。中继模式下浏览器画不了框，想让框跟着画面走就切
  「背包烧录」——板端把中文标签烧进 `preview` 流，中继也把这一路推上来
  （`RELAY_STREAMS` 需包含 `preview`，默认已含）。只看检出数量则不用切。
- 现场入口（<http://192.168.41.246:8080>，`deploy/site-console/`）仍然是最低延时的那条路：
  同一份 `dist`、WHEP 直连，部署与校验见 [`deploy/README.md`](../deploy/README.md)「数据采集背包」。
- 配置：`ROBODOG_BACKPACK_BROWSER_URL`（浏览器可达的 Agent 地址，直连模式；正式环境用它）
  或 `ROBODOG_BACKPACK_URL`（平台能代理到 Agent 时用代理模式）；`ROBODOG_BACKPACK_WEBRTC_URL`
  默认由 Agent 地址推导为同主机 `:8889`。两者都不配时仍可只靠隧道工作（页面提示改为
  「让板端客户端上线」）。
- AI 启停 / 换源 / 改阈值都转发到板端 Agent，NPU 单路独占（红外不参与）。
- **前置摄像头报「WHEP 协商失败（HTTP 404）」不是浏览器/网络问题**：`404 no stream is
  available on path 'dogcam'` 只说明板端 mediamtx 上没有这条流（`1080p60` 档靠 mediamtx
  直拉 `dogcam`，relay 不参与）。判别与修法（path 名必须叫 `dogcam`、source 要带 3588 的
  账号密码、改完 `snap restart mediamtx`）见
  [`docs/backpack-video-integration.md`](docs/backpack-video-integration.md) §6.7。
- **航点动作复用这份模型目录**：路线编辑器里动作类型选「AI 识别」时，识别场景 / 识别类别
  直接勾选实时视频页的那一套（`GET /api/backpack/status` → `detect.tasks`），写进动作的
  `scene` + `labels`，到点开窗口时原样下发给板端，不用再手填关键词（老数据仍兼容关键词匹配）。
  动作还有**检测类型**（`checkType`）两类：**检测是否存在**（找到就存证/上报，不判数量）与
  **检测数量是否正确**（再核对「检查目标数量 `expectCount`」，不一致**上报数量告警**——包括
  「一个都没找到」= 0 ≠ N，且要「看满了」——窗口正常到期，或狗停满 `dwell` 后离开
  （窗口时长是 `dwell+8`，只认「到期」会永远报不出来），被打断且没停够不记账）。老数据没有该字段时后端按
  有没有 `expectCount` 推断。开窗同时把计划 id（= 业务任务号）与航点下标下发板端，
  命中帧与 `result.json` 落到 `detect_jobs/<计划ID>/pt<航点下标>/`，可按计划回溯
  （**图进了企业微信群之后平台会下发 `ai_cleanup`，客户端收尾时把这个航点目录删掉**，
  否则一天几十轮巡检很快把板子存储写满；推送失败的保留，见下面的「证据图」）。
  窗口时长跟着该点的「停留时长」走（停留 + 8 s，最短 10 s、上限 `ai_window_max_secs`
  默认 300 s），**离开航点立即结束**——所以 `dwell=100` 的点会识别满整段停留，不再只覆盖前 30 s。
  但「立刻结束」只是不再识别：平台会再等客户端 12 s 把本点的结论说完（`ai_detect` /
  `ai_alert` / `ai_end`，窗口标成 `closing`），否则板端后到的「检出 1 个」会被平台记成
  「实际检出 0 个」、证据图一起丢（2026-11-20 13:38 现场）。
  链路与排查见 [`docs/official-app-parity.md`](docs/official-app-parity.md) §5.6、
  [`docs/backpack-video-integration.md`](docs/backpack-video-integration.md)。
- **正式环境的航点 AI 由设备客户端代调**（`[backpack] point_ai` / `ROBODOG_POINT_AI`）：
  业务平台在公网、Agent 只在现场内网，服务端直连必然超时——现场表现就是「路线里配了
  AI 识别，跑完却一条告警都没有」。客户端与 Agent 同机（`127.0.0.1:8000`），所以：

  ```text
  客户端到点(5009) → GET tasks/status(本机) → ai_point（含模型目录）→ 服务端算场景/类别/阈值/窗口
                → ai_start（含 planId/pointIndex/checkType/expectCount）
                → POST /api/detect/start|config → 读 SSE → ai_detect（按阈值+类别去重）
                → 数量任务数量不符 → ai_alert → 服务端写数量告警 + 企微
                → ai_end（带 expired）→ 服务端写 alerts + task_events + 企微推送
  任务终态/取消 → 服务端下发 ai_stop → 客户端 POST /api/detect/stop
  ```

  `auto`（默认）按部署形态选：配了 `ROBODOG_BACKPACK_URL`（服务端与板端同网段）就走服务端
  直连，否则交给客户端。决策逻辑（场景匹配、阈值解析、窗口时长）只有 `ai_patrol.rs` 一份，
  客户端只负责搬运，两边不会各说一套。客户端配置看 `client.toml` 的 `[ai]` 段
  （`agent_url` 默认 `http://127.0.0.1:8000`，`--no-ai` 可关闭）。
  **证据图**：检出 / 数量核对两类告警在企业微信里都会多一张板端命中帧（`msgtype=image`，
  卡片之后第二条）。数量核对用板端 `plan_hit.evidence` 给的路径；实时检出那一刻板端还在
  「连采 3 帧命中」采样、图还没落盘（实测比第一帧检出晚 4.6s），所以命中先挂起、等
  `plan_hit` 送 `evidence` 来一起上报（8s 等不到就先发卡片）。命中帧稀疏时 `plan_hit` 要到
  窗口收尾才发：客户端「先停识别再扫盘」，扫到图就单独报 `ai_evidence`，平台只补一张图
  （不重发卡片）。图优先读 `plan_hit.evidence`，读不到再扫 `[ai] jobs_dir` /
  `ROBODOG_AI_JOBS_DIR`（默认自动探测板端 `detect_jobs`，也会从 `plan_hit.evidence` 学到
  真实路径）下的 `<计划ID>/pt<航点>/` 取本轮最新一张。读不到图就只有卡片，告警本身照发
  （详见 [`docs/wecom-alert-push.md`](docs/wecom-alert-push.md)）。
  **推完即删**：图片那条企业微信回 `errcode 0` 之后，服务端在同一条设备 WS 上下发
  `ai_cleanup {planId, pointIndex}`，客户端删掉 `detect_jobs/<计划ID>/pt<航点>/`
  （画框 JPG + `result.json`，计划目录空了连它一起删）。窗口还开着时客户端先记账、
  等收尾（停识别 → 扫盘补图 → `ai_end`）之后再删，避免把还没推出去的图毁掉。
  **计划跑完删整个目录**：上面那条只覆盖「推过图的航点」，没命中的航点 / 数量任务的检出 /
  图被拒的都等不到它。所以任务终态 / 取消时客户端调 `AiPatrol::finish_plan` 把
  `detect_jobs/<计划ID>/` 整个删掉（窗口还在收尾就先记账等它跑完；同计划号重跑会撤销），
  启动时 `AiPatrol::sweep_stale_plans` 再清一把超过 3 天没动过的残留。
- **航点灯光（背包板补光灯 / LED）**：路线编辑器的航点动作多两个开关——
  `lightFill`（开补光灯，板端 GPIO 46）与 `lightLed`（开 LED 灯带，板端拉起厂商
  `test_rgbrun`，行为是循环闪烁）。客户端在 `patrol.point_arrived` / `patrol.waiting`
  时调板端 Agent `POST /api/lights`，**离开航点 / 到达无灯光动作的航点 / 任务终态 /
  任务取消**时调 `POST /api/lights/off` 关灯；Agent 自己还有看门狗
  （`LIGHT_MAX_ON_SECS`，默认 900 s），客户端掉线或被 kill 时兜底关灯。
  **航段动作也有同一对开关（「航段开灯」）**：进段开、**走完这一段才关**（离开起点
  航点正是航段的开始，在那里关灯等于点了就灭），行进途中由客户端按 `LIGHT_REFRESH_AFTER`
  重发续命，不会走到一半被看门狗灭灯；同一个点既配了航点灯又配了航段灯时航点优先。
  只开灯不干别的就用动作类型「补光」（两侧都能挂）。
  **计划里配了「自动开灯（补光灯 / LED）」（`tasks.light_fill` / `light_led`，迁移 032）
  时以计划为准**：航点 / 航段动作里的开灯、关灯全部不生效，灯从任务开始一直亮到
  终态 / 取消（现场要求「计划配了开灯，航点上的开关就不作数」）。两盏都不勾才回到
  上面的航点 / 航段动作行为。计划勾了「仅夜间」（`tasks.light_night_only`，迁移 030）时，
  **三种模式**都只把 `nightOnly` 转给板端，由**板端按本地时间窗**（计划里可自定义
  `lightNightStart`/`lightNightEnd`，留空用板端默认 `LIGHT_NIGHT_START`/`LIGHT_NIGHT_END`
  = 18:00-06:00）判定——白天板端直接跳过、不开灯，平台不参与算夜间（两地时区/时钟
  不一致时白天开灯最难查）。
  回归：`backend/tools/lights_selftest.py`（时间窗 / 白天跳过 / 看门狗）、
  `client lights::tests::*`（含航段灯 `leg_lights_stay_on_when_leaving_the_start_point`）、
  `client pump::tests::*`（计划灯全程开 / 航点模式照旧 / 航段灯亮完整段 / 收尾后迟到开灯被丢弃）、
  `api_smoke::task_light_night_only_reaches_dispatch_payload`、
  `api_smoke::plan_lights_reach_dispatch_payload_and_default_to_waypoint_actions`。
  设计与踩坑见 [`docs/plan-lights.md`](docs/plan-lights.md)。
- 分析与后续路线（遥测、SOS 告警、中继、录像）见
  [`docs/backpack-video-integration.md`](docs/backpack-video-integration.md)。

### 视频：协议里没有，统一走背包前置 `dogcam`

协议原文已归档在 `business/docs/zhihang-pro-api-v1.0.3.pdf`（84 页），
同目录 `.txt` 是便于检索的纯文本版；设备客户端与机器狗本体的连接方式（双通道建链、
心跳、重连、配置项）见 [`docs/client-connection-guide.md`](docs/client-connection-guide.md)。

智航 Pro v1.0.3 接口文档全文（84 页）**没有视频传输相关描述**：`视频` / `码流` /
`摄像头` / `图传` / `RTSP` / `HLS` / `WebRTC` / `video` / `camera` 均 0 次出现，
API Type 总表 60 个消息里也没有图像或流媒体类型。`bulkdata` 通道的大负载是
1401/1403/1405/1407 的位姿与路径数据，不是视频帧。

因此**实时视频只有一个来源**：数据采集背包的板端 relay 把本体前置 RTSP 归一化成
mediamtx 的 `dogcam`，浏览器直连板端 mediamtx（WHEP）或走平台中继（板端推到平台
mediamtx，同样是 WHEP）。

| 页面 | 组件 | 说明 |
| --- | --- | --- |
| 实时视频 | `views/monitor/index.vue` | 六路源可选，默认 `dogcam`，带 AI 与画框 |
| 设备管理（抽屉实时监控 / 手动遥控） | `components/FrontCameraVideo.vue` | 固定 `dogcam`，与实时视频同源同路 |
| 计划管理（实时监控 / 详情） | `components/TaskLiveMonitor.vue` → `FrontCameraVideo.vue` | 同上 |

设备档案里**不再有视频地址**（`video_url` / `video_type` 已删除）：2026-11-18 起删掉了
「设备客户端 ffmpeg 拉本体 RTSP → 平台中继」的整条旧链路，部署没接背包时页面直接给
配置说明，不做兜底播放。设计背景见 `docs/backpack-video-integration.md` §6.9。

### 平台中继：板端推流 → 平台 mediamtx → 浏览器（WebRTC）

跨网段 / 公网入口时（操作员电脑够不到背包的 `:8889`），板端 relay 主动把流推到平台，
浏览器只跟平台同源通信、媒体走 WebRTC：

```
背包本地 RTSP ──ffmpeg──► 板端 relay ──RTSP/TCP :8554/bp_<stream>──► 平台 mediamtx
                            (RtspUplink)                              │  └ WHEP（只绑回环 :8889）
平台控制面：板端 ⇄ ws://平台/ws/device/video（房间 bp:<stream>，只有文本帧）
浏览器：POST /api/backpack/whep/<stream>/whep（同源反代）+ 一条 .../video/ws?media=0 心跳
```

2026-11-19 起**只有这一条媒体通路**。之前的「FLV over WebSocket」中继（平台环形缓冲
扇出给 flv.js，延时 1~2s）连同它的补流头 / 当前 GOP / 积压断开 / `RELAY_OUTPUT` 退让
一起删除了：那一整套坑见 `docs/backpack-video-integration.md` §6，现行链路见 §7。

- **按需推流**：第一个观看者进来才下发 `video_start`（带 `stream` + 平台档位），最后一个
  离开**不立刻**停流：房间保留 30s 宽限（`STREAM_IDLE_GRACE`），看门狗超时才 `video_stop`。
  现场关抽屉再打开、切页签、浏览器 WS 抖一下都能复用还在跑的流，不会每次黑 3~7s。
  推流端重连时只要**还有人在看**，平台就**补发** `video_start` —— 否则那条请求打在已死的
  旧连接上丢了，观看者只能黑到前端重连（§6.10⑯）。
- **观看通道只当心跳**（`/api/backpack/video/ws`）：只计数 + 触发 `video_start`，一个媒体
  字节都不转发。没有它就没有「第一个观看者」，看门狗也会在宽限后把流停掉。鉴权用
  `?token=<JWT>`（浏览器 WS 握手不能带请求头，`require_auth` 为此放开了查询串令牌）。
- **平台部署要求**：服务器上要有 mediamtx（`deploy/external-hz/mediamtx/`），安全组放通
  **TCP 8554**（板端推流）与 **UDP 8189**（WebRTC 媒体，最容易漏；可选 TCP 8189 做
  ICE-TCP 回退）。业务平台通过 `ROBODOG_MEDIAMTX_URL`（默认 `http://127.0.0.1:8889`）
  反代 WHEP，mediamtx 的信令端口只绑回环。
- **先缩后编（默认 light 档位）**：`dogcam` 是 1080p60，`-c:v copy` 会把 4~8Mbps 原样
  推出去，而板子出公网的上行常常只有 1Mbps 量级。所以 relay 默认转成
  **720p15 / 1.0Mbps**（`RELAY_PROFILE=light`，编码器自动挑 rkmpp / v4l2m2m / libx264），
  带宽更紧用 `RELAY_BITRATE=800k RELAY_FPS=15`。只有上行 ≥6Mbps 才考虑 `RELAY_PROFILE=copy`。
- **墙钟时间戳**：现场相机 SDP 标 60fps、实际只投递 ~25 帧/秒，relay 用
  `-use_wallclock_as_timestamps 1` 把 PTS 换成到达时间，避免播放端反复饿死（慢放）。
- **上行掉线要自愈**（2026-11-18）：板端 relay 每次写平台带超时（默认 5s；原来 10s，
  那十秒里 ffmpeg 仍在攒帧，恢复后会以 2~3 倍速倒出来），超时即判「Wi-Fi 半开」并断开重连；
  15s 一个字节都没推出去（源卡住）就重启 `ffmpeg`；每 5s 巡检平台出口网卡，出口变了主动
  重建连接。rtsp 通路的字节**必须聚合后再写**（`RELAY_RTSP_BATCH_*`，24KB/60ms）：
  逐包 write 会让上行 TCP 变 app-limited，4G 的 96ms RTT 把吞吐压到 ~0.5Mbps。
- **画质自己降**（2026-11-19）：relay 每 5s 统计一次写平台的占用与实测码率，发现「被反压
  + 码率低于目标档位」就就地换 `640x360@12 / 500k`（发 `video_reset` + 重启 ffmpeg，观看端
  不用重连）；`RELAY_LAG_DEGRADE` 让「积压持续上涨」（出帧跟不上这一档）也降档，比
  `RELAY_MAX_LAG` 的硬重同步早得多。
  **升档（回档）一次只升一档**（极省流 → 省流 → 配置）+ `RELAY_PROBATION` 试用期：升上去
  积压超过 `RELAY_PROBATION_LAG` 就立即退回，并把那一档封顶一段时间不再试；没人在看时不升档。
  反复升降时窗口翻倍。降档是板端行为，平台/前端不用改；参数与状态机见
  `business/docs/backpack-video-integration.md` §6.10⑥⑨、§7.5。
- **观看端不许闪**（`web/src/components/VideoPlayer.vue`）：断流时先抓最后一帧当 poster
  继续显示（不黑屏）；重连交给 `lib/whep.ts` 的指数退避（1s → 8s，`close()` 会 DELETE
  会话，不在 mediamtx 里留僵尸 reader）；另有 8s「时间轴不前进」看门狗整体重来一次。
  **起流期间不许纯黑**：poster 顶到一次连接超时（20s）为止，并盖一层
  「起流中 · 已等 Ns」遮罩（板端起 ffmpeg + 等关键帧 1~7s），静帧不冒充直播。见 §6.10⑯。
- **板端没升级要知道**：板端起流时上报 `video_transport`，`GET /api/backpack/video/state`
  的 `transport` 字段就是它。现在只可能是 `rtsp`；`ws`/`null` = 那台板子的 relay 还是老
  版本（媒体走已删除的通路），前端直接报「升级 backpack-relay」而不是让人干等。

现场前置条件：

| 项 | 要求 |
| --- | --- |
| 板端 relay | `backpack-relay` 服务在跑，能出公网（`ws://<平台>/ws/device/video`），且 `/etc/robodog/relay.env` 里配了 **`RELAY_RTSP_URL`**（口令见平台 `/opt/hz/rust-admin/mediamtx-publish-url.txt`） |
| 板端出口路由 | 到平台的流量默认走 Wi-Fi（实测 Wi-Fi ~25Mbps、4G dongle 上行仅 0.25~0.6Mbps）：`deploy/backpack-relay/50-platform-route.sh` 给平台加 /32 路由（路由变化会重启 relay）。Wi-Fi 抖得厉害、宁可牺牲画质也要稳定时，把它改成 `PIN_IFACE=none` **钉死 4G**（钉死后先量 4G 上行：配 `RELAY_SLOW_IFACES=` 才不会被当成慢链路压到 320x180）。详见 `docs/backpack-video-integration.md` §6.10⑮ |
| 背包 mediamtx | `dogcam` path 能拉通（`ffprobe rtsp://<背包IP>:8554/dogcam`） |
| 平台 mediamtx | `systemctl status mediamtx` 在跑；安全组 **TCP 8554** + **UDP 8189**（+ 可选 TCP 8189）；平台 `ROBODOG_MEDIAMTX_URL` 已配 |
| 操作员网络 | 能到平台 UDP 8189（企业代理只放 443 时会协商成功但黑屏 —— 症状见 §7.6 排障表） |

延迟量级：直连板端 WHEP ≈200ms；平台中继 ≈300ms（设备 4G 时多一点）。

### 任务执行实时监控（视频 + 地图位置）

计划管理里所有 `执行中` / `已暂停` 的任务卡片都有一个「实时监控」按钮（详情弹窗顶部
也内嵌同一块面板），同时看到两样实时数据：

视频这一路固定是背包前置 `dogcam`（`web/src/components/FrontCameraVideo.vue`），
与「实时视频」页的前置摄像头同一条推流；**设备管理 / 计划管理默认走平台中继**，
只有中继推流端不在时才退回 WHEP。设备档案里的视频地址已删除，见
`docs/backpack-video-integration.md` §6.9。

| 区域 | 数据来源 | 刷新方式 |
| --- | --- | --- |
| 本体视频 | 背包 `dogcam`（本体前置 RTSP 归一化）—— 与「实时视频」页的前置摄像头同源同路 | 默认平台中继（`FrontCameraVideo.vue`）；中继不可用时退回 WHEP |
| 地图位置 | `devices.position`（本体 1401 位姿的地图米制 x/y） | 设备心跳 10s 上报 + 页面 5s 轮询 |
| 进度 / 阶段 | `tasks.progress` / `tasks.robot_stage` | 客户端 5s 上报 + 页面 5s 轮询 |

地图组件 `TaskLiveMap.vue` 把「路线航点 + 已走航段（绿色）+ 下一目标点（橙色）+
机器狗实时点位」画在同一套米制坐标上，所以需要在图上看到点位，要满足三个前提：

1. 任务绑定了巡检路线（临时路径没有航点，只显示设备点位）；
2. 路线的 `map_version_id` 是一张**有地图包的真实地图**（示意坐标系退回示意底图）；
3. 本体当前载图（`devices.active_map_id`）与路线地图一致，否则点位会画歪，
   面板会直接给出「载图不一致」的提示。

位置的对齐方式与下发一致：都在 `utils/coords.ts` / `server/src/coords.rs` 里换算
（`goals[].position` 的米制坐标），不要再引入第二套坐标公式。

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
自动上下堆叠。画面固定是背包前置 `dogcam`（与「实时视频」同源），按需起流由板端中继的
`video_start` 控制（见上节「平台中继」）。

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
  「分段导航任务（一段 = 一条 `5000`，`params.goals` 放该段全部航点）+ 客户端侧点位动作」
  实现，见 `client/README.md`。**不要逐点下发**：本体同一时刻只允许一个导航任务，
  逐点时第 2 个点起会全部回 `13897 capability busy`。
- 视频：唯一来源是背包前置 `dogcam`（板端 relay → mediamtx WHEP 直连，或推到平台
  mediamtx 走 WebRTC 中继），支持多观看者与按需起停；设备客户端不再参与拉流。
  中继没有做录制 / 回放，也不做多路混流。
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
- 线上事故修复要补回归用例，例如驾驶舱设备列缺字段 → `server/tests/api_smoke.rs`。

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
`ws://39.107.55.13:3008`）。

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
