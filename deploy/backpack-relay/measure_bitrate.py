#!/usr/bin/env python3
"""量一路平台中继的真实码率 / 帧率（在平台机器上跑，走 loopback）。

排查「平台中继很卡」时先量这个，再对比公网出口带宽：

```text
中继码率 > 出口带宽  → 必卡（观看端追不上，服务端会判 lagged 断开重连）
```

在 `39.107.55.13` 上执行（loopback 不经过公网，量到的是**推流端真实码率**）：

```bash
python3 measure_bitrate.py --stream dogcam --seconds 20
# 输出示例：dogcam: 1180 kbps, 20 fps, 收到 2.95 MB / 20.0s
```

对照：

| 场景 | 预期 |
| --- | --- |
| 板端 `RELAY_PROFILE=light`（默认） | ~1200 kbps / 20 fps / 720p |
| 板端 `RELAY_PROFILE=copy`（1080p60 源） | 4000~8000 kbps / 60 fps |
| 平台公网出口（`speedtest-cli --simple`） | 目前实测 ~2.5 Mbps |

依赖：`python3` + `websockets`（`pip3 install -i https://mirrors.aliyun.com/pypi/simple websockets`）。
"""

import argparse
import asyncio
import json
import os
import sys
import time
import urllib.request

import websockets

FLV_VIDEO_TAG = 0x09


def login(platform, username, password):
    """拿控制台 JWT（`/api/backpack/video/ws` 支持 `?token=`）。"""
    body = json.dumps({"username": username, "password": password}).encode()
    req = urllib.request.Request(
        f"{platform.rstrip('/')}/api/auth/login",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        data = json.load(resp)
    token = (data.get("data") or {}).get("token")
    if not token:
        raise SystemExit(f"登录失败：{data.get('msg') or data}")
    return token


def count_video_frames(buffer):
    """从字节流里数 FLV 视频 tag（帧），顺便丢掉已消费的字节。

    返回 (本批新增帧数, 剩余字节数)。FLV 是「tag 头 11B + 数据 + 4B 前一个 tag 长度」，
    tag 跨 WS 分片很正常，所以没凑齐就留着下次再解析。
    """
    frames = 0
    pos = 0
    if len(buffer) >= 13 and buffer[0:3] == b"FLV":
        pos = 13  # 跳过 FLV header（9）+ PreviousTagSize0（4）
    while len(buffer) >= pos + 11:
        tag_type = buffer[pos]
        size = int.from_bytes(buffer[pos + 1 : pos + 4], "big")
        if len(buffer) < pos + 11 + size + 4:
            break
        if tag_type == FLV_VIDEO_TAG:
            frames += 1
        pos += 11 + size + 4
    if pos:
        del buffer[:pos]
    return frames, len(buffer)


async def measure(platform, token, stream, seconds):
    base = platform.rstrip("/").replace("https://", "wss://").replace("http://", "ws://")
    url = f"{base}/api/backpack/video/ws?stream={stream}&token={token}"
    buffer = bytearray()
    total = 0
    frames = 0
    started = time.monotonic()
    try:
        async with websockets.connect(url, max_size=None) as ws:
            while time.monotonic() - started < seconds:
                try:
                    message = await asyncio.wait_for(
                        ws.recv(), timeout=max(0.1, seconds - (time.monotonic() - started))
                    )
                except asyncio.TimeoutError:
                    break
                if isinstance(message, (bytes, bytearray)):
                    total += len(message)
                    buffer.extend(message)
                    added, _ = count_video_frames(buffer)
                    frames += added
    except Exception as exc:  # noqa: BLE001 - 连不上时给可读原因
        print(f"连接中继失败：{exc!r}")
        print("先确认板端 backpack-relay 在线：GET /api/backpack/video/state 里应有 bp:<stream>")
        return 1
    elapsed = max(0.001, time.monotonic() - started)
    print(
        f"{stream}: {total * 8 / elapsed / 1000:.0f} kbps, "
        f"{frames / elapsed:.0f} fps, "
        f"收到 {total / 1024 / 1024:.2f} MB / {elapsed:.1f}s"
    )
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", default="http://127.0.0.1:3100")
    parser.add_argument("--stream", default="dogcam")
    parser.add_argument("--seconds", type=float, default=15.0)
    parser.add_argument(
        "--username", default=os.environ.get("ROBODOG_API_USERNAME", "admin")
    )
    parser.add_argument(
        "--password", default=os.environ.get("ROBODOG_API_PASSWORD", "admin123")
    )
    args = parser.parse_args()
    token = login(args.platform, args.username, args.password)
    # 目标平台是 CentOS 7 的 Python 3.6（没有 asyncio.run），用事件循环手动跑
    loop = asyncio.get_event_loop()
    return loop.run_until_complete(
        measure(args.platform, token, args.stream, args.seconds)
    )


if __name__ == "__main__":
    sys.exit(main())
