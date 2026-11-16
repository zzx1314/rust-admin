#!/usr/bin/env python3
"""板端推流服务：按需把背包的 RTSP 流转封装成 FLV，推给业务平台中继。

```text
本地 mediamtx  rtsp://127.0.0.1:8554/<stream>
    └─ ffmpeg -c:v copy -f flv -          只换容器，不重编码（几乎不占 CPU）
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
| `RELAY_STREAMS` | `cam0,cam1,cam2,cam3,dogcam,dog-ipcam` | 要中继的 mediamtx path |
| `RELAY_RTSP_BASE` | `rtsp://127.0.0.1:8554` | 本地 mediamtx |
| `RELAY_ROOM_PREFIX` | `bp:` | 房间前缀，必须与服务端 `video.rs::BACKPACK_ROOM_PREFIX` 一致 |
| `RELAY_FFMPEG` | `ffmpeg` | ffmpeg 路径 |
| `RELAY_TRANSCODE` | `0` | 设 `1` 强制重编码（源不是 H.264 时用，很吃 CPU） |

依赖：python3 + `websockets`（板子上随 FastAPI 生态已装）+ ffmpeg。
"""

import asyncio
import json
import os
import signal
import sys
from datetime import datetime, timezone

import websockets

PLATFORM = os.environ.get("RELAY_PLATFORM", "http://39.107.55.13:3100").rstrip("/")
STREAMS = [
    item.strip()
    for item in os.environ.get(
        "RELAY_STREAMS", "cam0,cam1,cam2,cam3,dogcam,dog-ipcam"
    ).split(",")
    if item.strip()
]
RTSP_BASE = os.environ.get("RELAY_RTSP_BASE", "rtsp://127.0.0.1:8554").rstrip("/")
ROOM_PREFIX = os.environ.get("RELAY_ROOM_PREFIX", "bp:")
FFMPEG = os.environ.get("RELAY_FFMPEG", "ffmpeg")
TRANSCODE = os.environ.get("RELAY_TRANSCODE", "0") == "1"

# 分片大小：一个 FLV tag 常见几 KB，8KB 兼顾低延时与帧数开销
CHUNK_BYTES = 8 * 1024
# WS 重连退避
MIN_BACKOFF = 1
MAX_BACKOFF = 15
# 起流后这么久一个字节都没有，就判定「ffmpeg 拉不到流」并打印排查提示
NO_DATA_TIMEOUT = 8.0


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


def ffmpeg_args(stream: str) -> list[str]:
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
        # 用到达时间当 PTS：`-c:v copy` 时源包的 PTS 可能没带过来，
        # 不补时间戳 FLV 就整条时间轴为 0，浏览器 MSE 会卡在 readyState=0 不出画面
        "-use_wallclock_as_timestamps",
        "1",
        "-fflags",
        "+genpts",
        "-i",
        source,
        "-an",  # 背包各流都没有音轨，FLV 里带空音轨会让 flv.js 报解析错
    ]
    if TRANSCODE:
        args += [
            "-c:v", "libx264",
            "-preset", "ultrafast",
            "-tune", "zerolatency",
            "-profile:v", "baseline",
            "-pix_fmt", "yuv420p",
            "-g", "60",
        ]
    else:
        # 只换容器：板端六路已经在用 h264_rkmpp 编码，重编码纯属浪费 CPU
        args += ["-c:v", "copy"]
    args += ["-f", "flv", "-flush_packets", "1", "pipe:1"]
    return args


