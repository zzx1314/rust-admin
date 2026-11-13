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

# 1.5) 假本体（离线联调）：进程内起一个智航 Pro 网关模拟器，
#      任务能真正跑完，能看到发给本体的真实 5000/5014 报文
cargo run -- --device-id RD-MOCK-01 --robot mock

# 2) 接其它本体：驱动智航 Pro
cargo run -- --device-id RD-001 --robot 192.168.10.21

# 3) 指定地图根目录与建图类型
cargo run -- --device-id RD-001 --robot ws://192.168.10.21:10010 \
  --map-root /ota/alg_data/map --map-kind indoor

# 4) 只做一次建图，结束即退出（运维手工建档用）
cargo run -- --robot 192.168.10.21 --map-only --map-name "B区-20261110"

# 5) 建图完成后自动回传中台（A 方案）
#    园区归属优先用控制台绑定的园区（注册回执下发），也可用 --park-id 覆盖；
#    都没有也能回传，地图会先入库为「设备地图」等人在控制台建园区时挑图
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

## 离线联调：假本体（`--robot mock`）

`--robot sim` 只回心跳，拿到 `client` 那一步就 return 了，**5000 之类的报文一条都不会生成**。
`--robot mock` 补上这段：客户端在自己进程里起一个本地 WS 服务（默认随机端口），实现真机
必经的那几条链路，然后按真机的方式连它——所以日志里的 `5000 载荷`、`5001 应答`
和接真机时逐字段一致，唯一区别是对端是模拟器，不需要现场那台狗。

```bash
# 终端 1：业务服务（配好 server.toml 的 [zentai]）
cd business/server && cargo run

# 终端 2：客户端 + 假本体，把报文全打出来
cd business/client
RUST_LOG=robodog_device_client=debug \
  cargo run -- --server ws://127.0.0.1:3100 --device-id RD-MOCK-01 --robot mock
```

首次注册仍是「待绑定」，要在控制台把 `RD-MOCK-01` 绑到园区才能下发任务。

### 模拟器实现了什么

| 链路 | 行为 |
| --- | --- |
| `1201` / `1202` / `1203` | control + bulkdata 双通道建链，`client_id` / `session_id` / bind token 全走真协议 |
| `1300` / `1301` | 网关心跳每秒应答 |
| `1315` | 1Hz 本体状态（电量 / 温度 / 里程 / `mode` / `ctrl_source=1` / `motion_status=stand` / 急停未触发） |
| `1207` | `sdk_node` / `slam` / `planner` 全部 available（否则客户端会以「SDK 节点未就绪」拒任务） |
| `1204` → `1205` | 广播 `status_code=0, client_id=本端`，客户端据此认为已拿到 RoamerX 控制权 |
| `4012` → `4013` | 上报 `map_path_list`（`map_route_exist` / `charge_point_exist` 可配） |
| `4030` → `4031` | 载图定位，随后 `5009` 的 `loc_state.status` 恒为 100 |
| `4044` → `4045` | 避障等级回显 `effective_level` |
| `5000 kind=2` → `5001` | 返回 `task_id`，`5009` 先 `running` 再 `success`，每个航点约 1.5s 走完 |
| `5000 kind=1` → `5002` → `4001` | 建图：取消后补一条保存结果（带 `map_path`） |
| `5000 kind=3/5`、`5014` → `5015` | 回充 / 出桩 / 返航充电，同样 1.5s 后 `success` |
| `5004` / `5006` / `5002` | 暂停 / 恢复 / 取消，生命周期同步进 `5009` |

一条巡检任务跑完，终端里能看到这样的序列（`-` 是客户端发的，`+` 是假本体回的）：

```text
+ 假本体已监听 ws://127.0.0.1:43995/control（联调模式，非真机）
- 连接智航 control 通道 / bulkdata 通道
+ 假本体: ← 4012 null            → 机器人地图就绪: ROS-MAP-… 目录=2026_11_10_16_15_06
+ 假本体: ← 4044 {"level":40}    → 导航避障等级已设置: 40
+ 假本体: ← 5000 {"kind":2,…}    → 5001 应答: {"code":0,"msg":"ok","task_id":"mock-1"}
+ 假本体: ← 5014 {"map_id":…}     → 巡检结束，返回充电点
```

