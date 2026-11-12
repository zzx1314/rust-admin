# 机器狗连接说明（智航 Pro v1.0.3）

> 依据：`business/docs/zhihang-pro-api-v1.0.3.txt`（下称「协议文档」）
> 对应实现：`business/client/`（`robodog-device-client`）
> 范围：设备客户端如何连上机器狗（本体）、如何维持链路、如何把连接参数配置化。

## 0. 快速结论

| 问题 | 结论 |
|------|------|
| 连什么？ | 连本体的 **WebSocket 网关**（默认 `ws://192.168.168.100:10010`），不是 HTTP 轮询 |
| 几条连接？ | **两条**：`/control` + `/bulkdata`，同端口 10010，同属一个逻辑客户端 |
| 谁分配身份？ | 网关在 `1201` 应答里分配 `client_id` / `session_id` / `bulkdata_bind_token` |
| 建链顺序能换吗？ | 不能。先 `control` 拿会话，再 `bulkdata` 用 `1202` 绑定，收到 `1203` 才算通 |
| 心跳？ | 客户端每 5s 发 `1300`，网关回 `1301`（实测 control 会话 10s 无心跳即被踢）；网关每秒广播 `1207` 模块可用性 |
| 数据怎么分流？ | 控制/状态/任务走 `control`；`1401/1403/1405/1407` 高频推送走 `bulkdata` |
| 需要配置文件吗？ | **需要且已支持**：`client.toml` + 环境变量 + 命令行覆盖，优先级见 §11 |

## 1. 网络与前置条件

协议文档 §1.1 以 Orin NX 16G 控制器为例：

1. 笔记本 / 上位机接入本体 Wi-Fi 热点：热点名以 `M1` 开头、以 SN 后三位结尾，初始密码 `12345678`；
2. SSH 进入 Orin NX：`ssh robot@192.168.168.100`（初始密码 `1`）；
3. M1 / L2 的智航服务地址固定为 `192.168.168.100`：

| 连接 | 地址 | 用途 |
|------|------|------|
| control | `ws://host:10010/control` | 握手、心跳、控制请求与响应 |
| bulkdata | `ws://host:10010/bulkdata` | 大负载 / 高频推送 |
| 文件服务 | `http://host:12000/<root>/…` | 地图包下载（§4，L2 根路径 `/userdata/alg_data`，M1 为 `/ota/alg_data`） |

> 现场要点：Wi-Fi 网段与 `192.168.168.x` 必须可达；客户端机器如果同时连办公网，注意路由优先级和防火墙；`ws://` 是明文，不要跨不可信网络直连。

## 2. 连接模型

一个逻辑客户端 = 两条物理 WebSocket：

```text
Logical Client (client_id = N)
├── control   -> ws://host:10010/control     握手 / 心跳 / 控制请求 / 状态与任务推送
└── bulkdata  -> ws://host:10010/bulkdata    1401 / 1403 / 1405 / 1407 高频推送
```

- `client_id` 由网关分配，`control` 与 `bulkdata` **不可当成两个独立客户端**；
- `bulkdata` 必须绑定到与 `control` 相同的 `session_id`；
- 未完成 `1203` 前，`bulkdata` 上不允许发送任何业务数据。

## 2.1 设备身份：协议里没有设备 ID

先说结论：**智航 Pro v1.0.3 没有机器狗自身的设备 ID / SN 字段**，因此
`client.toml` 里的 `server.device_id` 仍然是唯一身份来源，不能从协议回读。

依据（全文检索 `device_id` / `deviceId` / `设备ID` / `序列号` 均无机器人侧定义）：

| 位置 | 字段 | 能否当设备 ID | 原因 |
|------|------|------|------|
| `1201` | `client_id` | ❌ | 网关**分配**的逻辑客户端 ID，重连即变 |
| `1201` | `type` | ❌ | 设备**型号**（如 `ZSL-2-Ultra`），来自网关 `device_json_path`，同型号多台相同 |
| `1311` | `sn` | ❌ | 是**充电桩**序列号（`CP-001`），不是机器狗 |
| `1315` / `1317` / `1319` | — | ❌ | 状态/故障/诊断报文里没有身份字段 |
| `1501` / `1503` / `1521` | `client_id` / `client_type` | ❌ | 会话维度，非资产维度 |
| `5011` / `5020` | `task.uuid` | ❌ | 任务标识，不是设备 |
| §1.1 Wi-Fi | SSID 以 SN 后三位结尾 | ⚠️ | 只有 3 位，会撞号，且需要人工读取 |

