# dog-device-client

机器狗设备端客户端（Rust）。由 `mgmt_platform/backend/src/main/client` 下的 C 客户端迁移而来：

| 旧 C 程序 | 作用 | 迁移后 |
| --- | --- | --- |
| `https/client_auth.c`、`client_register.c` | 设备认证、注册 | `DeviceClient::authenticate` / `register` |
| `ws/src/client_https_demo.c`、`device_client/src/report/Report.c` | WebSocket 长连接、心跳、文件上传 | `DeviceLink`（心跳 / 收任务 / 回报 / 上传） |
| `tcp/tcpdemo.c` | 旧 TCP 文件帧上传（端口 9000） | 未迁移（服务端已统一为 WebSocket 通道） |
| `cer/*.crt`、`*.key` | mTLS 材料 | `TlsOptions`（rustls，读取同样的 PEM） |
| 硬编码 `deviceId` | 设备标识 | `device_id` 模块：从硬件信息派生（见第 3 节） |

不变的约定：认证/注册/通知的 URL 与旧客户端完全一致，所以现网 C 设备可以继续连新服务端。

## 1. 通信流程

```text
设备                                    rust-admin 后端
 │  GET /api/dogGateway/auth/{deviceId}          │  下发 10 分钟有效的设备 token
 │──────────────────────────────────────────────▶│
 │  POST /api/dogGateway/register  (Authorization: <token>) │
 │──────────────────────────────────────────────▶│  创建/更新 dog_device
 │  WS  /api/notice/{deviceId}/{token}           │
 │◀─────────────────────────────────────────────▶│  注册到内存连接表
 │  {"type":"heartbeat","drones":{...}}          │  更新状态/遥测、comm_time
 │──────────────────────────────────────────────▶│
 │  {"type":"report","status":"online"}          │
 │◀──────────────────────────────────────────────│
 │  {"type":"server-command","commandId":...,"params":{...}} │  管理端新建指令即推送
 │◀──────────────────────────────────────────────│
 │  {"type":"server-command","commandId":...,"status":"success","returnValue":"..."} │
 │──────────────────────────────────────────────▶│  写回 dog_command
 │  file_start → 二进制帧 → file_end             │  落盘 + 写入 dog_media
 │──────────────────────────────────────────────▶│
 │  {"type":"file_ack","streamId":...,"status":"success","path":"..."} │
 │◀──────────────────────────────────────────────│
```

同时兼容旧客户端的其它回报帧：`{"type":"report","id":...,"status":...,"returnValue":...}` 与
`{"type":"torosReport","missionId":...,...}`，都会写回对应指令的执行结果。

## 2. 断线重连与心跳

客户端默认**常驻自愈**：任何原因导致的连接中断都会自动重连，直到收到 `SIGINT` / `SIGTERM`。

### 2.1 断线判定

| 场景 | 检测方式 | 处理 |
| --- | --- | --- |
| 服务端主动关闭 / 服务重启 | 收到 WebSocket Close 帧或读错误 | 立即进入重连 |
| 网络中断、网线拔出、AP 掉线（半开连接） | **心跳带确认**：发送 heartbeat 后在 `--heartbeat-timeout`（默认 10s）内没收到服务端回包 | 判定链路失效，进入重连 |
| 空闲但链路正常 | 每 `--interval`（默认 30s）发一次 heartbeat 并等待确认 | 保持在线状态、刷新 `dog_device.comm_time` |

> 只用 TCP 连接状态无法发现"网线被拔"这类半开连接，所以客户端把心跳回包作为存活判据；服务端收到心跳必定回 `{"type":"report","status":"online"}`。

### 2.2 重连流程

