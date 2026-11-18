#!/usr/bin/env python3
"""板端推流服务：按需把背包的 RTSP 流转封装成 FLV，推给业务平台中继。

```text
本地 mediamtx  rtsp://127.0.0.1:8554/<stream>
    └─ ffmpeg -vf scale=720p,fps=20 -b:v 1200k -f flv -   （默认 light 档位）
         或 -c:v copy（RELAY_PROFILE=copy：只换容器，不重编码）
         └─ ws://<平台>/ws/device/video   video_register + 二进制 FLV 分片
              └─ 平台 VideoHub 房间 bp:<stream> ──ws──► 浏览器 flv.js
```

## 为什么需要它

现场入口（页面托管在内网、浏览器直连板端 WHEP）有两个硬前提：**操作员电脑与背包同
网段**、**浏览器放行 Local Network Access**。换 WiFi、出差远程、换台电脑或清一次浏览器
数据，画面就黑 —— 这不是配置问题，是浏览器策略。

这个服务把方向反过来：由**板子出公网推流**，浏览器只跟平台同源通信。代价是延时从
WebRTC 的 ~200ms 变成 1~2s，收益是任何网络、任何浏览器都能看。

## 按需

常驻 6 条 WS 连接（每条约 1KB 空闲开销）。平台在**第一个观看者进来时**才下发
`video_start`，这时才起 ffmpeg；最后一个观看者离开（或看门狗发现空转）下发
`video_stop`，立刻杀进程。没人看的时候不耗 CPU、不占上行带宽。

## 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `RELAY_PLATFORM` | `http://39.107.55.13:3100` | 平台基址，自动转成 `ws://…/ws/device/video` |
| `RELAY_STREAMS` | `cam0,cam1,cam2,cam3,dogcam,dog-ipcam,preview` | 要中继的 mediamtx path |
| `RELAY_RTSP_BASE` | `rtsp://127.0.0.1:8554` | 本地 mediamtx |
| `RELAY_ROOM_PREFIX` | `bp:` | 房间前缀，必须与服务端 `video.rs::BACKPACK_ROOM_PREFIX` 一致 |
| `RELAY_FFMPEG` | `ffmpeg` | ffmpeg 路径 |
| `RELAY_PROFILE` | `light` | 输出档位：`light`（默认，缩到 720p20 / 1.2Mbps）/ `copy`（只换容器）/ `transcode`（同 light）。**平台页面「系统设置 → 视频中继」下发默认档位后以平台为准**（见下面「平台默认档位」） |
| `RELAY_WIDTH` / `RELAY_HEIGHT` | `1280` / `720` | `light` 档位的目标外框（只缩不放，保持比例） |
| `RELAY_FPS` | `20` | `light` 档位输出帧率（源 60fps 时降到 20，MSE / 弱网友好） |
| `RELAY_BITRATE` | `1200k` | `light` 档位目标码率；平台公网出口小就再降（800k 也够看） |
| `RELAY_GOP` | `40` | 关键帧间隔（帧）：40@20fps = 2s，加入 / 重连等待上限 |
| `RELAY_ENCODER` | `auto` | `h264_rkmpp` → `h264_v4l2m2m` → `libx264` 自动挑，也可写死 |
| `RELAY_TRANSCODE` | — | **已废弃、忽略**（历史 unit 里的 `=0` 不会再把档位压回 copy） |
| `RELAY_SEND_TIMEOUT` | `10` | 单个分片写进 WS 的上限（秒）。超时 = 上行已死（Wi-Fi 掉线时 TCP 半开，send 不报错也不返回），断开重连 |
| `RELAY_STALL_TIMEOUT` | `15` | 起流后这么久一个字节都没推出去（源卡住 / RTSP 半死）就重启 ffmpeg |
| `RELAY_ROUTE_POLL` | `5` | 每这么久查一次平台出口网卡（`ip route get`），变了就重连；`0` 关闭 |
| `RELAY_DEGRADE_TTL` | `300` | 首次降档后至少在省流档里待多久（秒）；到期还要链路真的恢复才回配置档 |
| `RELAY_DEGRADE_TTL_MAX` | `1800` | 升降档来回抖时窗口逐次翻倍的上限（秒） |
| `RELAY_FLAP_WINDOW` | `600` | 升档后这段时间内又拥塞 = 这一档撑不住，窗口翻倍 + 封顶（秒） |
| `RELAY_DEGRADE_WIDTH` / `_HEIGHT` / `_FPS` / `_BITRATE` | `640` / `360` / `12` / `500k` | 省流档参数 |
| `RELAY_DEGRADE2_WIDTH` / `_HEIGHT` / `_FPS` / `_BITRATE` | `320` / `180` / `8` / `200k` | 极省流档（4G / 弱 Wi-Fi：链路比 500k 还小时用） |
| `RELAY_CONGEST_WINDOW` | `5` | 码率 / send 占用的统计窗口（秒） |
| `RELAY_CONGEST_DUTY` | `0.5` | 窗口内 `ws.send` 占用超过它算「被反压」 |
| `RELAY_CONGEST_BPS_RATIO` | `0.9` | 实测码率低于目标档位的这个比例（配合上一项）即判拥塞、降档 |
| `RELAY_RECOVER_DUTY` / `_STABLE` | `0.2` / `20` | send 占用持续低于它 `_STABLE` 秒才算链路恢复，可以试升一档 |
| `RELAY_PROBATION` / `_LAG` | `45` / `0.6` | 升档试用期：这么久内积压超过 `_LAG` 秒就退回上一档 |
| `RELAY_MAX_LAG` | `3` | 帧在管道里积压超过这么多秒就重拉 ffmpeg 重新对齐时间轴（延迟看门狗） |
| `RELAY_LAG_LOG` | `1` | 延迟超过它就每 30s 记一行 `画面延迟 x.xs`（线上量延迟看这个） |
| `RELAY_LAG_RESTART_MIN` | `20` | 两次「延迟重同步」的最小间隔（秒），防抖 |
| `RELAY_LAG_DEGRADE` / `_HOLD` / `_DEBOUNCE` | `1.2` / `10` / `30` | 积压持续上涨就降一档（比 `RELAY_MAX_LAG` 的硬重同步早得多） |
| `RELAY_STOP_GRACE` | `5` | 停流时等协程退出的上限；超时就掐连接（否则 session 会僵死） |

## 平台默认档位（页面配置）

平台「系统设置 → 视频中继」里可以选**默认档位**（`light` / `degrade` / `degrade2`）和是否允许
**自动降档**。第一个观看者进来时平台在 `video_start.payload.profile` 里下发：

```json
{"tier": "degrade", "auto": true}
```

板端把它当作「配置档」：`tier` 决定起点（`degrade` = 直接从省流档起，4G 主用），`auto=false`
则锁死在这一档（不再自动降档，只保留 `send` 超时的断线重连保护）。页面改了设置会再广播
`video_config`，正在看的流就地换档（观看端不重连）。**没有这个字段时**（老平台 / 手工运行）
沿用下面的 `RELAY_PROFILE` + `RELAY_*` 环境变量，行为与以前完全一致。

## 自适应档位

```text
配置档 720p15/1000k ⇄ 省流档 640x360@12/500k ⇄ 极省流档 320x180@8/200k
```

平台下发的默认档位就是这里的「配置档」：默认 `degrade` 时起点是省流档，还能再降一档到
极省流档（不会再往上升）。

**降档**（三档，一次降一档）：窗口内实测码率明显低于目标（`RELAY_CONGEST_*`）、
某个分片 `send` 超时，或**积压持续上涨**（`RELAY_LAG_DEGRADE`，出帧跟不上这一档）
→ 发 `video_reset` + 重启 `ffmpeg` 换档，观看端不用重连。

**升档**（回档）只升**一档**，而且必须同时满足：窗口到期、有人在看、`send` 占用
持续低于 `RELAY_RECOVER_DUTY`、当前档实测码率打得到本档目标；升上去之后进入
`RELAY_PROBATION` 试用期，积压超过 `RELAY_PROBATION_LAG` 就**立刻退回上一档**。
撑不住的档位记为 `_ceiling_level`，在 `hold_seconds()` 内不再尝试。

为什么这么小心（2026-11-18 现场）：升档以前是判断「send 占用 ≤20%」直接跳回配置档。
可是在极省流 200k 上 send 占用天然是 0%（根本没东西可发），于是被判「链路已恢复」
→ 回 1000k → 板子出帧只有 ~91%（rkmpp CBR 靠丢帧保码率）→ 积压以 ~1.6%/s 涨 →
5 分钟涨到 5.3s → 延迟看门狗重启 ffmpeg + 掰连接 → 观众被踢下线重连。
周期约 10 分钟，现场体感就是「平台中继看着看着就卡住了」。

反复升降时窗口翻倍（`RELAY_DEGRADE_TTL` → `_TTL_MAX`）。**没人在看时不升档**：
既没试探成本也没证据（`send 占用 0%` 什么也证明不了），保持当前档位等下一个观众。

## 延迟

`-muxdelay 0 -muxpreload 0 -flush_packets 1`（mux 缓冲归零）。积压延迟用 FLV 视频 tag 的
时间戳量：`lag = 现在墙钟 - (首帧时刻 + tag_ts 增量)`，超过 `RELAY_MAX_LAG`（默认 3s）就
重拉 ffmpeg 重新对齐（`video_reset`，观看端不重连）。日志里 `画面延迟 x.xs` 就是它。
积压**持续上涨**（`RELAY_LAG_DEGRADE`）会先降档——那是「这一档出帧跟不上」，不是丢包。

依赖：python3 + `websockets`（板子上随 FastAPI 生态已装）+ ffmpeg。
"""

import asyncio
import json
import os
import re
import signal
import socket
import subprocess
import sys
import time
from datetime import datetime, timezone

# `websockets` 只在真正连平台时用（`stream_worker` 里局部导入）：档位状态机
# （`test_relay_push.py`）不需要它，导入模块时不强制装好依赖。