工程结论：

1. **`server.device_id` 必须由配置提供**，且与业务库 `devices.device_id` 一致
   （客户端 `register` 上报的就是它，注册回执再下发园区绑定）；
2. `robot.host` 只是**网络地址**（DHCP / 换网会变），不要拿它当设备编号；
3. `client_id` 只能在会话内用于日志与控制权判断，**不得落库**；
4. 如果要资产级 SN（例如与 MES/资产台账对账），只能人工录入（机身标签、SSID 或 SSH 读取），
   协议无法回读校验。建议加一个只读字段 `robot.serial` 仅用于展示与排障，不参与任何路由；
5. `1201.type` 可以用于**机型自检**（例如配置文件写 `robot.model = "ZSL-2-Ultra"`，
   握手后比对，不一致就告警），但不能作为身份校验。

### 2.1.1 现场 SN 的读取方式（已内置进客户端）

机器狗 SN 存在本体 eMMC 的 `mmcblk0p5` 分区，SSH 进本体读取第三行删除分隔符即可：

```bash
root@ubuntu-arm64:~# hexdump -C /dev/mmcblk0p5 2>/dev/null \
  | sed -n '3s/.*|\(.*\)|.*/\1/p' | tr -d ' '
0202222672400230
```

**客户端不再把这个值写进配置或常量**：`client/src/device_sn.rs` 在启动时直接读
`/dev/mmcblk0p5` 现算（需要 root，systemd 单元就是 root 运行），换机器狗不用改任何
文件；读不到时报错并提示用 `--device-id` / `ROBODOG_DEVICE_ID` 覆盖。

实现要点：`hexdump -C` 会把开头连续全 0 的行折叠成 `*`，所以命令里的「第 3 行」
在现场分区上其实是偏移 `0x200`；客户端按 16 字节行扫描前 4KB，优先取 16 位十六
进制的行，不依赖行号（读块设备时长度取 4KB，满足 512/4096 逻辑块大小）。

当前现场部署：

| 项 | 值 |
|------|------|
| 客户端所在服务器 | `192.168.41.246` |
| 业务服务（同机） | `ws://192.168.41.246:3100`（REST：`http://192.168.41.246:3100`） |
| 设备编号（= 机器狗 SN，客户端自动读取） | `0202222672400230`（实测） |
| 机器狗本体 | `192.168.234.234:10010`（文件服务 `:12000`） |

## 3. 标准建链时序（必须按序执行）

```text
客户端                                网关
  │ 1. 连接 ws://host:10010/control     │
  │ ──────────────────────────────────> │
  │ 2. 1201 kHandShakeAck（网关主动）     │
  │ <────────────────────────────────── │ 保存 client_id / session_id / bulkdata_bind_token
  │ 3.（可选）1200 kHandShake 重新索取    │
  │ ──────────────────────────────────> │
  │ 4. 连接 ws://host:10010/bulkdata     │
  │ ──────────────────────────────────> │
  │ 5. 首帧 1202 kBindBulkDataChannel    │
  │ ──────────────────────────────────> │
  │ 6. 1203 kBindBulkDataChannelAck      │
  │ <────────────────────────────────── │ 绑定成功，此后才可收发 bulkdata 业务消息
```

### 3.1 control 建链与 1201

`control` 连上后网关**主动**下发 `1201`，客户端必须保存以下三样，后续都依赖它们：

```json
{
  "head": { "type": 1201, "time": 1757410000000, "src": 5 },
  "data": {
    "client_id": 17,
    "session_id": "8f6c2a7b9d",
    "bulkdata_bind_token": "b45a11d3ef",
    "control_path": "/control",
    "bulkdata_path": "/bulkdata",
    "bind_timeout_ms": 30000,
    "type": "ZSL-2-Ultra"
  }
}
```