### 可调项（环境变量）

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `ROBODOG_MOCK_PORT` | 0（随机空闲端口） | 固定端口，便于用别的工具连它看报文 |
| `ROBODOG_MOCK_STEP_MS` | 1500 | 单个航点「走完」耗时 |
| `ROBODOG_MOCK_DIRS` | `2026_11_10_16_15_06` | `4013` 里「本体现有地图目录」，逗号分隔 |
| `ROBODOG_MOCK_NO_ROUTE` | 空 | 设 1 时 `map_route_exist=false`，强制客户端回退 2D 栅格 |

不用现场默认地图时，用业务库里任一路线的 `map_versions.source_ref` 末段来填 `ROBODOG_MOCK_DIRS`，
否则客户端会因为「地图目录对不上」拒绝下发（那是真实行为，不是模拟器的 bug）。

### 报文对账：完整帧（head + data）

两侧都能看到**完整帧**，不需要真机：

```text
# 假本体侧（收到什么就打什么，与真机网关拿到的字节一致）
假本体: ← 5000 {"head":{"type":5000,"time":1794537199977,"src":1,"msg_id":5},
               "data":{"kind":2,"name":"… · 前往 新航点 1","params":{…}}}

# 假本体侧（发出去的帧，DEBUG；1315/5009 走 TRACE）
假本体: → {"head":{"type":5001,"time":…,"src":2,"msg_id":5},"data":{"code":0,"msg":"ok","task_id":"mock-1"}}

# 客户端侧（DEBUG）
→ 智航完整帧（含 head） msg_type=5000 msg_id=5 packet={"head":{…},"data":{…}}
← 智航完整帧（含 head） packet={"head":{"type":5001,…},"data":{"code":0,"msg":"ok","task_id":"mock-1"}}
```

想留档对账就加 `ROBODOG_FRAME_DUMP=/tmp/frames.jsonl`，双向逐帧落盘：

```bash
ROBODOG_FRAME_DUMP=/tmp/frames.jsonl RUST_LOG=robodog_device_client=debug \
  ./target/debug/robodog-device-client --server ws://127.0.0.1:3100 --device-id RD-001 --robot mock

jq -c 'select(.frame.head.type==5001)' /tmp/frames.jsonl     # 只看 5001 应答
jq -c 'select(.dir=="recv" and .frame.head.type==4013)' /tmp/frames.jsonl
```

与协议 §2.5.1.4 示例逐字段对照时，**唯一有意缺失的字段是 `params.fence_path`**：
业务侧还没建模电子围栏，而发空串 `""` 会让现场固件回「导航指令非法」，所以没围栏时
整个键都不出现（`zhpro/route.rs::navigation_task_payload`）。其余字段一个不少：
`kind/name/schedule/execute_at/priority/record` + `map_path/need_locate/map_type/goal_type/`
`goals[{position{x,y,z}, orientation{x,y,z,w}}]/speed_limit{vx,vy,vz}/goal_tolerance{x,y,theta}`。

> `goal_tolerance.y` 用的是航点 `radius`（不是示例里的 0）：到达判定是圆形半径，
> x/y 必须一致，否则只在一个方向上算到点。

### 边界

- **建图回传**仍然走 HTTP 文件服务（`--robot-file-base`），假本体不提供，所以联调建图任务时
  要么加 `--no-upload-map`，要么接受任务终态记「异常」（地图已保存但未入库）。
