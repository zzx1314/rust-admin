#!/usr/bin/env python3
"""用 ffmpeg `lavfi` 往本地 mediamtx 推「假相机」，替代 `.246` 上的 4 路 USB + 本体前置。

真板子上这几路是这么来的：

- `cam0`~`cam3`：`backpack-agent` 用 v4l2 采 USB 相机（1280x720@30 / 2Mbps）再推给
  mediamtx 的 `all_others`（`source: publisher`）；
- `dogcam`：Agent 拉本体 RTSP（`192.168.234.1:8554/test`，1080p60）后归一化再推；
- `preview`：AI 画框的那一路（`OSD` 烧录），也跟着 Agent 走。

本地没有相机、也不想依赖本体，所以这里直接 `ffmpeg -f lavfi -i testsrc2=…` 把带流名的
测试图推给同一个 mediamtx。对 relay / 平台 / 浏览器来说，它和真相机**没有区别**：
都是一条 RTSP path，中继照样转码、WHEP 照样能看。

`-g <fps>` 保证每秒一个关键帧（与现场 `RELAY_GOP` 对齐），起流/切通道等待短。

环境变量：

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `SIM_PUBLISH_STREAMS` | `dogcam,cam0,preview` | 要推哪几路（逗号分隔）。全量是 `cam0,cam1,cam2,cam3,dogcam,dog-ipcam,preview`，但 7 路 720p 会让笔记本风扇起飞 |
| `SIM_MEDIAMTX_RTSP` | `rtsp://127.0.0.1:8554` | mediamtx 的 RTSP 基址 |
| `SIM_CAM_FPS` | `15` | 帧率 |
| `SIM_CAM_BITRATE` | `800k` | 码率（只影响 x264 目标，不发真流量也没关系） |
| `SIM_CAM_SIZE` | `960x540` | cam*/preview 尺寸；`dogcam` 固定 `1280x720`（模拟 1080p 源归一化后的样子） |
| `SIM_FFMPEG` | `ffmpeg` | ffmpeg 路径 |
"""

import os
import shutil
import signal
import subprocess
import sys
import threading
import time

RTSP_BASE = os.environ.get("SIM_MEDIAMTX_RTSP", "rtsp://127.0.0.1:8554").rstrip("/")
STREAMS = [
    item.strip()
    for item in os.environ.get("SIM_PUBLISH_STREAMS", "dogcam,cam0,preview").split(",")
    if item.strip()
]
FPS = max(1, int(os.environ.get("SIM_CAM_FPS", "15")))
BITRATE = os.environ.get("SIM_CAM_BITRATE", "800k")
SIZE = os.environ.get("SIM_CAM_SIZE", "960x540")
FFMPEG = os.environ.get("SIM_FFMPEG", "ffmpeg")
LOG_DIR = os.path.abspath(os.environ.get("SIM_LOG_DIR", os.path.join(os.path.dirname(os.path.abspath(__file__)), "logs")))

FONT_CANDIDATES = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
]

STOP = threading.Event()


def log(message):
    print("[cams] %s" % message, flush=True)


def font_file():
    for path in FONT_CANDIDATES:
        if os.path.exists(path):
            return path
    return ""


def drawtext(stream):
    font = font_file()
    if not font:
        return ""
    # 字体路径里的冒号要转义；文字只用流名（别加时间戳：`:` 会被当成 filter 选项分隔符，
    # 见 2026-11-19 这次 `No option name near '17:41:fontcolor=…'`）
    safe_font = font.replace(":", "\\:")
    return (
        ",drawtext=fontfile=%s:text='%s':fontcolor=white:fontsize=42:"
        "x=24:y=24:box=1:boxcolor=black@0.45:boxborderw=12"
        % (safe_font, stream)
    )


def size_of(stream):
    return "1280x720" if stream == "dogcam" else SIZE