| 字段 | 含义 | 用途 |
|------|------|------|
| `client_id` | 逻辑客户端 ID | 日志对账、控制权 owner 判断 |
| `session_id` | 会话 ID | 填进 `1202.data.session_id` |
| `bulkdata_bind_token` | 绑定令牌 | 填进 `1202.data.token` |
| `control_path` / `bulkdata_path` | 通道路径 | 默认 `/control`、`/bulkdata`，网关可覆盖 |
| `bind_timeout_ms` | 绑定超时 | 默认 30000，客户端超时策略应小于它 |
| `type` | 设备型号 | 来自 `device_json_path`，缺失时为空串 |

首帧不是 `1201` 视为建链失败，必须断开重来。需要重新取会话时，在 `control` 上发 `1200`（`data: {}`），网关会再发一次 `1201`。

### 3.2 bulkdata 绑定 1202 / 1203

`bulkdata` 连接建立后的**第一条消息**必须是 `1202`，否则网关回 `1299`（首包不是绑定请求）并拒绝：

```json
{
  "head": { "type": 1202, "time": 1757410000000, "src": 1 },
  "data": { "session_id": "8f6c2a7b9d", "token": "b45a11d3ef" }
}
```

网关成功后回 `1203`：

```json
{
  "head": { "type": 1203, "time": 1757410000000, "src": 5 },
  "data": { "client_id": 17, "session_id": "8f6c2a7b9d", "channel": "bulkdata" }
}
```

### 3.3 建链约束与失败处理（协议文档 §1.1.3 / §1.1.5）

| 场景 | 处理 |
|------|------|
| `control` 连接失败 | 不得建立或继续使用 `bulkdata`，先重建 `control` |
| `bulkdata` 首帧不是 `1202` | 网关拒绝绑定；关闭该连接并重建 `bulkdata` |
| `session_id` / `token` 校验失败 | 丢弃当前 `bulkdata` 会话，重新走 `1200` → `1201` 后再绑定 |
| 未收到 `1203` | 不得发送业务数据；按超时策略重试绑定（客户端取 `BIND_TIMEOUT = 10s`，小于网关 30s） |
| 收到 `1299 kGatewayError` | 按 `data.message` 分类：path 不支持 / 首包错误 / 缺 session_id / 缺 token / token 校验失败 / 绑定超时 / 会话失效 |

## 4. 报文格式

所有消息都是 WebSocket **文本帧**，统一信封：

```json
{
  "head": { "type": 1300, "time": 1757310525615, "src": 1, "msg_id": 12345 },
  "data": {}
}
```

| `head` 字段 | 说明 |
|------|------|
| `type` | 消息类型；请求通常为偶数，响应为 `type + 1`，通知/推送为奇数 |
| `time` | 毫秒时间戳 |
| `src` | 来源：`1`=App、`2`=算法、`3`=云端、`4`=本体、`5`=gateway、`6`=SDK |
| `msg_id` | 请求与响应关联键 |

约定：

- 客户端解码时**先校验 `head.type` 与 `msg_id`**，再解析 `data`；
- 响应先看 `data.code`（`0` 成功）+ `data.msg`；
- 控制类响应只代表「请求已受理」，任务最终结果以状态推送 / 结果通知（如 `4001`、`5009`、`5019`）为准。

## 5. 会话保活与状态

| 机制 | 类型 | 说明 |
|------|------|------|
| 网关心跳 | `1300` → `1301` | 客户端每 5s 发一次 `1300` 刷新 control 会话；网关收到立即回 `1301`。**实测本网关 10s 收不到 1300 就断**（关闭帧 reason=`heartbeat timeout`，协议文档写 20s 但固件更严） |
| 模块可用性 | `1207`（网关→客户端，约每秒） | `data.modules[].available`，`sdk_node=false` 表示 SDK 未连接/正在重连 |
| SDK 心跳 | `1525`（SDK→网关） | 网关连续约 3s 收不到即把 `sdk_node.available` 置 false |
| 本体状态 | `1315`（1Hz，control） | 电量/温度/里程等，客户端用于回传业务服务心跳 |
| 故障 | `1317`（control） | 故障列表 |
| 任务与定位 | `5009`（默认 500ms，control） | V2 单帧快照，任务阶段 / 进度 / 定位状态；**不可与 1315 互相覆盖** |
| 高频位姿/路径 | `1401/1403/1405/1407`（bulkdata） | 位姿、建图轨迹、局部路径、全局路径 |

