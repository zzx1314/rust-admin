# 开发对接手册

本手册面向需要调用管理后台接口的开发者（设备端、算法侧、第三方系统）。所有接口均由 Rust 后端（axum + sea-orm + SQLite）提供，管理后台通过 Vite 代理把 `/api` 转发到 `http://127.0.0.1:3000`。

## 1. 服务地址与环境

| 环境     | 地址                                                           |
| -------- | -------------------------------------------------------------- |
| 后端     | `http://127.0.0.1:3000`（端口见 `backend/config/config.toml`） |
| 管理后台 | `pnpm dev` 后访问 Vite 输出地址，`/api` 自动代理到后端         |

数据库文件为 `backend/data/users.db`（SQLite），服务启动时自动执行 `backend/migrations` 下的 SQL 迁移。

---

## 2. 认证

### 2.1 获取 token

```http
POST /api/token
Content-Type: application/x-www-form-urlencoded
```

| 参数         | 必填 | 说明                                                                                                                |
| ------------ | ---- | ------------------------------------------------------------------------------------------------------------------- |
| `username`   | 是   | 用户名                                                                                                              |
| `password`   | 是   | 密码密文。为 AES-128-CBC + ZeroPadding 后的 Base64 字符串（与管理后台登录页一致，参考 `frontend/src/utils/aes.ts`） |
| `grant_type` | 是   | 固定 `password`                                                                                                     |
| `scope`      | 是   | 固定 `server`                                                                                                       |

响应（成功）：

```json
{
  "success": true,
  "data": {
    "avatar": null,
    "username": "admin",
    "nickname": "管理员",
    "roles": [1],
    "permissions": ["110"],
    "access_token": "eyJhbGciOi...",
    "refresh_token": "eyJhbGciOi...",
    "expires": "2025-11-05 20:00:00",
    "user_id": 1
  }
}
```

> 注意：`/api/token` 的响应字段是 **snake_case**（`access_token` / `refresh_token`），与管理后台其它接口的驼峰字段不同。

### 2.2 访问其它接口

除 `/api/token`（及刷新接口）外，所有接口都必须携带：

```http
Authorization: Bearer <access_token>
```

### 2.3 刷新 / 注销 / 校验

| 接口                                 | 方法 | 说明                                                                                             |
| ------------------------------------ | ---- | ------------------------------------------------------------------------------------------------ |
| `/api/token/refresh/{refresh_token}` | GET  | 用 refresh token 换新的 `accessToken`（响应为驼峰字段 `accessToken`/`refreshToken`/`expiresIn`） |
| `/api/token/logout`                  | GET  | 注销当前 token（需 Bearer）                                                                      |
| `/api/token/check_token`             | GET  | 校验 token 是否有效（需 Bearer）                                                                 |
| `/api/sysUser/info`                  | GET  | 当前登录用户信息（需 Bearer）                                                                    |
| `/api/sysMenu/user-menu`             | GET  | 当前用户可见菜单树（需 Bearer）                                                                  |

---

## 3. 通用约定

### 3.1 响应结构

```json
{ "code": 10200, "msg": "success", "data": {} }
```

- `code = 10200` 表示成功，其它值为错误码（HTTP 状态码同步为非 2xx）。
- 分页接口的 `data`：

```json
{ "records": [], "total": 0, "current": 1, "size": 10 }
```

### 3.2 请求体

除登录接口使用表单外，其余接口均为 `Content-Type: application/json`，字段使用 **小驼峰**（如 `deviceId`、`taskStatus`、`mediaSize`）。

### 3.3 其它约定

- 所有删除均为 **软删除**（`is_deleted = 1`），查询接口不会返回已删除数据。
- 时间字段在响应中统一为 **北京时间** 字符串 `YYYY-MM-DD HH:mm:ss`；查询条件 `beginTime` / `endTime` 也按北京时间解析（`YYYY-MM-DD` 或 `YYYY-MM-DD HH:mm:ss`，结束时间会自动补到当天 23:59:59）。
- 分页参数为 `current`（从 1 开始）与 `size`，默认 `1` / `10`。
- 主键 `id` 在创建时由服务端生成，客户端无需传入。
- 园区 / 地图 / 任务按创建者做数据隔离：普通用户只能读写 `userId = 当前用户` 的记录（详情 / 修改 / 删除他人数据返回 404）；管理员（角色编码 `110`）不受限制。`userId` 由服务端从 token 写入，请求体传了也会被忽略。
- 设备按绑定关系隔离：普通用户的设备列表只返回通过 `/api/dogUserDevice` 绑定给自己的设备；任务里的 `deviceId` 必须是可访问设备，否则返回 400。

