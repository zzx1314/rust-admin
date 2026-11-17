"""背包本地画框（板端 OSD）—— 把检测框直接烧进视频，浏览器不再叠 SVG。

链路（只在 AI 开启时存在）：

    mediamtx /<cam> 干净流
      -> ffmpeg 拉流 + overlay(自绘 RGBA 画布)
      -> H264 重编码
      -> mediamtx /preview
      -> 浏览器 WHEP 播带框视频

为什么用「RGBA 画布 + FIFO」：
  - 不依赖 Pillow / OpenCV / 板端字体文件：中文用 cjk_font.py 里**预烘焙**
    的 16x16 点阵字模（离线由 tools/gen_cjk_font.py 生成），纯标准库画
    矩形和文字。板端只需同步这一个 .py 文件；
  - 画布按 640x360 生成（放大到 1280x720 后文字 32px 高），ffmpeg 负责
    放大叠加与编码，板端只多一路 H264 编码；
  - 主视频不经过 Python，Python 只写半透明图层。

标签文字（版本 2.2）：由 detect.py._emit() 统一生成，形如「火焰 3处」，
即中文类名 + 当前帧同类检出数量。这里只负责把 box["text"] 画出来，
不再自己拼英文类名和置信度。字库里没有的字画空心方框，并把字列入
osd.status()["missing_glyphs"]，现场一眼能看出需要重跑字库生成脚本。

低延迟设计（版本 2.1，针对「AI 那一路比其他路慢好几秒」）：
  - 视频帧与画布帧解耦：视频由主输入按 30fps 驱动，画布 FIFO 只按
    OSD_FIFO_FPS(默认 8) 写，overlay 用 repeatlast 重复上一张画布。
    旧版画布也按 30fps 写，Python 跟不上就会通过 framesync 反压主视频，
    帧在 ffmpeg 队列里持续堆积，稳态延时能到 6 秒以上。
  - 两个输入都用 setpts=N/(fps*TB) 按帧序号重造时间戳：RTSP 输入的
    start_time 来自 mediamtx（可能是任意大值），rawvideo FIFO 却从 0 开始，
    不对齐时 overlay 会等待/缓存，同样造成积压。
  - 输入 nobuffer / avioflags direct / 小 thread_queue_size，输出
    muxdelay 0 / vsync 0 / flush_packets 1，把每段显式缓冲压到最小。
  推理仍只有 DETECT_FPS(默认 6Hz)，框 6Hz 更新、视频 30fps，互不牵连。

失败安全：ffmpeg / FIFO / 编码器任一环节不可用时 start() 返回 False，
detect.py 仍正常识别，前端检测到 osd.running=false 会自动回退浏览器画框。
设环境变量 DETECT_OSD_ENABLED=0 可强制回退，用于两种方式对照测试。
"""
import os
import shutil
import subprocess
import threading
import time

try:                       # Linux 板端有；Windows 开发机没有，降级即可
    import fcntl
    F_SETPIPE_SZ = 1031    # Linux <linux/fcntl.h>
except ImportError:
    fcntl = None
    F_SETPIPE_SZ = 0

import config
import stream  # 复用 ensure_encoder() 的 h264_rkmpp / libx264 探测

try:
    import cjk_font          # 预烘焙的 16x16 中英文点阵字库（tools/gen_cjk_font.py 生成）
except ImportError:          # 字库没同步到板端时降级回旧的英文 5x7 点阵，不中断识别
    cjk_font = None


def _log_tail(limit=8, width=170):
    """读 ffmpeg 日志尾部，供 /api/detect/status 回传前端。

    现场排障时用户往往只看网页、不方便 SSH 上板，把最后几行直接带回来，
    就能区分「preview path 未登记」「源流无 publisher」「编码器不可用」。
    """
    try:
        with open(config.OSD_LOG, "rb") as f:
            f.seek(0, os.SEEK_END)
            f.seek(max(0, f.tell() - 8192))
            data = f.read()
    except OSError:
        return "(日志不可读)"
    lines = [ln.strip()[:width] for ln in
             data.decode("utf-8", "replace").splitlines() if ln.strip()]
    return " / ".join(lines[-limit:]) if lines else "(日志为空)"


