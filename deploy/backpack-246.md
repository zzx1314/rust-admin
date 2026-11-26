# `.246` 背包机：服务与文件位置清单（2026-11-19）

> 现场那台「背包机」上到底跑了什么、配置在哪、哪些文件是这套仓库装上去的、备份在哪。
> 出问题时按这份清单找日志和配置，**不要靠猜**。
>
> 相关文档：本体侧（Orin NX `.234` / RK3588 `.1`）见 [`robodog-body.md`](robodog-body.md)；
> 中继原理与排障见 [`../business/docs/backpack-video-integration.md`](../business/docs/backpack-video-integration.md) §6；
> 本次「钉死 4G」的实施记录见 [`backpack-4g-pin-2026-11-19.md`](backpack-4g-pin-2026-11-19.md)。
>
> **板子连不上时**：`deploy/backpack-sim/` 能在本机复刻这台机器（mediamtx + 假相机 +
> 模拟 Agent + 原版 relay / client / site-console），见该目录 `README.md`。

## 0. 机器身份

| 项 | 值 |
| --- | --- |
| 主机名 | `ubuntu-arm64`（Ubuntu 22.04.5 LTS, aarch64） |
| 现场内网地址 | `192.168.41.246/22`（Wi-Fi `wlp4s0`，网关 `192.168.40.254`） |
| 本体内网地址 | `192.168.234.200/24`（`eth0`，网关 `192.168.234.1`） |
| 4G 地址 | `10.128.43.32/8`（`enx020c29a39b6d`，网关 `10.128.43.223`） |
| 登录 | `ssh root@192.168.41.246`（Wi-Fi 内网走直连路由，**改平台出口不影响这条**） |
| 磁盘 | `/dev/root 29G，已用 43%` |
| 日志 | 全部走 journald：`journalctl -u <unit>` |

## 1. 出网：三条默认路由（优先级从高到低）

```bash
default via 10.128.43.223  dev enx020c29a39b6d  metric 100    # 4G dongle
default via 192.168.234.1  dev eth0            metric 200    # 厂商 4g-connect.sh 加的（狗体内网，出网是黑洞）
default via 192.168.40.254 dev wlp4s0          metric 600    # 现场 Wi-Fi（一天掉 32 次）
default via 192.168.234.1  dev eth0            metric 1000   # netplan 兜底
```

- **2026-11-19 起平台流量固定走 4G**：`39.107.55.13` 不再有任何 `/32` 特例
  （`ip route get 39.107.55.13` → `via 10.128.43.223 dev enx020c29a39b6d`）；
- Wi-Fi 只用来登录 / 传文件 / 现场控制台（`:8080`），**不承载**视频中继与设备心跳；
- 详细说理与回滚见 [`backpack-4g-pin-2026-11-19.md`](backpack-4g-pin-2026-11-19.md)。

## 2. 服务一览

| unit | 作用 | 端口 | ExecStart / 工作目录 | 仓库来源 |
| --- | --- | --- | --- | --- |
| `snap.mediamtx.mediamtx.service` | RTSP 分发中枢：把本体/网络摄像头的流变成 7 条本地 path，供 relay、Agent、浏览器共用 | `8554` RTSP、`8888` HLS/HTTP、`8889` WebRTC | snap 托管 | 不在仓库（厂商镜像 + 现场配置） |
| `backpack-agent.service` | 背包数据采集 Agent：FastAPI + 6 路流 + RKNN AI 识别（航点存证 `detect_jobs/`） | `8000` | `/opt/xjpt/backpack_dashboard/backend` → `python3 agent.py` | `business/backpack_dashboard/` |
| `camera-init.service` | USB 相机初始化（`/opt/xjpt/camera/testc.sh`，启动前 sleep 8） | — | `backpack-agent` 的 `ExecStartPre` 也会重启它 | 不在仓库（厂商） |
| `asr.service` | 聆思 USB 麦克风语音识别（`asr_usb_record`，腾讯云 ASR/TTS）；Agent 的「语音助手」页面在它上面发命令 | — | `WorkingDirectory=/opt/xjpt/lingsi`、`LD_LIBRARY_PATH=/opt/xjpt/lingsi/lib`、`ExecStart=/opt/xjpt/lingsi/asr_usb_record` | 不在仓库（厂商） |
| `alarmlight.service` | **LED 灯带（72 颗 WS2812）红蓝 100ms 爆闪**：Agent 的「LED 爆闪」按钮与航点 / 计划灯光都靠 `systemctl start/stop alarmlight` 启停 | — | `ExecStart=/opt/xjpt/led/alarmlight`（unit 里 `KillSignal=SIGINT`、`TimeoutStopSec=10`；**disabled 是对的，别 enable**） | 仓库 `deploy/backpack-alarmlight.service` |
| `led_blue.service` | 老的 LED 灯带闪烁程序（`test_rgbrun`，白/红/绿/蓝循环）；**当前 disabled，已被 `alarmlight` 取代**（`test_rgbrun` 只挂 SIGINT，Agent 老逻辑发 SIGTERM 关灯时灯珠会停在最后一帧亮着） | — | `ExecStart=/opt/xjpt/led/test_rgbrun` | 不在仓库（厂商） |
| `backpack-relay.service` | **视频中继**：按需把本地 RTSP 推到平台 mediamtx（`RELAY_RTSP_URL`，浏览器 WHEP 看）；控制面走 `ws://39.107.55.13:3008/ws/device/video` | 无常驻端口（纯出站 WS） | `/usr/bin/python3 /opt/backpack-relay/relay_push.py` | `deploy/backpack-relay/` |
| `robodog-device-client.service` | 设备客户端：连平台 `:3008` 做注册/心跳/任务，反代本体 `:10010`，转发 Agent 事件与 AI 证据图 | 无常驻端口（出站 WS） | `/opt/robodog/client/robodog-device-client --config /etc/robodog/client.toml` | `business/client/`（`build-aarch64.sh` 出包） |
| `site-console.service` | 现场控制台门户：静态页 + 把 `/api` 转到平台，给同网段的浏览器用 | `8080` | `/opt/site-console` → `python3 serve.py` | `deploy/site-console/` |
| `4g-connect.service` | 厂商脚本：管理 4G dongle 拨号与默认路由（`Restart=always`，每轮会重建默认路由） | — | `/usr/local/bin/4g-connect.sh` | 不在仓库（厂商） |
| `ota_service.service` | 厂商 OTA | — | — | 不在仓库（厂商） |
| `frpc.service` | **外网运维隧道**：本机 22 → 公网 `39.107.55.13:3006`（见 §4.2） | 无常驻端口（出站到 `frps:8090`） | `/opt/frp/frpc -c /etc/frp/frpc.toml` | `deploy/backpack-246-frpc/` |

