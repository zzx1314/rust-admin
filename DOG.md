# 机器狗业务模块（dog）

本仓库的后端业务域已从早期的「应用商店 / Harbor 镜像审核」整体替换为 **机器狗（dog）业务模块**：园区、地图、设备档案、任务、路线、模型、媒体、指令、参数配置、服务与工作流的增删改查，以及首页统计。

> 历史说明：`business/harbor`、`business/app_review`、`/api/harbor/**`、`/api/appReviews/**`、`/api/webhooks/harbor`、`/api/deploy/config` 及前端 `views/harbor`、`api/harbor.ts`、`api/appReview.ts` 均已删除，配置中的 `[harbor]` 段也不再被读取。

---

## 1. 组成

| 层 | 位置 |
| --- | --- |
| 后端业务代码 | `backend/src/business/dog/{park,map,device,user_device,task,media,model,config,command,route_library,services,workflow,statistics}/` |
| 设备接入网关 | `backend/src/business/dog/gateway/`（设备 token、注册、WebSocket、任务下发、文件上传、结果回写） |
| 设备端客户端 | `client/`（Rust crate `dog-device-client`，见 `client/README.md`） |
| 通信端到端测试 | `backend/tests/business/dog_gateway_tests.rs`、`dog_data_scope_tests.rs`（数据权限） |
| 后端路由 | `backend/src/api/routes.rs`（每个子域一个 `dog_*_routes`，统一挂 `require_auth`） |
| 服务装配 | `backend/src/app.rs` → `App::build_state_with_token_store` |
| 数据库迁移 | `backend/migrations/dog/7_create_dog_tables.sql`（建表）、`10_add_park_map_tables.sql`、`11_add_task_user_scope.sql`、`13_create_user_device_bindings.sql`、`backend/migrations/p_sys/9_add_dog_menus.sql` / `12_add_park_map_menus.sql` / `14_add_device_binding_menu.sql`（菜单）、`15_flatten_dog_park_menus.sql`（把机器狗 / 园区菜单提升为一级菜单）、`16_remove_device_binding_menu.sql`（下线「设备绑定」菜单，改在设备列表中绑定）、`17_remove_workflow_menu.sql`（下线「工作流」菜单）、`18_reorder_map_menu.sql`（地图管理排在系统管理之下、设备列表之上）、`19_add_map_file_fields.sql`（地图改为上传 .zip 包，新增文件字段且 `park_id` 可空）、`backend/migrations/dog/21_add_route_park.sql`（路线新增「所属园区」`park_id` 字段） |
| 前端页面 | `frontend/src/views/dog/{park,map,device,task,route,model,media,command,config,services}/` |
| 前端请求封装 | `frontend/src/api/dog*.ts` |
| 首页仪表盘 | `frontend/src/views/welcome/index.vue` + `frontend/src/api/dogStatistics.ts` |
| 在线手册 | `frontend/src/views/manual/admin.md`（管理员）、`developer.md`（接口对接） |

每个子域内部固定分层：`domain.rs`（请求/VO/字段映射）、`entity.rs`（SeaORM 实体）、`repository.rs`（SQL/条件构造）、`service.rs`（业务编排与错误映射）、`handlers.rs`（HTTP 层）、`mod.rs`。

---

## 2. 数据表