# ---- 5x7 点阵字库（只覆盖标签里会出现的 ASCII；其余显示 ?）----
# 每行 5bit，位 1 表示该像素点亮。缩放 2 倍后约 14px 高，足够现场辨认。
_FONT = {
    '0': (0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E),
    '1': (0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E),
    '2': (0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F),
    '3': (0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E),
    '4': (0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02),
    '5': (0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E),
    '6': (0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E),
    '7': (0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08),
    '8': (0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E),
    '9': (0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C),
    'A': (0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11),
    'B': (0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E),
    'C': (0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E),
    'D': (0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E),
    'E': (0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F),
    'F': (0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10),
    'G': (0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F),
    'H': (0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11),
    'I': (0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E),
    'J': (0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C),
    'K': (0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11),
    'L': (0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F),
    'M': (0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11),
    'N': (0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11),
    'O': (0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E),
    'P': (0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10),
    'Q': (0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D),
    'R': (0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11),
    'S': (0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E),
    'T': (0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04),
    'U': (0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E),
    'V': (0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04),
    'W': (0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11),
    'X': (0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11),
    'Y': (0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04),
    'Z': (0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F),
    '.': (0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C),
    ':': (0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00),
    '-': (0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00),
    '_': (0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F),
    '%': (0x19, 0x19, 0x02, 0x04, 0x08, 0x13, 0x13),
    ' ': (0x00,) * 7,
    '?': (0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04),
}

LABEL_H = 19         # 标签条高度：16px 字模 + 上下留白（放大到画面上是 38px）
LINE_PX = 1          # 画布上框线宽（放大后 2px）
MAX_TEXT = 32        # 标签最多显示字符数（中文按 1 字符算）

# 字库缺字记录：改了 meta.json 的中文名却没重跑 gen_cjk_font.py 时，
# 对应位置会画空心方框，这里把字收集起来通过 /api/detect/status 报给前端。
_MISSING = set()
_MISS_LOCK = threading.Lock()
# 文字 -> (像素宽, 点坐标元组) 缓存。框文字在两次推理之间不变，但画布按
# 8fps 重绘，缓存后每帧只做「按点写像素」，省掉重复的点阵解码。
_TEXT_CACHE = {}
_TEXT_CACHE_LOCK = threading.Lock()
_TEXT_CACHE_MAX = 256
# 缺字占位：16x16 空心方框
_BOX_ROWS = tuple(0xFFFF if r in (0, 15) else 0x8001 for r in range(16))


def _text_pixels(text):
    """把一条标签文字渲染成 (总宽, ((x,y), ...))；字模缺失的字画空心方框。"""
    with _TEXT_CACHE_LOCK:
        hit = _TEXT_CACHE.get(text)
    if hit is not None:
        return hit
    pts = []
    x = 0
    for ch in text:
        g = cjk_font.glyph(ch) if cjk_font is not None else None
        if g is None:
            adv, rows = 16, _BOX_ROWS
        else:
            adv, rows = g
        for row, bits in enumerate(rows):
            if not bits:
                continue
            for col in range(16):
                if bits & (1 << (15 - col)):
                    pts.append((x + col, row))
        x += adv + 1
    result = (x, tuple(pts))
    with _TEXT_CACHE_LOCK:
        if len(_TEXT_CACHE) >= _TEXT_CACHE_MAX:
            _TEXT_CACHE.clear()
        _TEXT_CACHE[text] = result
    return result


def _rgba(color):
    """'#RRGGBB' -> bytes(r,g,b,255)；非法颜色用默认红。"""
    if not (isinstance(color, str) and len(color) == 7 and color[0] == "#"):
        color = "#E24B4A"
    try:
        return bytes((int(color[1:3], 16), int(color[3:5], 16),
                      int(color[5:7], 16), 255))
    except ValueError:
        return b"\xe2\x4b\x4a\xff"