## 6. 控制权（4000~5999 自由数据透传）

- 发送 `4000~5999` 的非白名单消息前，必须先 `1204 kTakeRoamerXControl` 申请控制权；
- 网关回 `1205 kTakeRoamerXControlNotify`：`status_code = 0` 成功，`client_id` 为当前 owner；
- 默认 owner 为 `UINT32_MAX`（无人持有），此时非白名单 `4000~5999` 一律不转发；
- 白名单（`ws_server.roamerx.whitelist_types`，默认 `4012 / 4022 / 5010`）无需控制权。
- 控制类请求用 `1302`（命令）/`1304`（摇杆）/`1310`（充电桩），以 `1303`、`1311` 的 `status_code` 判断受理。

### 6.1 控制类消息的四个坑（现场踩过）

| 坑 | 文档依据 | 正确做法 |
| --- | --- | --- |
| `1302 data.cmd` 是 **uint32 枚举**，不是字符串 | 1302 字段表：`cmd` 类型 `uint32`；枚举 `0` 紧急停止、`1` 解除急停、`10` 站立、`11` 匍匐、`12` 卧倒、`13` 锁定、`100~105` 补光灯、`110/111` 避障 | 发 `{"cmd": 10}`；发 `{"cmd":"stand_up"}` 本体不认 |
| 受理结果字段不是 `code` | 1303 `data`：`status_code` / `message` / `cmd` | 控制类看 `status_code`，算法侧（4001/4013/5001）才看 `code` / `msg` |
| **现场固件不回 `1303`** | 文档有 1303，但 192.168.234.234 实测：合法 `cmd=102` 与非法 `cmd=999` 都无应答 | `1302` 必须单向下发（等应答会每帧多等 8s 超时并误报失败）；效果看 `1315` 的 `motion_status` / `fill_light` / `estop` |
| `estop.software = true` 时动作被拒 | 1315 `estop{}` | `mode` / `motion_status` 一直 `unknown` 就是这个状态；先 `1302 cmd=1` 解除急停或现场复位急停按钮 |
| `1303` 不回 `msg_id` | 1303 示例 head 只有 `type` / `time` / `src` | 按「响应类型 = 请求类型 + 1」关联，不能只等同 `msg_id` |
| `1304` 没有响应类型 | 协议里不存在 `1305` | 摇杆必须单向下发；用请求-应答模式会每帧多等一个 8s 超时 |

另外 `1304` 的字段名是 `lx / ly / rx / ry / turn / high_low`（全部 `[-1, 1]`），
不是 ROS 风格的 `linear_x / angular_z`；`lx` 正数左移、`rx` 正数左旋，与
「右移/右转为正」的业务轴相反，需要取反。`1302` 不在 `4000~5999` 区间，
不受 RoamerX 控制权约束。

## 7. 文件服务（地图包）

- 独立 HTTP 服务，端口 `12000`，与 WebSocket 无关；
- L2 根路径 `/userdata/alg_data`，M1 根路径 `/ota/alg_data`；
- 例：`curl -fL -o map.zip "http://192.168.168.100:12000/userdata/alg_data/xxx.zip"`。

### 7.1 现场固件：`robot_http_server` 没有 zip 端点

`192.168.234.234:12000` 上跑的是本体自带的 `robot_http_server`：

```bash
$ curl -s http://192.168.234.234:12000/
{"service":"http-fileserver","version":"3.0.0","root_name":"alg_data", ...}
```

**它不提供任何 zip 打包端点**，地图目录只能按单文件下载：

