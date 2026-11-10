# RoboDog Device Client

设备端 WebSocket 客户端模拟器，用于连接 RoboDog OPS 业务服务的**设备网关**。

## 架构概览

```
┌─────────────────┐         WebSocket          ┌──────────────────────┐
│  设备客户端      │ ◄══════════════════════►   │  RoboDog OPS Server   │
│  (四足机器人)     │    ws://host:3100/ws/device │                      │
│                 │                              │  ┌────────────────┐  │
│  - 注册          │                              │  │  gateway.rs     │  │
│  - 心跳          │                              │  │  (WS 网关)      │  │
│  - 接收任务      │                              │  └───────┬────────┘  │
│  - 上报进度      │                              │          │           │
│  - 接收指令      │                              │  ┌───────▼────────┐  │
│                 │                              │  │  handlers/      │  │
└─────────────────┘                              │  │  (REST API)    │  │
                                                 │  └───────┬────────┘  │
                                                 │          │           │
                                                 │  ┌───────▼────────┐  │
                                                 │  │  SQLite DB     │  │
                                                 │  └────────────────┘  │
                                                 └──────────────────────┘
```

## 通信消息协议

所有消息采用统一 JSON 信封格式：

```json
{
  "msgType": "message_type",
  "ts": "2024-01-01 12:00:00",
  "payload": { ... }
}
```

### 设备 → 服务端

| 消息类型 | 说明 | 方向 |
|---------|------|------|
| `register` | 设备注册 | Client→Server |
| `heartbeat` | 心跳上报 | Client→Server |
| `task_progress` | 任务进度 | Client→Server |
| `event` | 设备事件 | Client→Server |
| `pong` | 心跳响应 | Client→Server |

### 服务端 → 设备

| 消息类型 | 说明 | 方向 |
|---------|------|------|
| `register_ack` | 注册确认 | Server→Client |
| `heartbeat_ack` | 心跳确认 | Server→Client |
| `task_dispatch` | 任务下发 | Server→Client |
| `task_control` | 任务控制(暂停/恢复/取消) | Server→Client |
| `command` | 远程指令 | Server→Client |
| `error` | 错误信息 | Server→Client |

### 消息详情

#### register（设备注册）

```json
{
  "msgType": "register",
  "ts": "2024-01-01 12:00:00",
  "payload": {
    "deviceId": "RD-001",
    "secret": null
  }
}
```

#### register_ack（注册确认）

```json
{
  "msgType": "register_ack",
  "ts": "2024-01-01 12:00:00",
  "payload": {
    "success": true,
    "message": "注册成功",
    "deviceId": "RD-001",
    "deviceName": "X30-Pro",
    "serverTs": "2024-01-01 12:00:01"
  }
}
```

#### heartbeat（心跳上报）

```json
{
  "msgType": "heartbeat",
  "ts": "2024-01-01 12:00:10",
  "payload": {
    "battery": 85,
    "signal": 4,
    "temperature": 38.6,
    "position": "31.2014, 121.6023",
    "odometer": 12.4,
    "runtimeMinutes": 267,
    "currentTask": "A区上午例行巡检",
    "firmware": "v2.6.1"
  }
}
```

#### task_dispatch（任务下发）

```json
{
  "msgType": "task_dispatch",
  "ts": "2024-01-01 12:00:10",
  "payload": {
    "taskId": 1,
    "taskCode": "TK-1109-01",
    "taskName": "A区上午例行巡检",
    "taskType": "定时巡检",
    "executeMode": "周期任务",
    "route": {
      "routeId": 1,
      "routeCode": "RT-001",
      "routeName": "A区产线巡检路线"
    },
    "routeData": {
      "schemaVersion": 2,
      "loop": "闭环",
      "points": [...],
      "legs": [...]
    },
    "planStart": "09:00",
    "planEnd": "09:40",
    "dispatchPolicy": "按路线顺序执行",
    "description": "覆盖产线 6 个关键点位"
  }
}
```

#### task_progress（任务进度上报）