- 假本体只模拟协议时序，不做物理仿真：没有避障、坡道、定位跳变，因此**不能**当验收依据；
  真机联调前请至少跑一遍现场参数（速度、航点、回充点）。

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
| `--robot` | `192.168.234.234` | **智航 Pro 本体地址**；填 `sim`/`none`/`off` = 纯模拟模式（不生成任何本体报文），填 `mock` = 进程内假本体（离线联调，见上文） |
| `--map-root` | `/ota/alg_data/map` | 本体地图根目录 |
| `--road-network` | `off` | `5000` 的 `map_type`/`goal_type`：`off`=强制 `1`（2D 栅格，默认，与协议文档示例一致）、`auto`=按航段与本体地图能力判定、`on`=强制 `3`（路网）；也可用 `ROBODOG_ROAD_NETWORK` 或 `client.toml` 的 `[robot] road_network` |
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
| `--park-id` | 控制台绑定园区 | 地图归属园区：不填则用注册回执里下发的绑定园区；两者都没有时地图先入库为「设备地图」（`parkId` 为空），设备绑定人之后在控制台创建园区时挑图绑定。手动指定时优先生效 |
| `--ingest-map-name` | 用 4001 返回的目录名 | 入库到中台的地图名 |
| `--api-base` | 由 `--server` 推导 | OPS 业务服务 HTTP 地址，如 `http://39.107.55.13:3100` |
| `--api-username` | `admin` | 业务服务登录用户名 |
| `--api-password` | `admin123` | 业务服务登录密码 |

除了建图完成后的自动回传（4001），控制台/中台还可以下发 `同步地图` 指令：
客户端用 4012 拉取本体地图列表，先向业务服务查询本设备已同步的 `sourceRef`
（`GET /api/map-versions/synced-sources?deviceId=...`），
只从文件服务下载缺失地图并回传；手动同步不会改写园区当前基线，也不会因
`--ingest-map-name` 把多张地图改成同一个名字。**同步地图不要求设备绑定园区。**

`--robot` 支持三种写法，等价：裸 IP、`ws://ip:10010`、`ws://ip:10010/control`。

园区归属优先级：`--park-id` > 服务端注册回执 / `park_binding` 推送（控制台设备管理里绑定）
> 两者都没有也照常回传：地图先只跟设备挂钩（`map_versions.park_id` 为空），
> 设备绑定人之后在业务控制台创建园区时把自己的设备地图挑一张绑成基线。
> 控制台绑定 / 解绑会实时推送给在线设备，不需要重启客户端。

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
- `routeData.legs[].mode` / `map_type`：**默认一律下发 `1`（2D 栅格）**，与协议文档的
  导航任务示例完全一致；`goal_type` 与 `map_type` 相同（协议只允许 `1` 和 `3`，`2` 是 3D
  预留、下发了会被拒，两者不一致导航不会启动）。客户端不信任服务端的 `mapType`，按
  `--road-network` 策略自己定：
  - `off`（默认）：恒 `1`；
  - `auto`：航段写「沿道路」且本体地图 `4013 map_route_exist=true`（目录下有
    `map.geojson`）时用 `3`；
  - `on`：恒 `3`（验证本体路网能力用，本体没路网会被固件拒）。

  每次派发都会打一行判定原因：

  ```text
  INFO 5000 地图类型 map_type/goal_type = 1（road_network 策略=off，航线全沿道路=true，本体 map.geojson=true）
  ```
- `routeData.fencePath` / `payload.fencePath`：非空时作为 5000 `params.fence_path`
  下发；空串/空白不下发，避免固件把它当非法路径（现场会回“导航指令非法”）。

### 本体健康门禁

任务下发、`command`、`teleop` 入口统一检查（对应官方 App 的 ready 门禁）：

| 状态 | 判据 | 拒绝时提示 |
| --- | --- | --- |
| 本体 1315 超时 | `body_online=false` | 本体离线或算法节点未启动 |
| SDK 节点未就绪 | `1207 sdk_node=false` | 等待初始化 / 重启本体算法节点 |
| 软件急停 | `1315 estop.software=true` | 先点「解除急停」（1302 cmd=1） |
| 控制权被抢占 | `1315 ctrl_source=3` | 断开外部遥控器后重试 |
| 卧倒姿态 | `1315 motion_status=lie_down` | 先点「站立」再执行任务/导航模式 |

急停/解除急停/避障开关属于恢复类动作，不受接管态和上述门禁限制。
1205 失败、`ctrl_source` 变成 0/3 时客户端会清掉本地 `control_owned` 标记，**并且同步清掉
`TeleopState` 的接管态与摇杆缓存**（`teleop::reset_after_control_lost`，顺带补一帧零摇杆），
下一次接管/摇杆/定位/任务会重新发 1204，不会再“误以为还持有控制权”。