| 用途 | 请求 | 说明 |
|------|------|------|
| 列目录 | `GET {base}/api/list?path=/map/history_map/<dir>` | JSON：`entries[].{name,type,size,path}` |
| 下载文件 | `GET {base}/files/<entries[].path>` | 二进制原文件 |
| 浏览目录 | `GET {base}/files/<dir>/` | 与列目录同样的 JSON |
| 上传 | `POST {base}/upload` | 本体侧重装地图用，客户端不用 |

`{base}` 的根**已经是 `alg_data`**，路径里不能再带 `/ota/alg_data`。

客户端 `mapupload` 因此是两段式：先按 6 种 zip URL 探测（老固件）；
全失败再走「文件服务打包」——列目录 → 递归收集（深度 ≤3）→ 逐文件下载 → 本机打 zip。
打包会跳过 `map.static/`（建图逐帧点云，实测单张地图 1700+ 个 pcd、几百 MB）、
单文件 >64MB（`model_map.bin` 222MB、`map.bin` 121MB）与总量 >200MB，最多 64 个文件。
实测一张地图：16 个文件、原始 32.7MB → zip 11.7MB，含 `map.yaml` / `map.pgm` /
`map.geojson` / `map.txt` / `map.pcd` / `map_edit/` / `nav_paths/`。

## 8. 断线重连策略

```text
control 断开
  └─ 关闭 bulkdata → 重建 control → 等 1201 → 重建 bulkdata → 1202 → 等 1203 → 恢复业务
```

- 重连后重新处理 `1207`、`1521`、`1525`，并用**消息时间戳**丢弃旧状态，避免旧帧覆盖新帧（协议文档 §3.2-5）；
- 绑定失败不要复用旧 `token`，重新 `1200` 取 `1201`；
- 客户端还应做**请求级超时**：当前实现 `REQUEST_TIMEOUT = 8s`，避免 pending 表泄漏；
- 业务侧（`/ws/device`）断线单独重连，退避 `1s→30s`，不影响本体链路。

## 9. 客户端与业务服务的连接（辅助链路）

完整数据通路是「业务服务 ⇄ 设备客户端 ⇄ 机器狗本体」：

```text
robodog-ops-server (ws://server:3100/ws/device)     智航 Pro (ws://192.168.168.100:10010)
        │  register / heartbeat / task_dispatch              │  5000 任务、5014 返充
        │  ← task_progress / event                           │  ← 5009 状态、4001 保存结果
        └──────────── robodog-device-client ─────────────────┘
```

- 业务服务侧：连上后先发 `register`（`deviceId` + 可选 `secret`），再按 `--heartbeat`（默认 10s）发 `heartbeat`；
- 本体不可达时客户端降级为「模拟模式」：只回传进度，不驱动机器人，便于前端联调；
- `--map-only` 模式可脱离业务服务，单独跑一次建图并回传中台。

## 10. 现有实现映射

| 能力 | 代码位置 |
|------|----------|
| 双通道建链 / 1201 / 1202 / 1203 | `business/client/src/zhpro/client.rs` |
| 协议常量与信封 | `business/client/src/zhpro/protocol.rs` |
| 网关心跳 5s、请求超时 8s、绑定超时 10s | `business/client/src/zhpro/client.rs`（`GATEWAY_HEARTBEAT` / `REQUEST_TIMEOUT` / `BIND_TIMEOUT`） |
| 端地址推导（裸 IP / `ws://ip:10010` / 带 path 均可） | `client.rs::endpoint_urls` / `host_base` |
| 遥控看门狗（1s 无新帧自动停车） | `business/client/src/zhpro/teleop.rs` |
| 业务服务注册 / 心跳 / 任务编排 | `business/client/src/main.rs` |
| 地图包下载与回传 | `business/client/src/mapupload.rs`（文件服务默认端口 12000） |
| 默认地图根目录 | `config.rs` → `/ota/alg_data/map`（`zhpro/mapping.rs` 转导出） |
| 配置解析（CLI / 环境变量 / TOML 三层） | `business/client/src/config.rs`，模板 `client.toml.example` |

当前连接参数全部来自命令行（`pico-args`，见 `Config::from_args` 与 `print_help`）。