```text
连接断开
  │
  ├─ 指数退避等待（1s, 2s, 4s, 8s ... 上限 --reconnect-max-delay，默认 30s，±12.5% 抖动）
  │
  ├─ 重新 GET /api/dogGateway/auth/{deviceId}       （token 只有 10 分钟，重连必换新）
  ├─ 重新 POST /api/dogGateway/register             （幂等 upsert，刷新 comm_time）
  ├─ 重新建立 WS  /api/notice/{deviceId}/{token}
  └─ 首次心跳确认成功 → 记为已恢复；连接稳定超过 3×心跳间隔（10~120s）后退避计数清零
```

- 连续失败次数可用 `--max-reconnect-attempts N` 限制（默认无限重试）；`--no-reconnect` 表示断线即退出。
- 运行中收到 `SIGINT`/`SIGTERM` 会先给服务端发 Close 帧再退出（systemd `stop` 可优雅下线）。
- **已下发但未回报的指令不会自动重放**（避免重复执行）：服务端会把该指令保留在 `sent` 状态，可在管理端重新下发或人工确认。
- 服务端可以主动断开指定设备（`POST /api/dogGateway/disconnect/{deviceId}`，内部为 `DogGateway::disconnect`），客户端会按上面的流程自动重连。

### 2.3 相关参数

| 参数 | 默认 | 说明 |
| --- | --- | --- |
| `--interval <SECS>` | 30 | 心跳间隔 |
| `--heartbeat-timeout <SECS>` | 10 | 心跳回包超时，超时即判定断线 |
| `--reconnect-delay <SECS>` | 1 | 首次重连等待 |
| `--reconnect-max-delay <SECS>` | 30 | 退避上限 |
| `--max-reconnect-attempts <N>` | 无限 | 连续失败上限 |
| `--no-reconnect` | 关 | 断线不重试，进程退出（可交给 systemd `Restart=always`） |

## 3. 设备 ID（从硬件派生）

每台设备的 `deviceId` 默认**从硬件信息自动派生**，因此无需逐台配置；同一个硬件每次启动都会得到同一个 ID。

探测优先级（第一个可用值即采用）：

| 优先级 | 来源 | 读取位置 | 说明 |
| --- | --- | --- | --- |
| 1 | 板级序列号 | `/sys/firmware/devicetree/base/serial-number`、`/proc/device-tree/serial-number` | RK35xx 等 ARM 板卡的 device tree 序列号 |
| 2 | CPU 序列号 | `/proc/cpuinfo` 的 `Serial` 行 | 树莓派等 |
| 3 | eMMC CID | `/sys/block/mmcblk*/device/cid` | eMMC 芯片唯一 ID（硬件级唯一） |
| 4 | DMI UUID / 主板序列号 | `/sys/class/dmi/id/product_uuid`、`/sys/class/dmi/id/board_serial` | x86 工控机 |
| 5 | 物理网卡 MAC | `/sys/class/net/*/address` | 跳过 `lo`、docker/veth/bridge 等虚拟网卡，以及**本地管理位**（随机/虚拟 MAC） |
| 6 | 系统 machine-id | `/etc/machine-id`、`/var/lib/dbus/machine-id` | **兜底**：克隆系统镜像会重复，不推荐依赖 |

规则与注意：

- 序列号会做归一化：只保留 ASCII 字母数字并转大写（例如 `a4:bb:6d:11:22:33` → `A4BB6D112233`）。
- 占位值会被忽略：长度 < 6、全 `0`、全 `F`、`NONE`/`UNKNOWN`/`TOBEFILLEDBYOEM` 等。
- 显式指定优先：`--device-id` / `DOG_DEVICE_ID` 会**原样使用**（仅去掉首尾空格），适合旧设备沿用原来的 16 位 ID（如 `00022025C0600001`）。
- 找不到任何硬件标识（如容器内只有虚拟网卡）时会报错并提示改用 `--device-id`，而不是生成随机 ID。
- 注册时会把来源一并通过 `deviceIdSource` 上报（如 `mac-address`），后端写入日志便于排查。
- 查看本机派生结果：

