# RoboDog 设备客户端

连接 RoboDog OPS 业务服务的设备客户端，同时作为**智航 Pro 机器狗本体**的驱动层。

一条任务的完整链路：

```
OPS 业务服务 ──task_dispatch──► 本客户端 ──智航 Pro WS──► 机器狗本体
（园区/任务/路线）                （协议翻译 + 执行引擎）    （建图/导航/回充）
       ▲                                │
       └──────task_progress 上报────────┘
```

建图任务还多一段**地图回传链路**（A 方案，见下文「建图回传中台」）：

```
机器狗本体 ──4001 推送地图路径──► 本客户端
                                      │ ① HTTP GET 拉 .zip 包
                                      ▼
                                本体文件服务（额外开放）
                                      │ ② POST /api/map-versions/ingest
                                      ▼
                              OPS 业务服务 ──► 中台 dog_map（同时写 map_versions）
```

`--robot` 为空时退化为纯模拟模式（只回心跳，不驱动机器人），便于前端联调。

本体建链细节（`control` / `bulkdata` 双通道、`1201/1202/1203` 时序、心跳与重连）
见 [`../docs/client-connection-guide.md`](../docs/client-connection-guide.md)。

## 架构

```
┌──────────────────┐   ws://server:3100/ws/device   ┌────────────────────┐
│  本客户端         │ ◄════════════════════════════► │  OPS Server         │
│                  │                                │  gateway.rs         │
│  main.rs         │                                └────────────────────┘
│   ├ dispatch.rs  │ 业务载荷 → FlightPlan
│   └ zhpro/       │
│      ├ client    │ ──── ws://robot:10010/control ────┐
│      ├ protocol  │ ──── ws://robot:10010/bulkdata ───┤  智航 Pro 网关
│      ├ status    │                                     │  (机器狗本体)
│      ├ mapping   │  建图 / 地图管理                     │
│      └ route     │  航线执行引擎                        ┘
│   └ mapupload.rs │ 建图完成后拉包 + 回传中台
└──────────────────┘
```

## 快速开始

```bash
cd business/client

# 0) 现场默认值已内置，直接跑即可
#    业务服务 ws://39.107.55.13:3100（正式环境外网）
#    设备编号启动时自动读本体 SN 分区（/dev/mmcblk0p5）
#    本体     192.168.234.234:10010
cargo run --

# 1) 纯模拟模式（不接本体，只跑业务协议）
cargo run -- --robot sim

# 2) 接其它本体：驱动智航 Pro
cargo run -- --device-id RD-001 --robot 192.168.10.21

# 3) 指定地图根目录与建图类型
cargo run -- --device-id RD-001 --robot ws://192.168.10.21:10010 \
  --map-root /ota/alg_data/map --map-kind indoor

# 4) 只做一次建图，结束即退出（运维手工建档用）
cargo run -- --robot 192.168.10.21 --map-only --map-name "B区-20261110"

# 5) 建图完成后自动回传中台（A 方案）
#    园区归属优先用控制台绑定的园区（注册回执下发），也可用 --park-id 覆盖
cargo run -- --device-id RD-001 --robot 192.168.10.21 \
  --robot-file-base http://192.168.10.21:12000 \
  --api-username admin --api-password admin123

# 查看全部参数
cargo run -- --help

# 6) 现场部署：用配置文件固定参数（推荐，见 deploy/）
cp client.toml.example client.toml   # client.toml 已被 .gitignore 忽略
cargo run -- --config client.toml
cargo run -- --print-config          # 打印脱敏后的生效配置
```

## 现场部署（192.168.41.246 + 机器狗 192.168.234.234）

| 项 | 值 | 来源 |
|----|----|------|
| 业务服务（外网） | `ws://39.107.55.13:3100` | 内置默认值 `DEFAULT_SERVER_URL` |
| 设备编号 | 启动时自动读本体 SN（现场实测 `0202222672400230`） | `device_sn` 模块读 `/dev/mmcblk0p5` |
| 机器狗本体 | `192.168.234.234:10010` | 内置默认值 `DEFAULT_ROBOT_HOST` |
| 设备地图/文件服务 | `http://192.168.234.234:12000` | 由本体地址 + `robot.file_port` 推导 |

