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
| `backpack-agent.service` | 背包数据采集 Agent：FastAPI + 6 路流 + RKNN AI 识别（航点存证 `detect_jobs/`） | `8000` | `/root/261113/backpack_dashboard/backend` → `python3 agent.py` | `business/backpack_dashboard/` |
| `camera-init.service` | USB 相机初始化（`/root/camera/testc.sh`，启动前 sleep 8） | — | `backpack-agent` 的 `ExecStartPre` 也会重启它 | 不在仓库 |
| `backpack-relay.service` | **视频中继**：按需把本地 RTSP 转码成 FLV 推到平台 `ws://39.107.55.13:3100/ws/device/video` | 无常驻端口（纯出站 WS） | `/usr/bin/python3 /opt/backpack-relay/relay_push.py` | `deploy/backpack-relay/` |
| `robodog-device-client.service` | 设备客户端：连平台 `:3100` 做注册/心跳/任务，反代本体 `:10010`，转发 Agent 事件与 AI 证据图 | 无常驻端口（出站 WS） | `/opt/robodog/client/robodog-device-client --config /etc/robodog/client.toml` | `business/client/`（`build-aarch64.sh` 出包） |
| `site-console.service` | 现场控制台门户：静态页 + 把 `/api` 转到平台，给同网段的浏览器用 | `8080` | `/opt/site-console` → `python3 serve.py` | `deploy/site-console/` |
| `4g-connect.service` | 厂商脚本：管理 4G dongle 拨号与默认路由（`Restart=always`，每轮会重建默认路由） | — | `/usr/local/bin/4g-connect.sh` | 不在仓库（厂商） |
| `ota_service.service` | 厂商 OTA | — | — | 不在仓库（厂商） |

`ss -lnt` 实测监听：`22`（ssh）、`8000`（Agent）、`8080`（site-console）、`8554`/`8888`/`8889`（mediamtx）。

## 3. 目录与文件位置

```text
/opt/backpack-relay/                     ← 中继（仓库：deploy/backpack-relay/）
├── relay_push.py                        主程序（2026-11-19 更新）
├── test_relay_push.py                   纯状态机回归（板端也留了一份，便于现场自测）
├── measure_bitrate.py                   量中继真实码率/帧率
└── relay_push.py.bak-* / .bak.*         历史备份（9 份，见 §5）

/opt/robodog/client/                     ← 设备客户端（仓库：business/client/）
├── robodog-device-client                aarch64 静态二进制
├── README.md                            客户端说明
└── robodog-device-client.bak-*          历史备份（35 份，占 275M，见 §5）

/opt/site-console/                       ← 现场控制台（仓库：deploy/site-console/）
├── serve.py                             静态服务 + /api 反代
└── web/                                 构建产物（另有 web.bak-* 3 份）

/root/261113/backpack_dashboard/         ← 背包 Agent（仓库：business/backpack_dashboard/，见同目录 AGENTS.md）
├── backend/agent.py                     FastAPI :8000
├── backend/detect_jobs/<plan>/pt<n>/     AI 存证图 + result.json（平台读不到，由客户端上报）
├── rknn/                                RKNN 运行库（LD_LIBRARY_PATH）
└── frontend/                            厂商调试页

/root/camera/testc.sh                    ← USB 相机初始化（camera-init.service）

/etc/systemd/system/
├── backpack-relay.service               中继 unit（仓库同名文件）
├── backpack-agent.service               Agent unit
├── camera-init.service                  相机初始化
├── robodog-device-client.service        客户端 unit（仓库：business/client/deploy/）
├── site-console.service                 控制台 unit
├── 4g-connect.service                   厂商
└── *.bak-*                              历史备份（逐个看过再删，见 §5）

/etc/NetworkManager/dispatcher.d/
├── 01-ifupdown                          NetworkManager 自带
└── 50-backpack-relay-route              平台出口路由脚本（仓库：deploy/backpack-relay/50-platform-route.sh）
                                         ⚠️ 目录里别再放可执行的 .bak：NM 会照样执行（见 §6）

/etc/netplan/01-eth0.yaml                eth0 默认路由 metric（1000，避免抢 Wi-Fi/4G 出网）
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
Environment=RELAY_PLATFORM=http://39.107.55.13:3100
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

## 5. 备份文件现状与清理建议

| 位置 | 数量 / 体积 | 说明 |
| --- | --- | --- |
| `/opt/backpack-relay/relay_push.py.bak*` | 9 份，约 0.5M | 11-18 反复调试留下的，**留最近 2 份即可** |
| `/opt/robodog/client/robodog-device-client.bak*` | 35 份，约 275M | 每次发布都留一份，建议只留最近 3 份 |
| `/var/snap/mediamtx/common/mediamtx.yml.bak*` | 5 份 | 现状稳定，留 1 份即可 |
| `/etc/systemd/system/*.service.bak-*` | 3 份 | 与本仓库 unit 重复，可删 |
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
systemctl status backpack-relay backpack-agent robodog-device-client site-console --no-pager
ip route get 39.107.55.13; ip route show default; ip route show 39.107.55.13/32

# 视频中继
journalctl -u backpack-relay -f | grep -E "出口|开始拉流|实测|延迟|降档|慢速"
python3 /opt/backpack-relay/measure_bitrate.py --stream dogcam --seconds 20
python3 /opt/backpack-relay/test_relay_push.py          # 状态机回归，不需要网络

# 设备客户端
journalctl -u robodog-device-client -f
cat /etc/robodog/client.toml | sed 's/token.*/token=***/'

# 采集 / AI
journalctl -u backpack-agent -f
ls -lt /root/261113/backpack_dashboard/backend/detect_jobs | head

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
| `deploy/backpack-relay/test_relay_push.py`、`measure_bitrate.py`、`latency_probe.py` | `/opt/backpack-relay/`（可选，现场自测用） | `scp` |
| `business/client/` | `/opt/robodog/client/robodog-device-client` | `business/client/deploy/build-aarch64.sh` 交叉编译 + `scp` |
| `deploy/site-console/` | `/opt/site-console/`（`serve.py` + `web/`） | `scp` 构建产物 + `restart site-console` |
| — | `/root/261113/backpack_dashboard/`（Agent） | `business/backpack_dashboard/` | 改完 `rsync backend/` + `systemctl restart backpack-agent`（模型/so 一并同步，见该目录 AGENTS.md） |
| — | `/var/snap/mediamtx/common/mediamtx.yml` | 不在本仓库，现场按 §3 的 7 条 path 配 |