`ss -lnt` 实测监听：`22`（ssh）、`8000`（Agent）、`8080`（site-console）、`8554`/`8888`/`8889`（mediamtx）。

## 3. 目录与文件位置

```text
/opt/backpack-relay/                     ← 中继（仓库：deploy/backpack-relay/）
├── relay_push.py                        主程序（2026-11-19 更新）
├── test_relay_push.py                   纯状态机回归（板端也留了一份，便于现场自测）
└── relay_push.py.bak-* / .bak.*         历史备份（9 份，见 §5）

/opt/robodog/client/                     ← 设备客户端（仓库：business/client/）
├── robodog-device-client                aarch64 静态二进制
├── README.md                            客户端说明
└── robodog-device-client.bak-*          历史备份（35 份，占 275M，见 §5）

/opt/site-console/                       ← 现场控制台（仓库：deploy/site-console/）
├── serve.py                             静态服务 + /api 反代
└── web/                                 构建产物（另有 web.bak-* 3 份）

/opt/xjpt/backpack_dashboard/         ← 背包 Agent（仓库：business/backpack_dashboard/，见同目录 AGENTS.md）
                                         2026-11-20 从 /root/261113/backpack_dashboard 迁来（见 §4.3）
├── backend/agent.py                     FastAPI :8000
├── backend/detect_jobs/<plan>/pt<n>/     AI 临时存证 + result.json（平台读不到，由客户端上报；
│                                         图进企业微信群后平台下发 ai_cleanup，客户端删掉该 pt 目录；
│                                         任务终态 / 取消时客户端再把整个 <plan>/ 删掉（没命中、
│                                         不推图的航点只靠 ai_cleanup 清不掉），启动时还会清
│                                         超过 3 天没动过的残留 —— 见 business/docs/wecom-alert-push.md）
├── /media/media-data/picture/<plan>/pt<n>/  媒体归档（N 帧画框图 + result.json）：中台设备页
│                                         「同步媒体」→ 客户端逐张传中台「媒体资源」→ 传成功才删。
│                                         目录/帧数在中台「参数配置」改（media_picture_dir / media_frames），
│                                         板端 `MEDIA_DATA_DIR` 只是兜底；`/media` 必须是可写挂载盘
├── rknn/                                RKNN 运行库（LD_LIBRARY_PATH）
└── frontend/                            厂商调试页

/opt/xjpt/camera/testc.sh                 ← USB 相机初始化（camera-init.service，原 /root/camera/）
/opt/xjpt/lingsi/                         ← 聆思语音（asr.service；原 /root/lingsi/）
├── asr_usb_record                       语音识别主程序 + `lib/` + `asr.conf`（腾讯云密钥）
└── device_selfcheck.sh                  厂商自检（已改为指向 /opt/xjpt/wenshidu）
/opt/xjpt/led/test_rgbrun                ← LED 灯带闪烁（白/红/绿/蓝循环；**已弃用**，只剩 Agent 没 unit 时的兜底；原 /root/led/）
/opt/xjpt/led/alarmlight (+ .c)          ← LED 灯带红蓝爆闪（`alarmlight.service` = Agent「LED 爆闪」/ 航点灯光；
                                          收到 SIGINT/SIGTERM 会先写全 0 + reset 再退出，所以 `systemctl stop` 才是真熄灯）
/opt/xjpt/LED/                           ← 厂商 LED 常亮服务源码/二进制（light_service、light_client；原 /root/LED/）
/opt/xjpt/wenshidu/sht4x_app             ← 温湿度传感器（Agent `/api/temp`；原 /root/wenshidu/）
/root/{LED,led,lingsi,wenshidu}          ← 已不存在（2026-11-20 搬进 /opt/xjpt 后软链也删了，见 §4.4）

/etc/systemd/system/
├── backpack-relay.service               中继 unit（仓库同名文件）
├── backpack-agent.service               Agent unit
├── camera-init.service                  相机初始化
├── robodog-device-client.service        客户端 unit（仓库：business/client/deploy/）
├── site-console.service                 控制台 unit
├── asr.service                         聆思语音 unit（WorkingDirectory=/opt/xjpt/lingsi）
├── led_blue.service                    LED 闪烁 unit（disabled，已被 alarmlight 取代；ExecStart=/opt/xjpt/led/test_rgbrun）
├── alarmlight.service                  LED 爆闪 unit（disabled；ExecStart=/opt/xjpt/led/alarmlight；KillSignal=SIGINT）
├── frpc.service                         外网 ssh 隧道 unit（仓库：deploy/backpack-246-frpc/）
├── 4g-connect.service                   厂商
└── *.bak-*                              历史备份（逐个看过再删，见 §5）

/etc/NetworkManager/dispatcher.d/
├── 01-ifupdown                          NetworkManager 自带
└── 50-backpack-relay-route              平台出口路由脚本（仓库：deploy/backpack-relay/50-platform-route.sh）
                                         ⚠️ 目录里别再放可执行的 .bak：NM 会照样执行（见 §6）

/etc/netplan/01-eth0.yaml                eth0 默认路由 metric（1000，避免抢 Wi-Fi/4G 出网）
/opt/frp/frpc                            frpc v0.68.0 aarch64（不在仓库，见 §4.2）
/etc/frp/frpc.toml                       frpc 配置（公网 frps + 22→3006 代理，600）
/etc/robodog/client.toml                 客户端配置（平台地址、token、本体地址）
/etc/robodog/client.env                  客户端环境变量（EnvironmentFile，含敏感项，600）
/usr/local/bin/4g-connect.sh             厂商 4G 拨号脚本（改过 eth0 metric，见 §6）
/var/snap/mediamtx/common/mediamtx.yml   mediamtx 活配置（7 条 path，见下）
/root/dispatcher-backup/                 2026-11-19 从 dispatcher.d 移出的旧脚本
```