PLATFORM = os.environ.get("RELAY_PLATFORM", "http://39.107.55.13:3100").rstrip("/")
STREAMS = [
    item.strip()
    for item in os.environ.get(
        "RELAY_STREAMS", "cam0,cam1,cam2,cam3,dogcam,dog-ipcam,preview"
    ).split(",")
    if item.strip()
]
RTSP_BASE = os.environ.get("RELAY_RTSP_BASE", "rtsp://127.0.0.1:8554").rstrip("/")
ROOM_PREFIX = os.environ.get("RELAY_ROOM_PREFIX", "bp:")
FFMPEG = os.environ.get("RELAY_FFMPEG", "ffmpeg")
# 输出档位。默认 light：平台公网出口往往只有 2~3Mbps（现场实测 2.5Mbps），
# 而 1080p60 的 dogcam 直接 copy 是 4~8Mbps —— 浏览器追不上就被服务端判 lagged
# 踢掉、重连，现场看到的就是「很卡」。缩到 720p20 / 1.2Mbps 后与旧客户端回传
# （15fps / 1.2Mbps）同一量级，现场才稳。
PROFILE = os.environ.get("RELAY_PROFILE", "light").strip().lower()
PROFILE = {"transcode": "light", "copy": "copy", "light": "light"}.get(PROFILE, "light")
# 旧开关 RELAY_TRANSCODE 已废弃：本仓库的 unit 里历史遗留 `RELAY_TRANSCODE=0`，
# 若继续兼容会把默认 light 覆盖回 copy（带宽问题原样复现）。想回到只换容器用
# RELAY_PROFILE=copy；旧变量现在完全忽略。
OUT_WIDTH = int(os.environ.get("RELAY_WIDTH", "1280"))
OUT_HEIGHT = int(os.environ.get("RELAY_HEIGHT", "720"))
OUT_FPS = max(1, int(os.environ.get("RELAY_FPS", "20")))
OUT_BITRATE = os.environ.get("RELAY_BITRATE", "1200k")
OUT_GOP = max(1, int(os.environ.get("RELAY_GOP", str(OUT_FPS * 2))))
ENCODER = os.environ.get("RELAY_ENCODER", "auto").strip()
# 解码器：`auto` = 先试板端硬件解码（rkmpp），不行就用 ffmpeg 默认（软解）
#
# 现场实测（1080p60 源 → 720p15 转码）：软解 ~122% CPU、硬解 ~81%，帧率还略高。
# 这块板子的 CPU/CMA 已经被 4 路 USB 相机常驻编码吃得很紧（见 deploy/README.md
# 「板子会硬复位」），省下来的 CPU 直接换来推流更稳。
DECODER = os.environ.get("RELAY_DECODER", "auto").strip()
# 排障开关：把分片大小、ffmpeg 退出码、完整 stderr 都打出来（生产默认关）
DEBUG = os.environ.get("RELAY_DEBUG", "0") == "1"

# 分片大小：一个 FLV tag 常见几 KB，8KB 兼顾低延时与帧数开销
CHUNK_BYTES = 8 * 1024
# WS 重连退避
MIN_BACKOFF = 1
MAX_BACKOFF = 15

# ---- 上行健康（Wi-Fi 掉线不再把画面冻住） ----------------------------------
#
# 现场事故：板子默认路由/平台 /32 路由都压在 Wi-Fi 上，Wi-Fi 一断，到平台的 TCP 变成
# 「半开」—— send 既不报错也不返回，整条管道被反压到停住，ffmpeg 进程还活着。
# 平台看到推流端「在线」但一个字节都没有，观看端画面定格；再进来的人触发 video_start，
# 又因为 `running == True` 被幂等判断吃掉，只能人工重启服务才恢复。
#
# 三件事把它兜住：分片发送上限（判死上行）、卡住自愈（源侧卡住不拖累连接）、
# 出口网卡巡检（Wi-Fi 恢复后旧连接不会自己换回快链路）。
SEND_TIMEOUT = float(os.environ.get("RELAY_SEND_TIMEOUT", "10"))
STALL_TIMEOUT = float(os.environ.get("RELAY_STALL_TIMEOUT", "15"))
ROUTE_POLL = float(os.environ.get("RELAY_ROUTE_POLL", "5"))


def parse_bitrate(value: str) -> int:
    """`1200k` / `1.5M` / `800000` → bps（解析不了返回 0 = 目标未知）。"""
    text = value.strip().lower()
    try:
        if text.endswith("k"):
            return int(float(text[:-1]) * 1000)
        if text.endswith("m"):
            return int(float(text[:-1]) * 1_000_000)
        return int(float(text))
    except ValueError:
        return 0


# ---- 自适应档位 ------------------------------------------------------------
#
# 现场教训：只认「发送超时」（10s 一个分片都写不出去）太晚也太糙 —— 卡顿的绝大部分时间
# 是「还能推，但被反压成 400kbps/7fps」，这时连接是活的、不会触发任何超时，用户看到的
# 就是卡；等真超时了才降档，正好错过了最卡的十几秒，而且还得断线重连才换得下来。
#
# 所以按窗口统计两个量来判断「吃不下」：
#   duty = 窗口内 ws.send 占用的时间比例（被反压就接近 1）
#   bps  = 窗口内实测推出去的码率（被反压就明显低于目标档位）
# 两者同时成立才降档。只判 duty 不够：链路恰好跑满时每个 8KB 分片也会阻塞几十 ms，
# 但画面是流畅的；只判 bps 也不够：`-c:v copy` 没有目标码率可言（此时按 duty 单独判）。
CONGEST_WINDOW = max(1.0, float(os.environ.get("RELAY_CONGEST_WINDOW", "5")))
CONGEST_DUTY = float(os.environ.get("RELAY_CONGEST_DUTY", "0.5"))
CONGEST_BPS_RATIO = float(os.environ.get("RELAY_CONGEST_BPS_RATIO", "0.9"))
RECOVER_DUTY = float(os.environ.get("RELAY_RECOVER_DUTY", "0.2"))
RECOVER_STABLE = float(os.environ.get("RELAY_RECOVER_STABLE", "20"))
RECOVER_POLL = float(os.environ.get("RELAY_RECOVER_POLL", "2"))
# duty 的 EWMA：单窗口的抖动不该立刻决定「链路恢复了没」（Wi-Fi 刚重连时会有一两个空窗口）
DUTY_ALPHA = 0.3

# 停流时等协程退出的上限（秒）：卡在 ws.send 上的协程只能靠掐连接才停得下来，
# 没有上限的 gather 会把整个 session（连同锁）冻死 —— 2026-11-18 事故见 §6.10⑦。
STOP_GRACE = float(os.environ.get("RELAY_STOP_GRACE", "5"))

# 延迟（积压）看门狗：tag 时间戳来自 `-use_wallclock_as_timestamps`（帧到达板端的时刻），
# 它和现在墙钟的差 = 这帧在管道里躺了多久。超过 MAX_LAG 就重拉 ffmpeg 重新对齐时间轴。
MAX_LAG = float(os.environ.get("RELAY_MAX_LAG", "3"))
LAG_LOG = float(os.environ.get("RELAY_LAG_LOG", "1"))
LAG_LOG_INTERVAL = 30.0
LAG_RESTART_MIN = float(os.environ.get("RELAY_LAG_RESTART_MIN", "20"))

# 延迟**增速**也当降档信号（比 `MAX_LAG` 的硬重启早得多）。
#
# 现场 2026-11-18：配置档 720p15/1000k 在板子上出帧只有 ~91%，rkmpp 的 CBR 靠丢帧保
# 码率 → tag 时间戳比墙钟走得慢 → 积压按 ~1.6%/s 涨。`_sample_congestion` 完全看不出来
# （bps 909k vs 目标 1000k = 0.909 > 0.9，duty 也不高），于是一路涨到 5.3s 才被 MAX_LAG
# 重启 —— 这 5 分钟观众看到的就是「越来越卡，然后掉线」。所以「延迟在涨」本身就要降档。
LAG_DEGRADE = float(os.environ.get("RELAY_LAG_DEGRADE", "1.2"))
# 积压要持续这么久才动手：单次抖动（等关键帧、Wi-Fi 轻微排队）不该立刻降档
LAG_DEGRADE_HOLD = float(os.environ.get("RELAY_LAG_DEGRADE_HOLD", "10"))
# 两次「因延迟降档」之间的最小间隔：降档后重新计时，别把刚起的流反复重启
LAG_DEGRADE_DEBOUNCE = float(os.environ.get("RELAY_LAG_DEGRADE_DEBOUNCE", "30"))

# 升档试用期：回档只升一档，升上去之后立刻盯积压，撑不住就退回上一档。
#
# 为什么必须有：`send 占用` 只在「档位高于链路」时才高，档位远低于链路时天然接近 0%，
# 拿它证明「链路恢复到能跑配置档」是假的（2026-11-18：极省流 200k 占用 0% → 回配置档
# 1000k → 5 分钟涨 5.3s 延迟 → 被掐连接）。试用期用「积压增长」当判据，30~45s 就能
# 看出这一档撑不撑得住，代价只有一次就地重启 ffmpeg（观看端不重连）。
PROBATION_SECS = float(os.environ.get("RELAY_PROBATION", "45"))
PROBATION_LAG = float(os.environ.get("RELAY_PROBATION_LAG", "0.6"))

# 省流档：Wi-Fi 断掉只剩 4G（实测 ~0.5Mbps）时，继续按 1200k 推只会每 10s 超时重连一次；
# 降到 500k 至少画面是连续的。降档是全局的（上行共享），到期按链路实际恢复情况决定
# 回不回配置档，反复升降时窗口翻倍。
DEGRADE_TTL = float(os.environ.get("RELAY_DEGRADE_TTL", "300"))
DEGRADE_TTL_MAX = float(os.environ.get("RELAY_DEGRADE_TTL_MAX", "1800"))
FLAP_WINDOW = float(os.environ.get("RELAY_FLAP_WINDOW", "600"))
DEGRADE_WIDTH = int(os.environ.get("RELAY_DEGRADE_WIDTH", "640"))
DEGRADE_HEIGHT = int(os.environ.get("RELAY_DEGRADE_HEIGHT", "360"))
DEGRADE_FPS = max(1, int(os.environ.get("RELAY_DEGRADE_FPS", "12")))
DEGRADE_BITRATE = os.environ.get("RELAY_DEGRADE_BITRATE", "500k")