设备编号是资产级 SN，协议里没有字段可回读，所以客户端**在代码里现算**：启动时读
本体 eMMC 的 `mmcblk0p5` 分区（`src/device_sn.rs`，等价下面这条命令），不需要写进
配置文件；读不到就报错并要求 `--device-id` 显式指定，避免注册出错误/幽灵设备。

```bash
root@ubuntu-arm64:~# hexdump -C /dev/mmcblk0p5 2>/dev/null \
  | sed -n '3s/.*|\(.*\)|.*/\1/p' | tr -d ' '
0202222672400230
```

> 注意 `hexdump -C` 会把开头全 0 的行折叠成 `*`，所以这条命令里的「第 3 行」在
> 现场分区上其实位于偏移 `0x200`；客户端不依赖行号，而是扫描前 4KB 里
> `hexdump -C` 的每条 16 字节行，优先取 16 位十六进制的行。

一项式安装（编译 release、装 systemd 单元，默认值即上表）：

```bash
sudo ./deploy/install.sh                    # 已按现场值装好，无需额外参数
sudo ./deploy/check.sh                      # 连接自检（服务端口 / health / 本体 10010+12000 / 注册探针）
sudo systemctl status robodog-device-client
journalctl -u robodog-device-client -f
```

`deploy/check.sh` 只读不写设备配置，会跑一次短时注册探针（业务服务会自动建档 / 刷心跳）；
`--no-probe` 时只查端口。临时换地址自检：
`SERVER_URL=ws://10.0.0.5:3100 ROBOT_HOST=192.168.1.10 ./deploy/check.sh`。

> 首次注册会在业务库自动建档为「待绑定」，还需要在控制台设备管理里把该设备绑定到园区，
> 否则心跳/任务不推进、建图也不会回传中台。

## 配置文件

优先级：**命令行 > 环境变量 > `client.toml` > 内置默认值**。

- 默认读取工作目录下的 `client.toml`；可用 `--config <path>` 或 `ROBODOG_CLIENT_CONFIG` 指定。
- 默认路径不存在时静默使用默认值；**显式指定**的路径不存在则启动失败。
- 密钥推荐走环境变量：`server.secret_env` / `api.password_env` 填写变量名，不要写明文。
- 未知字段会直接报错（防止 `secrect` 这类拼写错误被忽略），`--print-config` 可核对生效值与来源。

完整字段、默认值与 systemd 示例见
[`../docs/client-connection-guide.md`](../docs/client-connection-guide.md) §11。

## 命令行参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--server` | `ws://39.107.55.13:3100` | 业务服务地址 |
| `--device-id` | 自动读本体 SN 分区 | 仅用于本地调试 / 多实例覆盖；默认取 `/dev/mmcblk0p5` |
| `--secret` | 无 | 设备密钥（推荐 `ROBODOG_DEVICE_SECRET`） |
| `--heartbeat` | `10` | 心跳间隔（秒） |
| `--battery` | `86` | 初始电量 |
| `--signal` | `4` | 信号强度 |
| `--position` | `0.0, 0.0` | 初始地图坐标 |
| `--firmware` | `Hermes_M_v0.3.2` | 固件版本 |
| `--robot` | `192.168.234.234` | **智航 Pro 本体地址**；填 `sim`/`none`/`off` = 纯模拟模式 |
| `--map-root` | `/ota/alg_data/map` | 本体地图根目录 |
| `--model` | 空 | 设备型号：本体 `1201.type` 为空时用它（注册/心跳上报，中台台账展示） |
| `--map-only` | false | 只执行一次建图后退出 |
| `--map-name` | 时间戳 | `--map-only` 时的地图名 |
| `--map-kind` | `outdoor` | 建图类型：`outdoor` / `indoor` |

实时视频没有命令行开关：参数在 `client.toml` 的 `[video]` 段，拉流地址由业务平台下发。