class OsdEncoder:
    """一路带框预览编码器：detected boxes -> RGBA 画布 -> ffmpeg overlay -> RTSP。"""

    def __init__(self):
        self._lock = threading.RLock()
        self._proc = None
        self._writer = None
        self._stop = threading.Event()
        self._cam = ""
        self._boxes = []
        self._frame_wh = (config.CAPTURE_WIDTH, config.CAPTURE_HEIGHT)
        self._error = ""
        self._frames = 0
        self._session = 0           # 每次 start 递增；前端据此判断「是否该重试切 preview」
        self._log_fh = None
        self._w = config.OSD_W
        self._h = config.OSD_H
        self._zero = bytes(self._w * self._h * 4)
        self._buf = bytearray(self._zero)
        # 画布写入实测值，供 /api/detect/status 与状态栏做延时自诊断
        self._write_fps = 0.0
        self._render_ms = 0.0
        self._last_write_t = 0.0

    # ---------- 对外：接收检测结果 ----------
    def update(self, cam, boxes, frame_w, frame_h):
        """由 detect._emit() 在每次推理后调用；只接受当前 OSD 源那一路。"""
        with self._lock:
            if cam != self._cam or not self.running():
                return
            try:
                fw = float(frame_w) or config.CAPTURE_WIDTH
                fh = float(frame_h) or config.CAPTURE_HEIGHT
            except (TypeError, ValueError):
                fw, fh = config.CAPTURE_WIDTH, config.CAPTURE_HEIGHT
            items = []
            missed = set()
            for b in boxes or []:
                try:
                    # 版本 2.2：直接用 detect.py 统一生成的中文标签（含同类数量），
                    # 例「火焰 3处」。旧调用方没带 text 时退化为中文名/英文类名。
                    text = str(b.get("text") or b.get("zh")
                               or b.get("label") or "?")[:MAX_TEXT]
                    if cjk_font is not None:
                        missed.update(cjk_font.missing(text))
                    items.append((
                        float(b["x1"]), float(b["y1"]),
                        float(b["x2"]), float(b["y2"]),
                        _rgba(b.get("color")), text,
                    ))
                except (KeyError, TypeError, ValueError):
                    continue
            if missed:
                with _MISS_LOCK:
                    _MISSING.update(missed)
            self._boxes = items
            self._frame_wh = (fw, fh)

    # ---------- 对外：启停 ----------
    def start(self, cam):
        with self._lock:
            self._session += 1
        if not config.DETECT_OSD_ENABLED:
            self._error = "板端画框已禁用（DETECT_OSD_ENABLED=0）"
            return False
        if cam not in config.CAMERAS:
            self._error = "不支持的摄像头 %s" % cam
            return False
        self.stop()
        if shutil.which("ffmpeg") is None:
            self._error = "ffmpeg 不可用"
            return False
        try:
            if os.path.exists(config.OSD_FIFO):
                os.unlink(config.OSD_FIFO)
            os.mkfifo(config.OSD_FIFO)
        except (AttributeError, OSError) as e:
            self._error = "FIFO 创建失败: %s" % e
            return False

        # ffmpeg 的报错必须留痕：preview path 没登记、编码器不支持、源流没
        # publisher 都会让它 1~2s 内退出。以前 stderr 直接丢进 DEVNULL，
        # 前端只看到「running=true」就切流，结果把正常画面切成了黑屏。
        try:
            log_fh = open(config.OSD_LOG, "ab", buffering=0)
            log_fh.write(("\n==== OSD start %s cam=%s ====\n"
                          % (time.strftime("%Y-%m-%d %H:%M:%S"), cam)).encode())
        except OSError:
            log_fh = None
        sink = log_fh if log_fh else subprocess.DEVNULL
        try:
            enc = stream.ensure_encoder()
            proc = subprocess.Popen(self._build_cmd(cam, enc),
                                    stdin=subprocess.DEVNULL,
                                    stdout=sink, stderr=sink)
        except Exception as e:
            self._error = "ffmpeg 启动失败: %s" % e
            if log_fh:
                log_fh.close()
            return False

        with self._lock:
            self._proc = proc
            self._log_fh = log_fh
            self._cam = cam
            self._boxes = []
            self._error = ""
            self._frames = 0
            self._write_fps = 0.0
            self._render_ms = 0.0
            self._last_write_t = 0.0
            self._stop = threading.Event()
            self._writer = threading.Thread(target=self._writer_loop,
                                            name="osd-writer", daemon=True)
            self._writer.start()

        # 启动确认窗口：只有 ffmpeg 活着且画布确实被读走，才允许前端把干净流
        # 切成 preview。旧版固定 4s / 要求 2 帧太苛刻：RTSP 连接 + 硬编初始化
        # 稍慢就 kill 掉一个本来正常的 ffmpeg，preview 永远起不来。
        deadline = time.monotonic() + config.OSD_READY_TIMEOUT
        writer_err = ""
        while time.monotonic() < deadline:
            rc = proc.poll()
            if rc is not None:
                self._error = ("ffmpeg 启动后退出（退出码 %s）：检查 mediamtx 是否已重启"
                               "并登记 %s path、编码器 %s 是否可用、%s 源流是否有 publisher。"
                               "日志尾部：%s"
                               % (rc, config.OSD_STREAM, enc, cam, _log_tail()))
                self.stop()
                return False
            with self._lock:
                ready = self._frames >= 1
                writer_err = self._error or writer_err
            if ready:
                return True
            time.sleep(0.1)
        self._error = ("preview 流 %.0fs 内未建立（%s）：常见原因是 %s 源流没有 publisher、"
                       "mediamtx 未登记 %s path、或编码器 %s 不可用。日志尾部：%s"
                       % (config.OSD_READY_TIMEOUT,
                          writer_err or "ffmpeg 未读走画布",
                          cam, config.OSD_STREAM, enc, _log_tail()))
        self.stop()
        return False

    def stop(self):
        with self._lock:
            proc, writer, stop_evt = self._proc, self._writer, self._stop
            log_fh, self._log_fh = self._log_fh, None
            self._proc = None
            self._writer = None
            self._cam = ""
            self._boxes = []
            stop_evt.set()
        if proc:
            try:
                proc.terminate()
            except Exception:
                pass
            try:
                proc.wait(timeout=2)
            except Exception:
                try:
                    proc.kill()
                except Exception:
                    pass
        if writer and writer is not threading.current_thread():
            writer.join(timeout=2)
        if log_fh:
            try:
                log_fh.close()
            except OSError:
                pass
        try:
            if os.path.exists(config.OSD_FIFO):
                os.unlink(config.OSD_FIFO)
        except OSError:
            pass

    def running(self):
        with self._lock:
            return bool(self._proc and self._proc.poll() is None
                        and self._writer and self._writer.is_alive())

    def status(self):
        log_tail = _log_tail()      # 读文件不放在锁内
        with _MISS_LOCK:
            missing = sorted(_MISSING)
        with self._lock:
            running = self.running()
            return {
                "enabled": bool(config.DETECT_OSD_ENABLED),
                "running": running,
                "ready": bool(running and self._frames > 0),
                "session": self._session,
                "cam": self._cam,
                "stream": config.OSD_STREAM,
                "error": self._error,
                "frames": self._frames,
                "fps": config.OSD_FPS,
                "video_fps": config.OSD_FPS,
                "fifo_fps": config.OSD_FIFO_FPS,
                "write_fps": self._write_fps,
                "render_ms": self._render_ms,
                "low_latency": bool(config.OSD_LOW_LATENCY),
                # 中文字库状态：缺字时现场一眼能看出要重跑 gen_cjk_font.py
                "font": "cjk16" if cjk_font is not None else "ascii5x7",
                "font_glyphs": (cjk_font.GLYPH_COUNT if cjk_font is not None else 0),
                "missing_glyphs": missing,
                "canvas": [self._w, self._h],
                "fifo": config.OSD_FIFO,
                "log": config.OSD_LOG,
                "log_tail": log_tail,
            }

    # ---------- ffmpeg 命令 ----------
    def _build_cmd(self, cam, enc):
        src_stream = config.CAMERAS[cam]["stream"] if cam in config.CAMERAS else cam
        src = "rtsp://127.0.0.1:%d/%s" % (config.RTSP_PORT, src_stream)
        vfps = max(1, int(config.OSD_FPS))          # 视频输出帧率
        cfps = max(1, int(config.OSD_FIFO_FPS))     # 画布写入帧率（远低于 vfps）

        # 两个输入都按「帧序号」重造时间戳，而不是沿用各自来源的 PTS。
        # RTSP 输入的 start_time 由 mediamtx 决定（可能是任意大值），rawvideo
        # FIFO 却从 0 开始；framesync 会把差异理解成「某一路落后」，于是
        # 缓存/等待，主视频帧越堆越多 —— 这是几秒延时的第二大来源。
        main = ("setpts=N/(%d*TB),fps=%d,scale=%d:%d,format=yuv420p"
                % (vfps, vfps, config.CAPTURE_WIDTH, config.CAPTURE_HEIGHT))
        overlay = ("setpts=N/(%d*TB),scale=%d:%d,format=rgba"
                   % (cfps, config.CAPTURE_WIDTH, config.CAPTURE_HEIGHT))
        # repeatlast=1：画布 8fps 时，中间那些视频帧重复上一张画布，主视频
        # 完全不被阻塞；eof_action=pass：writer 退出后视频照常通过而非卡住。
        fc = ("[0:v]%s[bg];[1:v]%s[ov];"
              "[bg][ov]overlay=0:0:format=yuv420:shortest=0:repeatlast=1:"
              "eof_action=pass,format=yuv420p[out]" % (main, overlay))

        enc_args = ["-c:v", enc]
        if enc == "libx264":
            # zerolatency 关掉 rc-lookahead / sync-lookahead，避免编码器预读
            enc_args += ["-preset", "ultrafast", "-tune", "zerolatency"]

        # RTSP 输入必须关掉默认流探测（analyzeduration 默认最长 5s，探测没结束
        # 就不会去打开第二个输入 FIFO，启动窗口内一帧都读不走）。
        in_flags = ["-analyzeduration", "0", "-probesize", "32768",
                    "-flags", "low_delay"]
        if config.OSD_LOW_LATENCY:
            # nobuffer 不让 demux 预读；avioflags direct 绕开 AVIO 缓冲；
            # 小 thread_queue_size 让「跟不上时丢旧帧」而不是排队变成旧画面。
            in_flags += ["-fflags", "+genpts+nobuffer",
                         "-avioflags", "direct"]
        else:
            in_flags += ["-fflags", "+genpts"]
        in_flags += ["-thread_queue_size", "8" if config.OSD_LOW_LATENCY else "16"]

        out_flags = []
        if config.OSD_LOW_LATENCY:
            # muxdelay/muxpreload 归零、立即 flush、-vsync 0 禁止输出侧帧率
            # 转换再插一层缓冲。setpts 已从 0 起且 -bf 0（DTS=PTS），
            # 故不再需要 -avoid_negative_ts make_zero（它会让 muxer 预读平移）。
            out_flags += ["-muxdelay", "0", "-muxpreload", "0",
                          "-flush_packets", "1", "-vsync", "0"]

        return (["ffmpeg", "-hide_banner", "-loglevel", "warning", "-nostdin",
                 "-rtsp_transport", "tcp"] + in_flags + ["-i", src,
                 # 画布输入队列保持 1：宁可丢旧画布，也不让框越来越滞后
                 "-thread_queue_size", "1",
                 "-f", "rawvideo", "-pixel_format", "rgba",
                 "-video_size", "%dx%d" % (self._w, self._h),
                 "-framerate", str(cfps), "-i", config.OSD_FIFO,
                 "-filter_complex", fc, "-map", "[out]", "-an"]
                + enc_args
                + ["-b:v", config.H264_BITRATE, "-g", str(config.OSD_GOP),
                   "-bf", "0", "-color_range", "pc"]
                + out_flags
                + ["-rtsp_transport", "tcp", "-f", "rtsp",
                   "rtsp://127.0.0.1:%d/%s" % (config.RTSP_PORT,
                                               config.OSD_STREAM)])

    # ---------- 画布渲染（纯标准库）----------
    def _hline(self, x1, x2, y, color):
        y = int(y)      # 框坐标由原图缩放而来，可能是浮点数
        if y < 0 or y >= self._h:
            return
        a, b = max(0, int(x1)), min(self._w, int(x2))
        if b <= a:
            return
        off = (y * self._w + a) * 4
        self._buf[off:off + (b - a) * 4] = color * (b - a)

    def _vline(self, x, y1, y2, color):
        if x < 0 or x >= self._w:
            return
        a, b = max(0, int(y1)), min(self._h, int(y2))
        for y in range(a, b):
            off = (y * self._w + int(x)) * 4
            self._buf[off:off + 4] = color

    def _rect(self, x1, y1, x2, y2, color):
        for i in range(LINE_PX):
            self._hline(x1, x2, y1 + i, color)
            self._hline(x1, x2, y2 - i, color)
            self._vline(x1 + i, y1, y2, color)
            self._vline(x2 - i, y1, y2, color)

    def _label(self, x, y, text, color):
        """标签底色条 + 16x16 中英文点阵白字（例「火焰 3处」）。"""
        if cjk_font is None:
            return self._label_ascii(x, y, text, color)
        tw, pts = _text_pixels(text)
        x0, y0 = max(0, int(x)), max(0, int(y) - LABEL_H)
        bar_w = min(self._w - x0, tw + 4)
        for yy in range(y0, min(self._h, y0 + LABEL_H)):
            self._hline(x0, x0 + bar_w, yy, color)
        white = b"\xff\xff\xff\xff"
        px, py = x0 + 2, y0 + 1
        for dx, dy in pts:
            xx, yy = px + dx, py + dy
            if 0 <= xx < self._w and 0 <= yy < self._h:
                off = (yy * self._w + xx) * 4
                self._buf[off:off + 4] = white

    def _label_ascii(self, x, y, text, color):
        """降级路径：cjk_font.py 没同步到板端时用旧的 5x7 英文点阵。"""
        tw = len(text) * 6 + 4
        x0, y0 = max(0, int(x)), max(0, int(y) - LABEL_H)
        for yy in range(y0, min(self._h, y0 + LABEL_H)):
            self._hline(x0, x0 + tw, yy, color)
        white = b"\xff\xff\xff\xff"
        px, py = x0 + 2, y0 + 1
        for ch in text.upper():
            glyph = _FONT.get(ch if ch.isascii() else "?", _FONT["?"])
            for row, bits in enumerate(glyph):
                for col in range(5):
                    if bits & (1 << (4 - col)):
                        xx, yy = px + col, py + row
                        if 0 <= xx < self._w and 0 <= yy < self._h:
                            off = (yy * self._w + xx) * 4
                            self._buf[off:off + 4] = white
            px += 6

    def _render(self):
        with self._lock:
            boxes = list(self._boxes)
            fw, fh = self._frame_wh
        if not boxes:
            return self._zero        # 无检出时零渲染、零拷贝，直接写预置全透明帧
        self._buf[:] = self._zero
        sx = self._w / max(1.0, fw)
        sy = self._h / max(1.0, fh)
        for x1, y1, x2, y2, color, text in boxes:
            ax, ay = x1 * sx, y1 * sy
            bx, by = x2 * sx, y2 * sy
            self._rect(ax, ay, bx, by, color)
            self._label(ax, ay, text, color)
        return bytes(self._buf)

    # ---------- 写 FIFO 的节拍线程 ----------
    def _writer_loop(self):
        fifo, stop_evt = config.OSD_FIFO, self._stop
        fd = None
        tries = 0
        while not stop_evt.is_set():
            try:
                fd = os.open(fifo, os.O_WRONLY | os.O_NONBLOCK)
                break
            except OSError:
                tries += 1
                if tries > 100:      # 10s 内 ffmpeg 没来读，判定启动失败
                    with self._lock:
                        self._error = "ffmpeg 未打开 OSD 画布管道（10s 超时）"
                    return
                if stop_evt.wait(0.1):
                    return
        if fd is None:
            return
        # Linux：把管道缓冲扩到能装下整整一帧画布，0.9MB 一次 write 写完，
        # 省掉几十次系统调用，也避免写一半被阻塞打乱节拍。失败不影响功能。
        if fcntl is not None:
            try:
                fcntl.fcntl(fd, F_SETPIPE_SZ, self._w * self._h * 4 + 4096)
            except (OSError, ValueError):
                pass
        try:
            os.set_blocking(fd, True)
            # 关键：节拍按「画布帧率」而非视频帧率。视频 30fps 由 ffmpeg 主输入
            # 自己驱动，这里只负责 8fps 更新框，overlay 会重复上一张画布。
            interval = 1.0 / max(1, config.OSD_FIFO_FPS)
            while not stop_evt.is_set():
                t0 = time.monotonic()
                frame = self._render()
                view = memoryview(frame)
                while view:
                    n = os.write(fd, view)
                    view = view[n:]
                cost = time.monotonic() - t0
                now = time.monotonic()
                with self._lock:
                    self._frames += 1
                    self._render_ms = round(cost * 1000, 1)
                    if self._last_write_t:
                        inst = 1.0 / max(0.001, now - self._last_write_t)
                        self._write_fps = (round(self._write_fps * 0.7 + inst * 0.3, 1)
                                           if self._write_fps else round(inst, 1))
                    self._last_write_t = now
                # 渲染+写入超时就直接跑下一帧，绝不累积节拍债务
                if stop_evt.wait(max(0.0, interval - cost)):
                    break
        except OSError as e:
            with self._lock:
                self._error = "OSD 画布写入中断: %s" % e
        finally:
            try:
                os.close(fd)
            except OSError:
                pass


encoder = OsdEncoder()


def start(cam):
    return encoder.start(cam)


def stop():
    encoder.stop()


def update(cam, boxes, frame_w, frame_h):
    encoder.update(cam, boxes, frame_w, frame_h)


def status():
    return encoder.status()