> 为什么必须清两处：`engaged` 只是本地缓存，1204 是否真的持权以 `ZhProClient::control_owned` 为准。
> 遥控器抢权后只清后者时，下一次推摇杆会因为 `engaged == true` 跳过 1204，1304 在没有 owner
> 的情况下被网关静默丢弃——现场表现就是「平台显示已接管、摇杆按下不动，且没有任何报错」。

摇杆被门禁拒绝时（`ctrl_source=3`、卧倒、急停等），客户端会按「原因变化或 30s」限频上报一条
`warn` 事件（`摇杆被拒绝: <原因>`），落在设备事件流里，不再只是 `debug` 日志。

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

### 报文日志（双向，含 head）

- **收到的每一帧**：DEBUG，`client: ← 智航完整帧（含 head） packet={"head":{…},"data":{…}}`，
  覆盖 `1207 / 1301 / 4013 / 4045 / 5001 / 1205 / 1523 / 1303 / 4001` 等；
  高频推送（`1401/1403/1405/1407`、`1315`、`5009`）降到 TRACE，避免把响应刷没；
- **落盘成 JSON 文件**：`ROBODOG_FRAME_DUMP=/tmp/frames.jsonl` → 一行一帧
  `{"ts":<毫秒>,"dir":"send|recv","frame":{…原始 head+data…}}`，双向都记，可直接 `jq` 过滤；
- `5000` 任务载荷：DEBUG，`zhpro::route: 5000 载荷（第 x/y 点 …）: {…}`（只有 `data`，便于看目标点）；
- 任务类请求的**完整帧**：DEBUG，`client: → 智航完整帧（含 head） msg_type=5000 msg_id=5 packet={"head":{…},"data":{…}}`，
  与协议文档 §2.5.1.4 的示例逐字段对账就用这行（`4044/4030/5000/5002/5004/5006/5014` 都会打）；
- `5001/5015/5003/5005/5007` 任务类响应：DEBUG，`← 智航响应 response=5001 data={…}`；
- `5001 code!=0` 的错误文案现在是 `5001 响应失败: code=...`，不再误写成 `5000 失败`；
- `5009` 任务快照：DEBUG，`5009 任务(current/pending/terminal): id=… lifecycle=… stage=… error=… stop=…`，
  只在 lifecycle/stage/error 变化时打印，不跟 500ms 心跳刷屏；
- 所有 `request` / `send`：TRACE，打印完整出站 JSON（含 `5000/4030/4012/1302/1204`）；
- `1304` 摇杆是 50Hz，只有 TRACE 才打印，默认不会刷日志；
- 现场默认 `RUST_LOG=robodog_device_client=debug`：

```bash
# 看 5000/5001，和本文档字段逐项对账
journalctl -u robodog-device-client -f | grep -E "5000 载荷|5001 应答"

# 临时看所有出站报文（含 1302/1304/4030），注意 1304 会很多
RUST_LOG=robodog_device_client::zhpro::client=trace   /opt/robodog/client/robodog-device-client --config /etc/robodog/client.toml
```

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

`params` 里三个不能想当然的取值（都踩过）：

| 字段 | 取值 | 坑 |
|------|------|-----|
| `speed_limit.vz` | `1.5`（文档示例值） | 它是**角速度上限**。以前写死 `0.0` 等于告诉本体「不允许转向」，需要拐弯的航点会被规划器判为「无有效绕障轨迹，搜索失败，等待中」 |
| `fence_path` | **不传**（没围栏时） | 以前发空串 `""`，固件可能拿它当路径去打开，失败就回「导航指令非法」。业务侧还没建模围栏，将来接入时再补 |
| `goal_type` | 与 `map_type` 相同 | `map_type=1` 是 2D 栅格、`3` 是路网；两者不一致时导航不会启动 |