### 建图回传中台（A 方案）

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--robot-file-base` | 由 `--robot` 推导 | 本体文件服务基地址，如 `http://192.168.234.234:12000` |
| `--robot-file-port` | `12000` | 推导基地址时使用的端口（文档 §4 文件服务端口；`--robot-file-base` 显式给出时忽略） |
| `--no-upload-map` | false | 关闭建图后自动回传，只在本体落盘 |
| `--park-id` | 控制台绑定园区 | 地图归属园区：不填则用注册回执里下发的绑定园区；两者都没有时跳过回传。手动指定时优先生效 |
| `--ingest-map-name` | 用 4001 返回的目录名 | 入库到中台的地图名 |
| `--api-base` | 由 `--server` 推导 | OPS 业务服务 HTTP 地址，如 `http://39.107.55.13:3100` |
| `--api-username` | `admin` | 业务服务登录用户名 |
| `--api-password` | `admin123` | 业务服务登录密码 |

除了建图完成后的自动回传（4001），控制台/中台还可以下发 `同步地图` 指令：
客户端用 4012 拉取本体地图列表，先向业务服务查询已同步的 `sourceRef`，
只从文件服务下载缺失地图并回传；手动同步不会改写园区当前基线，也不会因
`--ingest-map-name` 把多张地图改成同一个名字。

`--robot` 支持三种写法，等价：裸 IP、`ws://ip:10010`、`ws://ip:10010/control`。

园区归属优先级：`--park-id` > 服务端注册回执 / `park_binding` 推送（控制台设备管理里绑定）
> 两者都没有则跳过回传。控制台绑定 / 解绑会实时推送给在线设备，不需要重启客户端。

## 实时视频回传（RTSP → FLV）

本体只对外提供 RTSP（现场 `rtsp://user:***@192.168.234.1:8554/test`），浏览器不能直连；
能访问本体网段的是本客户端，所以视频由客户端拉流回传：

```
本体 RTSP ──ffmpeg──► 本客户端（src/video.rs） ──ws://平台/ws/device/video──► 业务平台 ──ws──► 浏览器
                     H.264 / FLV / 低延迟              二进制帧中继                 flv.js（MSE）
```

**按需拉流**：控制台打开设备「实时监控」时才收到 `video_start`，携带的就是设备管理里
填的 `video_url`；最后一个观看者离开立刻 `video_stop`。没人看时本进程不起 ffmpeg。

- 地址不落配置：`video_url` 属于设备档案，运营在「设备管理 → 编辑 → 视频地址」改一次即可。
- 断流自动重连：ffmpeg 退出或中继连接断开后 3 秒重试，连续重试直到收到 `video_stop`。
- 编码参数在 `client.toml` 的 `[video]` 段（见 `client.toml.example`）：

| 键 | 默认值 | 说明 |
|----|--------|------|
| `enabled` | `true` | 是否响应 `video_start` |
| `ffmpeg` | `ffmpeg` | ffmpeg 可执行文件路径 |
| `transport` | `tcp` | RTSP 传输方式（非 `rtsp://` 地址不传该选项） |
| `encoder` | `auto` | 编码器：按 `libx264` → `h264_rkmpp` → `h264_v4l2m2m` 探测，也可写死 |
| `fps` | `15` | 输出帧率，同时决定关键帧间隔（`2×fps`） |
| `bitrate` | `1200k` | 目标码率，一路流约等于这个值 |
| `width` | `0` | 输出宽度，`0` = 保持原始分辨率（等比缩放） |

编码器说明（现场踩过）：机器狗（瑞芯微 RK 系列）自带的是定制 ffmpeg，`--enable-rkmpp`
但**没有 `--enable-libx264`**，带 `-preset` 会直接
`Unrecognized option 'preset'` 启动失败。所以 `encoder = "auto"` 会探测并选中
`h264_rkmpp`（硬件编码，实测 1080p60→1280x720/15fps 约 1 个核），
硬件编码器分支不会传 `-preset/-tune/-profile/-sc_threshold` 这些 x264 专属参数。

