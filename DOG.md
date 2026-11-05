# 机器狗业务模块（dog）

本仓库的后端业务域已从早期的「应用商店 / Harbor 镜像审核」整体替换为 **机器狗（dog）业务模块**：设备档案、任务、路线、模型、媒体、指令、参数配置、服务与工作流的增删改查，以及首页统计。

> 历史说明：`business/harbor`、`business/app_review`、`/api/harbor/**`、`/api/appReviews/**`、`/api/webhooks/harbor`、`/api/deploy/config` 及前端 `views/harbor`、`api/harbor.ts`、`api/appReview.ts` 均已删除，配置中的 `[harbor]` 段也不再被读取。

---

## 1. 组成

| 层 | 位置 |
| --- | --- |
| 后端业务代码 | `backend/src/business/dog/{device,task,media,model,config,command,route_library,services,workflow,statistics}/` |
| 后端路由 | `backend/src/api/routes.rs`（每个子域一个 `dog_*_routes`，统一挂 `require_auth`） |
| 服务装配 | `backend/src/app.rs` → `App::build_state_with_token_store` |
| 数据库迁移 | `backend/migrations/dog/7_create_dog_tables.sql`（建表）、`backend/migrations/p_sys/9_add_dog_menus.sql`（菜单） |
| 前端页面 | `frontend/src/views/dog/{device,task,route,model,media,command,config,services,workflow}/` |
| 前端请求封装 | `frontend/src/api/dog*.ts` |
| 首页仪表盘 | `frontend/src/views/welcome/index.vue` + `frontend/src/api/dogStatistics.ts` |
| 在线手册 | `frontend/src/views/manual/admin.md`（管理员）、`developer.md`（接口对接） |

每个子域内部固定分层：`domain.rs`（请求/VO/字段映射）、`entity.rs`（SeaORM 实体）、`repository.rs`（SQL/条件构造）、`service.rs`（业务编排与错误映射）、`handlers.rs`（HTTP 层）、`mod.rs`。

---

## 2. 数据表

| 表 | 说明 | 关键字段 |
| --- | --- | --- |
| `dog_device` | 设备 | `device_id`、`device_type`、`status`(默认 `offline`)、`device_ip`、`model`、`speed`、`height`、`battery`、`course`、`location`、`comm_time` |
| `dog_task` | 任务 | `task_name`、`task_description`、`task_status`(默认 `pending`)、`workflow_id`、`workflow_uuid`、`device_id`、`route_id` |
| `dog_route_library` | 路线 | `route_name`、`route_type`、`route_status`、`route_description`、`model_id`(NOT NULL)、`route_data` |
| `dog_model` | 模型 | `model_name`、`model_type`、`file_name`、`file_path`、`file_format`、`file_size`、`remarks` |
| `dog_media` | 媒体 | `media_name`、`media_type`、`media_path`、`media_size`、`remarks` |
| `dog_command` | 指令 | `command_name`、`command_type`、`command_params`、`device_id`、`status`(默认 `pending`)、`return_value`、`task_id` |
| `dog_config` | 参数配置 | `config_name`、`config_value`、`description` |
| `dog_services` | 服务 | `type`、`params` |
| `dog_workflow` | 工作流 | `uuid`、`nodes`、`edges`、`task_id`、`command_json_string` |

所有表都有 `id`（主键，创建时由服务端生成）、`create_time`、`update_time`、`is_deleted`（软删除标记），查询一律带 `is_deleted = 0`。

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
PUT    /api/dogDevice/status/{id}   仅回写设备状态
PUT    /api/dogTask/status/{id}     仅回写任务状态
PUT    /api/dogCommand/status/{id}  仅回写指令状态与返回值
GET    /api/dogWorkflow/uuid/{uuid} 按工作流 UUID 查询
GET    /api/dogStatistics/{all,allType,deviceTypeNumber}
```

- `module` ∈ `dogDevice`、`dogTask`、`dogMedia`、`dogModel`、`dogConfig`、`dogCommand`、`dogRoute`、`dogServices`、`dogWorkflow`。
- 除 `/api/dogServices`（只有全量）与 `/api/dogWorkflow`（无列表）外，其余模块均提供 `getPage` + `getAll`。
- 统计接口同样需要登录（`require_auth`）。

---

## 4. 运行约定

- **鉴权**：除 `/api/token*` 外全部接口需要 `Authorization: Bearer <token>`（Redis 存储会话，见 `system/auth`）。
- **响应**：`{ code: 10200, msg: "success", data }`；分页 `data` 为 `{ records, total, current, size }`。
- **字段命名**：请求/响应使用小驼峰（登录接口 `/api/token` 例外，为 snake_case）。
- **时间**：数据库存 UTC，响应统一转北京时间（UTC+8）字符串；查询区间按北京时间解析。
- **主键生成**：与系统模块保持一致，使用毫秒时间戳；如需批量导入请自行指定/规避同一毫秒。
- **菜单**：左侧菜单由数据库驱动（`p_sys_menu` + `p_sys_role_menu`），`component` 需能唯一匹配 `frontend/src/views/<component>/index.vue`，因此这里使用 `dog/device` 这类带目录前缀的写法（避免与 `views/charging/device` 冲突）。菜单图标使用离线图标，需在 `frontend/src/components/ReIcon/src/offlineIcon.ts` 注册。

---

## 5. 当前限制 / 待补功能

| 项 | 现状 |
| --- | --- |
| `数量统计`（上周/本周） | 后端返回全 0 的占位数据，尚未做按周聚合 |
| `设备统计`（按设备类型） | 后端返回空数组，尚未实现 |
| 工作流列表 | 无分页接口，前端表格无数据源，仅能新增 / 按 ID、UUID 查询 / 修改 / 删除 |
| 文件上传 | 模型与媒体只保存路径字段，没有上传/下载接口 |
| 设备通信 | 后端没有 MQTT/WebSocket 等通信层，指令与状态均为数据库记录，由外部系统回写 |
| 按钮级权限 | 已初始化 `dog_*_find` 权限码并挂到超级管理员角色，但页面暂未使用 `v-auth` 做按钮控制 |