---

## 4. 机器狗业务接口

### 4.1 设备 `/api/dogDevice`

| 接口                         | 方法   | 说明                                                                                                    |
| ---------------------------- | ------ | ------------------------------------------------------------------------------------------------------- |
| `/api/dogDevice/getPage`     | GET    | 分页查询：`deviceId`、`deviceType`（模糊）、`status`（精确）、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogDevice/getAll`      | GET    | 全量列表                                                                                                |
| `/api/dogDevice`             | POST   | 新增，`deviceId` 必填；`status` 默认 `offline`                                                          |
| `/api/dogDevice/{id}`        | GET    | 详情                                                                                                    |
| `/api/dogDevice/{id}`        | PUT    | 修改（只更新传入的字段）                                                                                |
| `/api/dogDevice/{id}`        | DELETE | 软删除                                                                                                  |
| `/api/dogDevice/status/{id}` | PUT    | 仅回写状态，请求体 `{ "status": "online" }`（不传 `status` 时按 `offline` 处理）                        |

字段：`deviceId`、`deviceType`、`status`、`deviceIp`、`model`、`speed`、`height`、`battery`、`course`、`location`、`commTime`（只读）、`createTime`、`updateTime`。

> 普通用户的设备查询只返回已绑定给自己的设备（见 4.14 用户设备绑定），且新增设备会自动绑定创建者；管理员不受限制。

### 4.2 任务 `/api/dogTask`

| 接口                       | 方法               | 说明                                                                                                      |
| -------------------------- | ------------------ | --------------------------------------------------------------------------------------------------------- |
| `/api/dogTask/getPage`     | GET                | `taskName`（模糊）、`taskStatus`（精确）、`deviceId`、`userId`、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogTask/getAll`      | GET                | 全量列表                                                                                                  |
| `/api/dogTask`             | POST               | 新增，`taskName` 必填；`taskStatus` 默认 `pending`                                                        |
| `/api/dogTask/{id}`        | GET / PUT / DELETE | 详情 / 修改 / 软删除                                                                                      |
| `/api/dogTask/status/{id}` | PUT                | 仅回写状态，请求体 `{ "taskStatus": "running" }`                                                          |

字段：`taskName`、`taskDescription`、`taskStatus`、`workflowId`、`workflowUuid`、`deviceId`、`routeId`、`createTime`、`updateTime`。

### 4.3 路线 `/api/dogRoute`

| 接口                    | 方法               | 说明                                                                                         |
| ----------------------- | ------------------ | -------------------------------------------------------------------------------------------- |
| `/api/dogRoute/getPage` | GET                | `routeName`、`routeType`、`routeStatus`、`parkId`、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogRoute/getAll`  | GET                | 全量列表                                                                                     |
| `/api/dogRoute`         | POST               | 新增，`routeName`、`modelId` 必填；`parkId` 可选                                             |
| `/api/dogRoute/{id}`    | GET / PUT / DELETE | 详情 / 修改 / 软删除                                                                         |

字段：`routeName`、`routeType`、`routeStatus`、`routeDescription`、`parkId`（所属园区，可选）、`modelId`（`NOT NULL`）、`routeData`（建议 JSON 字符串）、`createTime`、`updateTime`。

> `getPage` 会按 `parkId` 关联 `dog_park` 返回 `parkName`（所属园区名称）；指定 `parkId` 但园区不存在时返回 400。

### 4.4 模型 `/api/dogModel`

| 接口                    | 方法               | 说明                                                                |
| ----------------------- | ------------------ | ------------------------------------------------------------------- |
| `/api/dogModel/getPage` | GET                | `modelName`、`modelType`、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogModel/getAll`  | GET                | 全量列表                                                            |
| `/api/dogModel`         | POST               | 新增，`modelName` 必填                                              |
| `/api/dogModel/{id}`    | GET / PUT / DELETE | 详情 / 修改 / 软删除                                                |

字段：`modelName`、`modelType`、`fileName`、`filePath`、`fileFormat`、`fileSize`、`remarks`、`createTime`、`updateTime`。

### 4.5 媒体 `/api/dogMedia`