时间戳（现场踩过）：那条 RTSP 的 SDP 标 `60fps`、PTS 也按 `1/60` 递增，但实测只
投递 **约 25 帧/秒**（12s 抓包 299 包）。直接沿用相机时间轴时，媒体时间只走
0.42x 实时，浏览器 MSE 会反复饿死——播放器一直在「连接中 / LIVE」之间跳、画面慢放。

所以拉流固定带 `-use_wallclock_as_timestamps 1`：用**到达时间**当 PTS，
时间轴与真实时间 1:1（实测播放速率 0.98~1.00），配合 `-r 15` 输出 CFR。
这项和相机帧率无关，相机哪天真的按 60fps 出流也不会跑偏。

现场检查：

```bash
which ffmpeg && ffmpeg -version | head -1        # 没有就 apt install ffmpeg
ffprobe -rtsp_transport tcp <RTSP地址>           # 确认能拉到流（会打印编码/分辨率）
journalctl -u robodog-device-client -f | grep 视频
```

排障顺序：客户端日志出现「开始实时视频回传」→ 业务平台日志出现
`video publisher attached` → 浏览器出画面。只出现前者说明推流通道没连上；
都没有说明 `video_start` 没下发（检查设备是否「离线」、`video_url` 是否为空）。

## 支持的任务类型

`task_dispatch` 的 `taskType` 决定分派路径（见 `dispatch.rs::TaskMode`）：

| taskType | 智航动作 | 说明 |
|----------|----------|------|
| `建图` / `地图构建` | 5000 `kind=1` | 建图任务，结束触发 4001 保存 |
| `巡检` / 其它 | 航线执行引擎 | 逐航点下发 5000 `kind=2`，客户端侧执行点位动作 |
| `返航` / `返回充电` | 5014 | 返回已标定的回充点 |
| `回充` / `充电` | 5000 `kind=3` | 对桩充电 |
| `出桩` | 5000 `kind=5` | 从充电桩驶出 |

> 智航 v1.0.3 的 `kind=4`（patrol 原生巡检）**尚未开放**，因此巡检拆成
> 「导航任务逐点 curl + 客户端侧作业」。这样不受 `tasks` 动作闭集
> （只有 `0=等待`、`1=拍照`）的限制，后续接拍照/云台/红外都可以在客户端扩展。

## 业务侧下发载荷

`task_dispatch.payload` 的关键字段：

```json
{
  "taskId": 12,
  "taskCode": "TK-1110-01",
  "taskName": "A区上午例行巡检",
  "taskType": "巡检",
  "executeMode": "返航充电",
  "route": { "routeId": 3, "routeCode": "RT-003", "routeName": "A区产线" },
  "routeData": {
    "schemaVersion": 3,
    "coordFrame": "map",
    "loop": "闭环",
    "points": [
      { "name": "起点", "x": 0.0, "y": 0.0, "dwell": 0, "heading": 0,
        "radius": 0.3, "must": true, "onFail": "跳过继续", "actions": [] },
      { "name": "配电柜", "x": 5.0, "y": 2.0, "dwell": 15, "heading": 90,
        "radius": 0.3, "must": true, "onFail": "重试 2 次",
        "actions": [{ "type": "可见光拍照", "param": "1080P", "retry": 1 }] }
    ],
    "legs": [{ "mode": "沿道路", "speed": 1.5, "avoid": "标准" }],
    "returnLeg": null
  },
  "map": {
    "mapId": "ROS-MAP-3@v1.0",
    "mapDir": "2026_11_10_16_15_06",
    "mapPath": "2026_11_10_16_15_06",
    "mapType": 1,
    "parkId": 1,
    "mapName": "ROS-MAP-3",
    "version": "v1.0"
  },
  "speedLimit": 1.5,
  "avoidLevel": 40,
  "endTask": "return_to_charge"
}
```

- `map` 由服务端组装：优先用路线冻结的 `routes.map_version_id`，回退到园区当前基线
  `parks.map_version_id`。都没有则不下发，客户端会拒绝执行并上报异常。
- `routeData.points[].x/y` 是**地图坐标系下的米**（`coordFrame: "map"`，`schemaVersion: 3`），
  与 `5000 goals[].position` 同一坐标系（`map_path` 那张图的 `frame_id`）；
  `heading` 是度（客户端转弧度四元数）。服务端会拒发像素坐标 / 示意坐标 / 越界航点，
  客户端不需要也不会做任何比例换算。
