# 机器狗业务模块（dog）

中台保留 **地图、模型、媒体、参数配置** 四个资源域的增删改查与首页统计，并新增
**设备台账与用户分配**（设备接入、指令下发仍在业务平台）。

> 园区、任务、路线以及设备接入、指令下发、服务编排由仓库内的 `business/` 独立业务平台
> （Rust 后端 + Vue 控制台，见 `business/README.md`）负责。业务平台把设备同步到中台
> `dog_device` 台账，由中台管理员批量分配用户；业务平台用中台账号登录后只看到
> 自己管辖的设备与设备采集的地图。`business/server` 会直接读取中台 `dog_map` 表，
> 两边 SQLite 文件路径可通过 `ROBODOG_BACKEND_DB` 配置。

---

## 1. 组成

| 层 | 位置 |
| --- | --- |
| 后端业务代码 | `backend/src/business/dog/{map,media,model,config,statistics,device}/` |
| 后端路由 | `backend/src/api/routes.rs`（每个子域一个 `dog_*_routes`，统一挂 `require_auth`） |
| 服务装配 | `backend/src/app.rs` → `App::build_state_with_token_store` |
| 数据库迁移 | `.../7_create_dog_tables.sql`、`.../19_add_map_file_fields.sql`、`.../22_add_map_baseline_*.sql`、`.../23_add_map_dimensions.sql`、`.../24_drop_retired_dog_tables.sql`、`.../25_drop_command_services_tables.sql`、`.../26_add_map_park.sql`、`.../27_add_device_management.sql`（重建 `dog_device` / `dog_user_device`） |
| 菜单 | `backend/migrations/p_sys/9_add_dog_menus.sql`、`.../21_remove_retired_dog_menus.sql`、`.../22_remove_command_services_menus.sql`、`.../27_add_device_management_menu.sql` |
| 前端页面 | `frontend/src/views/dog/{map,media,model,config,device}/` |
| 前端请求封装 | `frontend/src/api/dog{Map,Media,Model,Config,Statistics,Device}.ts` |
| 首页仪表盘 | `frontend/src/views/welcome/index.vue` + `frontend/src/api/dogStatistics.ts` |
| 在线手册 | `frontend/src/views/manual/admin.md`（管理员）、`developer.md`（接口对接） |

每个子域内部固定分层：`domain.rs`（请求/VO/字段映射）、`entity.rs`（SeaORM 实体）、`repository.rs`（SQL/条件构造）、`service.rs`（业务编排与错误映射）、`handlers.rs`（HTTP 层）、`mod.rs`。

---

## 2. 数据表

| 表 | 说明 | 关键字段 |
| --- | --- | --- |
| `dog_map` | 地图基线版本 | `map_name`、`map_type`、`file_name`、`file_path`、`file_format`、`file_size`、`remarks`、`user_id`(创建者)、`map_version`、`frame_id`、`crs`、`map_origin`、`resolution`、`floors`、`road_count`、`nogo_count`、`map_status`(草稿/部分标定/已发布/已归档)、`map_hash`(上传时计算的 SHA-256)、`map_width`/`map_height`、`publish_time`、`park_code`/`park_name`(业务园区归属，可选)；地图以 `.zip` 包上传，文件落盘到 `<upload_dir>/maps/`。上传时后端解析 PGM/YAML 并回写 resolution/origin/尺寸/哈希，表单值与包内容冲突会被拒绝。发布后版本不可变 |
| `dog_model` | 模型 | `model_name`、`model_type`、`file_name`、`file_path`、`file_format`、`file_size`、`remarks` |
| `dog_media` | 媒体 | `media_name`、`media_type`、`media_path`、`media_size`、`remarks` |
| `dog_config` | 参数配置 | `config_name`、`config_value`、`description` |
| `dog_device` | 设备台账（业务平台同步） | `device_id`(业务设备编号)、`device_name`、`model`、`status`、`firmware`、`device_ip`、`park_code`/`park_name`、`user_id`(创建者) |
| `dog_user_device` | 设备-用户分配（多对多） | `user_id`、`device_id`(dog_device.id)、`is_deleted` |

所有表都有 `id`（主键，创建时由服务端生成）、`create_time`、`update_time`、`is_deleted`（软删除标记），查询一律带 `is_deleted = 0`。

已删除的表：`dog_park`、`dog_task`、`dog_route_library`、`dog_workflow`、`dog_command`、`dog_services`。
`dog_device` / `dog_user_device` 在迁移 24 删除后由迁移 27 按新结构重建。

---

## 3. 接口

完整清单、请求字段与调用示例见 **`frontend/src/views/manual/developer.md`**，速查：