# 第二档「极省流」：给 4G / 弱 Wi-Fi 用。
#
# 现场（2026-11-18 15:54）：板子出口掉到 4G dongle，实测上行只有 ~0.25Mbps，而省流档
# 目标就是 500kbps —— **链路比目标还小一半**，于是 send 永远 100% 占用、延迟涨到 5.9s、
# 每 40 秒被延迟看门狗重启一次（每次重启都要掐连接、把你踢下线）。
# 再往下一档 320x180@8 / 200k：糊，但能连续出图，不再反复重连。
DEGRADE2_WIDTH = int(os.environ.get("RELAY_DEGRADE2_WIDTH", "320"))
DEGRADE2_HEIGHT = int(os.environ.get("RELAY_DEGRADE2_HEIGHT", "180"))
DEGRADE2_FPS = max(1, int(os.environ.get("RELAY_DEGRADE2_FPS", "8")))
DEGRADE2_BITRATE = os.environ.get("RELAY_DEGRADE2_BITRATE", "200k")
DEGRADE2_BPS = parse_bitrate(DEGRADE2_BITRATE)

# 档位名 → 内部档位序号（0 配置 / 1 省流 / 2 极省流）。名字与平台
# `server/src/video.rs::RELAY_TIERS` 一致：平台页面选什么，`video_start` 就下发什么。
TIER_LEVELS = {"light": 0, "degrade": 1, "degrade2": 2}

# 平台下发的默认档位（`video_start` / `video_config` 的 payload.profile.tier）：
# `None` = 平台没下发，按环境变量 `RELAY_PROFILE` 走（与旧版本行为一致）。
_requested_tier: str | None = None
# 平台是否允许自动降档：`False` = 锁死在平台选的档位（4G 专用 SIM 有固定套餐时可关掉）
_auto_degrade = True
# 「锁定档位不再降档」的日志去重时刻（窗口每 5s 触发一次，别刷屏）
_locked_logged_at = 0.0

# 当前档位：0 = 配置档，1 = 省流档（640x360@12/500k），2 = 极省流档（320x180@8/200k）
#
# 注意这是**绝对档位**，起点由平台下发（`_requested_tier`）或环境变量决定，见 `base_level()`；
# `_degrade_level` 不会再低于起点。跟 `_congested_until` 分开：窗口到期只是「可以开始评估
# 能不能升一档」，链路没恢复就继续待在当前档，不是一到点就自己升回去。
_degrade_level = 0
# 最近一个 5s 窗口实测推出去的码率（bps）：决定「当前这一档是不是也吃不下」→ 要不要再降一档
_last_window_bps = 0.0
# 省流档最早什么时候可以结束
_congested_until = 0.0
# 连续「升一档 → 又拥塞」的次数：决定这次降档要撑多久（2 的幂翻倍）
_flaps = 0
_recovered_at = 0.0
# duty 的 EWMA 与「持续健康」的起点：升档前先确认链路真的闲下来了
_link_duty = 0.0
_healthy_since = 0.0
# 升档试用期：升上去的那一档能不能稳住（见 PROBATION_*）
_probation_level = -1
_probation_from = 0
_probation_until = 0.0
# 已知「这一档及以上吃不下」的封顶：试用期失败 / 刚升档就拥塞时记下来，
# 到期前不再往上试（避免「升 → 涨 5 分钟延迟 → 被掰 → 再升」的循环）
_ceiling_level: int | None = None
_ceiling_until = 0.0
# 没人看时的日志去重（看门狗每 2s 跑一次，别刷屏）
_idle_logged_at = 0.0
# stream -> StreamSession：档位变了要就地重启这些流
_sessions: dict[str, "StreamSession"] = {}

# 起流后这么久一个字节都没有，就判定「ffmpeg 拉不到流」并打印排查提示
NO_DATA_TIMEOUT = 8.0


def base_level() -> int:
    """这一轮起流的起点档位：平台下发的优先，否则用环境变量（配置档 = 0）。"""
    if _requested_tier in TIER_LEVELS:
        return TIER_LEVELS[_requested_tier]
    return 0


def degraded() -> bool:
    """当前是否比起点更低（真的自动降过档）。"""
    return _degrade_level > base_level()


def level_label(level: int | None = None) -> str:
    level = _degrade_level if level is None else level
    return {0: "配置档", 1: "省流档", 2: "极省流档"}.get(level, f"{level} 档")


def tier_label() -> str:
    """起点档位的描述：日志和页面排查时一眼看出「为什么起流就是 640x360」。"""
    base = base_level()
    if _requested_tier is None:
        return f"环境变量 RELAY_PROFILE（{level_label(base)}）"
    auto = "开" if _auto_degrade else "关"
    return f"平台下发 {level_label(base)}，自动降档{auto}"


def hold_seconds() -> float:
    """这次降档至少要撑多久：反复升降时逐次翻倍（上限 DEGRADE_TTL_MAX）。"""
    return float(min(DEGRADE_TTL * (2 ** _flaps), max(DEGRADE_TTL, DEGRADE_TTL_MAX)))


def apply_remote_profile(profile: object) -> bool:
    """把平台下发的默认档位写进状态机；返回是否真的变了。

    `profile` 来自 `video_start` / `video_config` 的 `payload.profile`：

    ```json
    {"tier": "light|degrade|degrade2", "auto": true}
    ```

    缺字段 / 名字不认识时**什么都不改**（老平台只发 `stream`，继续用环境变量）。
    平台重新选了档位 = 旧档位的拥塞窗口 / 封顶 / 试用期全部作废：起点变了，那些结论
    都是按旧起点算的。
    """
    global _requested_tier, _auto_degrade, _degrade_level, _congested_until
    global _flaps, _recovered_at, _probation_level, _probation_from, _probation_until
    global _ceiling_level, _ceiling_until, _healthy_since, _locked_logged_at
    global _last_window_bps, _link_duty
    if not isinstance(profile, dict):
        return False
    tier = str(profile.get("tier") or "").strip().lower()
    if tier not in TIER_LEVELS:
        return False
    auto = profile.get("auto", True)
    if isinstance(auto, str):
        auto = auto.strip().lower() not in ("0", "false", "no", "off")
    else:
        auto = bool(auto)
    if tier == _requested_tier and auto == _auto_degrade:
        return False
    _requested_tier = tier
    _auto_degrade = auto
    _degrade_level = TIER_LEVELS[tier]
    _congested_until = 0.0
    _flaps = 0
    _recovered_at = 0.0
    _probation_level = -1
    _probation_from = 0
    _probation_until = 0.0
    _ceiling_level = None
    _ceiling_until = 0.0
    _healthy_since = 0.0
    _link_duty = 0.0
    _last_window_bps = 0.0
    _locked_logged_at = 0.0
    log(
        f"平台下发中继档位：{level_label(_degrade_level)}"
        f"（{profile_text(active_profile())}，自动降档{'开' if auto else '关'}）"
    )
    return True


def active_profile() -> tuple:
    """当前该用的输出档位：拥塞窗口内按档位走省流 / 极省流。"""
    if _degrade_level >= 2:
        return (DEGRADE2_WIDTH, DEGRADE2_HEIGHT, DEGRADE2_FPS, DEGRADE2_BITRATE)
    if _degrade_level == 1:
        return (DEGRADE_WIDTH, DEGRADE_HEIGHT, DEGRADE_FPS, DEGRADE_BITRATE)
    if PROFILE == "copy":
        return ("copy",)
    return (OUT_WIDTH, OUT_HEIGHT, OUT_FPS, OUT_BITRATE)


def profile_text(profile: tuple) -> str:
    if profile and profile[0] == "copy":
        return "copy（只换容器）"
    return f"{profile[0]}x{profile[1]}@{profile[2]} {profile[3]}"


def profile_bps(profile: tuple) -> int:
    """档位的目标码率（bps）；`copy` 没有目标码率，返回 0 表示「按 send 占用判」。"""
    if profile and profile[0] == "copy":
        return 0
    return parse_bitrate(profile[3])


def note_link_window(duty: float) -> None:
    """记一个窗口的 send 占用，维护「链路已经健康多久」。"""
    global _link_duty, _healthy_since
    _link_duty = _link_duty * (1 - DUTY_ALPHA) + duty * DUTY_ALPHA
    now = time.monotonic()
    if _link_duty <= RECOVER_DUTY:
        if not _healthy_since:
            _healthy_since = now
    else:
        _healthy_since = 0.0