排查这两个故障码时把 `RUST_LOG=robodog_device_client=debug` 打开，客户端会打出完整的
`5000` 载荷和 `5001` 应答（成功也打）；失败时错误消息还带上该航点的
`goal / heading / need_locate / vx / vz / map_path`，直接进平台任务事件。

## 智航 Pro 协议对接要点

```
1. control 连上 → 收 1201（client_id / session_id / bulkdata_bind_token）
2. 带 token 建 bulkdata → 首帧 1202 → 收 1203 才算可用
3. 1207 模块可用性：sdk_node 3s 内无 1525 心跳即判不可用
4. 1299 网关传输层错误、1300/1301 网关心跳（**间隔 1s**，见下）
5. 4000~5999 自由数据区间需先 1204 取得 RoamerX 控制权
```

### 请求/响应如何关联（msg_id）

协议 §2.1 的约定是「用 `msg_id` 关联同一请求的响应，客户端应先校验 `head.type` **和**
`msg_id`，再解析 `data`」，实现分三层：

| 类别 | 归类方式 | 覆盖的报文 |
| --- | --- | --- |
| 请求-响应 | `pending: msg_id → (预期响应类型, 一次性通道)`；要求 `head.type == 请求 type + 1` 才交付 | `4012→4013`、`4030→4031`、`4044→4045`、`5000→5001`、`5002→5003`、`5004→5005`、`5006→5007`、`5014→5015` |
| 控制类应答（不带 `msg_id`） | `pending_types: 响应类型 → 一次性通道`，只接受登记过的类型 | `1303`（1302 的应答）、`1311`（1310 的应答） |
| 通知 / 状态 / 推送 | 不走 pending，直接广播给订阅者，各模块**按 `head.type` 分发** | `5009`、`1315`、`1205`/`1523`、`1207`、`4001`、`4035`、`1401` |

`msg_id` 是客户端自己的计数器（`msg_seq: AtomicU32` 从 1 递增，进程内全局、跨重连继续，
`wrapping_add` 回绕），与 `time` / `src=1` 一起写进 `head`；本体只需原样回显。

两处细节都是踩过坑才加上的：

- **心跳不再借用请求号段**：`1300` 按文档示例不带 `msg_id`（发成 notify）。以前借了请求的
  `msg_id` 并从 1Hz 本地递增，和 `5000/4030` 撞号后，回显了同号的 `1301` 会被当成
  `5001` 交给调用方，`5000` 于是报「导航任务未返回 `task_id`」——现场很难复现、也难定位。
- **交付前校验 `head.type`**：`msg_id` 命中但响应类型不符时**不消费**待应答请求，只打一条
  DEBUG（`响应的 msg_id 命中但 head.type 不符，按推送处理`）并继续等真正的应答。

### 网关心跳与「control 通道已关闭」

网关对 `1300` 的超时窗口**不同固件/不同状态下不一样**：2026-11-12 白天实测约 `10s`，
而同一台本体**重启后**约 `3s` 就踢（`18:57:00` 建链、`18:57:02.88` 被断开，原因串
`heartbeat timeout`）。所以 `GATEWAY_HEARTBEAT` 取 **1s**：一条空 JSON 的开销可忽略，
但能吃得下任何一版固件的超时窗口。

客户端**会自己重连本体**。链路一断（本体重启 / 网线抖动 / 网关踢会话），后续所有
`4000~5999` 请求都只会回「control 通道已关闭」，而在平台看来设备仍然「在线」——这是
最坑的一种状态。`ZhProClient::body_link()` 把断开事件抛给主循环后，客户端会：

1. 停掉旧推送任务、把正在跑的任务标为异常、废弃建图会话；
2. 补一帧停车清接管态，并上行 `teleop_state{holder:false}`（业务 WebSocket 没断，
   服务端等不到断连回调，不清就会留下「幽灵接管」）；
3. 带退避（1s→10s）重连 `control`/`bulkdata`，成功后重新挂推送任务。

