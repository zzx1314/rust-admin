"""机器狗前置 RTSP → 本机 mediamtx 的 dogcam path（归一化常驻 relay）

源是 3588 提供的 1920x1080@60 / H264 High@L4.0 / full-range / 负 start_time。
这里重编码成 DOGCAM_*（默认 1920x1080@30 / yuv420p / 4000k），
四路 USB 仍然是 CAPTURE_*（1280x720@30 / 2000k）——两组参数故意不共用，
因为 CAPTURE_* 同时管着 v4l2 采集尺寸；想让机器狗那一路回到 720p30，
设 DOGCAM_WIDTH=1280 DOGCAM_HEIGHT=720 DOGCAM_BITRATE=2000k 即可。

依据《ffprobe核验回执_分辨率与编码参数修订建议.md》§8.2.5 命令块 D：
  - fps=DOGCAM_FPS 重建均匀时间戳，解决源 NOPTS / 非单调 DTS；
  - 显式 in_range=full:out_range=full，避免 swscale 隐式压成 limited 导致偏灰；
  - -bf 0 / -g DOGCAM_GOP / -color_range pc 与四路干净流 SPS 一致。

安全约定：源 URL 含账号密码，只在进程内存中作为 ffmpeg 参数使用，
不写入 CAMERAS、/api/* 返回值、前端或日志；ffmpeg stderr 直接丢弃，
需要看告警时按回执命令块 D 手动跑一次即可。
"""
import subprocess
import threading
import time

import config
import stream  # 复用 ensure_encoder() 的 h264_rkmpp / libx264 探测

_PROC = None
_LOCK = threading.Lock()
_MONITOR_STARTED = False
_RESTARTS = 0
_LAST_ERROR = ""
_URL = None


def resolve_url():
    """环境变量优先，其次本目录 dog_rtsp_url 文件；都不存在返回 None。"""
    global _URL
    if _URL:
        return _URL
    url = (config.DOG_RTSP_URL or "").strip()
    if not url:
        try:
            with open(config.DOG_RTSP_URL_FILE, "r", encoding="utf-8") as f:
                url = f.read().strip()
        except OSError:
            url = ""
    _URL = url or None
    return _URL


def _build_cmd(url, enc):
    vf = ("fps=%d,scale=%d:%d:flags=bicubic:in_range=full:out_range=full,"
          "format=yuv420p") % (config.DOGCAM_FPS, config.DOGCAM_WIDTH,
                               config.DOGCAM_HEIGHT)
    enc_args = ["-c:v", enc]
    if enc == "libx264":
        enc_args += ["-preset", "ultrafast"]
    return [
        "ffmpeg", "-hide_banner", "-loglevel", "warning", "-nostdin",
        "-rtsp_transport", "tcp", "-fflags", "+genpts",
        "-i", url,
        "-an", "-vf", vf,
    ] + enc_args + [
        "-b:v", config.DOGCAM_BITRATE,
        "-g", str(config.DOGCAM_GOP), "-bf", "0", "-color_range", "pc",
        "-avoid_negative_ts", "make_zero",
        "-rtsp_transport", "tcp", "-f", "rtsp",
        "rtsp://127.0.0.1:%d/%s" % (config.RTSP_PORT,
                                     config.CAMERAS["dogcam"]["stream"]),
    ]


def _spawn(url, enc):
    global _PROC
    p = subprocess.Popen(_build_cmd(url, enc),
                         stdin=subprocess.DEVNULL,
                         stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)
    with _LOCK:
        _PROC = p
    return p


def _monitor(url, enc):
    """5 秒看门狗；连续快速失败按 5/10/20/30s 退避，避免疯狂重启。"""
    global _RESTARTS, _LAST_ERROR
    backoff = 5
    while True:
        time.sleep(backoff)
        with _LOCK:
            p = _PROC
        if p is None:
            continue
        rc = p.poll()
        if rc is None:
            backoff = 5
            continue
        _LAST_ERROR = "ffmpeg 退出码 %s" % rc
        _RESTARTS += 1
        print("[relay] dogcam 断开（%s），%ss 后重启" % (_LAST_ERROR, backoff),
              flush=True)
        try:
            _spawn(url, enc)
        except Exception as e:
            _LAST_ERROR = "ffmpeg 重启失败: %s" % e
        backoff = min(30, backoff * 2)


def start_all():
    """由 agent.py lifespan 拉起；未配置源时安静跳过，不影响其他路。"""
    global _MONITOR_STARTED
    if not config.DOG_RELAY_ENABLED:
        print("[relay] dogcam 已禁用（DOG_RELAY_ENABLED=0）", flush=True)
        return False
    url = resolve_url()
    if not url:
        print("[relay] 未配置 DOG_RTSP_URL / dog_rtsp_url，跳过 dogcam"
              "（四路与红外不受影响）", flush=True)
        return False
    enc = stream.ensure_encoder()
    try:
        _spawn(url, enc)
    except Exception as e:
        print("[relay] ffmpeg 启动失败: %s" % e, flush=True)
        return False
    with _LOCK:
        if not _MONITOR_STARTED:
            _MONITOR_STARTED = True
            threading.Thread(target=_monitor, args=(url, enc),
                             daemon=True).start()
    print("[relay] dogcam 已启动 -> rtsp://127.0.0.1:%d/%s（编码器 %s）"
          % (config.RTSP_PORT, config.CAMERAS["dogcam"]["stream"], enc),
          flush=True)
    return True


def stop_all():
    global _PROC
    with _LOCK:
        p, _PROC = _PROC, None
    if p:
        try:
            p.terminate()
        except Exception:
            pass


def status():
    """只读状态，绝不返回源 URL。"""
    with _LOCK:
        p, restarts = _PROC, _RESTARTS
    return {
        "enabled": bool(config.DOG_RELAY_ENABLED and resolve_url()),
        "running": bool(p and p.poll() is None),
        "restarts": restarts,
        "last_error": _LAST_ERROR,
        "stream": config.CAMERAS["dogcam"]["stream"],
    }
