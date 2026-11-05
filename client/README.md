# dog-device-client

机器狗设备端客户端（Rust）。由 `mgmt_platform/backend/src/main/client` 下的 C 客户端迁移而来：

| 旧 C 程序 | 作用 | 迁移后 |
| --- | --- | --- |
| `https/client_auth.c`、`client_register.c` | 设备认证、注册 | `DeviceClient::authenticate` / `register` |
| `ws/src/client_https_demo.c`、`device_client/src/report/Report.c` | WebSocket 长连接、心跳、文件上传 | `DeviceLink`（心跳 / 收任务 / 回报 / 上传） |
| `tcp/tcpdemo.c` | 旧 TCP 文件帧上传（端口 9000） | 未迁移（服务端已统一为 WebSocket 通道） |
| `cer/*.crt`、`*.key` | mTLS 材料 | `TlsOptions`（rustls，读取同样的 PEM） |
| 硬编码 `deviceId` | 设备标识 | `device_id` 模块：从硬件信息派生（见第 2 节） |

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

## 2. 设备 ID（从硬件派生）

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
Restart=always
RestartSec=5
StateDirectory=dog-device-client

[Install]
WantedBy=multi-user.target
```

- `--device-id-file` 首次运行时写入检测到的 ID，之后优先复用（来源显示为 `pinned`），因此换用户/换权限启动也不会变成另一个设备。
- 想重新按硬件生成，删掉该文件再启动即可；想临时覆盖仍可用 `--device-id`（优先级最高）。

## 3. 构建与运行

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
| `--upload <PATH>` | 链路建立后上传一次该文件 |
| `--exec <CMD>` | 收到任务时执行的 shell 命令（`DOG_COMMAND_ID` / `DOG_COMMAND_JSON` 环境变量） |
| `--ca/--cert/--key <PEM>` | `wss://` 与 mTLS 证书（不传则使用系统根证书） |
| `--once` | 只做认证、注册、一次心跳验证后退出 |

## 4. TLS / mTLS

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

## 5. 验证能通信

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

## 6. 目录

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
    ├── tls.rs         # rustls 连接器与 mTLS
    ├── error.rs       # ClientError
    └── main.rs        # CLI
```