- `mapDir` / `mapPath` 是**本体侧目录名**（4013 `path` 的最后一段，由建图会话决定），
  不是业务 `mapId`。服务端优先用 `map_versions.source_ref` 的真实目录名，缺失才退回
  `mapId` 的合成名（`@` → `_`）。**客户端不会直接信任它**：先拿 4012/4013 列表核对
  `mapDir` → `mapPath` → 合成名，命中就用本体上报的绝对 `path`（L2/M1 根目录不同，
  不能靠平台拼）；全都不命中就**拒绝下发**并在任务事件里列出本体现有目录，
  不再让 5000 指向空目录（那只会得到 `13898 map not found or invalid (missing map.yaml)`）。
  「重新定位」（4030）与返充（5014 的 `map_id` 也是目录名）走同一套解析。
- `endTask`：`return_to_charge` / `return_to_start` / `none`。
  缺省时客户端退化为按 `executeMode` 文案推断。
- `avoidLevel`：10 关闭 / 20 仅急停 / 30 感知 / 40 全避障。任务级生效，
  进入 STANDBY 后本体自动恢复 40。

## 上报载荷

`task_progress.payload`：

```json
{
  "taskCode": "TK-1110-01",
  "status": "执行中",
  "progress": 60,
  "pointIndex": 3,
  "totalPoints": 5,
  "message": "已完成 3/5 个航点",
  "stage": "navigation.path_running",
  "robotTaskId": "task-8f3a1c"
}
```

- `stage` 直接透传智航 5009 快照里的阶段串，服务端落到 `tasks.robot_stage`。
- `robotTaskId` 是 5001 返回的本体任务 ID，落 `tasks.robot_task_id`，用于对账机器人日志。
- 上报节奏：5009 推送是 500ms，客户端合流成 **5s 一次**，避免刷爆业务库。

`task_control` 支持 `pause` / `resume` / `cancel`，客户端收到后：
- 航线任务 → `RouteRunner::control`，暂停时同步给本体 5004，恢复 5006，取消 5002；
- 建图任务 → `cancel` 会触发 5002 并等待 4001 保存结果。

## 目标字段使用说明

`goals[]` 逐点下发（`MapManager` 前置校验通过后）：

| 业务字段 | 智航字段 | 说明 |
|----------|----------|------|
| `x` / `y` | `goals[].pose.position` | 地图坐标系（米） |
| `heading` | `goals[].pose.orientation` | 度 → 四元数 |
| `radius` | `goal_tolerance.x/y` | 到达判定半径，默认 0.25 |
| `dwell` | 客户端 sleep | 停留秒数 |
| `must` + `onFail` | 客户端重试策略 | `must=true` 不可跳过 |
| `actions[].type` | 客户端作业 | `可见光拍照` → `PointAction::Photo` |

## 智航 Pro 协议对接要点

```
1. control 连上 → 收 1201（client_id / session_id / bulkdata_bind_token）
2. 带 token 建 bulkdata → 首帧 1202 → 收 1203 才算可用
3. 1207 模块可用性：sdk_node 3s 内无 1525 心跳即判不可用
4. 1299 网关传输层错误、1300/1301 网关心跳
5. 4000~5999 自由数据区间需先 1204 取得 RoamerX 控制权
```

实现文件：

| 文件 | 职责 |
|------|------|
| `zhpro/protocol.rs` | 全部消息类型常量、枚举、`Envelope` 信封 |
| `zhpro/client.rs` | 双通道建链、单写循环、请求-响应关联、网关心跳 |
| `zhpro/status.rs` | 5009 V2 快照解析、`TaskMonitor` 去重与终态判定 |
| `zhpro/mapping.rs` | 建图会话（5000/4035/4001）与地图管理（4012~4045） |
| `zhpro/route.rs` | 航线执行引擎 `RouteRunner`、`FlightPlan`、`Waypoint` |
| `dispatch.rs` | 业务载荷 `routeData` → `FlightPlan` |