### mediamtx 的 7 条 path（relay / 页面上的「摄像头」就是这些名字）

| path | 来源 | 说明 |
| --- | --- | --- |
| `dogcam` | `rtsp://user:***@192.168.234.1:8554/test`（本体前置 1080p60） | `sourceOnDemand: yes` + `StartTimeout 15s` + `CloseAfter 120s` + `rtspTransport: tcp` |
| `dog-ipcam` | `rtsp://192.168.234.201/live` | 常拉（`sourceOnDemand: no`） |
| `cam0`~`cam3`、`preview` | `all_others: source: publisher` | 由 `backpack-agent` 推进 mediamtx（USB 相机 / 烧录预览流） |

### `backpack-relay.service` 的关键环境变量（现行值）

```ini
Environment=RELAY_PLATFORM=http://39.107.55.13:3008
Environment=RELAY_STREAMS=cam0,cam1,cam2,cam3,dogcam,dog-ipcam,preview
Environment=RELAY_RTSP_BASE=rtsp://127.0.0.1:8554
Environment=RELAY_PROFILE=light          # 配置档：1280x720@15 / 1000k
Environment=RELAY_BITRATE=1000k
Environment=RELAY_FPS=15
Environment=RELAY_WIDTH=1280
Environment=RELAY_HEIGHT=720
Environment=RELAY_GOP=15                 # 1s 一个关键帧（起流/切通道等待短）
Environment=RELAY_SLOW_IFACES=           # 空 = 不按网卡名一刀压档（4G 实测 8.5Mbps，见 §4）
```

## 4. 2026-11-19 这轮改动（平台出口钉死 4G）

| 文件 / 对象 | 改成了什么 | 原值 / 备份 | 回滚 |
| --- | --- | --- | --- |
| `/etc/NetworkManager/dispatcher.d/50-backpack-relay-route` | 换成 `PIN_IFACE=none`：不再给平台加 `/32`，任何网卡上线都把残留 `/32` 清掉 | 旧脚本已移到 `/root/dispatcher-backup/50-backpack-relay-route.bak-20261118113748` | `PIN_IFACE=wlp4s0` |
| `/opt/backpack-relay/relay_push.py` | 新版：新增 `_last_egress_iface` 缓存 + `apply_remote_profile()` 末尾的慢链路压档兜底 | 旧版只在历史 `.bak` 里 | `scp` 回旧版后重启 |
| `/etc/systemd/system/backpack-relay.service` | 加 `Environment=RELAY_SLOW_IFACES=`（4G 实测 8.5Mbps，不再一刀压到 320x180） | 改动前内容见本文件 §3 的 env 列表 | 注掉该行 + `daemon-reload` |
| 路由 | `ip route del 39.107.55.13/32`（平台改走默认路由 = 4G） | 原 `via 192.168.40.254 dev wlp4s0` | `ip route replace 39.107.55.13/32 via 192.168.40.254 dev wlp4s0` |
| 服务 | `systemctl restart backpack-relay` + `restart robodog-device-client` | — | 同样两条命令 |

平台侧（`39.107.55.13`，不在 `.246` 上）只做了**确认**，未改动：
`settings.video_relay_profile=light`、`settings.video_relay_auto_degrade=true`
（库：`/opt/hz/rust-admin/robodog/data/robodog.db`）。

## 4.1 2026-11-19 晚：Agent 降帧率 30 → 15fps

起因：平台中继看着「不流畅」。先量清楚再改——**问题不在中继的码率，在板端 CPU**
（改前 load average 3.96，四路 `h264_rkmpp` + 四路 MJPG 软解）。

关键发现：**四路 USB 相机 MJPG 只报 30.000fps 的离散帧间隔**，
`v4l2-ctl --list-formats-ext -d /dev/usbcam0` 里所有尺寸都只有 `Interval: Discrete 0.033s`。
所以 `CAPTURE_FPS=15` 会让 v4l2 直接 EINVAL、四路全黑，降帧率**只能在编码前丢帧**。

| 对象 | 改成什么 | 回滚 |
| --- | --- | --- |
| `backend/config.py` | 新增 `CAPTURE_OUT_FPS`（默认 0=不转换）/ `CAPTURE_NOUT_FPS` | `.bak-20261119-152702` |
| `backend/stream.py` | `-vf fps=$CAPTURE_OUT_FPS`（采集仍 30fps） | 同上 |
| `backend/relay.py` | dogcam720 的 `fps=` 改用 `CAPTURE_NOUT_FPS` | 同上 |
| `/etc/systemd/system/backpack-agent.service` | `Environment=CAPTURE_OUT_FPS=15` + `Environment=OSD_FPS=15` | `/root/backpack-agent.service.bak-20261119-152702` |

为什么值 15：中继 `RELAY_FPS=15`，源给 30 是「白解 30 帧、丢一半再编 15 帧」；
AI 抽帧是 `max(8, min(12, DETECT_FPS))` = **8fps**，15fps 源完全喂得动。
**别压到 8 以下**，否则抽帧与源帧率打架、漏检变多。

`CAPTURE_GOP`（新增，默认 = `CAPTURE_NOUT_FPS` = **1s** 一个关键帧）同样是这套参数的一部分：
录像（`record.py`）是 `-c copy`，只能从**本路的下一个关键帧**开始写，GOP 大就会「前置
先出画面、左前/右前晚 8~13s」，多路内容差 ~10s，`-t seconds` 也会录不满（2026-11-26 现场
「前置与左前/右前视频差 10s」就是这个）。只有确实要压码率时才在 unit 里调大
（`Environment=CAPTURE_GOP=30`），**别设 0 / 负数**（会按 1s 处理）。改完
`systemctl restart backpack-agent` 生效，复测：`ffprobe ... -show_entries packet=pts_time,flags`
看 K 帧间隔应 ≤1s。