**业务 WebSocket 全程不断**，设备不会因为一次抖动就从平台上掉线重登记，恢复窗口从
「systemd 重启 + 重新握手」的 5~15s 缩到 1~3s。只有 `BODY_RECONNECT_WINDOW`（120s）内
仍连不上（例如本体根本没起来）才退出进程，交给 systemd `Restart=always` 兜底。

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
| 跳过（`--no-upload-map`） | 已完成 | 100 | 消息注明跳过原因 |
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
| `开始巡检` | 1204 取得 RoamerX 控制权 + 1302 `cmd=62` 切导航模式 |
| `远程接管` | 1204 取得控制权 + 1302 `cmd=60` 切通用模式（1304 摇杆才生效） |
| `释放接管` | 停车 + 1302 `cmd=62` 切回导航模式（本体停在通用模式下导航任务会被拒） |
| `暂停任务` | 5004 暂停当前本体任务（**必须用真实 task_id**） |
| `继续任务` | 5006 恢复当前本体任务（**必须用真实 task_id**） |
| `召回充电` | 5014 返回回充点 |
| `同步地图` | 4012 拉地图列表并回传中台 |
| `重新定位` | 4030 载图 + 定位，等 `loc_state` 收敛到 100 |
| `本体重连` | 原地重建 control/bulkdata（**不退出进程**，平台连接保持），接管、任务、建图进行中会拒绝；120s 内仍连不上才退出交给 systemd |

本体动作（`payload.params.action`）除站立/卧倒/急停/灯光外，新增 1302 的三种模式：
`generic_mode`(60 通用，1304 的 lx/ly 才生效) / `in_place_mode`(61) / `navigation_mode`(62)。

> **不要用 `task_id: "current"`**：现场固件不认这个别名，会回 `13896 task not found`，
> 以前「暂停 / 继续」按钮因此实际上什么都没做。一律用 `RouteRunner::current_nav_task()`
> 拿到的真实 id。

### 完全接管（摇杆走不动怎么办）

`handle_teleop` 的接管流程是三步，缺一步就会出现「能站立但摇杆不走」：

1. `1204` 取控制权（等 `1205.status_code = 0`）；
2. `1302 cmd=60` 切通用模式——1304 的 `lx/ly` 在原地模式下无作用；
3. 摇杆按 50Hz 重发（`spawn_republisher`），松手立即发零帧。

另外两处现场因素靠日志排查：

- `1315 ctrl_source = 3`：硬件遥控器仍持权（协议没有禁用它的消息，只能关遥控器电源）；
- `1315 motion_status = lie_down`：卧倒状态下不会迈步，先站立。

接管态失效与恢复（防「接管了却走不动」）：

| 触发 | 客户端动作 |
| --- | --- |
| `1205.status_code != 0` | 清 `control_owned`，报「获取 RoamerX 控制权超时」 |
| `ctrl_source` 变成 0/3（遥控器抢权） | 清 `control_owned` + 清 `TeleopState`（含摇杆缓存），补一帧零摇杆 |
| 摇杆首帧、或重新点「远程接管」 | 只要 `control_owned` 或 `engaged` 任一为 false，就**重发** `1204` + `1302 cmd=60` |
| 摇杆被门禁拒绝 | 补停车帧 + 限频上报 `warn` 事件 |

### 接管后「对方」能看到什么

| 对方 | 能否收到「控制权被拿走」的通知 |
| --- | --- |
| 另一个平台 / 官方 APP（gateway `control` 通道的客户端） | **能**。`1205` 在 owner 切换成功时**广播**给所有 `control` 通道客户端，`client_id` 是新的 owner；`1523 kRoamerXControlNotify` 也会推送当前 owner。客户端只认 `client_id == 本端` 的成功通知，否则丢弃本地 `control_owned`。 |
| 物理遥控器（2.4G 硬件通道，`ctrl_source = 3`） | **不能**。协议里没有任何发往遥控器的下行消息，也没有禁用它的命令；而且方向是遥控器单向优先——官方 APP 的做法也只是暂停摇杆并提示「当前狗的控制权被其他遥控器抢断」。 |

所以「遥控器先控、平台再点接管」这个顺序**拿不到遥控器的手柄**：