| 接口                    | 方法               | 说明                                                                |
| ----------------------- | ------------------ | ------------------------------------------------------------------- |
| `/api/dogMedia/getPage` | GET                | `mediaName`、`mediaType`、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogMedia/getAll`  | GET                | 全量列表                                                            |
| `/api/dogMedia`         | POST               | 新增，`mediaName` 必填                                              |
| `/api/dogMedia/{id}`    | GET / PUT / DELETE | 详情 / 修改 / 软删除                                                |

字段：`mediaName`、`mediaType`、`mediaPath`、`mediaSize`、`remarks`、`createTime`、`updateTime`。

### 4.6 指令 `/api/dogCommand`

| 接口                          | 方法               | 说明                                                                                                    |
| ----------------------------- | ------------------ | ------------------------------------------------------------------------------------------------------- |
| `/api/dogCommand/getPage`     | GET                | `commandName`、`commandType`、`status`、`deviceId`、`taskId`、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogCommand/getAll`      | GET                | 全量列表                                                                                                |
| `/api/dogCommand`             | POST               | 新增，`commandName` 必填；`status` 默认 `pending`                                                       |
| `/api/dogCommand/{id}`        | GET / PUT / DELETE | 详情 / 修改 / 软删除                                                                                    |
| `/api/dogCommand/status/{id}` | PUT                | 仅回写状态与返回值，请求体 `{ "status": "done", "returnValue": "..." }`                                 |

字段：`commandName`、`commandType`、`commandParams`、`deviceId`、`status`、`returnValue`、`taskId`、`createTime`、`updateTime`。

### 4.7 参数配置 `/api/dogConfig`

