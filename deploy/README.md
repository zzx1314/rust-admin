# 部署手册

三套环境、两个部署目标，全部走仓库里的脚本，不手工拷文件：

| 目标 | 机器 | 装什么 | 脚本 |
| --- | --- | --- | --- |
| 正式环境 | `root@39.107.55.13` · `/opt/hz/rust-admin` | 中台（`x-rust` + 前端）+ 业务平台（`robodog-ops-server` + 控制台） | `deploy/external-hz/push.sh` |
| 现场设备 | `root@192.168.41.246` · `/opt/robodog/client` | 设备客户端 `robodog-device-client`（aarch64，systemd） | `business/client/deploy/build-aarch64.sh` + `scp` |
| 开发机 | 本机（`192.168.41.227`） | 中台 3000 / 业务平台 3100 / 前端 5198 · 5273 | `backend/run.sh`、`cargo run`、`pnpm dev` |

地址细节见 [环境与地址](#环境与地址)，正式环境的历史踩坑见
[`external-hz/README.md`](external-hz/README.md)。

---

## 环境与地址

| 环境 | 中台 | 业务平台 / 设备网关 | 备注 |
| --- | --- | --- | --- |
| **正式** | <http://39.107.55.13:3005> | <http://39.107.55.13:3100> · `ws://39.107.55.13:3100/ws/device` | 前端由 Rust 进程托管，**无 nginx、无 Redis**；3000 被本机 frps 占用所以中台用 3005 |
| **开发** | <http://127.0.0.1:3000>（前端 vite 5198 / 8848） | <http://127.0.0.1:3100>（前端 vite 5273） | `business/server/server.toml` 的 `[zentai]` 指向 `127.0.0.1:3000` |
| **现场内网** | 视部署而定，如 `ws://192.168.41.10:3000` | 设备客户端连 `ws://39.107.55.13:3100`（正式） | 机器狗本体 `192.168.234.234:10010`、文件服务 `:12000`、RTSP `192.168.234.1:8554` |

现场设备机 `192.168.41.246` 跑的是设备客户端，它再连机器狗本体：

```text
192.168.41.246 (aarch64 Ubuntu 22.04, systemd: robodog-device-client)
   ├─ ws://39.107.55.13:3100/ws/device   业务平台（正式环境外网）
   └─ ws://192.168.234.234:10010        机器狗本体（control + bulkdata）
```

设备编号不用配：客户端启动时读本体 SN 分区 `/dev/mmcblk0p5`（现场实测 `0202222672400230`）。

> 本体侧（背包 `.246` 之外的 Orin NX `.234` + RK3588 `.1`）的服务树、日志路径和排障顺序见
> [`robodog-body.md`](robodog-body.md)——`sdk_node=false` / 任务被拒 / 本体状态抖动先查那一份。
> 2026-11-14 现场那批问题（急停卡死、`dog_task` 崩溃、任务首点秒失败、**用官方 APK 可正常跑任务**）
> 的完整证据链、结论和给厂商的问题单见
> [`robodog-issues-2026-11-14.md`](robodog-issues-2026-11-14.md)。

---

## 一、正式环境发布

### 一键发布

```bash
cd <repo>
./deploy/external-hz/push.sh              # 全量：前端 + 两个二进制
./deploy/external-hz/push.sh --skip-web   # 只改了 Rust，跳过 pnpm build（快）
```

做的事：`build.sh` 在本机编 `x86_64-unknown-linux-musl` 静态二进制（目标机 CentOS 7 /
glibc 2.17，本机编出来直接跑会缺符号）+ 构建两份前端 → 打 tar 包 → `scp` 到
`/tmp/` → 远程跑 `install.sh`（幂等升级）。

### 前置条件（本机一次装好）

```bash
sudo apt install musl-tools                              # musl-gcc
rustup target add x86_64-unknown-linux-musl
pnpm install --dir frontend && pnpm install --dir business/web
ssh root@39.107.55.13 'hostname'                         # 免密登录
```

### install.sh 会做什么

1. 解包到 `/opt/hz/rust-admin`，旧文件保留、配置文件备份成 `*.bak.<时间戳>`；
2. **保留** `zentai.env` / `robodog.env`（不存在才生成，两个服务共用同一个
   `SERVICE_TOKEN`）；
3. 重装两个 systemd unit 并 `restart`（会话存内存，重启后用户需重新登录）；
4. 自检：打印 `:3005`、`:3100` 的 HTTP 状态和 `/api/health`。

### 发布后校验

```bash
# 服务与日志
ssh root@39.107.55.13 'systemctl is-active rust-admin-zentai rust-admin-robodog'
ssh root@39.107.55.13 'journalctl -u rust-admin-robodog -n 30 --no-pager'

# 外网可达 + 版本内容
curl -s -o /dev/null -w '%{http_code}\n' http://39.107.55.13:3005/
curl -s http://39.107.55.13:3100/api/health | python3 -m json.tool
```

`/api/health` 里要看到（正式环境）：

```json
{"status":"ok","database":"/opt/hz/rust-admin/robodog/data/robodog.db",
 "mapIngest":{"authMode":"service-token","ready":true,"zentaiUrl":"http://127.0.0.1:3005"}}
```

业务平台日志里应出现反向控制信道已连上中台：

```text
控制信道已连上中台: ws://127.0.0.1:3005/api/dogDevice/control-plane/ws
```

### 回滚

`install.sh` 只覆盖二进制和前端，数据库与密钥都保留。回滚 = 用上一版产物重跑
`install.sh`，或 `systemctl restart` + 把 `*.bak.<时间戳>` 换回来：

```bash
ssh root@39.107.55.13 'ls -lt /opt/hz/rust-admin/zentai /opt/hz/rust-admin/robodog | head'
```

---

## 二、现场设备客户端发布（192.168.41.246）

### 1. 交叉编译（本机）

目标机没有 Rust 工具链、也没有外网，必须本机编好再传。本机 glibc 比目标机
（2.35）新，所以要按 GLIBC 版本链接，否则报 `GLIBC_2.39 not found`：

```bash
cd business/client
rustup target add aarch64-unknown-linux-gnu     # 一次性
# 推荐：按 2.35 链接（脚本检测到 cargo-zigbuild + zig 会自动使用）
./deploy/build-aarch64.sh
```

脚本结尾会校验符号上限并打印产物路径 `target/aarch64-unknown-linux-gnu/release/robodog-device-client`。
看到 `✗ 产物要求 GLIBC_2.39 > 2.35` 就是没走 zigbuild，装 `cargo-zigbuild` + `zig` 后重跑。

### 2. 上传 + 重启

```bash
BIN=business/client/target/aarch64-unknown-linux-gnu/release/robodog-device-client
STAMP=$(date +%Y%m%d-%H%M%S)

# 先备份目标机上的在跑版本
ssh root@192.168.41.246 "cp -a /opt/robodog/client/robodog-device-client \
  /opt/robodog/client/robodog-device-client.bak-$STAMP"

# 注意：直接 scp 覆盖**正在运行**的二进制会 ETXTBSY（dest open: Failure），
# 必须先传成 .new 再 mv（mv 换目录项，运行中的进程仍持有旧 inode）
scp "$BIN" root@192.168.41.246:/opt/robodog/client/robodog-device-client.new
ssh root@192.168.41.246 'chmod 755 /opt/robodog/client/robodog-device-client.new &&
  mv -f /opt/robodog/client/robodog-device-client.new /opt/robodog/client/robodog-device-client &&
  systemctl restart robodog-device-client'
```

> 也可以把整个 `business/client` 目录 rsync 到目标机再跑 `sudo ./deploy/install.sh
> --skip-build`，但目标机没有 Rust，`install.sh` 的默认编译步骤要跳过；日常发布用上面的
> scp 三步即可。

### 3. 校验

```bash
# 服务状态与启动日志
ssh root@192.168.41.246 'systemctl is-active robodog-device-client'
ssh root@192.168.41.246 'journalctl -u robodog-device-client -n 40 --no-pager'

# 客户端自带自检（端口 / health / 本体 10010+12000 / 一次注册探针）
ssh root@192.168.41.246 'cd /opt/robodog/client && ./deploy/check.sh'
```

日志里要看到这几行：

```text
启动设备客户端: device_id=0202222672400230, 业务服务=ws://39.107.55.13:3100, 本体=192.168.234.234
本体连接就绪: client_id=…, 型号=…
握手成功 / bulkdata 绑定成功
设备注册成功: 0202222672400230（园区 …）
```

再到正式环境控制台 <http://39.107.55.13:3100> → 设备管理，确认该设备「在线待命」、
心跳在刷新。

### 4. 现场配置在哪里

| 文件 | 内容 | 说明 |
| --- | --- | --- |
| `/etc/robodog/client.toml` | `[server] url`、`[robot] host`、`[mapping]` 等 | 首次安装由 `deploy/client.toml.template` 生成，之后**保留不覆盖**（旧的 `[video]` 段已废弃，留着也不会报错） |
| `/etc/robodog/client.env` | `ROBODOG_DEVICE_SECRET`、`ROBODOG_API_PASSWORD` | `chmod 600`，systemd 用 `EnvironmentFile` 加载 |
| `/opt/robodog/client/robodog-device-client` | 二进制 | 发布时先备份成 `.bak-<时间戳>` |

核对生效配置（敏感字段已脱敏）：

```bash
ssh root@192.168.41.246 '/opt/robodog/client/robodog-device-client \
  --config /etc/robodog/client.toml --print-config'
```

### 5. 回滚

```bash
ssh root@192.168.41.246 'cd /opt/robodog/client &&
  cp -a robodog-device-client.bak-<时间戳> robodog-device-client &&
  systemctl restart robodog-device-client'
```

---

## 二·补充、数据采集背包：实时视频 + AI（2026-11-14 上线）

背包（`.246`，`/root/261113/backpack_dashboard`）自带 4 路 USB + 机器狗前置 RTSP + 红外，
板端 RKNN 单路识别。控制台新增「监控运维 → 实时视频」，但要分**三个组件**看：

| 组件 | 跑在哪 | 作用 |
| --- | --- | --- |
| `backpack-agent.service` | 板子 `.246` | 板端 Agent（`:8000`）：六路推流、AI 识别、板端画框、遥测/控制 |
| `backpack-relay.service` | 板子 `.246` | **板端推流中继**：按需把本地 RTSP 转 FLV 推到平台 `:3100`，公网入口也能看（见下方「推流中继」） |
| `site-console.service` | 板子 `.246`（可换任意内网机） | 内网托管同一份控制台 `dist`（`:8080`），见下方「为什么需要现场入口」 |
| `rust-admin-robodog` | 云端 `39.107.55.13:3100` | 业务平台；背包接入方式由 `ROBODOG_BACKPACK_BROWSER_URL` 决定（直连模式） |
| `robodog-device-client` | 板子 `.246` | 设备客户端。2026-11-16 起**兼任航点 AI 的本地代理**：到点自己调 `127.0.0.1:8000`，因为云端平台连不到内网 Agent |

### 业务连接断开：客户端原地重连（2026-11-17）

平台重启 / 发布 / 网络抖动导致设备 WS 断开时，客户端**不再退出进程**（以前会被
systemd 5s 后拉起，日志是 `Deactivated successfully` → `Scheduled restart`，
设备白掉线 10s+）：`run_client` 现在返回错误，`main` 的重连循环按
1s → 2s → … → 30s 退避原地重连；连接稳定跑过 30s 后再断，退避从 1s 重新算
（`next_retry`）。日志关键字：`业务连接断开 (…)，1s 后原地重连`。

限制：`active` / `local_runner` 是 `run_client` 内的局部状态，**重连后任务上下文会重建**，
服务端会把断连期间的任务判异常；要做到「重连不打断在跑的任务」，得把这些状态提到
`run_client` 外面（未做，按需再谈）。

### 航点 AI 识别：由客户端代调（2026-11-16）

现场表现：路线里首点配了「AI 识别」，任务跑完平台一条 AI 告警都没有。原因是业务平台在
公网、Agent 只在内网（`39.107.55.13 → 192.168.41.246:8000` 实测超时），服务端直连逻辑
只能跳过（日志 `AI 识别：本部署的服务端访问不到背包 Agent，跳过航点…`）。

现在由同机的设备客户端代调，**服务端不用开任何到板端的入站通道**：

```text
客户端到点(5009) ──ai_point(带板端模型目录)──► 平台：算场景/类别/阈值/窗口
     ▲                                            │
     └──── ai_start ───────────────────────────────┘
     │  POST 127.0.0.1:8000/api/detect/start|config，读 /api/events
     └──── ai_detect / ai_end ────────────────────► 平台：alerts + task_events + 企微
任务终态/取消 ◄── ai_stop ──────────────────────── 平台
```

- 服务端配置：`[backpack] point_ai` / `ROBODOG_POINT_AI`（默认 `auto`）——`auto` 时
  **没配** `ROBODOG_BACKPACK_URL` 就走客户端代调，配了就走老的直连模式，两者不会重复开窗；
- 客户端配置：`/etc/robodog/client.toml` 的 `[ai] agent_url = "http://127.0.0.1:8000"`
  （默认值就是它，一般不用写），`--no-ai` 可关闭；
- **必须升级客户端二进制**才生效（旧客户端不会上报 `ai_point`，平台会一直按直连模式跳过）。
  发布方式同上「二、现场设备客户端发布」。

验收：

```bash
# 客户端日志（到点时应出现这三行）
ssh root@192.168.41.246 'journalctl -u robodog-device-client -f | grep -E "航点 AI 识别"'
#  航点 AI 识别：请求开窗 任务 TK-… 航点 0「…」场景=Some("firextinguisher") …
#  航点 AI 识别开始：场景=firextinguisher 类别=extinguisher 阈值=0.35 相机=dogcam 窗口=30s
#  航点 AI 识别结束：场景=firextinguisher 窗口到期（30s），上报 1 类目标

# 平台侧：窗口态（source 应为 client）
curl -s -H "Authorization: Bearer $TOKEN" http://39.107.55.13:3100/api/backpack/ai-window
# 平台侧：告警与任务时间线
sqlite3 /opt/hz/rust-admin/robodog/data/robodog.db \
  "SELECT alert_code,content,occurred_at FROM alerts WHERE content LIKE 'AI 识别%' ORDER BY id DESC LIMIT 5;"
```

### 推流中继（2026-11-16 上线，公网入口可看视频）

「现场入口」这条路有两个绕不过去的前提：**操作员电脑与背包同网段**、**浏览器放行本地
网络访问（LNA）**。实测：换一个 WiFi、出差远程、换台电脑或清一次浏览器数据，画面就黑
——这不是配置问题，是浏览器策略（Chrome 141+ 灰度强制，157 上确认拦截）。

所以把方向反过来：**由板子出公网推流**，浏览器只跟平台同源通信。

```text
本地 mediamtx :8554 ──ffmpeg 720p15/1.0Mbps / 或 -c:v copy──► backpack-relay ──ws──► 平台 VideoHub
   房间名 bp:<stream>                                                      │
                                  浏览器 ◄── ws /api/backpack/video/ws ──┘（flv.js）
```

- 延时 1~2s（WHEP 是 ~200ms），代价换「任何网络、任何浏览器都能看」；
- **按需**：常驻 6 条 WS 连接（每条约 1KB），第一个观看者进来才起 ffmpeg，最后一个
  离开后平台留 60s 宽限再由看门狗 `video_stop`（切页签 / 刷新不用重新起 ffmpeg）；
- **默认转成 720p15 / 1.0Mbps**（`RELAY_PROFILE=light`，编码器自动挑
  `h264_rkmpp` → `h264_v4l2m2m` → `libx264`）：平台是阿里云 ECS，公网出口实测只有
  **≈2.5Mbps**，而 `dogcam` 是 1080p60、`-c:v copy` 会原样推 4~8Mbps —— 观看端追不上
  就被服务端判 `lagged` 踢掉重连，现场看到的就是「很卡」。带宽更紧再压
  `RELAY_BITRATE=800k RELAY_FPS=15`；真要在 6Mbps 以上的出口上看 1080p60 才用
  `RELAY_PROFILE=copy`；
- **Wi-Fi 掉线不再冻住画面**（2026-11-18）：relay 的分片发送有超时（`RELAY_SEND_TIMEOUT`，默认 10s），
  超时即判上行已死、断开重连；`RELAY_STALL_TIMEOUT`（默认 15s）内一个字节都没推出去就自己重启
  `ffmpeg`；每 5s 巡检平台出口网卡（`ip route get`），Wi-Fi 断开 / 恢复导致出口变更就重建连接；
  拥塞后自动降到 `640x360@12 / 500k`（`RELAY_DEGRADE_*`），只剩 4G 时也不至于每 10s 重连一次；
  降档**不等超时**：5s 窗口内 `ws.send` 占用高 + 实测码率明显低于目标档位（或分片发送超时）就
  立刻换档，`video_reset` + 重启 ffmpeg，观看端不重连；到期还会先确认链路真恢复才回配置档，
  反复升降时窗口翻倍（不来回闪）。细节见 `business/docs/backpack-video-integration.md` §6.10⑥；
  路由脚本在 /32 路由变化后会 `systemctl try-restart backpack-relay`。细节见
  `business/docs/backpack-video-integration.md` §6.10⑤；
- 前端 `/monitor` 页顶部有「现场直连 / 平台中继」切换，**直连失败或页面在公网时自动切中继**；
- 中继流里浏览器画不出框（FLV 没有与抽帧对齐的坐标），要看框就切「背包烧录」——
  板端把中文标签烧进 `preview` 流，中继也推这一路（`RELAY_STREAMS` 默认已含 `preview`，
  改过默认值的部署记得补上并 `systemctl restart backpack-relay`）；只看检出数量则不用切。
- **AI 的启停 / 模型目录 / 运行态不再依赖浏览器同网段**（2026-11-17 起走设备客户端隧道，
  见下一节）。

部署（幂等）：

```bash
ssh root@192.168.41.246 'mkdir -p /opt/backpack-relay'
scp deploy/backpack-relay/relay_push.py root@192.168.41.246:/opt/backpack-relay/
scp deploy/backpack-relay/backpack-relay.service root@192.168.41.246:/etc/systemd/system/
# 板子出公网要走 Wi-Fi / 有线，别走 4G dongle（默认路由 metric 100 时上行只有 ~0.5Mbps）
scp deploy/backpack-relay/50-platform-route.sh root@192.168.41.246:/etc/NetworkManager/dispatcher.d/50-backpack-relay-route
ssh root@192.168.41.246 'chmod 755 /etc/NetworkManager/dispatcher.d/50-backpack-relay-route && systemctl daemon-reload && systemctl enable --now backpack-relay'

# 验收：六条连接都在（journal 里应有 6 行「已连接平台，房间 bp:xxx」）
ssh root@192.168.41.246 'systemctl status backpack-relay --no-pager -n 10'
# 云端侧：来了观看者才会下发 video_start
ssh root@39.107.55.13 'journalctl -u rust-admin-robodog -f | grep -E "backpack stream|video stream reset|last viewer of bp"'
```

> **板子会硬复位（2026-11-18 当天 11:53 / 13:34 / 14:49 三次）**：`journalctl -b -1` 最后一条
> 停在应用日志、没有任何关机流程 = 断电式重启。`/proc/meminfo` 里 `CmaTotal: 16384 kB`、
> `CmaFree: 0 kB`，内核反复报 `cma: cma_alloc ... alloc failed`（开机时 + 每次有人看视频、
> 中继起转码时），而 cmdline 里**没有 `cma=`** —— 板上同时跑 5 个 rkmpp 编码器（4 路 USB 相机
> 常驻 `h264_rkmpp` + 中继按需转码），16MB CMA 根本不够。复位后 7 路推流静默 34s，平台
> 按 `PUBLISHER_SILENCE` 判死丢房间，现场看到的就是「画面突然冻住」。
>
> 处置优先级：① 现场确认 4 路 USB 相机是否需要常驻编码（改成按需 / 降到 640x360@15 400k，
> 脚本在 `/root/261113/backpack_dashboard`）；② 把 boot 的 CMA 提到 64~128MB（Rockchip
> Android 风格 boot，需现场/串口改 boot 配置，别远程动）；③ 别同时看多路（每路多一个转码）。
> 中继侧已加的缓解：`-bf 0 -refs 1`（少占编码缓冲）+ 关键帧 1s + 按需起流。

排障对照：

| 现象 | 看哪里 | 说明 |
| --- | --- | --- |
| 板端整机重启过（画面冻住几分钟） | `journalctl -b -1 -n 5`、`grep cma /proc/meminfo`、`dmesg | grep cma_alloc` | 见上方「板子会硬复位」 |
| `板端推流服务未连接` | 板端 `backpack-relay` 是否 active | 房间名 `bp:` 由服务端 `BACKPACK_ROOM_PREFIX` 定义，两边必须一致 |
| 有画面但卡住/黑屏 | 板端 `ffmpeg:` 警告 | `Timestamps are unset` → 必须带 `-use_wallclock_as_timestamps 1`（脚本已带） |
| 只有第一次能看 | 平台日志 `video stream reset` | 常驻连接复用了旧流头，服务端靠 `video_reset` 清 `init` |
| 帧率偏低/断流 | 现场上行带宽 + 平台公网出口 | 单路默认 1.2Mbps；平台出口只有 ≈2.5Mbps，**同时看两路就会卡** |
| 画面很卡、反复重连 | 平台日志 `viewer of bp:... lagged` / `left after ... kbps` | 平均 kbps 超过出口带宽就是它；降 `RELAY_BITRATE`/`RELAY_FPS`，或升级 ECS 公网带宽 |
| 想先量中继码率 | `measure_bitrate.py --stream dogcam`（在平台机器上跑，loopback） | 预期 light ≈1200kbps/20fps；`copy` 是 4000~8000kbps/60fps，对 2.5Mbps 出口必卡 |
| 切了通道自己跳回「机器狗前置」 | 页面轮询 | 已修（2026-11-16）：直连模式后端不下发 `sources`，刷新时别拿它比对 |
| 每 23s 左右断一次 | 服务端 `stream reset` 是否出现 | 已修（2026-11-18）：`init` 残留不再阻塞 `video_start`，回归用例 `video_relay::stale_stream_header_does_not_block_video_start` |
| 码率只有 ~400kbps / 7fps | 板端 `ip route get 39.107.55.13`、`ethtool` | 走了 4G dongle（上行 ~0.5Mbps）；装 `50-platform-route.sh` 让平台流量走 Wi-Fi（上行 ~25Mbps），改完重启 relay |
| 画面定格、重开页面也不动 | 板端 `journalctl -u backpack-relay` 搜 `上行阻塞` / `没有推出任何字节` | Wi-Fi 半开连接把管道反压死了（旧版 relay）；升级 relay 后会自动断开重连 / 重启 ffmpeg |
| Wi-Fi 断了再连上还是很卡 | 板端日志 `平台出口网卡 A → B` | 出口还挂在慢链路（4G / 狗体内网 eth0）；relay 每 5s 巡检、路由脚本也会重启它 |
| 想看房间/观看者状态 | `GET /api/backpack/video/state` | 返回每路的 `publisher/initBytes/viewers/streaming`，黑屏先看这张表 |

### 跨网段的接口 / AI：设备客户端隧道（2026-11-17 上线）

之前「操作员电脑与背包不同网段」等于「背包页只能看不能开 AI」：面板要读的模型目录、
运行态、AI 启停全靠浏览器直连 `192.168.41.246:8000`，而公网页面会被浏览器拦掉
（`corsError=InsecureLocalNetwork`）。

现在改用已有那条设备 WS：板端客户端本来就在背包那块板子上（`127.0.0.1:8000` 一定通），
注册成功后发一条 `agent_hello`，平台就把 `/api/backpack/*` 的请求（只放行 4 个只读接口 +
3 个 AI 动作）和检出事件挂到这条连接上：

```text
云端业务服务 ──agent_request(WS)──► 板端设备客户端 ──HTTP──► 127.0.0.1:8000
             ◄──agent_result──────             ◄──SSE───  /api/events（只转 detect 四类）
```

- 页面无感：`GET /api/backpack/status` 的 `mode` 变成 `tunnel`，`tunnel.device` 是承载隧道的设备号；
- 直连模式下（云端入口仍是 `ROBODOG_BACKPACK_BROWSER_URL` 形态）浏览器先自己探测，
  探不通且 `tunnel.available=true` 就自动带 `?transport=tunnel` 重拉，不需要改任何配置；
- 多背包园区必须配 `ROBODOG_BACKPACK_TUNNEL_DEVICE=<设备号>`，否则可能问到另一台狗。

上线要做的两件事：

```bash
# 1) 板端客户端升级到含 agent_relay 的版本（[ai] enabled 必须为 true，默认是）
#    编译与上传见本文「现场设备客户端」§1~2（aarch64-gnu + zigbuild，装到 /opt/robodog/client/）
cd business/client && ./deploy/build-aarch64.sh
#    按 §2 上传后：ssh root@192.168.41.246 'systemctl restart robodog-device-client'

# 2) 板端日志确认已登记（没有这行说明本机 Agent 不通，隧道不会生效）
ssh root@192.168.41.246 'journalctl -u robodog-device-client -n 80 | grep -E "板端 Agent 隧道"'
# 期望：板端 Agent 隧道已登记：http://127.0.0.1:8000（背包页的模型目录 / 运行态 / AI 启停…）

# 3) 平台侧验收：mode 应为 tunnel，agent.reachable=true
curl -s -H "Authorization: Bearer $TOKEN" http://39.107.55.13:3100/api/backpack/status \
  | python3 -c 'import sys,json;d=json.load(sys.stdin)["data"];print(d["mode"], d["agent"]["reachable"], d.get("tunnel"))'
```

排障：

| 现象 | 看哪里 | 说明 |
| --- | --- | --- |
| `tunnel.available=false` | 板端客户端日志有没有「板端 Agent 隧道已登记」 | 没有 → 本机 Agent 不通或 `[ai] enabled=false` |
| `available=true` 但 `reachable=false` | 板端 `curl 127.0.0.1:8000/api/cameras` | 客户端在线、Agent 挂了；重启 `backpack-agent` |
| 能开 AI 但页面没有框 | 是否切了「背包烧录」 | 中继流上浏览器画不了框；框靠板端烧进 `preview` |
| 页面问到了别的狗 | `tunnel.device` | 多背包必须配 `ROBODOG_BACKPACK_TUNNEL_DEVICE` |

### 为什么还需要「现场入口」

Chrome 的 **Local Network Access** 禁止**公网页面**访问**内网地址**（实测
`corsError=InsecureLocalNetwork`）：

```text
页面 http://39.107.55.13:3100  ──fetch──▶  http://192.168.41.246:8000   ✗ 被浏览器拦掉
页面 http://192.168.41.246:8080 ──fetch──▶  http://192.168.41.246:8000   ✓ 同为内网
```

视频更是浏览器直连板端 mediamtx（`:8889` WHEP，媒体走 UDP）。

「接口 / AI」现在有设备客户端隧道兜底（见上一节），「视频」有平台中继兜底，
所以现场入口不再是**唯一**能用的入口，但它仍是**延时最低、框最实时**的那条路：

- **常规后台**：<http://39.107.55.13:3100> —— 全部功能；视频走中继（1~2s），要看框得板端烧录；
- **现场入口**：<http://192.168.41.246:8080> —— 同一套后台，WHEP 直连（~200ms）、浏览器叠加可用。

两个入口共用同一份 `business/web/dist`；现场入口通过 `/runtime-config.js`
（运行时注入，不用重新构建）把 API 指回云端。

> ⚠️ **每次发布前端都要同步现场入口**：`push.sh` 只更新云端 `:3100` 的 `web/`，
> 现场入口 `/opt/site-console/web` 是**另一份拷贝**。忘了同步就会出现「云端已修、
> 现场入口还是旧界面」（2026-11-18：设备管理仍显示『未配置视频地址，请在设备档案中填写』）。
> `serve.py` 在**启动时**读一次 `index.html`，所以替换后必须 `systemctl restart site-console`。

### 部署步骤（幂等，可重复执行）

```bash
# 1) 云端：前端 + 两个二进制（含背包代理与直连模式）
./deploy/external-hz/push.sh

# 2) 云端：告诉业务服务「浏览器该往哪调背包 Agent」（写一次即可，install 会保留）
ssh root@39.107.55.13 'printf "%s\n"   ROBODOG_BACKPACK_BROWSER_URL=http://192.168.41.246:8000 >> /opt/hz/rust-admin/robodog.env &&
  systemctl restart rust-admin-robodog'

# 3) 板端：Agent 做成服务（开机自启 + 崩溃重启），并修掉 CORS 白名单
scp /root/261113/backpack_dashboard/backend/agent.py root@192.168.41.246:/root/261113/backpack_dashboard/backend/
scp deploy/backpack-agent.service root@192.168.41.246:/etc/systemd/system/
ssh root@192.168.41.246 'systemctl daemon-reload && systemctl enable --now backpack-agent'

# 4) 板端：现场入口静态服务（每次前端发布后重跑这一段）
ssh root@192.168.41.246 'test -f /opt/site-console/web/index.html &&
  mv /opt/site-console/web /opt/site-console/web.bak-$(date +%Y%m%d-%H%M%S) || true'
scp deploy/site-console/serve.py root@192.168.41.246:/opt/site-console/serve.py
scp -r business/web/dist/. root@192.168.41.246:/opt/site-console/web/
scp deploy/site-console/site-console.service root@192.168.41.246:/etc/systemd/system/
ssh root@192.168.41.246 'chown -R developer:developer /opt/site-console/web;
  systemctl daemon-reload && systemctl enable --now site-console && systemctl restart site-console'
```

### 校验

```bash
# 板端：六路都在推（都要有分辨率输出）
ssh root@192.168.41.246 'for p in cam0 cam1 cam2 cam3 dogcam dog-ipcam; do   printf "%s: " $p; ffprobe -v error -rtsp_transport tcp -select_streams v:0   -show_entries stream=width,height -of csv=p=0 rtsp://127.0.0.1:8554/$p; done'

# 板端：CORS 放行现场入口来源
ssh root@192.168.41.246 'curl -s -o /dev/null -D - http://127.0.0.1:8000/api/cameras   -H "Origin: http://192.168.41.246:8080" | grep -i access-control-allow-origin'

# 云端：直连模式下发给页面的两个地址
TOKEN=$(curl -s -X POST http://39.107.55.13:3100/api/auth/login -H 'Content-Type: application/json'   -d '{"username":"admin","password":"admin123"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["data"]["token"])')
curl -s -H "Authorization: Bearer $TOKEN" http://39.107.55.13:3100/api/backpack/status   | python3 -c 'import sys,json;d=json.load(sys.stdin)["data"];print(d["mode"], d.get("direct"))'
# 期望：direct {'apiBase': 'http://192.168.41.246:8000', 'webrtcBase': 'http://192.168.41.246:8889'}'

# 现场入口
curl -s http://192.168.41.246:8080/healthz   # ok
curl -s http://192.168.41.246:8080/runtime-config.js
```

浏览器验收（必须在 **192.168.41.x/22** 网段的机器上）：

1. 打开 <http://192.168.41.246:8080/monitor>，默认「机器狗前置」出图；
2. 依次点六路源都能出图（红外是 256×192、无 AI）；
3. 点「AI 识别」开关 → 状态变「识别中」；
4. 画框方式切「背包烧录」→ 底部说明变「preview 流自带中文标签」，画面不中断；
5. 停 AI → 画面回到干净流、框清空。

### 运维注意

- **Agent 没有鉴权**：`AGENT_CORS_ORIGINS` 只放行云端控制台，内网来源用
  `AGENT_CORS_ORIGIN_REGEX` 放行（现场入口可能架在任意内网 IP/端口）。不要把 `:8000`
  映射到公网。
- **板子 IP 是 DHCP**：`ROBODOG_BACKPACK_BROWSER_URL` 写的是 `192.168.41.246`，
  换 IP 后要改这一行并 `systemctl restart rust-admin-robodog`（建议在路由器上做静态租约）。
- **OSD_FIFO_FPS=4**：板端纯 Python 渲染实测 ~200ms/帧、写入 3.7fps，按 8fps 会反压主视频
  （表现为 AI 那路慢几秒）。已在 unit 里降为 4；想更快只能减少类别数或换掉 Python 画框。
- 板端改代码后要 `systemctl restart backpack-agent`（别再用手工 `bash start.sh`，
  终端一断四路推流就没了）。
- AI 只在开启时占 NPU；关掉后 `preview` 流随之消失，页面自动回干净流。
- 中继要看框时依赖 `RELAY_STREAMS` 含 `preview`（默认已含）；少了这一路会报
  「板端推流服务未连接」，此时页面会提示「检出数量看右侧」。

---

## 三、`.246` 的出网路由（踩过）

这台机器有两个网卡，两条默认路由，**机器人那条会赢**：

```bash
root@ubuntu-arm64:~# ip route show default
default via 192.168.234.1 dev eth0 proto static          # ← netplan 写的，无 metric，优先
default via 192.168.40.254 dev wlp4s0 proto dhcp metric 600
```

结果：`ip route get 39.107.55.13` 走 `eth0`（机器人网关），SYN 被吞，客户端日志停在
「正在连接业务服务: ws://39.107.55.13:3100/ws/device」，平台侧一直「离线」，
而且**不会报错**（TCP 连接一直挂着）。

修复：给机器人网卡的默认路由加 metric，让 wifi 的 DHCP 路由（600）胜出。
`/etc/netplan/01-eth0.yaml`：

```yaml
      routes:
        - to: default
          via: 192.168.234.1
          metric: 1000          # ← 加这一行；不写时 metric=0，优先级高于 wifi
```

```bash
chmod 600 /etc/netplan/01-eth0.yaml    # netplan 会告警权限过宽
netplan apply
ip route get 39.107.55.13              # 应显示 via 192.168.40.254 dev wlp4s0
systemctl restart robodog-device-client
```

机器人网段（`192.168.234.0/24`）走的是链路路由，不受影响；本体 `192.168.234.234` 仍然直连。

### 元凶：`4g-connect.service`（2026-11-13 实测）

只加 netplan metric **还不够**：这台机器上厂商的 4G 兜底脚本每 30~60 秒跑一轮，
失败后 cleanup 会执行

```bash
ip route add default via "$ETH0_GW" dev eth0      # 没有 metric → metric 0
```

于是机器人网卡又被抬成最高优先级默认路由，wifi 出网被冲掉。现象是**周期性掉线**：
客户端心跳能刷一阵，然后整机连不上 `39.107.55.13`，看门狗（90s）把设备标「离线」。

修 `/usr/local/bin/4g-connect.sh` 的 cleanup 分支：

```bash
# 原来
ip route add default via "$ETH0_GW" dev eth0 2>/dev/null
# 改成（机器人网卡只做兜底，让 wifi 的 600 赢）
ip route replace default via "$ETH0_GW" dev eth0 metric 1000 2>/dev/null
```

```bash
cp -a /usr/local/bin/4g-connect.sh{,.bak-$(date +%Y%m%d-%H%M%S)}
systemctl restart 4g-connect.service      # 让新进程读补丁版
ip route del default via 192.168.234.1 dev eth0   # 清掉当前这条 metric 0
```

> 注意：`systemctl restart` 时**旧进程**的 SIGTERM cleanup 还会用老代码加一次
> metric 0 路由，所以要「先重启服务、再删那条路由」，或者删完再看一眼。

排查命令（现场直接抄）：

```bash
ip route show default                     # 有没有不带 metric 的 eth0 默认路由
ip route get 39.107.55.13                 # 走 eth0 就是错的
journalctl -t 4g-connect --since today    # "Restored eth0 as default route"
ip monitor route                          # 抓到路由被谁改的时刻，再对照 journal
```

`4g-connect.service` 本身还会重写 `/etc/resolv.conf`；4G 模块一直注册不上
（`AT+COPS: 0` / `Failed to get IP ... after 30s`）时它就是在空转，确不需要 4G 兜底可以
`systemctl disable --now 4g-connect.service`。

## 四、正式环境与设备客户端的联动检查

发布会话结束后，按这条链路验证一遍（不改数据，只看状态）：

```bash
# 1) 正式环境健康
curl -s http://39.107.55.13:3100/api/health | python3 -m json.tool

# 2) 客户端在线（.246 本机看日志）
ssh root@192.168.41.246 'journalctl -u robodog-device-client -n 5 --no-pager'

# 3) 中台台账同步（控制台 :3005 → 设备管理，或直接看库）
ssh root@39.107.55.13 'sqlite3 /opt/hz/rust-admin/zentai/data/users.db \
  "select device_id,status,device_ip,park_code,update_time from dog_device
   where is_deleted=0 order by update_time desc limit 5;"'
```

三处对不上时，排查顺序：客户端日志「注册成功/断开重试」→ 业务平台
`journalctl -u rust-admin-robodog` 的 `sent task dispatch` / 心跳看门狗 →
中台 `dog_device` 的 `update_time`。

---

## 五、常见问题

| 现象 | 原因 / 处置 |
| --- | --- |
| 目标机 `GLIBC_2.34 not found` | 必须用 musl 静态构建（`build.sh` 已默认）；不要直接 `cargo build` |
| 客户端 `GLIBC_2.39 not found` | 用 `build-aarch64.sh`（zigbuild 按 2.35 链接），别用普通交叉编译 |
| 正式环境打不开 3000 | 中台在 **3005**，3000 被 frps 占用 |
| 业务平台报「无法校验设备分配关系」 | `zentai.env` 与 `robodog.env` 的令牌不一致，或只有地址没有令牌 |
| 客户端起来了但平台看不到 | 检查 `[server] url` 是否 `ws://39.107.55.13:3100`；`journalctl` 里找「设备注册成功」 |
| 任务下发后立刻「异常」 | 设备未绑园区 / 本体离线（`body_online=false`）/ 定位未就绪，原因在任务事件里 |
| 想在没有真机时验证全链路 | 本机 `--robot mock`（见 `business/client/README.md`），能看到真实 5000/5001/5009 报文 |
| 现场 journald 里看不到报文了 | 新版客户端把周期推送（`1315/1317/1325/1327/5009`）降到 TRACE，只保留应答类报文；看全量加 `RUST_LOG=robodog_device_client=debug,robodog_device_client::zhpro::client=trace`，或 `ROBODOG_FRAME_DUMP=/var/log/robodog-frames.jsonl` |
| 改了前端没生效（正式） | 正式环境前端由 Rust 进程托管，必须 `push.sh` 重新打包；`--skip-web` 不会更新前端 |
| 客户端「正在连接业务服务」之后没有任何日志、平台一直显示离线 | **`.246` 双默认路由**：机器人网卡 `eth0`（netplan，`via 192.168.234.1`，无 metric）抢了 wifi 的 DHCP 默认路由，出网流量被丢进机器人网关。排查 `ip route get 39.107.55.13`，修复见下节 |
| `scp` 报 `dest open ... Failure` | 目标二进制**正在运行**（ETXTBSY），传成 `.new` 再 `mv` |
| 平台提示「本体 SDK 节点未就绪（sdk_node=false）」 | 本体侧问题，不要先重启算法服务：先看 `.234` 的 `robot_sdk_node.stdout`（是否刷 `Model/transport not ready`）再看 `.1` 的 `dog_task` 与 `df -h /dev/shm`，完整链路与修法见 [`robodog-body.md`](robodog-body.md) §4 |
| 平台定位「未就绪」/「重新定位」报 4013 超时 / 位置 0,0 | 算法机（`.234`）的 `robot-alg-manager` 卡在等 `/robot_type`（`.1` 的机型上报）。先确认 `.1` 稳定，再 `ssh robot@192.168.234.234 'sudo systemctl restart robot-launch'`，然后点「重新定位」；完整判定与兜底见 [`robodog-body.md`](robodog-body.md) §8 |
| 任务全部回 `13897 capability busy` | 本体上还有未结束的任务（或本体栈崩了）。先按 [`robodog-body.md`](robodog-body.md) 看 `dog_task` 是否在崩溃循环，再考虑在官方 App 取消任务 / 重启本体栈 |
| 一下发导航任务，`sdk_node` 与 `estop` 每 5~15 秒抖一次、进度卡在 0/N | `.1` `dog_task.stdout` 里的 `Fast CDR ... NavigationState_. Not enough memory in the buffer stream` | 两机 `robots_dog_msgs` 版本不一致（`.234` 0.9.8 / `.1` 0.9.0），只能厂商升级；**期间用官方 APK 执行巡检任务**。证据与问题单见 [`robodog-issues-2026-11-14.md`](robodog-issues-2026-11-14.md) §3、§5 |

---

## 六、发布检查清单

- [ ] `cargo test` / `cargo clippy --all-targets -- -D warnings`（backend、business/server、business/client）
- [ ] `cd frontend && pnpm lint`、`cd business/web && pnpm typecheck`
- [ ] 正式环境：`push.sh` → 两个 unit `active` → `:3005` 200 → `/api/health` 正常 → 控制信道日志
- [ ] 客户端：`build-aarch64.sh` 符号校验通过 → 备份旧二进制 → `systemctl restart` → 「设备注册成功」
- [ ] 控制台设备管理里该设备「在线待命」、心跳刷新