def mark_congested(
    stream: str, reason: str, restream: bool = True, lag: bool = False
) -> None:
    """上行吃不下当前档位：切省流档；已经在省流档就只把窗口往后延。

    `restream=True` 时立刻让正在跑的流换档（发 `video_reset` + 重启 ffmpeg，WS 不动），
    不必再等一次「超时 → 断连 → 重连」—— 那中间的十几秒正是最卡的时候。

    `lag=True`：拥塞的证据是「积压持续上涨」（出帧跟不上），不是 send 反压。
    这种情况下不能再拿「实测码率还在目标附近」当「这一档还能跑」—— rkmpp 的 CBR
    正是靠丢帧保码率，码率看着没掉，画面却在一路掉队。
    """
    global _degrade_level, _congested_until, _flaps, _recovered_at
    global _ceiling_level, _ceiling_until, _locked_logged_at
    now = time.monotonic()
    # 刚升到的那一档又吃不下 = 这一档撑不住：记成封顶，到期前不再自己升回去
    # （否则就是「升 → 涨 5 分钟延迟 → 被掰 → 再升」的循环，2026-11-18 现场每 10 分钟一轮）
    failed = _degrade_level
    base = base_level()
    just_upgraded = bool(_recovered_at) and now - _recovered_at < FLAP_WINDOW
    _recovered_at = 0.0
    if failed >= 2:
        # 已经在最低档：只续窗口，不重复降档/重启
        _congested_until = max(_congested_until, now + hold_seconds())
        return
    if failed > base and not lag and _current_level_is_workable():
        # 当前这一档还能跑，只是链路状态仍差：只续窗口，不重复降档/重启
        _congested_until = max(_congested_until, now + hold_seconds())
        return
    if not _auto_degrade:
        # 平台锁定档位：不降档、不重启（`send` 超时仍会断开重连，那是链路已死）
        _congested_until = max(_congested_until, now + hold_seconds())
        if now - _locked_logged_at >= 60:
            _locked_logged_at = now
            log(
                f"[{stream}] {reason}，但平台已锁定{level_label(failed)}"
                "（自动降档关闭），保持当前档位"
            )
        return
    target = failed + 1
    if just_upgraded:
        _flaps = min(_flaps + 1, 5)
        _ceiling_level = failed
        _ceiling_until = now + hold_seconds()
    elif failed == base:
        _flaps = 0
    _degrade_level = target
    _congested_until = now + hold_seconds()
    log(
        f"[{stream}] {reason} → 改用{level_label()} {profile_text(active_profile())}"
        f"（至少保持 {hold_seconds():g}s）"
    )
    if restream:
        restream_all(f"降档到{level_label()}")


def _current_level_is_workable() -> bool:
    """当前档位的码率还推得出去吗？

    用最近一个窗口的实测值判断：明显低于本档目标（<90%）就是「这一档也吃不下」，
    该再降一档。没量到（0）时按「还能跑」处理，不冒进。
    """
    target = profile_bps(active_profile())
    if target <= 0 or _last_window_bps <= 0:
        return True
    return _last_window_bps >= target * CONGEST_BPS_RATIO


def probation_active() -> bool:
    """当前是否在「刚升了一档，正在试」的阶段。"""
    return _probation_until > 0


def start_probation(level: int, previous: int) -> None:
    global _probation_level, _probation_from, _probation_until
    _probation_level = level
    _probation_from = previous
    _probation_until = time.monotonic() + PROBATION_SECS


def expire_probation() -> None:
    """试用期到点且没有被退回 = 这一档稳住了。"""
    global _probation_until, _probation_level, _congested_until
    if not _probation_until:
        return
    now = time.monotonic()
    if now < _probation_until:
        return
    log(
        f"{level_label(_probation_level)} 试用期通过"
        f"（{PROBATION_SECS:g}s 内没有积压，{profile_text(active_profile())}）"
    )
    _probation_until = 0.0
    _probation_level = -1
    if degraded():
        # 还想继续往上试？按窗口节奏来，不是刚试完就再升一档
        _congested_until = now + hold_seconds()


def step_up_allowed(level: int, now: float | None = None) -> bool:
    """能不能升到这一档（`_ceiling_level` 是最近试过、已知撑不住的那一档）。

    档位数字越小画质越高（0 配置 / 1 省流 / 2 极省流）。“比它好”的档位同样要挡：
    省流档都撑不住时，配置档更不可能撑住。
    """
    if level < 0:
        return False
    now = time.monotonic() if now is None else now
    if _ceiling_level is not None and level <= _ceiling_level and now < _ceiling_until:
        return False
    return True


def relapse(stream: str, reason: str, now: float | None = None) -> bool:
    """升上去的这一档撑不住：立刻退回上一档，并把它记成「短期别再试」。

    比等 `MAX_LAG` 的硬重同步早得多，而且**不进退化循环**——退回的是上一档（已经
    验证过能跑），不是直接回配置档再降。
    """
    global _degrade_level, _probation_until, _probation_level, _probation_from
    global _ceiling_level, _ceiling_until, _congested_until, _flaps
    if not _probation_until:
        return False
    now = time.monotonic() if now is None else now
    failed = _probation_level
    _degrade_level = _probation_from
    _probation_until = 0.0
    _probation_level = -1
    _flaps = min(_flaps + 1, 5)
    hold = hold_seconds()
    _congested_until = now + hold
    _ceiling_level = failed
    _ceiling_until = now + hold
    log(
        f"[{stream}] {reason} → 退回{level_label()} {profile_text(active_profile())}"
        f"（{hold:g}s 内不再试{level_label(failed)}）"
    )
    restream_all(f"退回{level_label()}")
    return True


def step_up_profile() -> bool:
    """链路看起来恢复了：**一次只升一档**（2→1→0），升完进试用期。

    以前是直接 `_degrade_level = 0`：极省流 200k 时 send 占用天然是 0%，被当成
    「链路满了 → 回配置档 1000k」，结果板子出帧跟不上，5 分钟积压到 5.3s 再被掰。
    一档一档升 + 试用期，最坏情况也只是「一档高了一点点，几十秒内退回」；
    再加上 `_ceiling_level`，同一个坑不会每 10 分钟踩一次。
    """
    global _degrade_level, _congested_until, _recovered_at
    now = time.monotonic()
    if _degrade_level <= base_level():
        return False
    target = _degrade_level - 1
    if not step_up_allowed(target, now):
        hold = hold_seconds()
        _congested_until = now + hold
        log(
            f"省流窗口到期，但{level_label(target)}已知撑不住，"
            f"继续待在{level_label()}（{hold:g}s 后再试）"
        )
        return False
    previous = _degrade_level
    _degrade_level = target
    _congested_until = 0.0
    _recovered_at = now
    start_probation(target, previous)
    log(
        f"上行看起来恢复了（send 占用 {_link_duty * 100:.0f}%），升一档到{level_label()}"
        f" {profile_text(active_profile())}，{PROBATION_SECS:g}s 试用期"
        f"（积压超过 {PROBATION_LAG:g}s 就退回）"
    )
    restream_all(f"升一档到{level_label()}")
    return True


def restream_all(reason: str) -> None:
    """档位变了：让正在跑的流就地重启 ffmpeg 换参数（WS 保留，观看端不用重连）。"""
    for session in list(_sessions.values()):
        if not session.running:
            continue
        try:
            asyncio.get_running_loop().create_task(session.restart_for_profile(reason))
        except RuntimeError:  # 没有事件循环（被当模块导入时）：什么都不用做
            pass


async def profile_check() -> None:
    """省流窗口到期：链路真恢复了才**升一档**，否则继续省流（不盲目到点就升回去）。"""
    global _congested_until, _idle_logged_at, _healthy_since
    expire_probation()
    if not degraded():
        return
    now = time.monotonic()
    if now < _congested_until:
        return
    running = any(session.running for session in _sessions.values())
    if not running:
        # 没人在看：不下发试探（不占带宽、不闪），但也**不盲回配置档**。
        #
        # 2026-11-18 就是在这里把「send 占用 0%」当成「链路恢复了」——没人在看的 0%
        # 什么都没证明；而极省流 200k 时它天然就是 0%。保持当前档位，等真有人看、
        # 并且真的能跑满时再一档一档往上试。
        _congested_until = now
        _healthy_since = 0.0  # 没有窗口统计，别拿上一次观看时的记录判「健康」
        if now - _idle_logged_at >= 60:
            _idle_logged_at = now
            log(f"没人看：继续待在{level_label()}，有人看时再评估要不要升档")
        return
    # 有人在看：要 send 占用真的下来了才算健康
    healthy = _healthy_since > 0 and now - _healthy_since >= RECOVER_STABLE
    if not healthy:
        _congested_until = now + hold_seconds()
        log(
            f"省流窗口到期但链路仍拥塞（send 占用 {_link_duty * 100:.0f}%），"
            f"再省流 {hold_seconds():g}s"
        )
        return
    if not _current_level_is_workable():
        # 当前这一档自己都打不满目标码率：再往上试只会重演「越看越卡」
        _congested_until = now + hold_seconds()
        log(
            f"省流窗口到期但实测 {_last_window_bps / 1000:.0f}kbps"
            f"（本档目标 {profile_bps(active_profile()) / 1000:.0f}kbps），"
            f"不升档，再省流 {hold_seconds():g}s"
        )
        return
    step_up_profile()


async def profile_watcher() -> None:
    """每 `RELAY_RECOVER_POLL` 秒检查一次省流窗口要不要结束。"""
    while True:
        await asyncio.sleep(max(0.5, RECOVER_POLL))
        try:
            await profile_check()
        except Exception as exc:  # noqa: BLE001 - 这个巡检循环不能死
            log(f"档位巡检异常：{exc!r}")


def platform_host() -> str:
    """`RELAY_PLATFORM` 里的主机名（不带 scheme / 端口 / 路径）。"""
    return PLATFORM.split("://", 1)[-1].split("/", 1)[0].split(":", 1)[0]


_route_target: str | None = None


def platform_ip() -> str | None:
    """平台 IP（域名只解析一次，失败就本次跳过巡检）。"""
    global _route_target
    if _route_target:
        return _route_target
    host = platform_host()
    if not host:
        return None
    if re.fullmatch(r"[0-9.]+", host):
        _route_target = host
        return host
    try:
        info = socket.getaddrinfo(host, None, proto=socket.IPPROTO_TCP)
        _route_target = info[0][4][0]
    except OSError as exc:  # noqa: BLE001 - 解析失败就关掉巡检，不影响推流
        log(f"解析平台域名 {host} 失败：{exc!r}（跳过出口网卡巡检）")
        return None
    return _route_target


def egress_iface() -> str | None:
    """到平台走哪张网卡（`ip route get <平台IP>` 的 dev）；查不到返回 None。"""
    target = platform_ip()
    if not target:
        return None
    try:
        proc = subprocess.run(
            ["ip", "route", "get", target], capture_output=True, text=True, timeout=3
        )
    except Exception:  # noqa: BLE001 - 没有 ip 命令 / 超时都当查不到
        return None
    if proc.returncode != 0:
        return None
    match = re.search(r"\bdev\s+(\S+)", proc.stdout)
    return match.group(1) if match else None