- 服务端 `ensure_ready` 在 `ctrl_source == 3` 时直接 400「请断开外部遥控后重试」，连接管都点不成；
- 即使 `ctrl_source` 尚未刷新到 3，`1204` 在网关侧可能“成功”（owner 变成我们），但本体仍按遥控器给的指令走，
  平台摇杆会被客户端门禁拦下（现已上报 `warn` 事件，不再静默）；
- 唯一可靠的交接方式是**物理层**：关遥控器电源 / 切回软件控制，等 `1315 ctrl_source` 不再为 3 后再接管。

### 强制接管（遥控器已断电，本体还在报 `ctrl_source = 3`）

遥控器关电源后，本体未必马上把 `ctrl_source` 刷回 1/2（有的固件会一直停在 3）。
这时运维可以在控制台点「强制接管」，链路如下：

| 环节 | 行为 |
| --- | --- |
| 控制台 | 按钮变成「强制接管」，二次确认「遥控器已关闭」后 `POST /teleop/take {"force": true}` |
| 服务端 | 落库 `devices.teleop_force = 1`，只跳过 `ctrl_source` 这一条门禁；下发 `command: 远程接管` 时带 `payload.force = true` |
| 客户端 | 收到 `force` 后置 `RobotState.teleop_force`，`motion_block_reason_with` 只放行 `ctrl_source`，其余门禁（离线 / `sdk_node` / 急停 / 卧倒）照旧 |
| 心跳 | `heartbeat_ack.teleopForce` 每轮同步，客户端重启后不会再被自己拦下 |
| 释放 / 超时 / 掉线 | 标记清零，恢复遥控器抢占检测 |

默认（不带 `force`）行为不变：`ctrl_source = 3` 一律拒绝，避免遥控器还在使用时两边抢控制。

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

0. 先 `1204` 取 RoamerX 控制权——`4030` / `4044` / `5000` 全在 4000~5999 非白名单区间，
   没控制权时网关**直接丢弃**，表现为「等 4031 超时」而本体那边什么都没做；
   （白名单只有 `4012` / `4022` / `5010`，见协议 §4000~5999）
1. 读最近一帧 5009 的 `robot.loc_state.status`，已是 `100` 直接起步（不重载地图，保住位姿）；
2. 否则下发 `4030 op=1` 载图并等收敛（默认 60s）。`4031` 只等 3s，**等不到不判失败**，
   判据是 5009 `loc_state` 收敛到 100（现场固件的回执经常更慢）；
3. 仍未收敛或 SLAM 明确失败（`203` / `210`）→ **不发 5000**，直接把任务置为「异常」，
   进度消息写明「定位未就绪：... 请远程接管把设备移回地图覆盖范围，或下发「重新定位」后重试」；
4. 预检过程**真的裁过这张图**（走了 4030）时，首航点用 `need_locate=false` 复用这份定位，
   避免刚重定位完又载一次图；若 5009 上来就是 100（预检没动过地图），首航点仍发
   `need_locate=true`（协议默认）让本体重新载图定位——「已经定位」不等于「载的是这条路线
   绑的那张图」，可能还是上一单留下的另一张图；
5. 真正发 5000 前再补一次 `1302 cmd=62` 导航模式，并 `5002` 取消上一轮残留的本体任务。

这样「机器狗不在地图上」就不会再以一条含义不明的 5000 失败收场。

### 本体能力忙（13897 / 13896）

| 码 | 文案 | 含义与处置 |
|----|------|-----------|
| `13897` | `resource conflict (capability busy)` | 本体上还有没结束的任务（典型是被 `5004` 暂停后一直挂着的导航任务），新的 `5000` 被拒。客户端会按 5s × 6 次退避重试，并且**不消耗航点自己的重试次数**；仍失败时在事件里提示「先在本体 APP 取消该任务或重启本体算法服务」 |
| `13896` | `task not found` | 本体上已经没有这个任务（已终态 / 固件不认这个 id），取消时视为已收尾 |

平台侧对应约束：远程接管期间**不下发任务**（本体在通用/手动模式下，导航能力被判给手动控制），
服务端 `dispatch_task` 会直接返回 400 而不是让本体拒五次。

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