```bash
./target/release/dog-device-client --print-device-id
# C4C6E66E998E
# source: mac-address        （标准输出只有 ID，来源走 stderr）

# 排查"为什么是/不是某个 ID"
./target/release/dog-device-client --explain-device-id
```

`--explain-device-id` 会列出每个候选位置的探测结果（`FOUND` / `SKIP` / `DENIED` / `MISSING`），
例如在一台非 root 的 Ubuntu x86 机器上：

```text
device id : C4C6E66E998E
source    : mac-address
candidates:
  MISSING  device-tree-serial     /sys/firmware/devicetree/base/serial-number
  MISSING  cpu-serial             /proc/cpuinfo (Serial)
  DENIED   dmi-product-uuid       /sys/class/dmi/id/product_uuid        permission denied (run as root to use)
  DENIED   dmi-product-serial     /sys/class/dmi/id/product_serial      permission denied (run as root to use)
  DENIED   dmi-board-serial       /sys/class/dmi/id/board_serial        permission denied (run as root to use)
  FOUND    mac-address            /sys/class/net/enp0s31f6/address      C4C6E66E998E
  SKIP     mac-address            (30 virtual interfaces)
  FOUND    machine-id             /etc/machine-id                       C5157CDE...

hint: some hardware identifiers are root-only on Ubuntu; run as root or as a systemd service to use a more stable id
```

### Ubuntu 支持情况

| 平台 | 可用来源 | 说明 |
| --- | --- | --- |
| Ubuntu x86_64（工控机/笔记本） | DMI `product_uuid` / `product_serial` / `board_serial`（**root 才能读**，`0400`）→ 物理网卡 MAC → machine-id | 非 root 时自动退到 MAC；`enp*/eno*/wlp*` 均能识别，`docker/veth/br-/flannel/cni/zt` 等虚拟网卡会被跳过 |
| Ubuntu ARM64（RK3588 等） | `/proc/device-tree/serial-number`、`/sys/block/mmcblk0/device/cid`（**root 才能读**）→ MAC → machine-id | eMMC CID 是芯片级唯一，重装系统不变，强烈建议以 root/systemd 运行 |
| Ubuntu 容器 | 通常没有物理网卡（只有 veth） | 会在 `--explain-device-id` 中显示全部 `SKIP/DENIED`，此时请显式 `--device-id` |
| Ubuntu 虚拟机 | 虚拟网卡 MAC（QEMU/KVM 的 `52:54:*` 属本地管理地址，会被跳过） | 视虚拟化平台而定，必要时用 `--device-id` 或 `--device-id-file` 固定 |

> Ubuntu 上 `sysfs` 里的序列号/CID 默认是 `0400`（仅 root 可读）。**同一个设备如果是 root 启动一次、普通用户启动一次，可能得到两个不同的 ID**。
> 解决办法：固定启动身份（推荐 systemd 服务），并配合 `--device-id-file` 把首次检测结果钉住：

```ini
# /etc/systemd/system/dog-device-client.service
[Unit]
Description=Robot dog device client
After=network-online.target

[Service]
ExecStart=/opt/dog/bin/dog-device-client \
  --server https://agent:4433 \
  --device-id-file /var/lib/dog-device-client/device-id \
  --ca /opt/dog/certs/rootca.crt \
  --cert /opt/dog/certs/client.crt \
  --key /opt/dog/certs/client.key \
  --interval 30
# 客户端自身会重连；Restart 只是进程崩溃时的第二道保险
Restart=always
RestartSec=5
StateDirectory=dog-device-client

[Install]
WantedBy=multi-user.target
```

- `--device-id-file` 首次运行时写入检测到的 ID，之后优先复用（来源显示为 `pinned`），因此换用户/换权限启动也不会变成另一个设备。
- 想重新按硬件生成，删掉该文件再启动即可；想临时覆盖仍可用 `--device-id`（优先级最高）。

## 4. 构建与运行

