"""机器狗前置 RTSP → 本机 mediamtx 的 dogcam path（归一化常驻 relay）

源是 3588 提供的 1920x1080@60 / H264 High@L4.0 / full-range / 负 start_time。
这里统一重编码成与 cam0~cam3 逐字段一致的 1280x720@30 / yuvj420p(pc)，
于是 WHEP 播放、AI 旁路抽帧、后续 preview 切路拿到的规格完全相同。

依据《ffprobe核验回执_分辨率与编码参数修订建议.md》§8.2.5 命令块 D：
  - fps=30 重建均匀时间戳，解决源 NOPTS / 非单调 DTS；
  - 显式 in_range=full:out_range=full，避免 swscale 隐式压成 limited 导致偏灰；
  - -bf 0 / -g 30 / -color_range pc 与四路干净流 SPS 一致。

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
# 当前档位：1080p60（mediamtx 直拉 dogcam，无 relay）/ 720p30（本模块重编码推 dogcam720）
_PROFILE = None


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


def get_profile():
    """读取当前档位；状态文件不存在时默认 1080p60（方案 A 直拉）。"""
    global _PROFILE
    if _PROFILE:
        return _PROFILE
    try:
        with open(config.DOG_PROFILE_FILE, "r", encoding="utf-8") as f:
            v = f.read().strip()
    except OSError:
        v = "1080p60"
    _PROFILE = v if v in config.DOG_PROFILES else "1080p60"
    return _PROFILE


def _set_profile_file(profile):
    global _PROFILE
    with open(config.DOG_PROFILE_FILE, "w", encoding="utf-8") as f:
        f.write(profile + "\n")
    _PROFILE = profile


def dogcam_stream():
    """当前档位下 dogcam 对外实际使用的 mediamtx path 名。

    1080p60 -> dogcam（mediamtx 直拉源，无 relay）
    720p30  -> dogcam720（relay 重编码后推上来）
    前端 WHEP 与 AI 抓帧都据此选流，逻辑摄像头名 dogcam 不变。
    """
    return "dogcam" if get_profile() == "1080p60" else config.DOG_RELAY_STREAM


def set_profile(profile):
    """运行时切换前置分辨率档位：启停 relay，dogcam 断几秒后恢复。

    - 切到 1080p60：停 relay（mediamtx 直拉 dogcam 一直在，无需重启）；
    - 切到 720p30：启动 relay 重编码推 dogcam720。
    两档共用同一源 URL；源未配置时 720p30 无法启动，会返回错误。
    """
    global _MONITOR_STARTED
    if profile not in config.DOG_PROFILES:
        return {"ok": False, "error": "不支持的档位 %s（可选 %s）"
                % (profile, "/".join(config.DOG_PROFILES))}
    old = get_profile()
    if profile == "720p30" and not resolve_url():
        return {"ok": False, "error": "未配置 DOG_RTSP_URL / dog_rtsp_url，无法启动 720p30 重编码"}
    _set_profile_file(profile)
    if old == profile:
        return {"ok": True, "profile": profile, "stream": dogcam_stream(),
                "relay_running": status()["running"], "changed": False}

    if profile == "1080p60":
        stop_all()
        return {"ok": True, "profile": profile, "stream": dogcam_stream(),
                "relay_running": False, "changed": True}

    # 切到 720p30：启动 relay（若看门狗未起则一并启动）
    enc = stream.ensure_encoder()
    url = resolve_url()
    _probe_decoder(url)
    try:
        _spawn(url, enc)
    except Exception as e:
        return {"ok": False, "error": "relay 启动失败: %s" % e}
    with _LOCK:
        if not _MONITOR_STARTED:
            _MONITOR_STARTED = True
            threading.Thread(target=_monitor, args=(url, enc),
                             daemon=True).start()
    return {"ok": True, "profile": profile, "stream": dogcam_stream(),
            "relay_running": True, "changed": True}


_DECODER = None            # None = ffmpeg 默认软解；探到 h264_rkmpp 就用硬解
_DECODER_PROBED = False


def _probe_decoder(url):
    """探一次本体的流能不能用 h264_rkmpp 硬解（只探一次，结果缓存）。

    为什么要改：这一路是「本体 1080p60 → 720p 重编码」，默认软解时一个核的
    **71%** 全花在解 1080p60 上（2026-11-19 在 .246 实测），而板子还要跑四路
    USB 编码 + 中继转码。硬解之后同一路降到 **31%**（剩下的主要是 swscale）。

    为什么不用 `scale_rkrga`（RGA 硬缩放）：本机 ffmpeg 上它一进滤镜图就
    `Error reinitializing filters`，硬解帧喂不进去（同一天实测，三种写法都失败），
    所以缩放仍走 swscale —— 好在 `fps=15` 在 `scale` 前面，缩放只跑 15fps。

    为什么只看 `-decoders` 不够：列表里有不等于能解（设备没起 / 对某路源解不出来），
    必须真解出两帧像素；解不出来时 ffmpeg 不报错，只是永远等不到帧。
    """
    global _DECODER, _DECODER_PROBED
    if _DECODER_PROBED:
        return _DECODER
    _DECODER_PROBED = True
    try:
        listed = subprocess.run(["ffmpeg", "-hide_banner", "-decoders"],
                                capture_output=True, text=True, timeout=10).stdout
        if "h264_rkmpp" not in listed:
            return _DECODER
        proc = subprocess.run(
            ["ffmpeg", "-hide_banner", "-loglevel", "error",
             "-rtsp_transport", "tcp", "-c:v", "h264_rkmpp",
             "-i", url, "-frames:v", "2", "-f", "rawvideo", "-"],
            capture_output=True, timeout=8,
        )
        if proc.returncode == 0 and len(proc.stdout) > 0:
            _DECODER = "h264_rkmpp"
        else:
            print("[relay] h264_rkmpp 硬解探测失败（%s），继续软解"
                  % (proc.stderr.decode("utf-8", "replace").strip()[:80]
                     or "两帧都没解出像素"), flush=True)
    except Exception as e:  # noqa: BLE001 - 探测失败一律退回软解
        print("[relay] 硬解探测异常: %s（继续软解）" % e, flush=True)
    return _DECODER


def _build_cmd(url, enc):
    # 输出帧率用 CAPTURE_NOUT_FPS（= CAPTURE_OUT_FPS 或 CAPTURE_FPS）：
    # 现场相机只支持 30fps 采集，降帧率走输出侧 fps 滤镜，见 config.py。
    vf = ("fps=%d,scale=%d:%d:flags=bicubic:in_range=full:out_range=full,"
          "format=yuv420p") % (config.CAPTURE_NOUT_FPS, config.CAPTURE_WIDTH,
                               config.CAPTURE_HEIGHT)
    enc_args = ["-c:v", enc]
    if enc == "libx264":
        enc_args += ["-preset", "ultrafast"]
    dec_args = ["-c:v", _DECODER] if _DECODER else []
    return [
        "ffmpeg", "-hide_banner", "-loglevel", "warning", "-nostdin",
        "-rtsp_transport", "tcp", "-fflags", "+genpts",
    ] + dec_args + [
        "-i", url,
        "-an", "-vf", vf,
    ] + enc_args + [
        "-b:v", config.H264_BITRATE,
        "-g", "30", "-bf", "0", "-color_range", "pc",
        "-avoid_negative_ts", "make_zero",
        "-rtsp_transport", "tcp", "-f", "rtsp",
        "rtsp://127.0.0.1:%d/%s" % (config.RTSP_PORT, config.DOG_RELAY_STREAM),
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
    """由 agent.py lifespan 拉起。

    档位语义：
      1080p60（默认）：dogcam 由 mediamtx 直拉源，本模块不启动任何进程；
      720p30：启动 relay 重编码推 dogcam720。
    兼容旧开关：DOG_RELAY_ENABLED=0 时强制走 1080p60 直拉。
    """
    global _MONITOR_STARTED
    if not config.DOG_RELAY_ENABLED:
        print("[relay] DOG_RELAY_ENABLED=0：dogcam 走 1080p60 直拉，relay 不启动",
              flush=True)
        return False
    if get_profile() == "1080p60":
        print("[relay] 当前档位 1080p60：dogcam 由 mediamtx 直拉，relay 不启动",
              flush=True)
        return False
    url = resolve_url()
    if not url:
        print("[relay] 未配置 DOG_RTSP_URL / dog_rtsp_url，无法启动 720p30"
              "（四路与红外不受影响）", flush=True)
        return False
    enc = stream.ensure_encoder()
    dec = _probe_decoder(url)
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
    print("[relay] dogcam 720p30 已启动 -> rtsp://127.0.0.1:%d/%s"
          "（编码器 %s / 解码器 %s）"
          % (config.RTSP_PORT, config.DOG_RELAY_STREAM, enc, dec or "软解"),
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
    """只读状态，绝不返回源 URL。

    两档都让「狗前置可用」= true（前端据此保留卡片与 AI 选项）：
      1080p60：dogcam 由 mediamtx 直拉源，relay 不跑，mode=direct；
      720p30 ：relay 重编码推 dogcam720，mode=relay，running 看 ffmpeg 进程。
    stream = 当前档位下 dogcam 对外实际 path（前端 WHEP / AI 抓帧据此选流）。
    """
    with _LOCK:
        p, restarts = _PROC, _RESTARTS
    prof = get_profile()
    if prof == "1080p60":
        return {
            "enabled": True,
            "running": True,
            "mode": "direct",
            "profile": "1080p60",
            "stream": "dogcam",
            "profiles": list(config.DOG_PROFILES),
            "restarts": 0,
            "last_error": "",
        }
    return {
        "enabled": bool(resolve_url()),
        "running": bool(p and p.poll() is None),
        "mode": "relay",
        "profile": "720p30",
        "stream": config.DOG_RELAY_STREAM,
        "profiles": list(config.DOG_PROFILES),
        "restarts": restarts,
        "last_error": _LAST_ERROR,
        "decoder": _DECODER or "sw",
    }