改后实测：`ffprobe` 四路 `1280,720,15/1`；load average 3.96 → 2.6，CPU idle 73%。

```bash
# 验证
for c in cam0 cam1 cam2 cam3; do printf "%s: " $c; ffprobe -v error -select_streams v:0 \
  -show_entries stream=width,height,avg_frame_rate -of csv=p=0 rtsp://127.0.0.1:8554/$c; done
ps -eo args | grep '[v]4l2' | head -1   # 应能看到 -framerate 30 ... -vf fps=15
```

⚠️ 同一时间段 `dogcam` 那一路仍在降档（`极省流档 320x180@8 200k`，15:12 日志
`省流窗口到期但实测 190kbps（本档目标 500kbps）`）——**这条跟四路 USB 无关**：
dogcam 是 mediamtx 直拉本体 1080p60，relay 用软解拉到 320x180（单进程 70% CPU）。
要查它得看 4G 上行实测和 `RELAY_DECODER`（是否真用上了 `h264_rkmpp` 硬解）。

### 4.1.1 顺手治了「吃 70% CPU 的那条 ffmpeg」

先纠正一个现场常见误判：吃 70% CPU 的**不是中继**，是 **Agent 的 `relay.py`** ——
`dogcam_profile` 停在 `720p30`，于是 Agent 常驻一条 `本体 1080p60 → 720p` 重编码
（`dogcam720`），给 AI / 调试页用。软解 1080p60 一个核就没了。

| 方案（.246 实测） | 单核占用 |
| --- | --- |
| 现状：软解 1080p60 + swscale + 硬编 | **71%** |
| 加 `-c:v h264_rkmpp` 硬解（已改） | **23%**（线上实测） |
| 硬解 + `scale_rkrga`（RGA 硬缩放） | ❌ `Error reinitializing filters`，三种写法都失败 |
| 干脆切回 `1080p60` 档（不做这路转码） | 0%（但 AI/调试页 拿 1080p60） |

改动：`backend/relay.py` 新增 `_probe_decoder()`（探一次真解出 2 帧才用，失败退回软解），
`_build_cmd()` 在 `-i` 前插 `-c:v h264_rkmpp`，`/api/relay/status` 多一个 `decoder` 字段。
输出仍为 `1280x720@15`（`CAPTURE_NOUT_FPS`）。

🔴 **真正的隐患：CMA 只有 16MB，已经长期打满**。改前后两次 boot 都大量刷
`cma: cma_alloc: reserved: alloc failed, req-size: 8~16 pages, ret: -12`（本次 boot 45 条），
而且 **15:46:33 板子无任何关机日志硬重启**——时间点紧挨着一次 1080p60 解码压测。
也就是说 `scale_rkrga` 失败和那次重启大概率是同一件事：CMA 被打爆。
要根治（按侵入性排序）：切回 `1080p60` 档省掉一整套 1080p 解码/720p 编码的 CMA；
或给内核 cmdline 加 `cma=256M`（需改 boot args + 重启，现场窗口要做）；
或把 AI 不跑时的 `dogcam720` 改成按需启停。

观测命令：
```bash
journalctl -b | grep -c cma            # 期望个位数，几十条就是快满了
cat /proc/meminfo | grep Cma
curl -s localhost:8000/api/relay/status   # decoder 应为 h264_rkmpp
```

## 4.2 2026-11-19 晚：装 frpc，公网 `3006` → 本机 `22`

现场不在同一网段时这台机器完全失联，于是复用了公网那台已有的 `frps`
（`39.107.55.13:8090`，就是占着 3000 的那个）做一条反向隧道：

```
ssh -p 3006 root@39.107.55.13  →  frps  →  .246 的 frpc  →  127.0.0.1:22
```

- 板端：`/opt/frp/frpc`（frp **v0.68.0 aarch64**，与公网 `frps` 同版本）、
  `/etc/frp/frpc.toml`、`/etc/systemd/system/frpc.service`（`enable --now` + `Restart=always`）；
  仓库副本 `deploy/backpack-246-frpc/`（含 README 的安装/验证/回滚）
- 公网侧**没改动**：`frps.toml` 只有 `bindPort = 8090`，没有 `allowPorts`，
  远端端口默认放开；3006 在阿里云安全组里本来就通
- 隧道跑在 4G 上（`ip route get 39.107.55.13` → `enx…`，metric 100），
  Wi-Fi 掉了不影响；4G 重拨时 `loginFailExit = false` + `Restart=always` 自己重连
- ⚠️ `frps` **没有配 `auth.token`**（历史和现有客户端都这样），任何能连 8090 的人都能
  抢占远端端口；隧道里只走 key 登录，别传明文密码。要收紧得给 `frps.toml` 加
  `auth.token` 并同步所有客户端（会重启 frps，3000/3007 那条隧道会短暂断）
- 验证：公网 `ss -lntp | grep 3006` 有 `frps` 监听 + 任意外网机器
  `ssh -p 3006 root@39.107.55.13 'hostname'` → `ubuntu-arm64`

## 4.3 2026-11-20：Agent 目录迁到 `/opt/xjpt`

`/root/261113/backpack_dashboard` → **`/opt/xjpt/backpack_dashboard`**（`/opt` 与 `/root`
同一文件系统 `/dev/root`，`mv` 是瞬时的，不用拷 267M）。同批把更早搬过去的
`/opt/xjpt/camera`（`camera-init.service` 已经指它）也算在这一套里。

Agent 代码里**没有写死绝对路径**（`config.py` 全部以 `_HERE` 相对推导：`detect_jobs/`、
`detect_events.jsonl`、`rknn/models`、`rknn/code/code/build/yolov8_detector`），`start.sh`
也是 `dirname $0` 推导，所以这次只需要改 unit 和客户端配置：