```bash
cd client
cargo build --release

# 查看本机设备 ID（硬件派生）
./target/release/dog-device-client --print-device-id

# 最小验证：认证 → 注册 → 建立 WS → 心跳收到服务端确认 → 退出
./target/release/dog-device-client --server http://127.0.0.1:3000 --once

# 常驻运行：每 30 秒心跳，执行下发的任务，任务 JSON 通过 DOG_COMMAND_JSON 传给命令
./target/release/dog-device-client \
  --server http://127.0.0.1:3000 \
  --device-type X1 \
  --interval 30 \
  --exec 'echo "task $DOG_COMMAND_ID: $DOG_COMMAND_JSON"'

# 建立链路后上传一个文件（落盘并登记到「媒体资源」）
./target/release/dog-device-client --upload ./sample.jpg

# 巡检任务：用本地 ROS 地图包校验下发的 mapHash，不一致时拒绝执行
./target/release/dog-device-client \
  --server http://127.0.0.1:3000 \
  --map-package /opt/dog/maps/model.zip \
  --exec 'ros2 run dog_patrol patrol --mission "$DOG_COMMAND_JSON"'
```

参数说明：

| 参数 | 说明 |
| --- | --- |
| `--server <URL>` | 服务端根地址，`http(s)://host:port`（环境变量 `DOG_SERVER`，默认 `http://127.0.0.1:3000`） |
| `--device-id <ID>` | 覆盖硬件派生 ID（环境变量 `DOG_DEVICE_ID`，可选，优先级最高） |
| `--device-id-file <PATH>` | 把检测结果钉在该文件并复用（推荐 systemd 场景） |
| `--print-device-id` | 打印最终设备 ID 后退出 |
| `--explain-device-id` | 打印所有候选硬件标识的探测结果后退出 |
| `--device-type <TYPE>` | 注册时上报的设备类型 |
| `--ip <ADDR>` | 注册时上报的设备 IP |
| `--interval <SECS>` | 心跳间隔，默认 30 秒 |
| `--heartbeat-timeout <SECS>` | 心跳回包超时（判定半开连接），默认 10 秒 |
| `--reconnect-delay/--reconnect-max-delay/--max-reconnect-attempts/--no-reconnect` | 断线重连退避策略，见第 2 节 |
| `--upload <PATH>` | 链路建立后上传一次该文件 |
| `--exec <CMD>` | 收到任务时执行的 shell 命令（`DOG_COMMAND_ID` / `DOG_COMMAND_JSON` 环境变量） |
| `--map-package <PATH>` | 本地 ROS 地图包（`.zip`），启动时计算 SHA-256；任务携带 `mapHash` 时用于一致性校验 |
| `--map-hash <HEX>` | 直接指定本地地图 SHA-256（优先于 `--map-package`） |
| `--ca/--cert/--key <PEM>` | `wss://` 与 mTLS 证书（不传则使用系统根证书） |
| `--once` | 只做认证、注册、一次心跳验证后退出 |

## 5. 地图基线校验

服务器下发引用路线的任务时，会在 `params.map` 中携带创建路线时冻结的地图基线：

```json
{
  "type": "server-command",
  "commandId": 1700000000001,
  "params": {
    "mission": "patrol",
    "map": {
      "mapId": 1700000000000,
      "mapVersion": "v1.0",
      "mapHash": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
      "frameId": "map",
      "resolution": "0.05",
      "origin": "-8.7, -17.7, 0.0",
      "width": 1024,
      "height": 768
    }
  }
}
```

客户端在 `--exec` 之前会：

1. 读取 `params.map.mapHash`；缺失 → 返回 `error`，不执行任务。
2. 读取本地地图哈希（`--map-package` 计算 / `--map-hash` 指定）；未配置 → 返回 `error`。
3. 两者不一致 → 返回 `error`，并在 `returnValue` 中带上期望值与本地值。