class StreamSession:
    """一路流的拉取 + 推送（起停由平台的 video_start / video_stop 驱动）"""

    def __init__(self, stream: str) -> None:
        self.stream = stream
        self.proc: asyncio.subprocess.Process | None = None
        self.pump: asyncio.Task | None = None
        self.stderr: asyncio.Task | None = None
        self.bytes_sent = 0

    @property
    def running(self) -> bool:
        return self.proc is not None and self.proc.returncode is None

    async def start(self, ws) -> None:
        if self.running:
            return
        self.bytes_sent = 0
        log(f"[{self.stream}] 平台有观看者，开始拉流")
        # 告诉平台「这是一条新流」：常驻连接里每次起流都换新的 FLV 头与新时间戳，
        # 平台必须先丢掉上一轮的流头，否则后加入的观看者会拿到错位的字节（黑屏）
        await ws.send(json.dumps({"msgType": "video_reset", "payload": {"stream": self.stream}}))
        self.proc = await asyncio.create_subprocess_exec(
            *ffmpeg_args(self.stream),
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        self.pump = asyncio.create_task(self._pump(ws))
        self.stderr = asyncio.create_task(self._drain_stderr())

    async def stop(self, reason: str = "没人看了") -> None:
        if self.proc is None:
            return
        log(f"[{self.stream}] {reason}，停止拉流（已推 {self.bytes_sent} 字节）")
        for task in (self.pump, self.stderr):
            if task and not task.done():
                task.cancel()
        self.pump = self.stderr = None
        if self.proc.returncode is None:
            try:
                self.proc.terminate()
                await asyncio.wait_for(self.proc.wait(), timeout=3)
            except (asyncio.TimeoutError, ProcessLookupError):
                # ffmpeg 卡在 RTSP 读上时 terminate 不一定见效
                self.proc.kill()
        self.proc = None

    async def _pump(self, ws) -> None:
        """把 ffmpeg 的 FLV stdout 原样灌进 WS（平台按二进制帧转发给浏览器）"""
        assert self.proc and self.proc.stdout
        first = True
        try:
            while True:
                chunk = await self.proc.stdout.read(CHUNK_BYTES)
                if not chunk:
                    break
                if first:
                    first = False
                    if not chunk.startswith(b"FLV"):
                        # 平台要按 FLV 魔数解析流头，不是 FLV 就没法给中途加入的观众补头
                        log(
                            f"[{self.stream}] 警告：输出不是 FLV（可能是 H.265 源），"
                            "可用 RELAY_TRANSCODE=1 重编码"
                        )
                await ws.send(chunk)
                self.bytes_sent += len(chunk)
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # noqa: BLE001 - 连接断了交给外层重连
            log(f"[{self.stream}] 推流中断：{exc!r}")
        finally:
            # ffmpeg 自己退出（拉不到流 / 源断了）：告诉平台侧没人推了
            if self.proc and self.proc.returncode is not None and self.bytes_sent == 0:
                log(
                    f"[{self.stream}] ffmpeg 退出且没推出任何数据："
                    f"检查 {RTSP_BASE}/{self.stream} 是否在推流"
                )

    async def _drain_stderr(self) -> None:
        """ffmpeg 的 warning 打在 stderr，不读会堵住管道"""
        assert self.proc and self.proc.stderr
        try:
            while True:
                line = await self.proc.stderr.readline()
                if not line:
                    break
                text = line.decode("utf-8", "replace").strip()
                if text:
                    log(f"[{self.stream}] ffmpeg: {text}")
        except (asyncio.CancelledError, ValueError):
            raise


async def stream_worker(stream: str) -> None:
    """一条流一个长连接：注册房间 → 等 video_start / video_stop → 重连退避"""
    room = f"{ROOM_PREFIX}{stream}"
    session = StreamSession(stream)
    backoff = MIN_BACKOFF
    while True:
        try:
            async with websockets.connect(
                endpoint(),
                ping_interval=20,
                ping_timeout=20,
                max_size=None,
                open_timeout=10,
            ) as ws:
                log(f"[{stream}] 已连接平台，房间 {room}")
                backoff = MIN_BACKOFF
                await ws.send(
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
                )
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
                            await session.start(ws)
                        elif kind == "video_stop":
                            await session.stop("平台结束观看")
                finally:
                    # 连接断了也要杀掉 ffmpeg，不然没人看还在编码
                    await session.stop("推流连接断开")
        except asyncio.CancelledError:
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
    tasks = [asyncio.create_task(stream_worker(stream)) for stream in STREAMS]

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