| 表 | 说明 | 关键字段 |
| --- | --- | --- |
| `dog_device` | 设备 | `device_id`、`device_type`、`status`(默认 `offline`)、`device_ip`、`model`、`speed`、`height`、`battery`、`course`、`location`、`comm_time` |
| `dog_user_device` | 用户设备绑定 | `user_id`、`device_id`(指向 `dog_device.id`)，唯一索引 `(user_id, device_id)` |
| `dog_park` | 园区 | `park_name`、`park_description`、`park_address`、`user_id`(创建者) |
| `dog_map` | 地图 | `map_name`、`map_type`、`file_name`、`file_path`、`file_format`、`file_size`、`remarks`、`park_id`(所属园区；新增必填，历史数据可为空)、`user_id`(创建者)；地图以 `.zip` 包上传，文件落盘到 `<upload_dir>/maps/` |
| `dog_task` | 任务 | `task_name`、`task_description`、`task_status`(默认 `pending`)、`workflow_id`、`workflow_uuid`、`device_id`、`route_id`、`user_id`(创建者) |
| `dog_route_library` | 路线 | `route_name`、`route_type`、`route_status`、`route_description`、`park_id`(所属园区，可选)、`model_id`(NOT NULL)、`route_data` |
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
POST   /api/dogMap/upload           上传 .zip 地图包（multipart：file + mapInfo）
PUT    /api/dogDevice/status/{id}   仅回写设备状态
PUT    /api/dogTask/status/{id}     仅回写任务状态
PUT    /api/dogCommand/status/{id}  仅回写指令状态与返回值
GET    /api/dogWorkflow/uuid/{uuid} 按工作流 UUID 查询
GET    /api/dogWorkflow/byTask/{taskId} 按任务查询已保存的流程（流程设计回显）
GET    /api/dogStatistics/{all,allType,deviceTypeNumber}

# 用户设备绑定
GET    /api/dogUserDevice/getPage       分页（userId/deviceId + current/size）
GET    /api/dogUserDevice/getAll        全量
POST   /api/dogUserDevice               绑定设备（管理员可传 userId，普通用户只能绑自己）
GET    /api/dogUserDevice/{id}          绑定详情
DELETE /api/dogUserDevice/{id}          解绑（软删除）
GET    /api/dogUserDevice/byUser/{userId} 某用户的全部绑定

# 设备网关（设备 token 鉴权）
GET    /api/dogGateway/auth/{deviceId}        下发设备 token（10 分钟有效）
POST   /api/dogGateway/register               注册/刷新设备（Authorization: <token>）
WS     /api/notice/{deviceId}/{token}         设备长连接（心跳 / 任务 / 结果 / 文件）