## 建图流程

```
--map-only 或 taskType=建图
  → 5000 kind=1, params.map_type=0(室外)/2(室内)
  → 4035 实时位姿与剩余里程（日志）
  → 结束原因：Ctrl+C / task_control cancel
  → 5002 取消建图
  → 等 4001 保存结果（含 map_path）
  → 上报 task_progress 90，stage=mapping.saved
  → 地图回传中台（见下节）
  → 上报 task_progress 终态（已完成/异常）
```

### 建图回传中台（A 方案）

智航 Pro 协议 40xx 全是操作类消息（列表/改名/删除/设回充点/载图定位），
**没有下载地图文件的接口**，所以地图包不能通过 WS 取回。A 方案的做法是：
本体额外开放一个 HTTP 文件服务，客户端在建图完成后自行拉包并上传。

```
4001 成功 ──► 取得地图目录名（见下方「注意」）
           ──► GET {base}/.../history_map/{dir}.zip   ← 依次探测 6 种 URL，校验 PK 魔数
           ──► POST {api_base}/api/map-versions/ingest  （multipart: file + mapInfo）
           ──► 业务服务转推中台 dog_map，同时写 map_versions
```

文档 §4「文件服务」明确：**HTTP 端口 12000**，L2 文件根 `/userdata/alg_data`、
M1 文件根 `/ota/alg_data`；文档 §2.2.4.1「4013 地图列表」明确单张地图路径为
`<map_dir_prefix>/history_map/<目录名>`，其中 `map_dir_prefix = /ota/alg_data/map`。
据此探测的 6 种 URL 依次为（命中即停，全部要求返回 `PK` 开头的 zip）：

```
{base}/ota/alg_data/map/history_map/{dir}.zip       ← 文档 M1 绝对路径
{base}/userdata/alg_data/map/history_map/{dir}.zip  ← 文档 L2 绝对路径
{base}/history_map/{dir}.zip                        ← base 已含 map 根目录
{base}/{dir}.zip
{base}/history_map/{dir}?format=zip
{base}/{dir}?format=zip
```

`{base}` 由 `--robot-file-base` 显式给出，否则从 `--robot` 推导成
`http://<host>:12000`。探测失败时错误信息会带上最后一次尝试的 URL 与失败原因
（HTTP 状态码 / 非 zip 内容），便于对着本体路由排查。

**现场固件（`robot_http_server` / `http-fileserver 3.0.0`）没有任何 zip 端点**，
只提供 `GET /api/list?path=<相对 alg_data 的路径>` 列目录和 `GET /files/<path>`
单文件下载。这种情况下 6 种 zip 探测全部 404，客户端会退到「本机打包」：

1. `GET {base}/api/list?path=/map/history_map/<目录名>` 递归列目录（深度 ≤3）；
2. `GET {base}/files/<entries[].path>` 逐文件下载；
3. 在本机打成 zip 后再走 `POST /api/map-versions/ingest`。

打包会跳过 `map.static/`（建图逐帧点云，实测单张地图 1700+ 个 pcd）、单文件
>64MB（`model_map.bin` 222MB、`map.bin` 121MB）、总量 >200MB，最多 64 个文件。
实测一张地图：16 个文件、原始 32.7MB → zip 11.7MB（含 `map.yaml`、`map.pgm`、
`map.geojson`、`map.txt`、`map.pcd`、`map_edit/`、`nav_paths/`）。

> **注意：地图路径来源尚未与文档对齐**
> 文档 §2.2.1.1 的 4001 **只返回 `data.code` 和 `data.msg`，不含地图路径**。
> 当前 `MappingSaved` 里的 `map_path` / `map_name` 提取属于兼容性兜底：若本体固件
> 额外回了路径就能用，按标准 v1.0.3 则取不到，拉包会失败并把任务记「异常」。
> 权威来源是 **4013 的 `map_path_list[].path`**（绝对路径）。真机联调前需在
> 4001 之后主动发一次 4012，按目录名/最新时间从列表里取到新地图路径再拉包。
> `zhpro/mapping.rs::RobotMap` 已经解析好 `path`，可直接复用。