def command(stream):
    w, h = size_of(stream).split("x")
    source = "testsrc2=size=%sx%s:rate=%d" % (w, h, FPS)
    # 每 10s 变一次 pattern，避免画面完全静止（中继/编码器都是按帧率跑，静止也没事，
    # 但人眼看画面在动更容易判断「卡住了没有」）
    vf = "format=yuv420p%s" % drawtext(stream)
    return [
        FFMPEG,
        "-nostdin",
        "-hide_banner",
        "-loglevel", "warning",
        "-re",
        "-f", "lavfi",
        "-i", source,
        "-vf", vf,
        "-c:v", "libx264",
        "-preset", "veryfast",
        "-tune", "zerolatency",
        "-pix_fmt", "yuv420p",
        "-g", str(FPS),
        "-b:v", BITRATE,
        "-maxrate", BITRATE,
        "-bufsize", BITRATE,
        "-f", "rtsp",
        "-rtsp_transport", "tcp",
        "%s/%s" % (RTSP_BASE, stream),
    ]


def pump(stream):
    backoff = 1.0
    os.makedirs(LOG_DIR, exist_ok=True)
    log_path = os.path.join(LOG_DIR, "cam-%s.log" % stream)
    while not STOP.is_set():
        started = time.time()
        log("推流 %s → %s/%s（%s@%dfps %s）" % (stream, RTSP_BASE, stream, size_of(stream), FPS, BITRATE))
        try:
            with open(log_path, "wb") as err:
                process = subprocess.Popen(command(stream), stdout=subprocess.DEVNULL,
                                           stderr=err, start_new_session=True)
                while not STOP.is_set() and process.poll() is None:
                    time.sleep(0.5)
                if STOP.is_set():
                    terminate(process)
                    return
                process.wait(timeout=5)
        except FileNotFoundError:
            log("找不到 ffmpeg（%s）：装好 ffmpeg 再跑，或设 SIM_FFMPEG" % FFMPEG)
            return
        detail = tail(log_path)
        # mediamtx 没起来 / 端口占用的前几秒会一直失败：退避重连，别刷屏
        log("推流 %s 中断（%s），%.0fs 后重连" % (stream, detail, backoff))
        if time.time() - started > 30:
            backoff = 1.0
        else:
            backoff = min(10.0, backoff * 2)
        STOP.wait(backoff)


def tail(path, limit=160):
    try:
        with open(path, "rb") as handle:
            lines = handle.read().decode("utf-8", "replace").strip().splitlines()
    except OSError:
        return "无日志"
    return (lines[-1][:limit] if lines else "无输出")


def terminate(process):
    try:
        os.killpg(os.getpgid(process.pid), signal.SIGTERM)
    except Exception:  # noqa: BLE001
        pass
    try:
        process.wait(timeout=3)
    except Exception:  # noqa: BLE001
        try:
            os.killpg(os.getpgid(process.pid), signal.SIGKILL)
        except Exception:  # noqa: BLE001
            pass


def main():
    if not shutil.which(FFMPEG):
        print("[cams] 缺少 ffmpeg，无法生成假相机", file=sys.stderr)
        return 2
    if not STREAMS:
        log("SIM_PUBLISH_STREAMS 为空，什么都不推")
        return 0
    log("准备推 %d 路：%s" % (len(STREAMS), ", ".join(STREAMS)))
    threads = [threading.Thread(target=pump, args=(stream,), daemon=True) for stream in STREAMS]

    def shutdown(*_):
        STOP.set()

    signal.signal(signal.SIGINT, shutdown)
    signal.signal(signal.SIGTERM, shutdown)
    for thread in threads:
        thread.start()
    while not STOP.is_set():
        if all(not thread.is_alive() for thread in threads):
            break
        STOP.wait(0.5)
    STOP.set()
    for thread in threads:
        thread.join(timeout=5)
    log("已停止")
    return 0


if __name__ == "__main__":
    sys.exit(main())