# 设备网关（用户 JWT 鉴权）
POST   /api/dogGateway/dispatch               手动向设备推送任意 JSON
POST   /api/dogGateway/disconnect/{deviceId}  主动断开设备（客户端自动重连）
GET    /api/dogGateway/connections            当前在线设备
POST   /api/dogGateway/assign                 按硬件 deviceId 绑定设备给用户（设备列表页使用）
GET    /api/dogGateway/assign/{deviceId}      查询设备归属用户
DELETE /api/dogGateway/assign/{deviceId}      解除归属（?userId= 仅管理员有效）
```

- `module` ∈ `dogDevice`、`dogUserDevice`、`dogTask`、`dogPark`、`dogMap`、`dogMedia`、`dogModel`、`dogConfig`、`dogCommand`、`dogRoute`、`dogServices`、`dogWorkflow`。
- 除 `/api/dogServices`（只有全量）与 `/api/dogWorkflow`（无列表）外，其余模块均提供 `getPage` + `getAll`。
- 统计接口同样需要登录（`require_auth`）。

---

## 3.1 数据权限（园区 / 地图 / 设备 / 任务）

`dog_park`、`dog_map`、`dog_task` 带 `user_id`，按“谁创建谁可见”做行级隔离；设备通过 `dog_user_device` 绑定到用户（多对多）。

| 操作 | 普通用户 | 管理员 |
| --- | --- | --- |
| 新增园区 / 任务 | `user_id` 由服务端从 JWT 写入（忽略请求体） | 同左 |
| 园区 / 地图 / 任务列表 | 仅 `user_id = 当前用户` | 全部 |
| 园区 / 地图 / 任务详情、修改、删除 | 仅本人数据，否则 404 | 全部 |
| 地图关联园区（可选） | 指定园区时只能挂到自己的园区，否则 400；不指定则不挂园区 | 任意园区 |
| 设备列表 / 详情 / 修改 / 删除 | 仅已绑定到自己的设备 | 全部（含未绑定） |
| 绑定设备 | 只能认领尚未被任何人绑定的设备，`userId` 强制为自己 | 可绑定任意用户，一台设备可被多人绑定 |
| 按硬件编号分配（`/api/dogGateway/assign`） | 同上（只能认领未绑定设备） | 任意已注册设备 + 任意用户 |
| 解绑 | 只能解绑自己的绑定 | 任意绑定 |
| 任务引用设备 | `deviceId` 必须是已绑定给自己的设备，否则 400 | 任意设备 |
| 首页数量统计 | 设备数 = 已绑定设备数；任务数 = 本人任务数 | 全部 |

管理员判定：用户拥有内置角色 `p_sys_role.code = '110'`（`sysadm`，种子角色 id 1）。判定在 `require_auth` 中完成，`RequestUser { user_id, username, is_admin }` 同时写入请求与响应 extensions（handler 用 `Extension<RequestUser>` 取，审计日志从响应用）。

`DataScope`（`common/data_scope.rs`）是统一的过滤载体：`owner_filter()` 对管理员返回 `None`，对普通用户返回 `Some(user_id)`。设备查询通过 `dog_user_device` 子查询完成（`SeaOrmDogDeviceRepository::apply_scope`），网关注册/心跳使用独立的 `gateway/repository.rs`，不受用户数据范围影响。路线 / 模型 / 媒体 / 指令等表暂无归属字段，仍为全局可见。

---

## 4. 运行约定

- **鉴权**：除 `/api/token*` 外全部接口需要 `Authorization: Bearer <token>`（Redis 存储会话，见 `system/auth`）。
- **响应**：`{ code: 10200, msg: "success", data }`；分页 `data` 为 `{ records, total, current, size }`。
- **字段命名**：请求/响应使用小驼峰（登录接口 `/api/token` 例外，为 snake_case）。
- **时间**：数据库存 UTC，响应统一转北京时间（UTC+8）字符串；查询区间按北京时间解析。
- **数据库迁移**：启动时先清理 `seaql_migrations` 中“文件已删除”的历史记录（如早年的 `add_app_review`、`add_startup_config`），再执行待应用迁移。因此删除旧业务迁移后，老库仍可平滑升级，不会因 `Migration file of version 'xxx' is missing` 启动失败。
- **主键生成**：系统模块沿用毫秒时间戳；dog 业务统一使用 `common::util::generate_id()`（毫秒 + 原子递增），避免同一毫秒内创建多行时主键冲突。
- **设备 ID**：由设备端从硬件信息派生（板级序列号 / eMMC CID / DMI UUID / 物理 MAC，兜底 `/etc/machine-id`），注册时通过 `deviceIdSource` 上报来源；也可用 `--device-id` 覆盖。Ubuntu 上 `sysfs` 的序列号/eMMC CID/DMI 标识默认仅 root 可读（`--explain-device-id` 可查看每个来源的状态），建议以 systemd 服务运行并配合 `--device-id-file` 固定首次检测结果。详见 `client/README.md`。
- **设备接入**：设备先取 token（`GET /api/dogGateway/auth/{deviceId}`），再注册（`dog_device` 初始 `offline`），随后连 `WS /api/notice/{deviceId}/{token}`；心跳会把 `status` 置为 `online` 并刷新 `comm_time` 与遥测字段；WebSocket 断开（正常关闭或连接异常）时置回 `offline`。新注册的设备默认未绑定用户，需在「设备列表」页绑定给用户后才能被普通用户看到；管理员通过 `POST /api/dogDevice` 手工创建的设备同样保持未绑定，普通用户手工创建则会自动绑定创建者。注册报文本身不携带用户信息，因此设备归属由独立的 `POST /api/dogGateway/assign`（按硬件 `deviceId`）接口建立，便于后续接入配网/出厂预置流程时直接复用。
- **任务下发**：管理端 `POST /api/dogCommand` 创建指令后，若目标设备在线则立即通过 WebSocket 推送并把指令状态置为 `sent`；设备回报后写回 `status`/`return_value`。也可用 `POST /api/dogGateway/dispatch` 推送自定义 JSON。
- **设备上传**：`file_start` → 二进制帧（单流上限 256 MiB）→ `file_end`，文件落盘到 `[dog] upload_dir`（默认 `data/device_uploads`）并写入 `dog_media`。
- **设备认证信任边界**：`GET /api/dogGateway/auth/{deviceId}` 与旧网关一致，按设备号直接下发 token，不校验证书；生产环境需在入口层做 mTLS / 内网隔离 / 反向代理鉴权，或在该接口前置设备白名单。
- **断线重连**：客户端默认指数退避无限重连（`--reconnect-*` 可调），每次重连都重新取 token、注册、建链；半开连接靠"心跳带确认"发现（服务端收到心跳必回 `report`）。服务端可用 `DogGateway::disconnect(device_id)` 主动断开某台设备（客户端会自动重连），`DogGateway::accepted_connections()` 可用于观察重连次数。
- **在线连接表**：内存态（`DeviceRegistry`），因此网关按单实例部署；多实例需引入共享总线（如 Redis pub/sub）。
- **菜单**：左侧菜单由数据库驱动（`p_sys_menu` + `p_sys_role_menu`），`component` 需能唯一匹配 `frontend/src/views/<component>/index.vue`，因此这里使用 `dog/device` 这类带目录前缀的写法（避免与 `views/charging/device` 冲突）。菜单图标使用离线图标，需在 `frontend/src/components/ReIcon/src/offlineIcon.ts` 注册。机器狗与园区的各个页面都是**一级菜单**（`parent_id = -1`，原先的 `机器狗管理` / `园区管理` 目录已被 `15_flatten_dog_park_menus.sql` 退役），路由地址（如 `/dog/device/index`）保持不变。`设备绑定` 不再单独占菜单（`16_remove_device_binding_menu.sql`），绑定/解绑用户改在「设备列表」页完成；`工作流` 页面已下线（`17_remove_workflow_menu.sql`），后端 `/api/dogWorkflow*` 接口保留。

---

## 5. 当前限制 / 待补功能

| 项 | 现状 |
| --- | --- |
| `数量统计`（上周/本周） | 后端返回全 0 的占位数据，尚未做按周聚合 |
| `设备统计`（按设备类型） | 后端返回空数组，尚未实现 |
| 工作流 | 管理后台已不再提供工作流页面（菜单随 `17_remove_workflow_menu.sql` 下线），后端仅保留新增 / 按 ID、UUID 查询 / 修改 / 删除接口，任务上的 `workflow_id` 仍可填写 |
| 文件上传 | 设备上传已实现（`/api/notice` 二进制流 → `dog_media`）；模型/媒体的管理页面仍只填写路径，没有浏览器上传接口 |
| 设备通信 | 已实现 WebSocket 网关（心跳/任务/结果/文件/断线重连）；尚未迁移旧 C 客户端的 TCP:9000 文件帧通道与其他 MQTT 接入方式 |
| 离线指令重放 | 断线期间已下发未回报的指令保留在 `sent` 状态，不会自动重发（避免重复执行） |
| 网关多实例 | 连接表在进程内存中，多副本部署时设备只会连到其中一个实例 |
| 数据权限范围 | 园区 / 地图 / 任务按创建者隔离，设备按用户绑定隔离；路线、模型、媒体、指令仍全局可见 |
| 设备绑定入口 | 设备由网关自动注册后默认无人可见，管理员可在「设备列表」行内「绑定用户」弹窗绑定/解绑（普通用户只能解绑自己） |
| 历史任务归属 | 迁移只新增 `user_id` 列，历史任务保持 `NULL`，仅管理员可见，需要时手工回填 |
| 按钮级权限 | 已初始化 `dog_*_find` 权限码并挂到超级管理员角色，但页面暂未使用 `v-auth` 做按钮控制 |