`mapHash` 是上传地图包时后端计算的 SHA-256（zip 文件内容），因此设备上的地图包必须是**同一份文件**。没有 `map` 字段的普通指令（如设备自检）不受影响。

## 6. TLS / mTLS

```bash
./target/release/dog-device-client \
  --server https://agent:4433 \
  --device-id 00022025C0600001 \
  --ca  ../mgmt_platform/backend/src/main/client/cer/rootca.crt \
  --cert ../mgmt_platform/backend/src/main/client/cer/client.crt \
  --key  ../mgmt_platform/backend/src/main/client/cer/client.key
```

`--ca` 用于校验服务端证书，`--cert` + `--key` 用于客户端证书认证；两者可单独使用。

> 注意：服务端 `GET /api/dogGateway/auth/{deviceId}` 沿用原网关语义，只按设备号下发 token，
> 身份校验依赖入口层（mTLS / 内网隔离 / 反向代理）。上生产时请务必启用其中一种。

## 7. 验证能通信

自动化（推荐，随 `cargo test` 一起跑）：

```bash
cd backend
cargo test --test dog_gateway_tests -- --nocapture
```

`tests/business/dog_gateway_tests.rs` 会启动真实的 axum 服务（临时 SQLite + 迁移），然后用本库完成：

1. 设备认证 + 注册（`dog_device` 落库，初始 `offline`）；
2. 建立 WebSocket 并发送心跳，校验服务端回包 `status=online`，`dog_device.status/comm_time/battery/location` 更新；
3. 管理端通过 `POST /api/dogCommand` 创建任务，校验指令状态变为 `sent`；
4. 客户端收到 `server-command`，校验 `commandId/params`；
5. 客户端回报结果，校验 `dog_command.status=success`、`returnValue` 落库，且 REST 查询接口返回一致；
6. 上传文件，校验落盘内容与 `dog_media` 记录；
7. 关闭连接，校验设备回到 `offline`。

重连行为由 `test_device_gateway_reconnects_after_server_disconnect` 覆盖：通过管理端接口 `POST /api/dogGateway/disconnect/{deviceId}` 主动断开设备 →
客户端自动重连（鉴权/注册/建链全套重来）→ 新链路上继续收到管理端下发的任务 → 优雅退出。

手工验证：

```bash
# 终端 1：后端（自动执行迁移）
cd backend && cargo run

# 终端 2：客户端（设备 ID 自动从硬件获取）
cd client && cargo run -- --interval 10

# 终端 3：登录后创建一条指令（设备在线则立即推送）
TOKEN=$(curl -s -X POST http://127.0.0.1:3000/api/token \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode 'username=admin' --data-urlencode 'password=<AES_BASE64>' \
  --data-urlencode 'grant_type=password' --data-urlencode 'scope=server' | jq -r .access_token)

# 设备 ID 用 --print-device-id 得到
curl -s -X POST http://127.0.0.1:3000/api/dogCommand \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"commandName":"巡检","commandType":"task","deviceId":"<DEVICE_ID>","commandParams":"{\"head\":\"SCAN\"}"}'
# 终端 2 的客户端日志会打印 "task received"
```

## 8. 目录

```text
client/
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs         # 对外导出
    ├── auth.rs        # DeviceClient：认证 / 注册 / 生成 WS 地址 / 连接
    ├── link.rs        # DeviceLink：心跳、收任务、回报、上传文件
    ├── protocol.rs    # 与后端共享的报文结构（camelCase）
    ├── device_id.rs   # 硬件信息探测与设备 ID 归一化
    ├── session.rs     # 会话主管：鉴权/注册/建链/心跳/断线重连/优雅退出
    ├── reconnect.rs   # 重连退避策略（指数 + 抖动 + 尝试上限）
    ├── tls.rs         # rustls 连接器与 mTLS
    ├── error.rs       # ClientError
    └── main.rs        # CLI
```