async def close_quietly(ws) -> None:
    """尽量关掉一条 WS；死链路上连关握手都会挂，超时就直接掐底层连接。"""
    try:
        await asyncio.wait_for(ws.close(), 3)
    except Exception:  # noqa: BLE001
        try:
            transport = getattr(ws, "transport", None)
            if transport is not None:
                transport.abort()
        except Exception:  # noqa: BLE001
            pass


# stream -> 当前推流连接。出口网卡变化时要主动断开它们（重连才会走新网卡）
_connections: dict[str, object] = {}


async def route_watcher() -> None:
    """出口网卡一变就重连。

    Wi-Fi 断开 / 恢复、4G dongle 顶上来、机器人网卡抢默认路由，都会换出口；而**已经建立
    的 TCP 不会自己换**（源地址/网关已经绑死）。不主动断开的话，Wi-Fi 恢复后推流还挂在
    慢速备用链路上，用户看到的就是「Wi-Fi 好了还是很卡」。
    """
    if ROUTE_POLL <= 0:
        return
    last = egress_iface()
    log(f"平台出口网卡：{last or '未知'}（每 {ROUTE_POLL:g}s 巡检，变更即重建连接）")
    while True:
        await asyncio.sleep(ROUTE_POLL)
        current = egress_iface()
        if current is None or current == last:
            continue
        log(f"平台出口网卡 {last or '未知'} → {current}，重建全部推流连接")
        last = current
        for stream, ws in list(_connections.items()):
            log(f"[{stream}] 出口变更，断开旧连接")
            await close_quietly(ws)


def endpoint() -> str:
    base = PLATFORM
    if base.startswith("https://"):
        base = "wss://" + base[len("https://") :]
    elif base.startswith("http://"):
        base = "ws://" + base[len("http://") :]
    return f"{base}/ws/device/video"


def log(message: str) -> None:
    # systemd 收 stdout 进 journal，这里不做额外缓冲
    print(f"[relay] {message}", flush=True)


_encoder_pick: str | None | bool = False


def _double_bitrate(value: str) -> str:
    """`1200k` → `2400k`；带 M 后缀或纯数字时原样双倍，解析不了就不设 buffer。"""
    text = value.strip()
    try:
        if text.lower().endswith("k") and text[:-1].isdigit():
            return f"{int(text[:-1]) * 2}k"
        if text.lower().endswith("m"):
            return f"{int(float(text[:-1]) * 2)}M"
        return f"{int(float(text) * 2)}"
    except ValueError:
        return value