```json
{
  "msgType": "task_progress",
  "ts": "2024-01-01 12:05:00",
  "payload": {
    "taskId": null,
    "taskCode": "TK-1109-01",
    "status": "执行中",
    "progress": 25,
    "pointIndex": 3,
    "message": "航点 3 执行完毕，温度 39.2℃"
  }
}
```

#### command（远程指令）

```json
{
  "msgType": "command",
  "ts": "2024-01-01 12:00:10",
  "payload": {
    "command": "开始巡检",
    "params": null,
    "issuedAt": "2024-01-01 12:00:05"
  }
}
```

#### event（设备事件上报）

```json
{
  "msgType": "event",
  "ts": "2024-01-01 12:10:00",
  "payload": {
    "eventType": "info",
    "message": "温度传感器读数正常: 38.5℃",
    "data": {
      "temperature": 38.5,
      "humidity": 55
    }
  }
}
```

## 快速开始

### 1. 启动服务端

```bash
cd business/server
cargo run
```

### 2. 启动设备客户端

```bash
# 默认连接 RD-001
cd business/client
cargo run

# 指定设备 RD-007
cargo run -- --device-id RD-007

# 自定义服务端地址
cargo run -- --server ws://192.168.1.100:3100 --device-id RD-003

# 模拟低电量设备
cargo run -- --device-id RD-004 --battery 42 --position "充电座 P-01"

# 查看所有选项
cargo run -- --help
```

### 3. 验证连接

客户端启动后会自动：
1. 连接 WebSocket → 发送 register
2. 收到 register_ack → 开始心跳
3. 如有待执行任务 → 自动接收 task_dispatch
4. 定期上报 task_progress
5. 可接收远程 command

### 4. 远程下发指令

服务端运行时，调用 REST API：

```bash
# 向 RD-001 发送指令（会同时通过 WebSocket 推送）
curl -X POST http://127.0.0.1:3100/api/devices/1/command \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"command":"开始巡检"}'
```

### 5. 创建任务并推送给设备

```bash
# 创建任务，如果 deviceId 指定了在线设备，会通过 WS 推送
curl -X POST http://127.0.0.1:3100/api/tasks \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "taskName": "远程下发测试任务",
    "deviceId": 1,
    "routeId": 1,
    "parkId": 1
  }'
```

## 命令行参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--server` | `ws://127.0.0.1:3100` | 服务端地址 |
| `--device-id` | `RD-001` | 设备编号 |
| `--secret` | 无 | 设备密钥（可选） |
| `--heartbeat` | `10` | 心跳间隔（秒） |
| `--battery` | `86` | 初始电量 |
| `--signal` | `4` | 信号强度 |
| `--position` | `31.2014, 121.6023` | GPS 坐标 |
| `--firmware` | `v2.6.1` | 固件版本 |

## 设备模拟行为

客户端启动后模拟真实设备行为：
- **电量**: 缓慢下降（任务执行时 ~2%/tick，待命时 ~1%/tick）
- **信号**: 随机波动 (-1~+1)
- **温度**: 随运行缓慢上升，上限 65℃
- **位置**: 模拟 GPS 漂移
- **里程**: 模拟移动，每 tick 增加 0~2m
- **任务进度**: 每 15 秒上报一次，5% 概率报异常
- **随机事件**: 每 2 分钟发送一次设备事件

## 与 REST API 集成

创建/更新 REST API 时，WebSocket 网关会自动推送：

| REST API | WebSocket 推送 |
|----------|---------------|
| `POST /api/tasks` (指定 deviceId) | → 推送 `task_dispatch` |
| `POST /api/devices/{id}/command` | → 推送 `command` |
| 设备注册 | ← 自动推送待执行任务 |

## 文件说明

```
client/
├── Cargo.toml          # Rust 依赖
├── README.md           # 本文档
└── src/
    └── main.rs         # 客户端实现（模拟器）
```

## 服务端网关模块

```
server/src/
├── gateway.rs          # WebSocket 网关实现
├── gateway_types.rs    # 消息协议类型定义
└── handlers/
    ├── devices.rs      # 设备 REST API + WS 推送集成
    └── tasks.rs        # 任务 REST API + WS 推送集成
```