## 11. 配置文件（已实现）

### 11.1 结论

需要，并且已落地：`business/client/src/config.rs` 支持 TOML 配置文件，
模板见 `business/client/client.toml.example`，现场复制成 `client.toml` 即可
（`client.toml` 已加入 `.gitignore`，避免设备编号与密钥引用入库）。

```bash
cd business/client
cp client.toml.example client.toml
chmod 600 client.toml
cargo run -- --print-config      # 查看脱敏后的生效配置
```

原因：参数 20+ 个、现场差异集中在少数几项（服务地址、设备编号、本体地址、园区），
纯命令行在 `systemd` 里难维护；敏感项出现在命令行会被 `ps` 看到并进 shell history。

### 11.2 优先级与查找顺序

```text
命令行 > 环境变量 > 配置文件 > 内置默认值
```

配置文件的确定顺序：

1. `--config <path>`；
2. 环境变量 `ROBODOG_CLIENT_CONFIG`；
3. 默认 `./client.toml`（工作目录，**不存在时静默使用默认值**）；
4. 显式指定的路径不存在 → 启动失败并报错（避免「以为改了配置其实没生效」）。

### 11.3 字段与默认值

| 配置文件 | 环境变量 | 命令行 | 默认值 |
|------|------|------|------|
| `server.url` | `ROBODOG_SERVER_URL` | `--server` | `ws://192.168.41.246:3100` |
| ~~`server.device_id`~~ | `ROBODOG_DEVICE_ID` | `--device-id` | **不在配置文件里**：默认自动读 `/dev/mmcblk0p5`（`ROBODOG_SN_PATH` 可改路径），写进 `client.toml` 会按未知字段报错 |
| `server.secret` / `server.secret_env` | `ROBODOG_DEVICE_SECRET` | `--secret` | 空（可选） |
| `server.heartbeat_secs` | `ROBODOG_HEARTBEAT` | `--heartbeat` | `10`（最小 1） |
| `api.base` | `ROBODOG_API_BASE` | `--api-base` | 由 `server.url` 推导 |
| `api.username` | `ROBODOG_API_USERNAME` | `--api-username` | `admin` |
| `api.password` / `api.password_env` | `ROBODOG_API_PASSWORD` | `--api-password` | `admin123`（**线上必须覆盖**） |
| `robot.host` | `ROBODOG_ROBOT` | `--robot` | `192.168.234.234`（填 `sim`/`none`/`off` = 模拟模式） |
| `robot.file_port` | `ROBODOG_ROBOT_FILE_PORT` | `--robot-file-port` | `12000` |
| `robot.file_base` | `ROBODOG_ROBOT_FILE_BASE` | `--robot-file-base` | 按 host + port 推导 |
| `robot.map_root` | `ROBODOG_MAP_ROOT` | `--map-root` | `/ota/alg_data/map` |
| `robot.model` | — | `--model` | 空（用本体 `1201.type`；本体不报型号时用它兜底） |
| `mapping.kind` | — | `--map-kind` | `outdoor`（仅 `outdoor` / `indoor`） |
| `mapping.name` | — | `--map-name` | `map-<时间戳>` |
| `mapping.upload` | — | `--no-upload-map` | `true` |
| `mapping.park_id` | `ROBODOG_PARK_ID` | `--park-id` | 空（用控制台绑定的园区） |
| `mapping.ingest_name` | — | `--ingest-map-name` | 空（用本体目录名） |
| `mapping.once` | — | `--map-only` | `false` |
| `video.enabled` | — | — | `true`（是否响应业务平台的 `video_start`） |
| `video.ffmpeg` | — | — | `ffmpeg`（可执行文件路径） |
| `video.transport` | — | — | `tcp`（仅 `tcp` / `udp`，非 `rtsp://` 地址不传） |
| `video.encoder` | — | — | `auto`（按 `libx264` → `h264_rkmpp` → `h264_v4l2m2m` 顺序探测；机器狗上是 `h264_rkmpp`） |
| `video.fps` | — | — | `15`（关键帧间隔取 `2×fps`） |
| `video.bitrate` | — | — | `1200k` |
| `video.width` | — | — | `0`（保持原始分辨率，等比缩放） |
| `simulation.battery` / `position` / `firmware` | — | `--battery` / `--position` / `--firmware` | `86` / `0.0, 0.0` / `Hermes_M_v0.3.2` |
| `log.filter` | `RUST_LOG`（优先） | — | `robodog_device_client=info` |