回传结果直接决定任务终态：

| 结果 | 任务状态 | progress | 说明 |
|------|----------|----------|------|
| 上传成功 | 已完成 | 100 | 消息里带中台 `mapId` |
| 跳过（`--no-upload-map` / 设备未绑定园区且未给 `--park-id`） | 已完成 | 100 | 消息注明跳过原因 |
| 拉包或上传失败 | **异常** | 90 | 地图已在本体落盘，但未入库中台 |

之所以失败记「异常」而不是「已完成」：地块已建但没入库，后续无法绑定路线，
按成功上报会掩盖问题。包在业务服务 `data/map_ingest/` 留档，可凭日志路径重放。

建好的地图落盘在本体 `--map-root` 下。若走 A 方案自动回传，中台 `dog_map` 与业务库
`map_versions` 都会写入，无需再手工导入；否则业务侧需要用
`POST /api/map-versions/import-backend` 或在前端「地图基线」页做导入后才能绑定路线。

## 航线执行流程

```
1. 解析 routeData → FlightPlan
2. 前置校验：4013 地图列表里必须存在该 map_dir
3. 若 endTask=return_to_charge，校验 map_route_exist / charge_point_exist
4. 4044 设置任务级避障等级
5. 逐航点 5000 kind=2：
     need_locate 只在第 1 个点 true（避免每点重载地图）
     等 5009 生命周期终态
     失败按 on_fail 重试 / 跳过（must=true 不跳过）
6. 结束后按 endTask：
     return_to_charge → 5014
     return_to_start  → 再下发一次首点导航
     none             → 原地待命
```

## 业务协议速查

统一信封：`{ msgType, ts, payload }`

### 设备 → 服务端

| msgType | 说明 |
|---------|------|
| `register` | 设备注册（带上型号 / 固件版本，供中台台账同步） |
| `heartbeat` | 心跳（10s） |
| `task_progress` | 任务进度（5s） |
| `event` | 设备事件 / 告警 |
| `pong` | 心跳响应 |

注册 / 心跳里上报的台账字段（服务端落 `devices` 后同步中台 `dog_device`）：

```json
// register
{ "deviceId": "0202222672400230", "secret": null,
  "model": "ZSL-2-Ultra", "firmware": "Hermes_M_v0.3.2" }

// heartbeat（model 仅在取到本体 1201 的 type 时才带）
{ "battery": 86, "firmware": "Hermes_M_v0.3.2", "model": "ZSL-2-Ultra",
  "bodyOnline": true, "mode": "robot", "charging": false, "robotMode": "remote" }
```

- `model` 取本体 `1201` 的 `type`；本体没在 `device_json_path` 配型号时用 `robot.model` / `--model` 兜底，两者都空则不报该字段。
- `device_ip` 不需要客户端上报：业务服务取 WebSocket 对端 IP。
- 型号 / 固件 / 状态只在变化时触发中台重同步，心跳频率不影响中台接口压力。
- `bodyOnline` / `mode`：本体 `1315` 30s 内没上报 → `bodyOnline=false`（本体掉线或模拟模式），
  `mode` 为 `robot` / `sim`。业务平台据此不再展示假电量。**本体连不上时客户端直接退出**，
  由 systemd `Restart=always` 带退避重启，不会带着模拟数据冒充真机。
- `robotMode`：取本体 `1315` 的 `mode`（协议里的 MachineStatus，实测 `remote`），
  业务平台「工作模式」列直接展示；`1235/machine_status` 若存在也兼容。
- `signal` **已移除**：智航协议没有信号强度指标（`1315` 全字段核对过），
  不再上报也不再提供 `--signal` / `[simulation] signal` 配置。
  > 升级注意：配置解析是 `deny_unknown_fields`，老机器的 `/etc/robodog/client.toml`
  > 里如果还有 `[simulation] signal = 4`，新客户端会启动失败并提示
  > `unknown field 'signal'`。升级前先删掉这一行（或整段 `[simulation]`）。