| 文件 | 改动 |
| --- | --- |
| `/etc/systemd/system/backpack-agent.service` | `WorkingDirectory=` 与 `LD_LIBRARY_PATH=` 换成新路径（备份 `*.bak-20261120-151458`） |
| `/etc/robodog/client.toml` | 新增 `[ai] jobs_dir = "/opt/xjpt/backpack_dashboard/backend/detect_jobs"` 钉死存证目录（备份 `*.bak-20261120-151458`），否则客户端只能靠 `DEFAULT_JOBS_DIRS` 猜或等板端 `plan_hit.evidence` 反推 |
| `rknn/code/code/build/**` | `CMakeCache.txt` / `Makefile` 里的绝对路径 `sed` 成新路径（只影响以后重新编译 detector，编译产物本来靠 `LD_LIBRARY_PATH`） |
| `/root/261113/backpack_dashboard` | 一开始留了指向新目录的软链做兼容，**2026-11-20 15:30 已删**（见 §4.4 末尾：「软链已删」）；回滚就是把它 `mv` 回来 |

回滚：`systemctl stop backpack-agent` → `mv /opt/xjpt/backpack_dashboard /root/261113/backpack_dashboard`
→ 恢复 unit / `client.toml` 的 `.bak-20261120-151458` → `daemon-reload` + `restart`。

验证（都是实测过的）：`systemctl is-active backpack-agent`=active、`curl -s 127.0.0.1:8000/api/detect/status`
里 `bin`/`models_dir`/`events_file` 全指向 `/opt/xjpt/…` 且 `bin_exists:true`、
`rknn/code/code/build/yolov8_detector --help` 能跑（带 `LD_LIBRARY_PATH`）、四路 `cam0~cam3`
ffmpeg 推流在、客户端日志 `ai.jobs_dir = /opt/xjpt/backpack_dashboard/backend/detect_jobs`。

> ⚠️ 顺便踩到的：`agent.py` 不处理 SIGTERM，`systemctl stop/restart backpack-agent`
> **要等满 `TimeoutStopSec`（默认 90s）再 SIGKILL**（journal 里是 `State 'stop-sigterm' timed out.
> Killing.`），这次迁移的“停服务”就是这样过的。影响：重启一次视频断约 1.5 分钟。
> 想快就在 unit 里加 `TimeoutStopSec=10`（反正最后还是 SIGKILL），本次没改。

## 4.4 2026-11-20：硬件目录也收进 `/opt/xjpt`（`LED` / `led` / `lingsi` / `wenshidu`）

跟 Agent 同一批：厂商那四个硬件程序目录从 `/root` 搬到 `/opt/xjpt`，
搬的时候先留了软链，**当天 15:30 全部删掉**（见本节末「软链已删」）。

| 原路径 | 现路径 | 谁在用 |
| --- | --- | --- |
| `/root/lingsi/` | `/opt/xjpt/lingsi/` | `asr.service`（`WorkingDirectory` + `LD_LIBRARY_PATH` + `ExecStart` 原本就已经指 `/opt/xjpt/lingsi`，优先搬的；旧目录是一份**内容完全相同的拷贝**，已改名为 `/root/lingsi.bak-20261120-152530`）|
| `/root/led/` | `/opt/xjpt/led/` | Agent 「LED 闪烁」（`config.py` 的 `LED_TEST_BIN`）+ `led_blue.service` 的 `ExecStart` |
| `/root/wenshidu/` | `/opt/xjpt/wenshidu/` | Agent `/api/temp`（`config.py` 的 `SHT4X_APP`）+ 两个厂商自检脚本 |
| `/root/LED/` | `/opt/xjpt/LED/` | 没人引用（厂商 `light_service`/`light_client` 源码与二进制），纯粹跟着搬 |

需要改引用的文件（均已备份 `*.bak-20261120-152530`）：

| 文件 | 改动 |
| --- | --- |
| `/etc/systemd/system/led_blue.service` | `ExecStart=/opt/xjpt/led/test_rgbrun`（已被 `alarmlight.service` 取代，留在盘上备查） |
| `/root/yd_test_v4.0.sh` | 厂商总自检脚本，11 处 `/root/led/`、`/root/wenshidu/` 全改成 `/opt/xjpt/…` |
| `/opt/xjpt/lingsi/device_selfcheck.sh` | 2 处 `/root/wenshidu/sht4x_app` → `/opt/xjpt/wenshidu/sht4x_app` |
| `/opt/xjpt/backpack_dashboard/backend/config.py` | `SHT4X_APP` / `LED_TEST_BIN` / `LINGS_PLAY_WAV` 三个默认值（仓库 `business/backpack_dashboard/backend/config.py` 同步改，两边 md5 一致）|

`asr.service` 本来就已经指向 `/opt/xjpt/lingsi`，**不用重启**（`ps` 里 `asr_usb_record`
一直跑在 `/opt/xjpt/lingsi/asr_usb_record`）；Agent 换了 `config.py` 所以要重启（这一轮
stop 又等了 90s 才 SIGKILL，见 §4.3 尾注）。

验证（实测）：`/opt/xjpt/wenshidu/sht4x_app` → `Temperature: 28.88 °C…`、`/api/temp` 同值；
`timeout 2 /opt/xjpt/led/test_rgbrun` 正常跑（`ldd` 无缺失）、`/api/asr` → `active`、
`/api/lingsi` → `asr_running:true`；`grep -rI '/root/\(LED\|led\|lingsi\|wenshidu\)'`
除备份文件外无残留（`/root/261117/…` 是另一份历史快照，不算）。

> 顺带发现：`LINGS_PLAY_WAV`（默认 `/opt/xjpt/lingsi/call.wav`）在 `telemetry.py` 里
> **只 import 未使用**，而且那个 `call.wav` 目录里从来没有（只有 `tencent_test.pcm`）——
> 死配置，改路径只是为了不留旧路径。

### 软链已删（2026-11-20 15:30）

两个迁移（§4.3 / §4.4）最初都留了 `/root/…` 旧路径软链做过渡，当天就删了：

