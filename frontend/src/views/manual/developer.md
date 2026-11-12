# 开发对接手册

本手册面向需要调用管理后台接口的开发者。所有接口均由 Rust 后端（axum + sea-orm + SQLite）提供，管理后台通过 Vite 代理把 `/api` 转发到 `http://127.0.0.1:3000`。

> 园区、设备、任务、路线、指令下发与服务编排已由 `business/` 独立业务平台承载，中台不再提供这些接口。

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

除登录接口使用表单外，其余接口均为 `Content-Type: application/json`，字段使用 **小驼峰**（如 `mediaName`、`mapName`）。

### 3.3 其它约定

- 所有删除均为 **软删除**（`is_deleted = 1`），查询接口不会返回已删除数据。
- 时间字段在响应中统一为 **北京时间** 字符串 `YYYY-MM-DD HH:mm:ss`；查询条件 `beginTime` / `endTime` 也按北京时间解析（`YYYY-MM-DD` 或 `YYYY-MM-DD HH:mm:ss`，结束时间会自动补到当天 23:59:59）。
- 分页参数为 `current`（从 1 开始）与 `size`，默认 `1` / `10`。
- 主键 `id` 在创建时由服务端生成，客户端无需传入。
- 地图按创建者做数据隔离：普通用户只能读写 `userId = 当前用户` 的记录（详情 / 修改 / 删除他人数据返回 404）；管理员（角色编码 `110`）不受限制。`userId` 由服务端从 token 写入，请求体传了也会被忽略。
- 模型、媒体、参数配置为共享资源，不做用户级隔离。

---

## 4. 中台业务接口

### 4.1 模型 `/api/dogModel`

| 接口                    | 方法               | 说明                                                                |
| ----------------------- | ------------------ | ------------------------------------------------------------------- |
| `/api/dogModel/getPage` | GET                | `modelName`、`modelType`、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogModel/getAll`  | GET                | 全量列表                                                            |
| `/api/dogModel`         | POST               | 新增，`modelName` 必填                                              |
| `/api/dogModel/{id}`    | GET / PUT / DELETE | 详情 / 修改 / 软删除                                                |

字段：`modelName`、`modelType`、`fileName`、`filePath`、`fileFormat`、`fileSize`、`remarks`、`createTime`、`updateTime`。

### 4.2 媒体 `/api/dogMedia`

| 接口                    | 方法               | 说明                                                                |
| ----------------------- | ------------------ | ------------------------------------------------------------------- |
| `/api/dogMedia/getPage` | GET                | `mediaName`、`mediaType`、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogMedia/getAll`  | GET                | 全量列表                                                            |
| `/api/dogMedia`         | POST               | 新增，`mediaName` 必填                                              |
| `/api/dogMedia/{id}`    | GET / PUT / DELETE | 详情 / 修改 / 软删除                                                |

字段：`mediaName`、`mediaType`、`mediaPath`、`mediaSize`、`remarks`、`createTime`、`updateTime`。

### 4.3 参数配置 `/api/dogConfig`

| 接口                     | 方法               | 说明                                                    |
| ------------------------ | ------------------ | ------------------------------------------------------- |
| `/api/dogConfig/getPage` | GET                | `configName`、`beginTime`、`endTime`、`current`、`size` |
| `/api/dogConfig/getAll`  | GET                | 全量列表                                                |
| `/api/dogConfig`         | POST               | 新增，`configName` 必填                                 |
| `/api/dogConfig/{id}`    | GET / PUT / DELETE | 详情 / 修改 / 软删除                                    |

字段：`configName`、`configValue`、`description`、`createTime`、`updateTime`。

### 4.4 统计 `/api/dogStatistics`

| 接口                         | 方法 | 说明                                                          |
| ---------------------------- | ---- | ------------------------------------------------------------- |
| `/api/dogStatistics/all`     | GET  | 地图 / 模型 / 媒体 / 参数配置总量与本周每日新增、周同比百分比 |
| `/api/dogStatistics/allType` | GET  | 上周 / 本周四类资源的新增数量序列（各 7 天）                  |

`/api/dogStatistics/all` 返回 4 个对象，字段为 `name`、`value`、`percent`、`data`、`bgColor`、`color`、`duration`。

`/api/dogStatistics/allType` 返回 2 个对象（上周、本周），字段为 `mapCount`、`modelCount`、`mediaCount`、`configCount`。

### 4.5 地图 `/api/dogMap`