| 接口                     | 方法               | 说明                                                    |
| ------------------------ | ------------------ | ------------------------------------------------------- |
| `/api/dogConfig/getPage` | GET                | `configName`、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogConfig/getAll`  | GET                | 全量列表                                                |
| `/api/dogConfig`         | POST               | 新增，`configName` 必填                                 |
| `/api/dogConfig/{id}`    | GET / PUT / DELETE | 详情 / 修改 / 软删除                                    |

字段：`configName`、`configValue`、`description`、`createTime`、`updateTime`。

### 4.8 服务 `/api/dogServices`

| 接口                      | 方法               | 说明                                                       |
| ------------------------- | ------------------ | ---------------------------------------------------------- |
| `/api/dogServices/getAll` | GET                | 全量列表（无分页）：`type`（模糊）、`beginTime`、`endTime` |
| `/api/dogServices`        | POST               | 新增                                                       |
| `/api/dogServices/{id}`   | GET / PUT / DELETE | 详情 / 修改 / 软删除                                       |

字段：`type`、`params`、`createTime`、`updateTime`。

### 4.9 工作流 `/api/dogWorkflow`

| 接口                               | 方法               | 说明                                       |
| ---------------------------------- | ------------------ | ------------------------------------------ |
| `/api/dogWorkflow`                 | POST               | 新增，`uuid` 必填                          |
| `/api/dogWorkflow/{id}`            | GET / PUT / DELETE | 详情 / 修改 / 软删除                       |
| `/api/dogWorkflow/uuid/{uuid}`     | GET                | 按 UUID 查询                               |
| `/api/dogWorkflow/byTask/{taskId}` | GET                | 按任务查询已保存的流程（流程设计页面回显） |

字段：`uuid`、`nodes`、`edges`、`taskId`、`commandJsonString`、`createTime`、`updateTime`。

新增 / 修改时若带 `taskId`，会同步回写 `dog_task.workflow_id` 与 `dog_task.workflow_uuid`，
任务启动和再次进入流程设计都依赖该关联。

新增 / 修改时若未显式传 `commandJsonString`，后端会根据 `nodes` / `edges` 重新生成指令 JSON
（设备、航线、服务参数均会被展开），结构与 mgmt_platform 的 `DronesCommandWebsocket` 一致：

```json
{
  "type": "toros",
  "deviceId": "DOG-001",
  "route": [{ "wpId": 1, "lat": 0.0, "lon": 0.0, "alt": 0.5 }],
  "tasks": [
    {
      "taskId": "node_x",
      "fromWp": { "wpId": 1, "lat": 0.0, "lon": 0.0, "alt": 0.5 },
      "toWp": { "wpId": 2, "lat": 1.0, "lon": 2.0, "alt": 0.5 },
      "event": "init_x",
      "actions": [
        {
          "actionId": "node_y",
          "after": "init_x",
          "type": "TAKEOFF",
          "params": {}
        }
      ]
    }
  ],
  "services": [{ "type": "YOLO", "params": {} }],
  "onErrorActions": [
    { "after": "fail_y", "type": "", "params": { "reason": "超时" } }
  ]
}
```

流程设计页的「查看JSON」展示的就是该 `commandJsonString`（未保存或生成失败时提示获取失败）。
地图航点为世界坐标，`x` 作为 `lon`、`y` 作为 `lat`，`wpId` 从 1 开始递增。

> 管理后台已不再提供工作流页面（菜单已下线），这些接口仅保留给任务 / 设备侧使用；也没有工作流列表（分页）接口。

### 4.10 统计 `/api/dogStatistics`

| 接口                                  | 方法 | 说明                                              |
| ------------------------------------- | ---- | ------------------------------------------------- |
| `/api/dogStatistics/all`              | GET  | 设备 / 任务 / 模型 / 路线总量                     |
| `/api/dogStatistics/allType`          | GET  | 上周 / 本周数量趋势（**占位实现，当前返回全 0**） |
| `/api/dogStatistics/deviceTypeNumber` | GET  | 设备类型分布（**占位实现，当前返回空数组**）      |

`/api/dogStatistics/all` 返回 4 个对象，字段为 `name`、`value`、`percent`、`data`、`bgColor`、`color`、`duration`。

---

### 4.11 设备网关 `/api/dogGateway`

设备（机器人本体）接入用的接口，鉴权方式是 **设备 token**，与用户 JWT 无关。

`deviceId` 建议由设备端从硬件信息派生，保证一台设备一个固定 ID（客户端实现见下文）：

| 优先级 | 来源                                   | 说明                   |
| ------ | -------------------------------------- | ---------------------- |
| 1      | 板级序列号（device tree）              | RK35xx 等 ARM 板卡     |
| 2      | `/proc/cpuinfo` 的 `Serial`            | 树莓派等               |
| 3      | eMMC CID                               | 芯片级唯一             |
| 4      | DMI product UUID / board serial        | x86 工控机             |
| 5      | 物理网卡 MAC（跳过虚拟网卡与随机 MAC） | 通用兜底               |
| 6      | `/etc/machine-id`                      | 仅兜底，克隆镜像会重复 |

注册时可选上报 `deviceIdSource`（如 `mac-address`），便于排查 ID 来源。

> Ubuntu 上 `/sys/class/dmi/id/*`、`/sys/block/mmcblk*/device/cid` 默认仅 root 可读；非 root 运行时会退化为物理网卡 MAC。客户端 `--explain-device-id` 可打印每个来源的探测状态，`--device-id-file` 可固定首次检测结果。

| 接口                                           | 方法   | 说明                                                                                                                                                                                               |
| ---------------------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/api/dogGateway/auth/{deviceId}`              | GET    | 下发设备 token（10 分钟有效，使用时自动续期）                                                                                                                                                      |
| `/api/dogGateway/register`                     | POST   | 注册/刷新设备，需 `Authorization: <token>`（兼容 `Bearer <token>`）；设备初始状态 `offline`；`deviceId` 由设备硬件派生，可选字段 `deviceIdSource` 说明来源                                         |
| `/api/notice/{deviceId}/{token}`               | WS     | 设备长连接，用 URL 中的 token 鉴权                                                                                                                                                                 |
| `/api/dogGateway/dispatch`                     | POST   | 管理端手动推送任意 JSON 到设备（需用户 JWT）                                                                                                                                                       |
| `/api/dogGateway/connections`                  | GET    | 当前在线设备列表（需用户 JWT）                                                                                                                                                                     |
| `/api/dogGateway/disconnect/{deviceId}`        | POST   | 主动断开设备连接（需用户 JWT）；客户端会自动重连，可用于强制刷新 token/注册或验证重连                                                                                                              |
| `/api/dogGateway/assign`                       | POST   | 绑定/分配设备（需用户 JWT，前端「设备列表 → 绑定用户」使用）：按硬件 `deviceId` 绑定设备，请求体 `{ "deviceId": "dog-001", "userId": 123 }`；`userId` 省略时为当前用户，普通用户只能认领未绑定设备 |
| `/api/dogGateway/assign/{deviceId}`            | GET    | 查询设备已分配的用户（普通用户只返回自己）                                                                                                                                                         |
| `/api/dogGateway/assign/{deviceId}?userId=123` | DELETE | 解绑：普通用户只能解绑自己，管理员可指定 `userId`                                                                                                                                                  |

> 设备注册报文不包含用户信息（`register` 只上报硬件与系统信息）。归属关系通过 `assign` 接口单独建立，设备注册前不可分配（返回 400）；注册完成后可随时分配或调整。