> `server.device_id` 是**唯一设备身份**（必须与业务库 `devices.device_id` 一致）；
> 协议里没有设备 ID / SN 可回读，`client_id` 与 `robot.host` 都不能替代它，详见 §2.1。

不在配置文件里的项是协议常量，不应按现场随意改：`control`/`bulkdata` 端口 `10010`、
网关心跳 `5s`（网关实测 10s 无 1300 即断，不能按文档写 20s）、请求超时 `8s`、
bulkdata 绑定超时 `10s`、遥控看门狗 `1s`。

### 11.4 密钥与校验

- 密钥推荐 `secret_env = "ROBODOG_DEVICE_SECRET"` / `password_env = "ROBODOG_API_PASSWORD"`，
  优先级：命令行 > 环境变量 > 文件里的 `*_env` 引用 > 文件明文；
- `--print-config` 与启动日志都会**打码**密钥与密码；
- 配置错误会在启动时直接失败（不静默降级）：
  - 非 `ws://` / `wss://` 的 `server.url`；
  - `mapping.kind` 不是 `outdoor` / `indoor`；
  - `video.transport` 不是 `tcp` / `udp`；
  - `park_id` 非正整数；
  - 配置文件拼写了未知字段（`deny_unknown_fields`，避免 `secrect` 这类错误被忽略）；
- `robot.host` 有值但连不上时仍按原策略降级为模拟模式并打印错误日志。

### 11.5 现场部署（systemd）

现成脚本：`client/deploy/install.sh`（编译 release → 装到 `/opt/robodog/client` →
装 `/etc/systemd/system/robodog-device-client.service` → 启动），配置模板
`client/deploy/client.toml.template`、密钥模板 `client/deploy/client.env.example`。
内置默认值已经是现场值（`192.168.41.246` / `192.168.234.234`），设备编号启动时从
`/dev/mmcblk0p5` 自动读取，所以装完即可直接连。

```bash
cd business/client
sudo ./deploy/install.sh              # 一条命令装完并启动
sudo ./deploy/check.sh                # 连接自检（端口 / health / 10010 / 12000 / 注册）
journalctl -u robodog-device-client -f
```

等价的 unit 内容（脚本装的就是它）：

```ini
[Unit]
Description=RoboDog device client
After=network-online.target

[Service]
WorkingDirectory=/opt/robodog/client
EnvironmentFile=-/etc/robodog/client.env
ExecStart=/opt/robodog/client/robodog-device-client --config /etc/robodog/client.toml
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

一台机器人一份 `client.toml`（`device_id` + 本体 IP + 园区），便于审计与批量下发。

## 12. 联调排障清单

| 现象 | 排查 |
|------|------|
| 连不上 `10010` | Wi-Fi 是否连到本体热点、`ping 192.168.168.100`、端口是否被防火墙拦 |
| 首帧不是 `1201` | 确认连的是 `/control`；网关版本是否匹配（v1.0.3） |
| `bulkdata` 绑定失败 | 首帧是否 `1202`、`session_id`/`token` 是否最新、是否超过 `bind_timeout_ms` |
| 收到 `1299` | 看 `data.message`：path 不支持 / 缺 token / token 校验失败 / 会话失效 |
| 控制指令无响应 | 是否持有 RoamerX 控制权（`1204`→`1205` `status_code=0`） |
| 状态不刷新 | `1207` 里 `sdk_node.available` 是否为 true（`1525` 3s 超时会被置 false） |
| 任务下发后无进展 | 看 `5009` 的 `task.stage` / `lifecycle`，`5001` 只代表受理 |
| 地图拉不到 | 文件服务 `http://host:12000` 是否可达、根路径是否为 `/ota/alg_data`（M1）或 `/userdata/alg_data`（L2） |
