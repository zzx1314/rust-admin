# dog-device-client

机器狗设备端客户端（Rust）。由 `mgmt_platform/backend/src/main/client` 下的 C 客户端迁移而来：

| 旧 C 程序 | 作用 | 迁移后 |
| --- | --- | --- |
| `https/client_auth.c`、`client_register.c` | 设备认证、注册 | `DeviceClient::authenticate` / `register` |
| `ws/src/client_https_demo.c`、`device_client/src/report/Report.c` | WebSocket 长连接、心跳、文件上传 | `DeviceLink`（心跳 / 收任务 / 回报 / 上传） |
| `tcp/tcpdemo.c` | 旧 TCP 文件帧上传（端口 9000） | 未迁移（服务端已统一为 WebSocket 通道） |
| `cer/*.crt`、`*.key` | mTLS 材料 | `TlsOptions`（rustls，读取同样的 PEM） |

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

## 2. 构建与运行

```bash
cd client
cargo build --release

# 最小验证：认证 → 注册 → 建立 WS → 心跳收到服务端确认 → 退出
./target/release/dog-device-client --server http://127.0.0.1:3000 --device-id dog-001 --once

# 常驻运行：每 30 秒心跳，执行下发的任务，任务 JSON 通过 DOG_COMMAND_JSON 传给命令
./target/release/dog-device-client \
  --server http://127.0.0.1:3000 \
  --device-id dog-001 \
  --device-type X1 \
  --interval 30 \
  --exec 'echo "task $DOG_COMMAND_ID: $DOG_COMMAND_JSON"'

# 建立链路后上传一个文件（落盘并登记到「媒体资源」）
./target/release/dog-device-client --device-id dog-001 --upload ./sample.jpg
```

参数说明：

| 参数 | 说明 |
| --- | --- |
| `--server <URL>` | 服务端根地址，`http(s)://host:port`（环境变量 `DOG_SERVER`，默认 `http://127.0.0.1:3000`） |
| `--device-id <ID>` | 设备 ID（必填，环境变量 `DOG_DEVICE_ID`） |
| `--device-type <TYPE>` | 注册时上报的设备类型 |
| `--ip <ADDR>` | 注册时上报的设备 IP |
| `--interval <SECS>` | 心跳间隔，默认 30 秒 |
| `--upload <PATH>` | 链路建立后上传一次该文件 |
| `--exec <CMD>` | 收到任务时执行的 shell 命令（`DOG_COMMAND_ID` / `DOG_COMMAND_JSON` 环境变量） |
| `--ca/--cert/--key <PEM>` | `wss://` 与 mTLS 证书（不传则使用系统根证书） |
| `--once` | 只做认证、注册、一次心跳验证后退出 |

## 3. TLS / mTLS

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

## 4. 验证能通信

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

# 终端 2：客户端
cd client && cargo run -- --device-id dog-001 --interval 10

# 终端 3：登录后创建一条指令（设备在线则立即推送）
TOKEN=$(curl -s -X POST http://127.0.0.1:3000/api/token \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode 'username=admin' --data-urlencode 'password=<AES_BASE64>' \
  --data-urlencode 'grant_type=password' --data-urlencode 'scope=server' | jq -r .access_token)

curl -s -X POST http://127.0.0.1:3000/api/dogCommand \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"commandName":"巡检","commandType":"task","deviceId":"dog-001","commandParams":"{\"head\":\"SCAN\"}"}'
# 终端 2 的客户端日志会打印 "task received"
```

## 5. 目录

```text
client/
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs         # 对外导出
    ├── auth.rs        # DeviceClient：认证 / 注册 / 生成 WS 地址 / 连接
    ├── link.rs        # DeviceLink：心跳、收任务、回报、上传文件
    ├── protocol.rs    # 与后端共享的报文结构（camelCase）
    ├── tls.rs         # rustls 连接器与 mTLS
    ├── error.rs       # ClientError
    └── main.rs        # CLI
```