WebSocket 报文：

| 方向          | 报文                                                                                                                                                                                  | 说明                                                                                                                                             |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| 设备 → 服务端 | `{"type":"heartbeat","deviceId":"dog-001","status":"success","drones":{"speed":"1.2","height":"0","battery":"88","course":"90","location":"31.23,121.47"}}`                           | 心跳；服务端回 `{"type":"report","status":"online","msg":"success"}`，并更新 `dog_device.status/comm_time` 及遥测字段                            |
| 服务端 → 设备 | `{"type":"server-command","deviceId":"dog-001","missionId":"<指令ID>","commandId":<指令ID>,"commandName":"巡检","commandType":"task","taskId":null,"params":{...},"timestamp":"..."}` | 新建指令时若设备在线立即推送；`params` 为指令的 `commandParams`（字符串会尝试解析为 JSON）                                                       |
| 设备 → 服务端 | `{"type":"server-command","commandId":<指令ID>,"status":"success","returnValue":"..."}`                                                                                               | 执行结果，写回 `dog_command.status/return_value`（也兼容 `report` 的 `id` 与 `torosReport` 的 `missionId`）                                      |
| 设备 → 服务端 | `{"type":"file_start","streamId":"stream-1","fileName":"a.jpg","mediaType":"image"}` → 若干二进制帧 → `{"type":"file_end","streamId":"stream-1"}`                                     | 文件上传；服务端回 `{"type":"file_ack","streamId":"stream-1","status":"success","path":"..."}`，文件写入 `[dog] upload_dir` 并登记到 `dog_media` |

连接与重连约定：

- 客户端每 `interval` 秒发一次 heartbeat，**必须收到服务端回包**才算链路健康；收不到回包（半开连接）应主动断开并重连。
- 断线后应重新 `GET /api/dogGateway/auth/{deviceId}` 取新 token（有效期 10 分钟）、重新 `register`、再建 WS；本仓库客户端默认按指数退避自动重连。
- 服务端主动关闭（Close 帧）同样按断线处理，客户端应自动重连。
- 断线期间未回报的指令保留在 `sent` 状态，不会自动重放。

其它说明：

- 单条上传流上限 256 MiB，超过会被丢弃；文件名会被清洗（去掉目录与特殊字符）。
- 连接表保存在后端进程内存中，网关按单实例部署；多副本需要引入共享总线。
- 设备端实现见仓库 `client/`（`dog-device-client`，`client/README.md`），端到端验证见 `backend/tests/business/dog_gateway_tests.rs`。

---

### 4.12 园区 `/api/dogPark`

| 接口                   | 方法               | 说明                                                          |
| ---------------------- | ------------------ | ------------------------------------------------------------- |
| `/api/dogPark/getPage` | GET                | `parkName`（模糊）、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogPark/getAll`  | GET                | 全量列表（仅当前用户可见范围）                                |
| `/api/dogPark`         | POST               | 新增，`parkName` 必填；`parkDescription`、`parkAddress` 可选  |
| `/api/dogPark/{id}`    | GET / PUT / DELETE | 详情 / 修改 / 软删除                                          |

响应字段：`id`、`parkName`、`parkDescription`、`parkAddress`、`userId`、`userName`、`createTime`、`updateTime`。

> `getPage` 会按 `userId` 关联 `p_sys_user` 返回 `userName`（所属用户名），前端列表展示该字段；`getAll` 与详情 / 新增 / 修改接口的 `userName` 为 `null`。

---

### 4.13 地图 `/api/dogMap`

| 接口                  | 方法               | 说明                                                                                                                                                                                                           |
| --------------------- | ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/api/dogMap/getPage` | GET                | `mapName`（模糊）、`mapType`、`fileName`（模糊）、`parkId`、`beginTime`、`endTime`、`current`、`size`                                                                                                          |
| `/api/dogMap/getAll`  | GET                | 全量列表（仅当前用户可见范围）                                                                                                                                                                                 |
| `/api/dogMap`         | POST               | 新增，`mapName`、`parkId` 必填；`mapType`、`remarks` 可选                                                                                                                                                      |
| `/api/dogMap/upload`  | POST               | 上传 `.zip` 地图包（`multipart/form-data`）：`file` = 地图包，`mapInfo` = JSON 字符串 `{ "parkId": 1, "mapName": "...", "mapType": "离线地图", "remarks": "..." }`；带 `id` 时替换已有地图的包与信息，否则新建 |
| `/api/dogMap/{id}`    | GET / PUT / DELETE | 详情 / 修改 / 软删除                                                                                                                                                                                           |