```
/root/261113/backpack_dashboard  /root/LED  /root/led  /root/lingsi  /root/wenshidu   ← 均已 rm
```

删前确认过没人再依赖旧路径：

- 板端 `grep -rIl '/root/\(261113\|LED\|led\|lingsi\|wenshidu\)'`（`/etc`、`/usr/local`、
  `/opt/{xjpt/camera,backpack-relay,site-console}`、`/root`）只剩 `*.bak-*` 与
  `/etc/robodog/client.toml` 里**一句注释**，没有活引用；
- 厂商 `yd_test_v4.0.sh` / `device_selfcheck.sh` 已改成 `/opt/xjpt/…`；
- 客户端不再靠候选路径猜：`[ai] jobs_dir` 钉死在 `/opt/xjpt/…`，而且当天把
  aarch64 客户端重新编了（`DEFAULT_JOBS_DIRS` 多了 `/opt/xjpt/…` 这一项）并装上：

```bash
cd business/client && ./deploy/build-aarch64.sh        # GLIBC_2.34 ≤ 目标机 2.35
scp target/aarch64-unknown-linux-gnu/release/robodog-device-client \
  root@192.168.41.246:/tmp/robodog-device-client.new   # 直接覆盖会 ETXTBSY，先传 /tmp
# 板上：install -m 0755 /tmp/robodog-device-client.new /opt/robodog/client/robodog-device-client
#       systemctl restart robodog-device-client
#       旧二进制备份：robodog-device-client.bak-20261120-153044（md5 f6905d4a…→ 新 33c6fd2c…）
```

删完复测：6 个 unit 全 active（`backpack-agent`/`asr`/`backpack-relay`/`robodog-device-client`/
`site-console`/`camera-init`）、`/api/temp` 仍读到 `28.9 °C`（走 `/opt/xjpt/wenshidu`）、
`ls -ld /root/{LED,led,lingsi,wenshidu}` 全部 `No such file`。
> 若真要回滚到旧路径，需要**手动** `ln -s /opt/xjpt/<名字> /root/<名字>` 或改回引用——
> 软链已经不在了。

## 4.5 2026-11-25：LED 灯带改由 systemd 管（`alarmlight.service`）

以前 Agent 自己 `Popen(/opt/xjpt/led/test_rgbrun)`，关灯 = 按进程组 `SIGTERM`。两个坑：

1. **关灯不灭**：`test_rgbrun` 只在 SIGINT 里写「全 0 + reset」，SIGTERM 是默认终止 ——
   进程没了，72 颗灯珠停在最后一帧亮着；
2. **孤儿进程**：`Popen(..., start_new_session=True)` 的子进程不会随 Agent 退出，
   `systemctl restart backpack-agent` 后它继续抢 SPI，再开灯就是两个写者。

所以板端新增 `alarmlight.service`（`ExecStart=/opt/xjpt/led/alarmlight`，`alarmlight.c`
由厂商 `test_blue_red.c` 改的：红蓝 100ms 爆闪 + **SIGINT/SIGTERM/SIGHUP/SIGQUIT
都会先熄灯再退出**），unit 里再配 `KillSignal=SIGINT` / `KillMode=process` /
`TimeoutStopSec=10`。Agent 改成：

```bash
systemctl start alarmlight    # 开（幂等，已在跑是空操作）
systemctl stop  alarmlight    # 关（SIGINT → 程序先写全 0 + reset 再退）
systemctl is-active alarmlight # /api/lights 的 led / led_service_state 就是问它
```

部署（仓库 → 板端）：

```bash
# 1) unit（仓库 deploy/backpack-alarmlight.service 与板端 /etc/systemd/system/alarmlight.service 同内容）
scp deploy/backpack-alarmlight.service root@192.168.41.246:/etc/systemd/system/alarmlight.service
ssh root@192.168.41.246 'systemctl daemon-reload'
# 2) Agent 代码（control.py + config.py；⚠️ agent 不处理 SIGTERM，restart 要等 90s，视频断 1.5min）
rsync -av business/backpack_dashboard/backend/{control.py,config.py} \
  root@192.168.41.246:/opt/xjpt/backpack_dashboard/backend/
ssh root@192.168.41.246 'systemctl restart backpack-agent'
# 3) 验证
ssh root@192.168.41.246 'curl -s 127.0.0.1:8000/api/lights | python3 -m json.tool'   # led_control=systemd
curl -s -XPOST 127.0.0.1:8000/api/led/blink                                       # 灯红蓝爆闪
systemctl is-active alarmlight                                                     # active
curl -s -XPOST 127.0.0.1:8000/api/lights/off -H 'Content-Type: application/json' -d '{"reason":"manual"}'
systemctl is-active alarmlight; pgrep -af 'test_rgbrun|alarmlight'                  # inactive，无进程
```

⚠️ **不要 `systemctl enable alarmlight`**：灯由 Agent 按需 start，enable 等于一上电就一直爆闪。
`led_blue.service` 保留在盘上（已 disabled），它跑的是老 `test_rgbrun`，别再启。
升级后第一次开关灯时，Agent 会自动 `pkill -f /opt/xjpt/led/test_rgbrun` 清掉
老版本留下的孤儿进程（只按完整路径匹配）；现场也可手动清。

## 4.6 2026-11-26 晚：四路推流加关键帧间隔（1s），修「前置与左前/右前录像差 10s」

**起因**：现场反馈航点 / 航段录像里「前置摄像头与背包左前、右前的内容差 ~10s」。
根因不在起录时序（四路是同一时刻 `Popen` 的）：录像走 `-c copy`，ffmpeg 默认丢掉
首个关键帧之前的包，所以每个 mp4 都从**那一路自己的下一帧关键帧**开始写；而
`stream.py` 的四路推流**从来没设过 `-g`**，走编码器默认 keyint 250 → 15fps 下
**16.66s** 一个关键帧（改前实测：cam1 的 K 帧在 5.08s / 21.75s / 38.42s），
`dogcam` 那边 relay 是 1s 一个 → 两路内容起点差 8~13s，`-t seconds` 的窗口还被吃掉一截。