```text
GET    /api/<module>/getPage        分页（current/size + 各模块筛选项）
GET    /api/<module>/getAll         全量
POST   /api/<module>                新增
GET    /api/<module>/{id}           详情
PUT    /api/<module>/{id}           修改
DELETE /api/<module>/{id}           软删除
POST   /api/dogMap/upload           上传 .zip 地图包（multipart：file + mapInfo）
GET    /api/dogMap/{id}/image       地图栅格 PNG + 分辨率 / 原点
POST   /api/dogMap/{id}/publish     发布地图版本（记录发布时间）
POST   /api/dogMap/{id}/newVersion  派生并发布下一个不可变版本
GET    /api/dogStatistics/all       地图 / 模型 / 媒体 / 参数配置总量与本周曲线
GET    /api/dogStatistics/allType   上周 / 本周数量趋势

GET    /api/dogDevice/getPage       设备分页（deviceId/deviceName/status/parkCode/userId）
GET    /api/dogDevice/getAll        设备全量（含已分配用户）
POST   /api/dogDevice               新增设备
POST   /api/dogDevice/upsert        业务平台按 deviceId 幂等同步设备
GET    /api/dogDevice/{id}          设备详情
PUT    /api/dogDevice/{id}          修改设备
DELETE /api/dogDevice/{id}          软删除设备
GET    /api/dogDevice/users         可分配用户下拉
GET    /api/dogDevice/byCode/{code} 按设备编号查询（业务平台内部使用）
POST   /api/dogDevice/assignUsers   批量分配 { deviceIds, userIds, mode: append|replace }
POST   /api/dogDevice/unassignUsers 批量解除分配 { deviceIds, userIds }
```

- `module` ∈ `dogMap`、`dogMedia`、`dogModel`、`dogConfig`，均提供 `getPage` + `getAll`。
- 统计接口同样需要登录（`require_auth`）。

### 3.0 地图归属园区

园区台账在 `business/` 业务平台（`parks` 表），中台不建园区表、不做外键约束，
`dog_map` 只保存业务侧的 `park_code` / `park_name`（均可空）：

- 设备端建图回传时，业务平台会把设备绑定的园区随 `mapInfo` 写入
  （`parkCode` / `parkName`），因此每张机器人地图都自带园区归属。
- `GET /api/dogMap/getPage` 支持 `parkCode` 精确过滤；`GET /api/dogMap/getAll` 同样返回这两个字段。
- 业务平台拉取地图（`business/server` 的 `/api/map-versions/backend`）时可按 `parkCode` 过滤，
  导入业务库时未指定园区也能按 `parkCode` 自动回填。
- 历史地图没有园区信息，落为 `NULL`，不影响现有查询。

### 3.1 数据权限

`dog_map` 与 `dog_device` 都带 `user_id`：地图按“谁创建谁可见”做行级隔离；
设备按 `dog_user_device` 分配关系隔离（一个设备可分配给多个用户）。模型 / 媒体 / 参数配置为全局共享。

| 操作 | 普通用户 | 管理员 |
| --- | --- | --- |
| 新增地图 | `user_id` 由服务端从 JWT 写入（忽略请求体） | 同左；可通过 `mapInfo.ownerUserId` 指定归属 |
| 地图列表 / 详情 / 修改 / 删除 | 仅 `user_id = 当前用户`，否则 404 | 全部 |
| 设备列表 / 详情 / 修改 / 删除 | 仅被分配的设备 | 全部 |
| 设备分配 / 解除分配 | 拒绝 | 允许，支持批量 |
| 首页统计 | 全部资源总量 | 同左 |

管理员判定：用户拥有内置角色 `p_sys_role.code = '110'`（`sysadm`，种子角色 id 1）。判定在 `require_auth` 中完成，`RequestUser { user_id, username, is_admin }` 同时写入请求与响应 extensions。

`DataScope`（`common/data_scope.rs`）是统一的过滤载体：`owner_filter()` 对管理员返回 `None`，对普通用户返回 `Some(user_id)`。

---

## 4. 运行约定

- **鉴权**：除 `/api/token*` 外全部接口需要 `Authorization: Bearer <token>`（Redis 存储会话，见 `system/auth`）。
- **响应**：`{ code: 10200, msg: "success", data }`；分页 `data` 为 `{ records, total, current, size }`。
- **字段命名**：请求/响应使用小驼峰（登录接口 `/api/token` 例外，为 snake_case）。
- **时间**：数据库存 UTC，响应统一转北京时间（UTC+8）字符串；查询区间按北京时间解析。
- **数据库迁移**：启动时先清理 `seaql_migrations` 中“文件已删除”的历史记录，再执行待应用迁移。删除业务迁移后老库仍可平滑升级。
- **主键生成**：dog 业务统一使用 `common::util::generate_id()`（毫秒 + 原子递增）。
- **上传目录**：`backend/config/config.toml` 的 `[dog] upload_dir`（默认 `data/device_uploads`），地图包存放在其 `maps/` 子目录；该目录只由地图上传使用。
- **菜单**：左侧菜单由数据库驱动（`p_sys_menu` + `p_sys_role_menu`），保留的一级菜单为 设备管理、地图管理、模型管理、媒体资源、参数配置。菜单图标使用离线图标，需在 `frontend/src/components/ReIcon/src/offlineIcon.ts` 注册。

---

## 5. 当前限制 / 待补功能

| 项 | 现状 |
| --- | --- |
| 设备接入 | 已迁移到 `business/`；根后台不提供 WebSocket / 设备 token / 文件上传接口 |
| 设备台账 | 业务平台通过 `/api/dogDevice/upsert` 单向同步到中台；中台侧只维护分配关系，不反向覆盖业务运行状态 |
| 指令下发 | 指令接口与菜单已移除，下发由 `business/` 业务平台负责 |
| 文件上传 | 仅地图包有上传接口；模型 / 媒体的管理页面仍只填写路径 |
| 地图发布归档 | 发布只推进状态，不再联动园区基线或自动归档旧版本；版本管理由业务侧人工完成 |
| 按钮级权限 | 已初始化 `dog_*_find` 权限码并挂到超级管理员角色，但页面暂未使用 `v-auth` 做按钮控制 |
