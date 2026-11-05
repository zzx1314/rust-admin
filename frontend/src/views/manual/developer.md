# 开发对接手册

本手册面向需要调用管理后台接口的开发者（设备端、算法侧、第三方系统）。所有接口均由 Rust 后端（axum + sea-orm + SQLite）提供，管理后台通过 Vite 代理把 `/api` 转发到 `http://127.0.0.1:3000`。

## 1. 服务地址与环境

| 环境   | 地址                                        |
| ------ | ------------------------------------------- |
| 后端   | `http://127.0.0.1:3000`（端口见 `backend/config/config.toml`） |
| 管理后台 | `pnpm dev` 后访问 Vite 输出地址，`/api` 自动代理到后端 |

数据库文件为 `backend/data/users.db`（SQLite），服务启动时自动执行 `backend/migrations` 下的 SQL 迁移。

---

## 2. 认证

### 2.1 获取 token

```http
POST /api/token
Content-Type: application/x-www-form-urlencoded
```

| 参数         | 必填 | 说明                                                         |
| ------------ | ---- | ------------------------------------------------------------ |
| `username`   | 是   | 用户名                                                       |
| `password`   | 是   | 密码密文。为 AES-128-CBC + ZeroPadding 后的 Base64 字符串（与管理后台登录页一致，参考 `frontend/src/utils/aes.ts`） |
| `grant_type` | 是   | 固定 `password`                                              |
| `scope`      | 是   | 固定 `server`                                                |

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

| 接口                                  | 方法 | 说明                                       |
| ------------------------------------- | ---- | ------------------------------------------ |
| `/api/token/refresh/{refresh_token}`  | GET  | 用 refresh token 换新的 `accessToken`（响应为驼峰字段 `accessToken`/`refreshToken`/`expiresIn`） |
| `/api/token/logout`                   | GET  | 注销当前 token（需 Bearer）                |
| `/api/token/check_token`              | GET  | 校验 token 是否有效（需 Bearer）           |
| `/api/sysUser/info`                   | GET  | 当前登录用户信息（需 Bearer）              |
| `/api/sysMenu/user-menu`              | GET  | 当前用户可见菜单树（需 Bearer）            |

---

## 3. 通用约定

### 3.1 响应结构

```json
{ "code": 10200, "msg": "success", "data": { } }
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

---

## 4. 机器狗业务接口

### 4.1 设备 `/api/dogDevice`

| 接口                            | 方法   | 说明 |
| ------------------------------- | ------ | ---- |
| `/api/dogDevice/getPage`        | GET    | 分页查询：`deviceId`、`deviceType`（模糊）、`status`（精确）、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogDevice/getAll`         | GET    | 全量列表 |
| `/api/dogDevice`                | POST   | 新增，`deviceId` 必填；`status` 默认 `offline` |
| `/api/dogDevice/{id}`           | GET    | 详情 |
| `/api/dogDevice/{id}`           | PUT    | 修改（只更新传入的字段） |
| `/api/dogDevice/{id}`           | DELETE | 软删除 |
| `/api/dogDevice/status/{id}`    | PUT    | 仅回写状态，请求体 `{ "status": "online" }`（不传 `status` 时按 `offline` 处理） |

字段：`deviceId`、`deviceType`、`status`、`deviceIp`、`model`、`speed`、`height`、`battery`、`course`、`location`、`commTime`（只读）、`createTime`、`updateTime`。

### 4.2 任务 `/api/dogTask`

| 接口                          | 方法   | 说明 |
| ----------------------------- | ------ | ---- |
| `/api/dogTask/getPage`        | GET    | `taskName`（模糊）、`taskStatus`（精确）、`deviceId`、`current`、`size` |
| `/api/dogTask/getAll`         | GET    | 全量列表 |
| `/api/dogTask`                | POST   | 新增，`taskName` 必填；`taskStatus` 默认 `pending` |
| `/api/dogTask/{id}`           | GET / PUT / DELETE | 详情 / 修改 / 软删除 |
| `/api/dogTask/status/{id}`    | PUT    | 仅回写状态，请求体 `{ "taskStatus": "running" }` |

字段：`taskName`、`taskDescription`、`taskStatus`、`workflowId`、`workflowUuid`、`deviceId`、`routeId`、`createTime`、`updateTime`。

### 4.3 路线 `/api/dogRoute`

| 接口                       | 方法   | 说明 |
| -------------------------- | ------ | ---- |
| `/api/dogRoute/getPage`    | GET    | `routeName`、`routeType`、`routeStatus`、`current`、`size` |
| `/api/dogRoute/getAll`     | GET    | 全量列表 |
| `/api/dogRoute`            | POST   | 新增，`routeName`、`modelId` 必填 |
| `/api/dogRoute/{id}`       | GET / PUT / DELETE | 详情 / 修改 / 软删除 |

字段：`routeName`、`routeType`、`routeStatus`、`routeDescription`、`modelId`（`NOT NULL`）、`routeData`（建议 JSON 字符串）、`createTime`、`updateTime`。

### 4.4 模型 `/api/dogModel`