| 对象 | 改成什么 | 回滚 |
| --- | --- | --- |
| `backend/config.py` | 新增 `CAPTURE_GOP`（默认 = `CAPTURE_NOUT_FPS` = 1s） | `config.py.bak-20261126-193811` |
| `backend/stream.py` | 四路 `-c:v … -b:v 2000k` 后面加 `-g str(CAPTURE_GOP)` | `stream.py.bak-20261126-193811` |
| `backend/relay.py` | `-g 30`（配 `CAPTURE_OUT_FPS=15` 其实是 2s）改成 `-g str(CAPTURE_GOP)` | 同名 `.bak-20261126-193811` |
| `backend/tools/record_selftest.py` | 新增「四路推流带 `-g`」/「relay 关键帧间隔与四路一致」；两条真 ffmpeg 用例不再写死 libx264（板端只有 rkmpp）；「每路各拉各的流」改按 `stream_of()` 比（720p30 档位下 `dogcam`→`dogcam720`） | 同上 |

生效：`systemctl restart backpack-agent`（会重启 stream / relay / 相机流，视频断 ~1.5min；
`agent.py` 不处理 SIGTERM）。实测（2026-11-26 19:46，同一时刻起录、都配 15s）：

```
改前（K 帧 16.66s）：cam0 14.8s / cam1 15.0s / dogcam 13.6s        ← relay 还是 -g 30（2s）
改后（K 帧  1.00s）：cam0 14.9s / cam1 14.7s / dogcam 14.6s        ← 差 ≤0.3s
```

日志可验：`journalctl -u backpack-agent | grep 'gop'` → `fps [15:1:fix] … gop 15`；
中继进程 `pgrep -af dogcam720` → `-g 15`；K 帧间隔见 §4.6 开头那条 `ffmpeg -skip_frame nokey`。

## 4.7 2026-11-26 晚：空壳 mp4（一帧都没录到）不再当录像回传

**起因**：现场反馈「航点 1 配了狗头 + 左前录像 5s，狗头只有 3s，另一路干脆没有」。
翻 `TK-1126-20`（17:44:56 起录，路线里那条动作实际配的是 `dogcam` + `cam1`（右前）5s）：

| 相机 | 文件 | 大小 | 边车 `durationSec` |
| --- | --- | --- | --- |
| dogcam | `pt0-dogcam-20261126-174456.mp4` | 804 KB | 3.1s（= 5s 减去等 relay 2s 关键帧） |
| cam1 | `pt0-cam1-20261126-174456.mp4` | **293 B** | 5.3s（**墙钟**，不是画面） |

293 B 那个只有 `ftyp` + 空 `moov`（一帧都没有）：当时 `stream.py` 还没 `-g`（keyint 250 →
16.7s 一个关键帧），5s 窗口里等不到 I 帧。真因已在 §4.6 修掉，但**空壳本身被当成
正常录像**这件事是个独立缺陷——`_finalize` 读不到 `mvhd` 就退回墙钟时长，回传到平台
就是一段「进度条 5.3s 但打不开」的假视频，还会把真凶掩掉。

| 对象 | 改成什么 | 备份 |
| --- | --- | --- |
| `backend/record.py` | 新增 `_is_empty_capture()`（体积 `< 4KB` **且** 读不出 `mvhd`）+ `_MIN_PLAYABLE_BYTES`；`_finalize` 把空壳删掉、不写边车、不进 `list`，日志写「窗口内没有关键帧」；`stop` 回执多一个 `discarded:[{camera,name,size,windowSec}]` | `record.py.bak-20261126-195310` |
| `backend/tools/record_selftest.py` | 假 ffmpeg 改成写带 `mvhd` 的真 mp4（同时钉住「小体积但有 `mvhd` 不许误删」），新增「空壳 mp4」4 条 | `tools/record_selftest.py.bak-20261126-195310` |
| `robodog-device-client` | `stop` 回执里的 `discarded` 写一行 warn（哪一路没画面、为什么） | `robodog-device-client.bak-20261126-1954xx`（`md5 b658a487…`，GLIBC_2.34） |

实测（2026-11-26 19:54，板端当前代码）：

```
# 现场那三个文件直接用新口径判
现场空壳(293B)      size=293     mvhd=None  empty=True     ← 会被丢掉
现场正常(804KB/3.1s) size=804416  mvhd=3.134 empty=False
现场航段(8MB)       size=8184115  mvhd=32.734 empty=False
# 同一时刻起录、配 5s：dogcam 5.0s / cam0 4.4s，stop 回执 discarded=[]
```

回归：`python3 backend/tools/record_selftest.py`（板端 ok，只有「造不出空 edit」那条 skip）、
`cargo test record::`。回滚：把上表两个 `.bak` 拷回去 + `systemctl restart backpack-agent`
（客户端回滚后 `discarded` 只烂在回执里，不影响任何行为）。

## 5. 备份文件现状与清理建议

| 位置 | 数量 / 体积 | 说明 |
| --- | --- | --- |
| `/opt/backpack-relay/relay_push.py.bak*` | 9 份，约 0.5M | 11-18 反复调试留下的，**留最近 2 份即可** |
| `/opt/robodog/client/robodog-device-client.bak*` | 35 份，约 275M | 每次发布都留一份，建议只留最近 3 份 |
| `/var/snap/mediamtx/common/mediamtx.yml.bak*` | 5 份 | 现状稳定，留 1 份即可 |
| `/etc/systemd/system/*.service.bak-*` | 4 份 | 与本仓库 unit 重复，可删（最新一份是 2026-11-20 迁移前的 `backpack-agent.service.bak-20261120-151458`） |
| `/etc/robodog/client.toml.bak-*` | 1 份 | 2026-11-20 迁移前（没有 `[ai] jobs_dir`）的副本，确认新配置生效后可删 |
| `/root/lingsi.bak-20261120-152530/` | 1 份，21M | 2026-11-20 搬 `lingsi` 前留下的旧拷贝（与 `/opt/xjpt/lingsi` 内容完全相同），**可删** |
| `/root/yd_test_v4.0.sh.bak-*`、`/opt/xjpt/lingsi/device_selfcheck.sh.bak-*`、`/opt/xjpt/backpack_dashboard/backend/config.py.bak-*` | 各 1 份 | 2026-11-20 改硬件路径前的副本，可删 |
| `/root/dispatcher-backup/` | 1 份 | **别删**：这是那次把路由抢回 Wi-Fi 的元凶，留作证据（也别放回 `dispatcher.d`） |

