#!/usr/bin/env python3
"""量平台中继的**积压延迟**：在平台机器上跑（loopback）。

原理：板端 relay 用 `-use_wallclock_as_timestamps 1`，所以 FLV 视频 tag 里的时间戳就是
「这一帧到达板端 ffmpeg 的时刻」。它和墙钟的差 = 这帧在管道里躺了多久，**相对第一帧的
增长**就是积压累积（正数 = 越看越晚，这才是「延迟大」的真正原因）。

```bash
python3 latency_probe.py --stream dogcam --seconds 60
#   t+ 10.2s  积压增长 -0.02s  （帧 189，当前 tag_ts 9350ms）
# 结论：60s 内积压增长 -0.02s（≈0 表示不累积；正数=越看越延迟）
```

对照：

| 场景 | 预期 |
| --- | --- |
| 链路正常 | 增量 ≈ 0（`fps` 滤镜的 CFR 时间轴只会把积压排空，所以略负也正常） |
| 上行拥塞 / ffmpeg 被反压 | 持续正增长（每秒涨几百 ms 以上）→ 看板端 `画面延迟 x.xs` 日志 |
| 板端 <1s 但人看到好几秒 | 延迟在源侧（本体 RTSP）或浏览器，不在中继 |

依赖：`python3` + `websockets`（同 `measure_bitrate.py`）。
"""

import argparse
import asyncio
import json
import os
import sys
import time
import urllib.request

import websockets

PLATFORM = os.environ.get("ROBODOG_PLATFORM", "http://127.0.0.1:3100")
# 平台控制台账号（只用来换 `/api/backpack/video/ws?token=`）
USERNAME = os.environ.get("ROBODOG_API_USERNAME", "admin")
PASSWORD = os.environ.get("ROBODOG_API_PASSWORD", "admin123")


def login(platform: str, username: str, password: str) -> str:
    body = json.dumps({"username": username, "password": password}).encode()
    request = urllib.request.Request(
        f"{platform.rstrip('/')}/api/auth/login",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        data = json.load(response)
    token = (data.get("data") or {}).get("token")
    if not token:
        raise SystemExit(f"登录失败：{data.get('msg') or data}")
    return token


def drain(buffer: bytearray, on_tag) -> bytearray:
    """尽量从缓冲里解析 FLV tag（回调 type / ts_ms），返回吃剩下的字节。"""
    pos = 0
    if len(buffer) >= 13 and buffer[0:3] == b"FLV":
        pos = 13  # FLV header(9) + PreviousTagSize0(4)
    while len(buffer) >= pos + 11:
        tag_type = buffer[pos]
        size = int.from_bytes(buffer[pos + 1 : pos + 4], "big")
        timestamp = int.from_bytes(buffer[pos + 4 : pos + 7], "big") | (buffer[pos + 7] << 24)
        end = pos + 11 + size + 4
        if len(buffer) < end:
            break
        on_tag(tag_type, timestamp)
        pos = end
    return buffer[pos:]


async def measure(platform: str, token: str, stream: str, seconds: float) -> int:
    base = platform.rstrip("/").replace("https://", "wss://").replace("http://", "ws://")
    url = f"{base}/api/backpack/video/ws?stream={stream}&token={token}"
    buffer = bytearray()
    state = {"prev": None, "drift": 0.0, "frames": 0, "skipped": 0}
    started = time.monotonic()
    reported = started

    def on_tag(tag_type: int, timestamp: int, now: float) -> None:
        if tag_type != 9:  # 只看视频 tag
            return
        state["frames"] += 1
        previous = state["prev"]
        state["prev"] = (now, timestamp)
        if previous is None:
            return
        d_ts = (timestamp - previous[1]) / 1000.0
        d_wall = now - previous[0]
        if d_ts > 5 or d_ts < -5:
            # 加入时平台补发的关键帧 → 直播帧，不是积压，跳过这一次
            state["skipped"] += 1
            return
        state["drift"] += d_wall - d_ts

    try:
        async with websockets.connect(url, max_size=None) as ws:
            while time.monotonic() - started < seconds:
                timeout = max(0.1, seconds - (time.monotonic() - started))
                try:
                    message = await asyncio.wait_for(ws.recv(), timeout=timeout)
                except asyncio.TimeoutError:
                    break
                if not isinstance(message, (bytes, bytearray)):
                    continue
                now = time.monotonic()
                buffer.extend(message)
                buffer = bytearray(drain(buffer, lambda t, ts, now=now: on_tag(t, ts, now)))
                if now - reported >= 2.0:
                    reported = now
                    latest = state["prev"][1] if state["prev"] else 0
                    print(
                        f"  t+{now - started:5.1f}s  积压增长 {state['drift']:+.2f}s  "
                        f"（帧 {state['frames']}，当前 tag_ts {latest}ms）"
                    )
    except Exception as exc:  # noqa: BLE001 - 连不上给可读原因
        print(f"连接中继失败：{exc!r}")
        print("先确认板端 relay 在线：GET /api/backpack/video/state 里应有 bp:<stream>")
        return 1

    if state["frames"] == 0:
        print("没测到视频帧（推流端没起流？）")
        return 1
    print(
        f"结论：{seconds:.0f}s 内积压增长 {state['drift']:+.2f}s"
        "（≈0 表示不累积；正数=越看越延迟）"
    )
    return 0 if state["drift"] < 1.0 else 2


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", default=PLATFORM)
    parser.add_argument("--stream", default="dogcam")
    parser.add_argument("--seconds", type=float, default=60.0)
    parser.add_argument("--username", default=USERNAME)
    parser.add_argument("--password", default=PASSWORD)
    args = parser.parse_args()
    token = login(args.platform, args.username, args.password)
    # 目标机是 CentOS 7 的 Python 3.6（没有 asyncio.run），用事件循环手动跑
    loop = asyncio.get_event_loop()
    return loop.run_until_complete(measure(args.platform, token, args.stream, args.seconds))


if __name__ == "__main__":
    sys.exit(main())