def pick_encoder() -> str | None:
    """挑 H.264 编码器：板子是瑞芯微就 rkmpp，普通 x86 就 libx264。

    现场教训：机器狗 / 背包那套定制 ffmpeg 通常**没有 libx264**，写死 libx264 会直接
    `Unknown encoder` 起不来流；只有 rel/ik 系列才有 rkmpp。所以运行时探测一次。
    """
    global _encoder_pick
    if _encoder_pick is not False:
        return _encoder_pick  # type: ignore[return-value]
    if ENCODER and ENCODER != "auto":
        _encoder_pick = ENCODER
        return ENCODER
    chosen: str | None = None
    try:
        out = subprocess.run(
            [FFMPEG, "-hide_banner", "-encoders"],
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout
        listed = [name for name in ("h264_rkmpp", "h264_v4l2m2m", "libx264") if name in out]
        for candidate in listed:
            # 编译进来 ≠ 能用：板子上 v4l2m2m / rkmpp 都可能没设备节点，
            # 直接拿 1 帧黑图试编，失败就换下一个（否则每次起流都是「0 字节 + 退出」）。
            if _encoder_works(candidate):
                chosen = candidate
                break
        if chosen is None and listed:
            log(f"警告：{listed} 都在编码器列表里但试编失败，退回 -c:v copy")
    except Exception as exc:  # noqa: BLE001 - 探测失败就当没有，退回 copy
        log(f"编码器探测失败：{exc!r}")
    _encoder_pick = chosen
    return chosen


# 每条流各自的解码器判定：`{stream: decoder|None}`（None = ffmpeg 默认软解）。
#
# ⛔ 不能做成全局缓存：板子上 `h264_rkmpp` 解本体 1080p（dogcam）没问题，解四路 USB
# 相机（cam0~cam3，同样是本机 rkmpp 编出来的 720p）却**一帧都解不出来** —— ffmpeg
# 挂在 `-frames:v 2` 上永远等不到帧。原来只探测「第一条进来的流」并把结论套给所有流，
# 于是 dogcam 探测通过后 cam* 也拿到 h264_rkmpp → 每 15s 被卡住看门狗重启一次、已推
# 0 字节，前端「平台中继」的 USB 相机通道永远没画面（2026-11-18 事故，见
# business/docs/backpack-video-integration.md §6.10⑪）。
#
# 也不在起流路径上同步探测：probe 是同步 subprocess，解不出来的流要等超时，会把事件
# 循环卡住几秒，连带正在推的其他流 `ws.send` 阻塞（看门狗会误判上行拥塞去降档）。
# 所以：**没探测过的流先用软解**（永远能用），开机在后台逐条探一次，结果留给下次起流。
_decoder_picks: dict[str, str | None] = {}
DECODER_PROBE_TIMEOUT = float(os.environ.get("RELAY_DECODER_PROBE_TIMEOUT", "6"))


def pick_decoder(stream: str) -> str | None:
    """这条流该用哪个解码器；`None` = 让 ffmpeg 自己用软解。

    `RELAY_DECODER` 写死（例如 `h264_rkmpp`）或 `sw` 时直接照办，不探测、不缓存。
    没探测过的流返回 None（软解）—— 起流路径上不许阻塞，见上面的说明。
    """
    if DECODER == "sw":
        return None
    if DECODER and DECODER != "auto":
        return DECODER
    return _decoder_picks.get(stream)


def probe_decoder(stream: str) -> str | None:
    """探测一条流的解码器并缓存（同步、可能阻塞几秒，只从 executor 里调）。

    跟 `pick_encoder` 一样要**真解出像素**才算数：板子上解码器可能编排在列表里但没设备，
    也可能列表里有、设备也在，但对某一路源就是解不出来（cam0~3）。
    """
    if DECODER == "sw":
        _decoder_picks[stream] = None
        return None
    if DECODER and DECODER != "auto":
        _decoder_picks[stream] = DECODER
        return DECODER
    if stream in _decoder_picks:
        return _decoder_picks[stream]
    chosen: str | None = None
    try:
        listed = subprocess.run(
            [FFMPEG, "-hide_banner", "-decoders"], capture_output=True, text=True, timeout=10
        ).stdout
        if "h264_rkmpp" in listed and _decoder_works("h264_rkmpp", stream):
            chosen = "h264_rkmpp"
    except Exception as exc:  # noqa: BLE001 - 探测失败就当没有，退回软解
        log(f"[{stream}] 解码器探测失败：{exc!r}（用 ffmpeg 默认软解）")
    log(f"[{stream}] 解码器：{chosen or 'ffmpeg 默认（软解）'}")
    _decoder_picks[stream] = chosen
    return chosen


async def warm_decoders() -> None:
    """开机后台逐条流探一次解码器（可用的 ~1s/条，解不出来的等探测超时才放弃）。"""
    if DECODER == "sw" or (DECODER and DECODER != "auto"):
        return
    loop = asyncio.get_event_loop()
    for stream in STREAMS:
        try:
            await loop.run_in_executor(None, probe_decoder, stream)
        except Exception as exc:  # noqa: BLE001 - 预热失败不影响推流
            log(f"[{stream}] 解码器预热失败：{exc!r}（继续用软解）")


def _decoder_works(name: str, stream: str) -> bool:
    """拿真实源解两帧：**真的解出像素**才算可用。

    只看 `returncode == 0` 不可靠：一帧都没解出来时 ffmpeg 只打一行
    `Nothing was written into output file…` 也可能返回 0。所以用 rawvideo 数输出
    字节，并给探测本身兜个超时 —— 解不出来的解码器不是报错退出，而是永远等不到帧。
    """
    try:
        proc = subprocess.run(
            [
                FFMPEG,
                "-hide_banner",
                "-loglevel",
                "error",
                "-rtsp_transport",
                "tcp",
                "-timeout",
                "5000000",
                "-c:v",
                name,
                "-i",
                f"{RTSP_BASE}/{stream}",
                "-frames:v",
                "2",
                # rawvideo 落 stdout：2 帧全尺寸像素至少几 KB，0 字节 = 一帧都没解出来
                "-f",
                "rawvideo",
                "-",
            ],
            capture_output=True,
            timeout=DECODER_PROBE_TIMEOUT,
        )
        return proc.returncode == 0 and len(proc.stdout) > 0
    except Exception:  # noqa: BLE001 - 超时 / 起不来一律当不可用
        return False


def _encoder_works(name: str) -> bool:
    """拿一帧黑图试编，确认编码器在这台机器上真的可用。"""
    try:
        proc = subprocess.run(
            [
                FFMPEG,
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=320x240:r=5",
                "-frames:v",
                "2",
                "-c:v",
                name,
                "-f",
                "null",
                "-",
            ],
            capture_output=True,
            timeout=15,
        )
        return proc.returncode == 0
    except Exception:  # noqa: BLE001
        return False


def ffmpeg_args(stream: str) -> list[str]:
    profile = active_profile()
    # 「低档」按绝对档位判：平台把起点设成省流档（1）时也要走低档的 GOP 与重编码参数
    low = _degrade_level > 0
    source = f"{RTSP_BASE}/{stream}"
    args = [
        FFMPEG,
        "-hide_banner",
        "-loglevel",
        "warning",
        "-nostdin",
        # 现场 RTSP 走 TCP 更稳（UDP 丢包花屏）
        "-rtsp_transport",
        "tcp",
        # 源侧卡住（相机/本体不吐数据）时 socket 读不能无限等：8s 没数据就让 ffmpeg
        # 报错退出，外层 watchdog 重启拉流（否则进程活着、画面永远定格）
        "-timeout",
        "8000000",
        # 用到达时间当 PTS：`-c:v copy` 时源包的 PTS 可能没带过来，
        # 不补时间戳 FLV 就整条时间轴为 0，浏览器 MSE 会卡在 readyState=0 不出画面
        "-use_wallclock_as_timestamps",
        "1",
        "-fflags",
        "+genpts",
    ]
    decoder = pick_decoder(stream)
    if decoder:
        args += ["-c:v", decoder]
    args += [
        "-i",
        source,
        "-an",  # 背包各流都没有音轨，FLV 里带空音轨会让 flv.js 报解析错
    ]
    # 只有 `RELAY_PROFILE=copy` 才只换容器。降档时**必须**重编码 —— copy 改不了码率，
    # 1080p60 会原样推 4~8Mbps（2026-11-18 事故：降档判断对了，但这里 `or low` 把
    # encoder 也置成了 None → 降档后反而推 8Mbps，浏览器全部 lagged）。
    encoder = None if profile[0] == "copy" else pick_encoder()
    if encoder is None:
        # 只换容器：源已经是 H.264 且码率不高时才该走这里
        args += ["-c:v", "copy"]
    else:
        width, height, fps, bitrate = profile
        # light：先缩后编。`force_original_aspect_ratio=decrease` 只缩不放，
        # 所以 640x360 的 preview 不会被放大成 720p（白吃码率与 CPU）。
        args += [
            "-vf",
            f"scale={width}:{height}:force_original_aspect_ratio=decrease,fps={fps}",
            "-c:v",
            encoder,
            "-pix_fmt",
            "yuv420p",
            "-b:v",
            bitrate,
            # 不用 B 帧：少一层重排延迟，也少占一份编码缓冲（板子 CMA 只有 16MB，
            # 4 路 USB 相机常驻编码 + 本路转码已经把它吃满，日志一直刷
            # `cma: cma_alloc ... failed`；`-bf 0 -refs 1` 是这里最省的配置）
            "-bf",
            "0",
            "-refs",
            "1",
            "-g",
            # 关键帧间隔按实际帧率走 2s；显式配了 RELAY_GOP 时以配置为准（省流档除外）
            str(OUT_GOP if not low else max(1, fps * 2)),
        ]
        if encoder == "libx264":
            args += [
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-profile:v",
                "baseline",
                "-maxrate",
                bitrate,
                "-bufsize",
                _double_bitrate(bitrate),
            ]
    # 低延迟：`-flush_packets 1` 每个包立刻吐；`-muxdelay/-muxpreload` 默认是 0.7s/0.5s 的
    # mux 缓冲（文档里一直写着这两个参数，代码里其实一直漏着）→ 白送 0.5~1.2s 延迟。
    args += [
        "-muxdelay",
        "0",
        "-muxpreload",
        "0",
        "-f",
        "flv",
        "-flush_packets",
        "1",
        "pipe:1",
    ]
    return args


class FlvScanner:
    """从 ffmpeg 的 FLV 字节流里取视频 tag 的时间戳（只解析 tag 头，跳过本体）。

    有了它才能量「这一帧从进 ffmpeg 到现在多久了」：tag 时间戳来自
    `-use_wallclock_as_timestamps`（输入到达时刻），和墙钟的差就是积压延迟。
    分片边界会切断 tag 头 / 本体，所以状态要跨分片保存。
    """

    MAX_TAG = 32 * 1024 * 1024

    def __init__(self) -> None:
        self._head = bytearray()
        self._skip = 0           # 还没跳过的 tag 本体 + PreviousTagSize 字节数
        self._started = False
        self.broken = False      # 解析错位（不是 FLV / size 离谱）就彻底停用，别吐垃圾
        self.last_video_ts: int | None = None

    def feed(self, chunk: bytes) -> None:
        if self.broken:
            return
        if not self._started:
            # 文件头（9 + 4 字节）本身也可能被分片切开，先攒够再说
            self._head += chunk
            if len(self._head) < 13:
                return
            if self._head[:3] != b"FLV":
                self.broken = True
                self._head.clear()
                return
            rest = bytes(self._head[13:])
            self._head.clear()
            self._started = True
            if rest:
                self.feed(rest)
            return
        n = len(chunk)
        pos = 0
        while pos < n:
            if self._skip:
                take = min(self._skip, n - pos)
                self._skip -= take
                pos += take
                continue
            need = 11 - len(self._head)
            take = min(need, n - pos)
            self._head += chunk[pos : pos + take]
            pos += take
            if len(self._head) < 11:
                return
            size = int.from_bytes(self._head[1:4], "big")
            if size <= 0 or size > self.MAX_TAG:
                self.broken = True
                self._head.clear()
                return
            if self._head[0] == 9:  # video tag
                self.last_video_ts = int.from_bytes(self._head[4:7], "big") | (
                    self._head[7] << 24
                )
            self._skip = size + 4  # tag body + PreviousTagSize
            self._head.clear()


class StreamSession:
    """一路流的拉取 + 推送（起停由平台的 video_start / video_stop 驱动）"""

    def __init__(self, stream: str) -> None:
        self.stream = stream
        self.proc: asyncio.subprocess.Process | None = None
        self.pump: asyncio.Task | None = None
        self.stderr: asyncio.Task | None = None
        self.watch: asyncio.Task | None = None
        self.bytes_sent = 0
        self.chunks = 0
        # 最后一次真正写进 WS 的时刻：卡住判定（源不吐字节）用它
        self.last_send = 0.0
        # 当前这条流用的档位（降档/回档时对比它，决定用不用重启 ffmpeg）
        self.profile: tuple | None = None
        # 推流用的 WS：降档/回档要就地重启 ffmpeg，不需要重新建连接
        self.ws = None
        # 延迟统计：ffmpeg 起流时刻（墙钟 + 单调钟）+ FLV tag 时间戳扫描器
        self._started_wall = 0.0
        self._started_mono = 0.0
        self._flv = FlvScanner()
        self.lag = 0.0
        # 延迟基准：**首帧**到达时的时间和它自己的 tag 时间戳
        #
        # 只用这两个算「积压增长」：起流那一下要等 RTSP 握手 + 首个关键帧（0.5~2s，取决于
        # 运气），把它算进延迟会让看门狗一起流就误判超限 —— 2026-11-18 15:19 就这么
        # 45 秒重启一次的。起流耗时单独打一行日志，不进指标。
        self._lag_ref: tuple[float, int] | None = None
        self._lag_logged_at = 0.0
        self._lag_restart_at = 0.0
        # 延迟降档：积压超过 LAG_DEGRADE 且持续 LAG_DEGRADE_HOLD 才动手，动手后
        # LAG_DEGRADE_DEBOUNCE 内不再动（降档会重启本路的 ffmpeg）
        self._lag_degrade_since = 0.0
        self._lag_degrade_at = 0.0
        # 停流时连接被掐断（协程卡在 ws.send 上停不下来）：别在这条连接上重拉
        self._conn_broken = False
        # 拥塞统计窗口：send 占用的累计时长 / 推出去的字节
        self._win_at = 0.0
        self._win_blocked = 0.0
        self._win_bytes = 0
        # 主动停流（video_stop / 断链清理）时置 True：推流协程退出时据此判断
        # 「是我自己关的」，不要顺手把 WS 也关掉
        self._stopping = False
        self._lock = asyncio.Lock()

    @property
    def running(self) -> bool:
        return self.proc is not None and self.proc.returncode is None

    async def start(self, ws, profile: object = None) -> None:
        """平台要求起流。`profile` = `payload.profile`（默认档位 + 是否允许自动降档）。

        老平台只发 `stream`，这时 `profile=None`，继续按环境变量走；新平台每次都会带上
        当前设置 —— 观看期间在页面上改了档位，下次 `video_start` 也能收敛。
        """
        changed = apply_remote_profile(profile)
        async with self._lock:
            # 幂等：已经在推就不重启（重复 video_start 不能黑 3~7s，也不能换新 FLV 头）。
            # 但如果平台这次的档位跟正在用的不一样，就地换档（观看端不重连）。
            if self.running:
                if changed:
                    log(f"[{self.stream}] 平台更新了中继档位，就地换档")
                    asyncio.create_task(self.restart_for_profile("平台更新了中继档位"))
                return
            await self._start_locked(ws)

    async def _start_locked(self, ws) -> None:
        # 幂等：已经在推就不重启（重复 video_start 不能黑 3~7s，也不能换新 FLV 头）
        if self.running:
            return
        self._stopping = False
        self.bytes_sent = 0
        self.chunks = 0
        self.last_send = time.monotonic()
        self.ws = ws
        self._conn_broken = False
        self._started_wall = time.time()
        self._started_mono = time.monotonic()
        self._flv = FlvScanner()
        self.lag = 0.0
        self._lag_ref = None
        self._lag_degrade_since = 0.0
        self._lag_degrade_at = 0.0
        self._win_at = self.last_send
        self._win_blocked = 0.0
        self._win_bytes = 0
        self.profile = active_profile()
        args = ffmpeg_args(self.stream)
        # 编码器一律从**真实参数**里取：上次的教训是日志写着 enc=h264_rkmpp、
        # 实际 args 里是 `-c:v copy`，把「降档为什么没降下来」藏了两天。
        codec = args[args.index("-c:v") + 1] if "-c:v" in args else "?"
        log(
            f"[{self.stream}] 平台有观看者，开始拉流"
            f"（profile={level_label()}，{profile_text(self.profile)}"
            f" 编码={codec}，出口 {egress_iface() or '未知'}；{tier_label()}）"
        )
        if DEBUG:
            log(f"[{self.stream}] ffmpeg: {' '.join(args)}")
        # 告诉平台「这是一条新流」：常驻连接里每次起流都换新的 FLV 头与新时间戳，
        # 平台必须先丢掉上一轮的流头，否则后加入的观看者会拿到错位的字节（黑屏）
        try:
            await asyncio.wait_for(
                ws.send(json.dumps({"msgType": "video_reset", "payload": {"stream": self.stream}})),
                SEND_TIMEOUT,
            )
        except asyncio.TimeoutError:
            # 这条连接已经死了：不起 ffmpeg，断开让外层重连
            # （restream=False：链路已死，其他流换档会在重连时自己生效）
            mark_congested(
                self.stream, f"video_reset 发不出去（{SEND_TIMEOUT:g}s）", restream=False
            )
            log(f"[{self.stream}] 上行已死（video_reset 发不出去），断开重连")
            await close_quietly(ws)
            return
        self.proc = await asyncio.create_subprocess_exec(
            *args,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        self.pump = asyncio.create_task(self._pump(ws))
        self.stderr = asyncio.create_task(self._drain_stderr())
        self.watch = asyncio.create_task(self._watch_stall(ws))

    async def stop(self, reason: str = "没人看了") -> None:
        async with self._lock:
            await self._stop_locked(reason)

    async def _stop_locked(self, reason: str) -> None:
        if self.proc is None:
            return
        self._stopping = True
        log(f"[{self.stream}] {reason}，停止拉流（已推 {self.bytes_sent} 字节）")
        tasks = [t for t in (self.pump, self.stderr, self.watch) if t and not t.done()]
        self.pump = self.stderr = self.watch = None
        for task in tasks:
            task.cancel()
        if tasks:
            # 必须等上一轮协程真的退出：否则它的 finally 可能在新一轮 start() 之后才跑，
            # 把 `_stopping=False` 误当成「连接该关了」。
            #
            # 但**不能无上限地等**：协程卡在 `ws.send()`（上行拥塞、transport 写不出去）时，
            # `cancel()` 只是打个标记，协程停不下来。2026-11-18 事故就是这里无限等 → 锁被
            # 一直占着，session 变僵尸：日志打了「降档」但档位永远不生效，ffmpeg 被反压着
            # 继续推高码率，观看端越看越延迟，而且后面的 video_stop 也进不来。
            done, pending = await asyncio.wait(tasks, timeout=STOP_GRACE)
            if pending:
                log(
                    f"[{self.stream}] {len(pending)} 个协程 {STOP_GRACE:g}s 没停下"
                    "（多半卡在 ws.send），掐断连接"
                )
                await close_quietly(self.ws)
                await asyncio.wait(pending, timeout=STOP_GRACE)
                self._conn_broken = True
        if self.proc.returncode is None:
            try:
                self.proc.terminate()
                await asyncio.wait_for(self.proc.wait(), timeout=3)
            except (asyncio.TimeoutError, ProcessLookupError):
                # ffmpeg 卡在 RTSP 读上时 terminate 不一定见效
                self.proc.kill()
        self.proc = None

    async def _restart(
        self, ws, reason: str = "卡住，重启拉流", only_if_profile_changed: bool = False
    ) -> None:
        """重启 ffmpeg，但保留 WS（观看端不用重连，平台靠 video_reset 清旧流头）。

        `only_if_profile_changed=True`：档位已经和目标一致就不折腾 —— 降档/回档时用，
        避免把本来就在省流档的流也重启一遍（白闪一下 + 重新等关键帧）。
        """
        try:
            async with self._lock:
                if not self.running:
                    return  # 期间被 video_stop 停了，不能偷偷把流拉起来
                if only_if_profile_changed and active_profile() == self.profile:
                    return
                await self._stop_locked(reason)
                if self._conn_broken:
                    # 连接已经掐了：外层会重连，重连后按新档位起流（别在死 WS 上贴流）
                    log(f"[{self.stream}] 连接已断，交给外层重连后按新档位起流")
                    return
                await self._start_locked(ws)
        except Exception as exc:  # noqa: BLE001 - 重启失败就断开，交给外层重连
            log(f"[{self.stream}] 重启拉流失败：{exc!r}")
            await close_quietly(ws)

    async def restart_for_profile(self, reason: str) -> None:
        """档位变了：就地重启 ffmpeg 换参数（观看端不用重连）"""
        if self.profile is not None and active_profile() == self.profile:
            return  # 已经在这个档位（_restart 里面还会在锁内再确认一次）
        log(
            f"[{self.stream}] {reason}：重启拉流换档位 → "
            f"{profile_text(active_profile())}"
        )
        await self._restart(self.ws, reason, only_if_profile_changed=True)

    def _update_lag(self, now: float) -> None:
        """量「帧在管道里躺了多久」，太久就重拉 ffmpeg 重新对齐（延迟看门狗）。

        和「拥塞降档」是两个问题：上行跑得动、duty 也不高，但 ffmpeg 的输入/编码队列
        积压时，码率看着正常，画面却越看越晚（旧版没有任何东西管这件事）。
        """
        tags = self._flv.last_video_ts
        if tags is None or not self._started_wall:
            return
        if self._lag_ref is None:
            # 首帧：建基准。起流耗时（RTSP 握手 + 等关键帧）单独记，不进积压指标
            self._lag_ref = (now, tags)
            startup = now - self._started_mono
            if startup >= 1.0:
                log(f"[{self.stream}] 起流 {startup:.1f}s 后出首帧（RTSP 握手 / 等关键帧）")
            return
        base_wall, base_ts = self._lag_ref
        self.lag = max(0.0, (now - base_wall) - (tags - base_ts) / 1000.0)
        lag = self.lag
        if lag >= LAG_LOG and now - self._lag_logged_at >= LAG_LOG_INTERVAL:
            self._lag_logged_at = now
            log(f"[{self.stream}] 画面延迟 {lag:.1f}s（帧在管道里积压）")
        # 先试「降一档」，降不下来再靠下面的 MAX_LAG 硬重同步：
        # 硬重同步会重拉 ffmpeg（每次都可能搞丢协程/掰连接），而降档只是就地换参数。
        self._check_lag_growth(now)
        if lag < MAX_LAG or now - self._lag_restart_at < LAG_RESTART_MIN:
            return
        self._lag_restart_at = now
        self._lag_degrade_since = 0.0
        log(
            f"[{self.stream}] 延迟 {lag:.1f}s 超过上限 {MAX_LAG:g}s，"
            "重启 ffmpeg 重新对齐时间轴"
        )
        asyncio.create_task(self._restart(self.ws, f"延迟 {lag:.1f}s 超限"))

    def _check_lag_growth(self, now: float) -> None:
        """积压一直往上涨 = 这一档的出帧跟不上（不是丢包，也不是 send 反压）。

        两种处理：
        - 试用期内（刚升了一档）→ 立刻退回上一档（几十秒内就结束，不等到 3s）；
        - 平时 → 持续 `LAG_DEGRADE_HOLD` 秒后降一档，比 `MAX_LAG` 的硬重启早得多。
        """
        if probation_active() and now < _probation_until:
            # 试用期的阈值（PROBATION_LAG，~0.6s）比平时的 LAG_DEGRADE（~1.2s）低：
            # 刚升到高一点的档位，积压一开始涨就是信号，没必要等到「明显不对」。
            if self.lag >= PROBATION_LAG:
                relapse(self.stream, f"试用期延迟 {self.lag:.1f}s（这一档出帧跟不上）", now)
            else:
                self._lag_degrade_since = 0.0
            return
        if self.lag < LAG_DEGRADE:
            self._lag_degrade_since = 0.0
            return
        if not self._lag_degrade_since:
            self._lag_degrade_since = now
            return
        if now - self._lag_degrade_since < LAG_DEGRADE_HOLD:
            return
        if now - self._lag_degrade_at < LAG_DEGRADE_DEBOUNCE:
            return
        self._lag_degrade_at = now
        mark_congested(
            self.stream,
            f"延迟 {self.lag:.1f}s 持续上涨（{profile_text(active_profile())} 出帧跟不上）",
            lag=True,
        )

    def _sample_congestion(self, now: float) -> bool:
        """按窗口统计「实测码率 / send 占用」，判断上行是不是吃不下当前档位。

        只看 send 耗时不够：链路恰好跑满时每个 8KB 分片也会阻塞几十 ms，画面却是流畅的。
        真正能说明问题的是「send 占用接近 100% **且**实测码率明显低于目标档位」—— 那就是
        ffmpeg 被反压、帧被拖慢，用户看到的就是卡。`copy` 没有目标码率，此时按 send 占用判。
        """
        elapsed = now - self._win_at
        if elapsed < CONGEST_WINDOW:
            return False
        bps = self._win_bytes * 8 / elapsed
        duty = min(1.0, self._win_blocked / elapsed)
        self._win_at = now
        self._win_bytes = 0
        self._win_blocked = 0.0
        note_link_window(duty)
        global _last_window_bps
        _last_window_bps = bps
        target = profile_bps(active_profile())
        congested = duty >= CONGEST_DUTY and (
            target <= 0 or bps <= target * CONGEST_BPS_RATIO
        )
        if congested:
            log(
                f"[{self.stream}] {CONGEST_WINDOW:g}s 内实测 {bps / 1000:.0f}kbps"
                f"（目标 {target / 1000:.0f}kbps，send 占用 {duty * 100:.0f}%）"
            )
        return congested

    async def _watch_stall(self, ws) -> None:
        """ffmpeg 活着但一个字节也产不出来（Wi-Fi 掉线 / 相机卡住）时自己重启。

        以前这里没有看门狗：rtsp 源卡住时 ffmpeg 不报错、不退出，平台看到的是
        「推流端在线、画面不动」，再进来的人触发 video_start 又被幂等判断吃掉。
        """
        interval = max(0.5, STALL_TIMEOUT / 3)
        while self.running:
            await asyncio.sleep(interval)
            if self._stopping or not self.running:
                return
            if time.monotonic() - self.last_send <= STALL_TIMEOUT:
                continue
            log(
                f"[{self.stream}] {STALL_TIMEOUT:g}s 没有推出任何字节"
                "（源卡住 / 上行阻塞），重启拉流"
            )
            asyncio.create_task(self._restart(ws))
            return

    async def _pump(self, ws) -> None:
        """把 ffmpeg 的 FLV stdout 原样灌进 WS（平台按二进制帧转发给浏览器）"""
        proc = self.proc
        assert proc and proc.stdout
        first = True
        try:
            while True:
                chunk = await proc.stdout.read(CHUNK_BYTES)
                if not chunk:
                    break
                if first:
                    first = False
                    if not chunk.startswith(b"FLV"):
                        # 平台要按 FLV 魔数解析流头，不是 FLV 就没法给中途加入的观众补头
                        log(
                            f"[{self.stream}] 警告：输出不是 FLV（可能是 H.265 源），"
                            "把 RELAY_PROFILE 设成 light（重编码成 H.264）"
                        )
                try:
                    # 上行死掉时 send 会永久挂着（TCP 半开：不报错也不返回），
                    # ffmpeg 被反压到停住但进程还在 —— 现场就是「Wi-Fi 断了画面冻住，
                    # 重连也拉不起来」。超时就当链路已死，断开重连。
                    started = time.monotonic()
                    await asyncio.wait_for(ws.send(chunk), SEND_TIMEOUT)
                except asyncio.TimeoutError:
                    mark_congested(
                        self.stream,
                        f"上行阻塞超过 {SEND_TIMEOUT:g}s（Wi-Fi 掉线 / 平台不可达）",
                        restream=False,
                    )
                    log(
                        f"[{self.stream}] 上行阻塞超过 {SEND_TIMEOUT:g}s"
                        "（Wi-Fi 掉线 / 平台不可达），断开重连"
                    )
                    await close_quietly(ws)
                    return
                now = time.monotonic()
                self.last_send = now
                self.bytes_sent += len(chunk)
                self.chunks += 1
                self._win_blocked += now - started
                self._win_bytes += len(chunk)
                self._flv.feed(chunk)
                self._update_lag(now)
                if DEBUG and self.chunks <= 5:
                    log(
                        f"[{self.stream}] chunk#{self.chunks} {len(chunk)}B "
                        f"send {1000 * (now - started):.1f}ms 累计 {self.bytes_sent}B"
                    )
                # 还没到超时那么糟，但已经能看出「推不动了」：立刻降档（restream_all
                # 会重启本路 ffmpeg 换参数）。这里**不能 return** —— 退出 _pump 会走
                # finally 把 WS 关掉，那就变成「断线重连」而不是就地换档了。
                if self._sample_congestion(now):
                    # 降档后仍然拥塞也要上报：`mark_congested` 会判「这一档也吃不下」再降一档
                    mark_congested(self.stream, "上行吃不下当前档位")
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # noqa: BLE001 - 连接断了交给外层重连
            log(f"[{self.stream}] 推流中断：{exc!r}")
        finally:
            # ffmpeg 自己退出（拉不到流 / 源断了）：把退出码打出来，日志里能直接定位
            code = proc.returncode
            log(
                f"[{self.stream}] 推流结束：{self.chunks} 个分片 / {self.bytes_sent}B，"
                f"ffmpeg 退出码 {code}"
            )
            if code not in (None, 0) and self.bytes_sent == 0:
                log(
                    f"[{self.stream}] ffmpeg 退出且没推出任何数据："
                    f"检查 {RTSP_BASE}/{self.stream} 是否在推流"
                )
            if not self._stopping:
                # 不是平台让停的（ffmpeg 挂了 / 上行超时）：把连接也断开，
                # 让外层走重连重试，而不是留一条「在线但没数据」的死连接
                await close_quietly(ws)

    async def _drain_stderr(self) -> None:
        """ffmpeg 的 warning 打在 stderr，不读会堵住管道"""
        proc = self.proc
        assert proc and proc.stderr
        try:
            while True:
                line = await proc.stderr.readline()
                if not line:
                    break
                text = line.decode("utf-8", "replace").strip()
                if text:
                    log(f"[{self.stream}] {text}")
        except (asyncio.CancelledError, ValueError):
            raise


async def stream_worker(stream: str) -> None:
    """一条流一个长连接：注册房间 → 等 video_start / video_stop → 重连退避"""
    import websockets  # 局部导入：状态机可以脱离依赖被测试

    room = f"{ROOM_PREFIX}{stream}"
    session = StreamSession(stream)
    # 登记进去：降档/回档要看有谁在跑（`restream_all` 用）
    _sessions[stream] = session
    backoff = MIN_BACKOFF
    while True:
        try:
            async with websockets.connect(
                endpoint(),
                # 10s 一次心跳：Wi-Fi 掉线时服务端 30s 内就能判死这条推流连接
                # （以前 20s 心跳 + 60s 阈值，画面要靠 1 分钟才自愈）
                ping_interval=10,
                ping_timeout=10,
                max_size=None,
                open_timeout=10,
            ) as ws:
                _connections[stream] = ws
                log(
                    f"[{stream}] 已连接平台，房间 {room}"
                    f"（出口 {egress_iface() or '未知'}）"
                )
                backoff = MIN_BACKOFF
                try:
                    await asyncio.wait_for(
                        ws.send(
                            json.dumps(
                                {
                                    "msgType": "video_register",
                                    # 服务端信封里的 ts 只用于排查日志（缺省也接受，这里给上便于对照）
                                    "ts": datetime.now(timezone.utc).isoformat(),
                                    "payload": {
                                        "deviceId": room,
                                        "kind": "backpack",
                                        "stream": stream,
                                    },
                                }
                            )
                        ),
                        SEND_TIMEOUT,
                    )
                except asyncio.TimeoutError:
                    log(f"[{stream}] 注册消息发不出去（上行已死），断开重连")
                    await close_quietly(ws)
                    raise
                try:
                    async for message in ws:
                        if isinstance(message, (bytes, bytearray)):
                            continue  # 平台不会给推流端发二进制，忽略即可
                        try:
                            payload = json.loads(message)
                        except ValueError:
                            log(f"[{stream}] 平台发来非 JSON 文本：{message[:80]!r}")
                            continue
                        kind = payload.get("msgType")
                        if kind == "video_start":
                            body = payload.get("payload")
                            requested = body.get("profile") if isinstance(body, dict) else None
                            await session.start(ws, requested)
                        elif kind == "video_config":
                            # 页面改了「视频中继」设置：立即生效，不用等下一次起流
                            body = payload.get("payload")
                            requested = body.get("profile") if isinstance(body, dict) else None
                            if apply_remote_profile(requested):
                                restream_all("平台更新了中继档位")
                        elif kind == "video_stop":
                            await session.stop("平台结束观看")
                finally:
                    _connections.pop(stream, None)
                    # 连接断了也要杀掉 ffmpeg，不然没人看还在编码
                    await session.stop("推流连接断开")
        except asyncio.CancelledError:
            _connections.pop(stream, None)
            await session.stop("服务退出")
            raise
        except Exception as exc:  # noqa: BLE001 - 网络抖动一律退避重连
            log(f"[{stream}] 连接平台失败/断开：{exc!r}，{backoff}s 后重连")
        await asyncio.sleep(backoff)
        backoff = min(backoff * 2, MAX_BACKOFF)


async def main() -> int:
    if not STREAMS:
        log("RELAY_STREAMS 为空，没有要中继的流")
        return 2
    log(f"目标平台 {endpoint()}；通道 {', '.join(STREAMS)}；源 {RTSP_BASE}")
    log(
        f"上行保护：分片发送上限 {SEND_TIMEOUT:g}s / 卡住重启 {STALL_TIMEOUT:g}s / "
        f"出口巡检 {ROUTE_POLL:g}s / 省流档 {DEGRADE_WIDTH}x{DEGRADE_HEIGHT}@{DEGRADE_FPS} "
        f"{DEGRADE_BITRATE} / 极省流档 {DEGRADE2_WIDTH}x{DEGRADE2_HEIGHT}@{DEGRADE2_FPS} "
        f"{DEGRADE2_BITRATE}（{CONGEST_WINDOW:g}s 内实测码率 ≤{CONGEST_BPS_RATIO:.0%} 目标且 "
        f"send 占用 ≥{CONGEST_DUTY:.0%} 即降档，至少保持 {DEGRADE_TTL:g}s，"
        f"send 占用 ≤{RECOVER_DUTY:.0%} 持续 {RECOVER_STABLE:g}s 才试升一档，"
        f"升上去 {PROBATION_SECS:g}s 内积压超 {PROBATION_LAG:g}s 就退回）"
    )
    if PROFILE == "copy":
        log("输出档位 copy：只换容器（1080p60 源会原样推 4~8Mbps，公网出口小必卡）")
    else:
        log(
            f"输出档位 light：{OUT_WIDTH}x{OUT_HEIGHT}@{OUT_FPS} {OUT_BITRATE}，"
            f"编码器 {pick_encoder() or '（无可用编码器，运行时退回 copy）'}"
        )
    log(
        f"起点档位：{tier_label()} → {profile_text(active_profile())}"
        "（平台页面「系统设置 → 视频中继」可改默认档位与是否自动降档）"
    )
    tasks = [asyncio.create_task(stream_worker(stream)) for stream in STREAMS]
    # 解码器按流后台探测（不阻塞起流：没探完的流先用软解）
    tasks.append(asyncio.create_task(warm_decoders()))
    tasks.append(asyncio.create_task(route_watcher()))
    # 省流窗口到期后按链路实际情况决定要不要升一档
    tasks.append(asyncio.create_task(profile_watcher()))

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for signal_name in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(signal_name, stop.set)
    await stop.wait()

    log("收到退出信号，停止全部推流")
    for task in tasks:
        task.cancel()
    await asyncio.gather(*tasks, return_exceptions=True)
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