清理时只删「可执行且不在生效路径」的备份；**`/etc/NetworkManager/dispatcher.d/` 里不要留任何可执行文件**（见 §6 第 1 条）。

## 6. 踩过的坑（在这台机器上）

1. **`dispatcher.d` 里的 `.bak` 会被 NetworkManager 执行。** 2026-11-19 出现「钉死 4G 不生效、
   路由自己跑回 `wlp4s0`」，就是 `50-backpack-relay-route.bak-20261118113748` 每次事件把 `/32`
   加回去。装这类脚本时**旧副本必须移出该目录**。
2. **改完路由必须重启服务。** 已建立的 TCP 绑在旧网卡/旧源地址上，改路由不会让它搬家；
   `50-platform-route.sh` 里「路由真变了才 `try-restart backpack-relay`」就是为此。
3. **`4g-connect.sh` 会重排默认路由。** 它每轮先 `ip route del default`，再把 eth0 按 **metric 200**
   加回来 —— 4G 重拨的那几秒里，平台流量可能被送进狗体内网（黑洞），表现为推流/心跳短暂失败。
   建议把那行改成 `metric 5000`（让 Wi-Fi 600 兜底），改前 `cp -a` 备份。
4. **mediamtx 的 `dogcam` 是 `sourceOnDemand`。** 板子刚启动时本体 RTSP 可能没就绪，浏览器直连
   会拿到 WHEP 404，10~30 秒内自动恢复；改了 `mediamtx.yml` 要
   `snap restart mediamtx`（或重启 `snap.mediamtx.mediamtx`）。
5. **`backpack-agent` 起不来会连带没有 `cam0~cam3`。** 这几路是 Agent 推给 mediamtx 的
   （`all_others: source: publisher`），Agent 挂了中继就只会剩 `dogcam`/`dog-ipcam` 有画面。

## 7. 命令速查

```bash
# 状态
systemctl status backpack-relay backpack-agent robodog-device-client site-console frpc --no-pager

# 外网 ssh 隧道（frpc → 39.107.55.13:3006）
journalctl -u frpc -f | grep -E "login|proxy|start"
systemctl restart frpc
ip route get 39.107.55.13; ip route show default; ip route show 39.107.55.13/32

# 视频中继
journalctl -u backpack-relay -f | grep -E "出口|开始拉流|实测|延迟|降档|慢速"
# 媒体走 RTSP→平台 mediamtx：板上可以直接读回来验有没有在推（本机 mediamtx 上的 bp_<stream>）
ffprobe -v error -rtsp_transport tcp -i rtsp://127.0.0.1:8554/dogcam -show_entries stream=codec_name
python3 /opt/backpack-relay/test_relay_push.py          # 状态机回归，不需要网络

# 设备客户端
journalctl -u robodog-device-client -f
cat /etc/robodog/client.toml | sed 's/token.*/token=***/'

# 采集 / AI
journalctl -u backpack-agent -f
ls -lt /opt/xjpt/backpack_dashboard/backend/detect_jobs | head

# mediamtx
ffprobe -v error -rtsp_transport tcp -i rtsp://127.0.0.1:8554/dogcam -show_entries stream=codec_name,width,height,avg_frame_rate
snap restart mediamtx

# 发布（开发机上）
scp deploy/backpack-relay/relay_push.py root@192.168.41.246:/opt/backpack-relay/
scp deploy/backpack-relay/backpack-relay.service root@192.168.41.246:/etc/systemd/system/
ssh root@192.168.41.246 'systemctl daemon-reload && systemctl restart backpack-relay'
```

重启顺序（改了 mediamtx 配置时）：`camera-init` → `backpack-agent` → `backpack-relay`；
`backpack-relay` 只依赖 `mediamtx`，`robodog-device-client` 与它们互不依赖，可单独重启。

## 8. 与本仓库的对应关系

| 仓库文件 | 板端位置 | 发布方式 |
| --- | --- | --- |
| `deploy/backpack-relay/relay_push.py` | `/opt/backpack-relay/relay_push.py` | `scp` + `systemctl restart backpack-relay` |
| `deploy/backpack-relay/backpack-relay.service` | `/etc/systemd/system/backpack-relay.service` | `scp` + `daemon-reload` + `restart` |
| `deploy/backpack-relay/50-platform-route.sh` | `/etc/NetworkManager/dispatcher.d/50-backpack-relay-route` | `scp` + `chmod 755` |
| `deploy/backpack-relay/test_relay_push.py` | `/opt/backpack-relay/`（可选，现场自测用） | `scp` |
| `business/client/` | `/opt/robodog/client/robodog-device-client` | `business/client/deploy/build-aarch64.sh` 交叉编译 + `scp` |
| `deploy/site-console/` | `/opt/site-console/`（`serve.py` + `web/`） | `scp` 构建产物 + `restart site-console` |
| `deploy/backpack-246-frpc/frpc.toml`、`frpc.service` | `/etc/frp/frpc.toml`、`/etc/systemd/system/frpc.service` | `scp` + `daemon-reload` + `restart frpc`（二进制手工 `scp`，见其 README） |
| — | `/opt/xjpt/backpack_dashboard/`（Agent） | `business/backpack_dashboard/` | 改完 `rsync backend/` + `systemctl restart backpack-agent`（模型/so 一并同步，见该目录 AGENTS.md） |
| — | `/var/snap/mediamtx/common/mediamtx.yml` | 不在本仓库，现场按 §3 的 7 条 path 配 |