- 地图包仅支持 `.zip`，保存到 `[dog] upload_dir` 下的 `maps/` 目录；响应中的 `fileName`、`filePath`、`fileFormat`、`fileSize` 由服务端写入。
- `parkId` 指向园区：新增（含上传新建）必填，必须是当前用户可访问的园区，否则返回 400；修改时可不传（保持不变）。
- `getPage` 会按 `parkId` 关联 `dog_park` 返回 `parkName`（所属园区名称），前端列表展示该字段；详情 / 新增 / 修改接口同样返回 `parkName`。
- 响应字段：`id`、`mapName`、`mapType`、`mapData`、`parkId`、`parkName`、`userId`、`fileName`、`filePath`、`fileFormat`、`fileSize`、`remarks`、`createTime`、`updateTime`。

---

### 4.14 用户设备绑定 `/api/dogUserDevice`

设备与用户是多对多关系（一台设备可被多个用户使用）。

| 接口                                 | 方法         | 说明                                                                                         |
| ------------------------------------ | ------------ | -------------------------------------------------------------------------------------------- |
| `/api/dogUserDevice/getPage`         | GET          | `userId`、`deviceId`、`current`、`size`                                                      |
| `/api/dogUserDevice/getAll`          | GET          | 全量（按当前用户可见范围）                                                                   |
| `/api/dogUserDevice`                 | POST         | 绑定，请求体 `{ "deviceId": 123, "userId": 456 }`；`userId` 仅管理员有效，普通用户强制为自己 |
| `/api/dogUserDevice/{id}`            | GET / DELETE | 详情 / 解绑（软删除，可重新绑定）                                                            |
| `/api/dogUserDevice/byUser/{userId}` | GET          | 某用户的全部绑定                                                                             |

- `deviceId` 是 `dog_device.id`（不是硬件编号）。`设备绑定` 已不再单独占用菜单，绑定/解绑在「设备列表」页完成（`/api/dogGateway/assign*` 按硬件编号操作）。
- 普通用户只能绑定尚未被绑定的设备（认领）；管理员可把同一设备绑定给多个用户。
- 响应字段：`id`、`userId`、`deviceId`、`deviceCode`（硬件设备号）、`createTime`。

## 5. 调用示例

```bash
# 1. 登录（password 需替换为前端 AES 加密后的密文）
TOKEN=$(curl -s -X POST http://127.0.0.1:3000/api/token \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode 'username=admin' \
  --data-urlencode 'password=<AES_BASE64>' \
  --data-urlencode 'grant_type=password' \
  --data-urlencode 'scope=server' | jq -r '.data.access_token')

# 2. 分页查询设备
curl -s 'http://127.0.0.1:3000/api/dogDevice/getPage?current=1&size=10&status=online' \
  -H "Authorization: Bearer $TOKEN"

# 3. 创建设备
curl -s -X POST http://127.0.0.1:3000/api/dogDevice \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"deviceId":"dog-001","deviceType":"X1","status":"online","deviceIp":"192.168.1.20"}'

# 4. 设备端回写状态
curl -s -X PUT http://127.0.0.1:3000/api/dogDevice/status/1700000000000 \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"status":"online"}'
```

---

## 6. 本地开发命令

```bash
# 后端（默认 3000 端口，启动时自动执行迁移）
cd backend && cargo run

# 前端
cd frontend && pnpm dev

# 设备客户端（认证 → 注册 → 建链 → 心跳验证）
cd client && cargo run -- --device-id dog-001 --once

# 后端自测（含设备网关端到端测试）
cd backend && cargo test && cargo clippy -- -D warnings

# 客户端自测
cd client && cargo test
```

新增菜单/页面时需要同时维护：

1. `backend/src/api/routes.rs` 注册路由；
2. `backend/migrations/*` 中的菜单数据（左侧菜单由数据库 `p_sys_menu` 驱动，并需在 `p_sys_role_menu` 中授权）；
3. `frontend/src/views/**` 页面与 `frontend/src/api/*.ts` 请求封装，菜单的 `component` 字段需能在 `frontend/src/views` 下唯一匹配到 `index.vue`；
4. 若菜单用到的图标是离线图标，还需在 `frontend/src/components/ReIcon/src/offlineIcon.ts` 注册。

设备上传目录通过 `backend/config/config.toml` 的 `[dog] upload_dir` 配置（默认 `data/device_uploads`）。