- `charging`：取本体 `1315 kRobotStatus` 的 `charging_pile_connect` 或电池
  `power_supply_status1/2`（1=充电中、2=充满）；电量取 `battery.power1/power2`，
  温度取 `temp`（为 0 退回 `battery.temperature1`），里程取 `mile_data`。
  这些数值**只来自 1315**，没收到就报 `bodyOnline=false`，不做推断。

### 服务端 → 设备

| msgType | 说明 |
|---------|------|
| `register_ack` | 注册确认 |
| `heartbeat_ack` | 心跳确认 |
| `task_dispatch` | 任务下发 |
| `task_control` | 暂停 / 恢复 / 取消 |
| `command` | 远程指令 |
| `error` | 错误信息 |

### 远程指令

`command.payload.command` 取值：

| 指令 | 客户端动作 |
|------|-----------|
| `开始巡检` | 1204 取得 RoamerX 控制权 |
| `暂停任务` | 5004 暂停当前本体任务 |
| `继续任务` | 5006 恢复当前本体任务 |
| `召回充电` | 5014 返回回充点 |
| `同步地图` | 4012 拉地图列表并回传中台 |
| `重新定位` | 4030 载图 + 定位，等 `loc_state` 收敛到 100 |

本体动作（`payload.params.action`）除站立/卧倒/急停/灯光外，新增 1302 的三种模式：
`generic_mode`(60 通用，1304 的 lx/ly 才生效) / `in_place_mode`(61) / `navigation_mode`(62)。

### 完全接管（摇杆走不动怎么办）

`handle_teleop` 的接管流程是三步，缺一步就会出现「能站立但摇杆不走」：

1. `1204` 取控制权（等 `1205.status_code = 0`）；
2. `1302 cmd=60` 切通用模式——1304 的 `lx/ly` 在原地模式下无作用；
3. 摇杆按 50Hz 重发（`spawn_republisher`），松手立即发零帧。

另外两处现场因素靠日志排查：

- `1315 ctrl_source = 3`：硬件遥控器仍持权（协议没有禁用它的消息，只能关遥控器电源）；
- `1315 motion_status = lie_down`：卧倒状态下不会迈步，先站立。

`重新定位` 的附加参数在 `payload.params.params`：

```json
{ "command": "重新定位", "params": { "params": { "mapId": "ROS-MAP-3@v1.0",
  "pose": { "x": 1.5, "y": -2.0, "heading": 90 } } } }
```

`pose` 缺省（或 `null`）时下发零四元数，由 SLAM 读上一份位姿文件做完整重定位；
`heading` 单位是度。`mapId` 缺省时用机器人当前已载入地图（`state.active_map`）。

### 巡检起步前的定位守卫

航线下发（`task_dispatch` 且 `routeData.points` 非空）后，客户端在发第一条
`5000 kind=2` 之前会做一次定位预检：

1. 读最近一帧 5009 的 `robot.loc_state.status`，已是 `100` 直接起步（不重载地图，保住位姿）；
2. 否则下发 `4030 op=1` 载图并等收敛（默认 60s）；
3. 仍未收敛或 SLAM 明确失败（`203` / `210`）→ **不发 5000**，直接把任务置为「异常」，
   进度消息写明「定位未就绪：... 请远程接管把设备移回地图覆盖范围，或下发「重新定位」后重试」；
4. 预检成功后首航点用 `need_locate=false` 复用这份定位，避免刚重定位完又载一次图。

这样「机器狗不在地图上」就不会再以一条含义不明的 5000 失败收场。

## 与 REST API 的联动

| REST API | WebSocket 行为 |
|----------|---------------|
| `POST /api/tasks`（带 `deviceId`） | → 推送 `task_dispatch` |
| `POST /api/devices/{id}/command` | → 推送 `command` |
| `POST /api/devices/{id}/locate` | → 推送 `command: 重新定位`（载图 + 定位） |
| `PUT /api/tasks/{id}/status`（`已暂停`/`执行中`/`已完成`） | → 推送 `task_control` |
| 设备注册 | ← 补发所有 `待执行` 任务 |

## 校验

```bash
cd business/client
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
cargo test
```