| 接口                          | 方法               | 说明                                                                                                                                                                                                                |
| ----------------------------- | ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/api/dogMap/getPage`         | GET                | `mapName`（模糊）、`mapType`、`mapStatus`、`fileName`（模糊）、`deviceCode`（模糊）、`parkCode`（精确）、`beginTime`、`endTime`、`current`、`size`                                                                  |
| `/api/dogMap/getAll`          | GET                | 全量列表（仅当前用户可见范围）                                                                                                                                                                                      |
| `/api/dogMap`                 | POST               | 新增，`mapName` 必填；`mapType`、`parkCode`、`parkName`、`deviceCode`、`remarks`、`mapVersion`、`frameId`、`crs`、`mapOrigin`、`resolution`、`floors`、`roadCount`、`nogoCount`、`mapStatus`（草稿 / 部分标定）可选 |
| `/api/dogMap/upload`          | POST               | 上传 `.zip` 地图包（`multipart/form-data`）：`file` = 地图包，`mapInfo` = JSON 字符串，字段同新增；带 `id` 时替换已有地图的包与信息，否则新建                                                                       |
| `/api/dogMap/{id}/image`      | GET                | 返回地图包栅格图（PNG base64）与 `resolution` / `origin` / 宽高                                                                                                                                                     |
| `/api/dogMap/{id}/publish`    | POST               | 发布版本：状态推进为「已发布」（部分标定保持「部分标定」）并记录发布时间                                                                                                                                            |
| `/api/dogMap/{id}/newVersion` | POST               | 基于当前版本派生下一个版本号并立即发布                                                                                                                                                                              |
| `/api/dogMap/{id}`            | GET / PUT / DELETE | 详情 / 修改 / 软删除                                                                                                                                                                                                |

- 地图包仅支持 `.zip`，保存到 `[dog] upload_dir` 下的 `maps/` 目录；响应中的 `fileName`、`filePath`、`fileFormat`、`fileSize` 由服务端写入。
- **园区归属**：`parkCode` / `parkName` 来自 `business/` 业务平台的园区台账（`parks.park_code` / `parks.park_name`），中台不建园区表、不做外键约束。设备端建图回传时由业务平台随 `mapInfo` 写入，业务平台可用 `parkCode` 反查某个园区下的地图；历史地图为空。
- **上报设备**：`deviceCode` 是采集这张地图的设备编号（业务平台 `devices.device_id`），同样由业务平台随 `mapInfo` 写入，中台只做冗余存储、不建外键。手工上传的地图可以留空；派生新版本时自动继承。
- 响应字段：`id`、`mapName`、`mapType`、`parkCode`、`parkName`、`deviceCode`、`mapData`、`userId`、`fileName`、`filePath`、`fileFormat`、`fileSize`、`remarks`、`mapVersion`、`frameId`、`crs`、`mapOrigin`、`resolution`、`floors`、`roadCount`、`nogoCount`、`mapStatus`、`mapHash`、`mapWidth`、`mapHeight`、`publishTime`、`createTime`、`updateTime`。
- **上传即解析**：后端从地图包内 `model.pgm` / `model.yaml` 解析 `resolution`、`origin: [x, y, yaw]`、像素宽高，并计算整个 `.zip` 的 SHA-256 写入 `mapHash`、`mapWidth`、`mapHeight`。前端表单里的 `resolution` / `mapOrigin` 只用于校验：与包内容不一致会返回 400，留空则由服务端解析结果回写。
- **版本不可变**：版本一旦发布（`已发布` / `部分标定`），`mapVersion`、`frameId`、`mapOrigin`、`resolution`、地图包不再允许修改；需要新坐标系时请上传新包或调用 `newVersion`。

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

# 2. 分页查询模型
curl -s 'http://127.0.0.1:3000/api/dogModel/getPage?current=1&size=10' \
  -H "Authorization: Bearer $TOKEN"

# 3. 新增模型
curl -s -X POST http://127.0.0.1:3000/api/dogModel \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"modelName":"巡检模型","modelType":"detection","fileFormat":"onnx"}'

# 4. 分页查询地图
curl -s 'http://127.0.0.1:3000/api/dogMap/getPage?current=1&size=10' \
  -H "Authorization: Bearer $TOKEN"
```

---

## 6. 本地开发命令

```bash
# 后端（默认 3000 端口，启动时自动执行迁移）
cd backend && ./run.sh

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

地图上传目录通过 `backend/config/config.toml` 的 `[dog] upload_dir` 配置（默认 `data/device_uploads`）。
