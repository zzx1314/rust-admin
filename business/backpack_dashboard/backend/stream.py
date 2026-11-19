"""四路摄像头推流编排：ffmpeg 读取 v4l2(MJPG) -> H264 -> 推 RTSP 到 mediamtx

摄像头原生输出 MJPG（实测 v4l2-ctl），WebRTC 需要 H264，因此这里做转码后推流。
mediamtx 已通过 snap 安装（yd_test_v4.0.sh 已验证），负责 RTSP/WebRTC 分发。
"""
import subprocess
import threading
import time
import os
from config import (CAMERAS, RTSP_PORT, CAPTURE_WIDTH, CAPTURE_HEIGHT,
                     CAPTURE_FPS, CAPTURE_PIXFMT, H264_ENCODER, H264_BITRATE)

_PROCS = {}
_ENCODER = None
_LOCK = threading.Lock()


def _probe_encoder():
    """检测可用的 H264 编码器，优先硬编 h264_rkmpp。"""
    try:
        out = subprocess.run(["ffmpeg", "-hide_banner", "-encoders"],
                             capture_output=True, text=True, timeout=10).stdout
        if "h264_rkmpp" in out:
            return "h264_rkmpp"
    except Exception:
        pass
    return "libx264"


def ensure_encoder():
    global _ENCODER
    _ENCODER = _probe_encoder()
    return _ENCODER


def _build_cmd(cam):
    enc = _ENCODER or H264_ENCODER
    cmd = [
        "ffmpeg", "-y",
        "-f", "v4l2",
        "-input_format", CAPTURE_PIXFMT,
        "-video_size", f"{CAPTURE_WIDTH}x{CAPTURE_HEIGHT}",
        "-framerate", str(CAPTURE_FPS),
        "-i", cam["device"],
        "-c:v", enc,
        "-b:v", H264_BITRATE,
    ]
    if enc == "libx264":
        cmd += ["-preset", "ultrafast", "-pix_fmt", "yuv420p"]
    cmd += [
        "-rtsp_transport", "tcp",
        "-f", "rtsp",
        f"rtsp://127.0.0.1:{RTSP_PORT}/{cam['stream']}",
    ]
    return cmd


def _spawn(cam):
    cmd = _build_cmd(cam)
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _PROCS[cam["stream"]] = p
    return p


def start_all():
    """启动全部摄像头推流（依赖 mediamtx 已运行）。"""
    ensure_encoder()
    for cam in CAMERAS.values():
        device = cam.get("device")
        # dogcam 没有本地设备节点，由 relay.py 从外部 RTSP 归一化后推上来
        if not device:
            continue
        if not os.path.exists(device):
            print(f"[stream] 跳过 {device}：设备不存在")
            continue
        _spawn(cam)
    # 后台监控：进程意外退出自动拉起
    t = threading.Thread(target=_monitor, daemon=True)
    t.start()


def _monitor():
    while True:
        time.sleep(5)
        for cam in CAMERAS.values():
            device = cam.get("device")
            if not device:      # dogcam 由 relay.py 自己的看门狗负责
                continue
            p = _PROCS.get(cam["stream"])
            if p is None or p.poll() is not None:
                if os.path.exists(device):
                    print(f"[stream] 重启推流 {cam['stream']}")
                    _spawn(cam)


def stop_all():
    for stream, p in list(_PROCS.items()):
        try:
            p.terminate()
        except Exception:
            pass
    _PROCS.clear()


def status():
    return {stream: (p.poll() is None) for stream, p in _PROCS.items()}