| 接口                       | 方法   | 说明 |
| -------------------------- | ------ | ---- |
| `/api/dogModel/getPage`    | GET    | `modelName`、`modelType`、`current`、`size` |
| `/api/dogModel/getAll`     | GET    | 全量列表 |
| `/api/dogModel`            | POST   | 新增，`modelName` 必填 |
| `/api/dogModel/{id}`       | GET / PUT / DELETE | 详情 / 修改 / 软删除 |

字段：`modelName`、`modelType`、`fileName`、`filePath`、`fileFormat`、`fileSize`、`remarks`、`createTime`、`updateTime`。

### 4.5 媒体 `/api/dogMedia`

| 接口                       | 方法   | 说明 |
| -------------------------- | ------ | ---- |
| `/api/dogMedia/getPage`    | GET    | `mediaName`、`mediaType`、`current`、`size` |
| `/api/dogMedia/getAll`     | GET    | 全量列表 |
| `/api/dogMedia`            | POST   | 新增，`mediaName` 必填 |
| `/api/dogMedia/{id}`       | GET / PUT / DELETE | 详情 / 修改 / 软删除 |

字段：`mediaName`、`mediaType`、`mediaPath`、`mediaSize`、`remarks`、`createTime`、`updateTime`。

### 4.6 指令 `/api/dogCommand`

| 接口                            | 方法   | 说明 |
| ------------------------------- | ------ | ---- |
| `/api/dogCommand/getPage`       | GET    | `commandName`、`commandType`、`status`、`deviceId`、`taskId`、`current`、`size` |
| `/api/dogCommand/getAll`        | GET    | 全量列表 |
| `/api/dogCommand`               | POST   | 新增，`commandName` 必填；`status` 默认 `pending` |
| `/api/dogCommand/{id}`          | GET / PUT / DELETE | 详情 / 修改 / 软删除 |
| `/api/dogCommand/status/{id}`   | PUT    | 仅回写状态与返回值，请求体 `{ "status": "done", "returnValue": "..." }` |

字段：`commandName`、`commandType`、`commandParams`、`deviceId`、`status`、`returnValue`、`taskId`、`createTime`、`updateTime`。

### 4.7 参数配置 `/api/dogConfig`

| 接口                        | 方法   | 说明 |
| --------------------------- | ------ | ---- |
| `/api/dogConfig/getPage`    | GET    | `configName`、`current`、`size` |
| `/api/dogConfig/getAll`     | GET    | 全量列表 |
| `/api/dogConfig`            | POST   | 新增，`configName` 必填 |
| `/api/dogConfig/{id}`       | GET / PUT / DELETE | 详情 / 修改 / 软删除 |

字段：`configName`、`configValue`、`description`、`createTime`、`updateTime`。

### 4.8 服务 `/api/dogServices`

| 接口                        | 方法   | 说明 |
| --------------------------- | ------ | ---- |
| `/api/dogServices/getAll`   | GET    | 全量列表（无分页） |
| `/api/dogServices`          | POST   | 新增 |
| `/api/dogServices/{id}`     | GET / PUT / DELETE | 详情 / 修改 / 软删除 |

字段：`type`、`params`、`createTime`、`updateTime`。

### 4.9 工作流 `/api/dogWorkflow`

| 接口                                | 方法   | 说明 |
| ----------------------------------- | ------ | ---- |
| `/api/dogWorkflow`                  | POST   | 新增，`uuid` 必填 |
| `/api/dogWorkflow/{id}`             | GET / PUT / DELETE | 详情 / 修改 / 软删除 |
| `/api/dogWorkflow/uuid/{uuid}`      | GET    | 按 UUID 查询 |

字段：`uuid`、`nodes`、`edges`、`taskId`、`commandJsonString`、`createTime`、`updateTime`。

> 目前没有工作流列表（分页）接口，管理后台页面表格因此没有数据源。

### 4.10 统计 `/api/dogStatistics`

| 接口                              | 方法 | 说明 |
| --------------------------------- | ---- | ---- |
| `/api/dogStatistics/all`          | GET  | 设备 / 任务 / 模型 / 路线总量 |
| `/api/dogStatistics/allType`      | GET  | 上周 / 本周数量趋势（**占位实现，当前返回全 0**） |
| `/api/dogStatistics/deviceTypeNumber` | GET | 设备类型分布（**占位实现，当前返回空数组**） |

`/api/dogStatistics/all` 返回 4 个对象，字段为 `name`、`value`、`percent`、`data`、`bgColor`、`color`、`duration`。

---

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

# 后端自测
cd backend && cargo test && cargo clippy -- -D warnings
```

新增菜单/页面时需要同时维护：

1. `backend/src/api/routes.rs` 注册路由；
2. `backend/migrations/*` 中的菜单数据（左侧菜单由数据库 `p_sys_menu` 驱动，并需在 `p_sys_role_menu` 中授权）；
3. `frontend/src/views/**` 页面与 `frontend/src/api/*.ts` 请求封装，菜单的 `component` 字段需能在 `frontend/src/views` 下唯一匹配到 `index.vue`；
4. 若菜单用到的图标是离线图标，还需在 `frontend/src/components/ReIcon/src/offlineIcon.ts` 注册。
