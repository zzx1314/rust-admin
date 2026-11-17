"""任务存证：把命中那一帧的检出框画到图片上，连同 JSON 描述落盘。

设计目标（板端零新依赖）：
  - 不装 Pillow / OpenCV；中文字用 cjk_font.py 里预烘焙的 16x16 点阵，
    与板端 OSD / 浏览器回退共用同一份 box["text"]（例「火焰 3处 0.90」）。
  - 画框流程：原始 JPEG 由 detect 抓帧线程提供；这里在**原始分辨率**上
    画一个 RGBA 覆盖层，再用 ffmpeg overlay 合成回图片，输出 JPG。
    overlay 层是纯内存里的 RGBA 字节，经管道喂给 ffmpeg，不落临时透明图。
  - 失败安全：ffmpeg 不可用 / 画框异常时，退回「原图直接复制」，保证
    任务仍能留下证据图，只是没有框；绝不因为画框失败让任务丢失。

落盘结构：
  <DETECT_JOBS_DIR>/<job_id>/
      <cam>_<job_id>.jpg    命中帧（画框）
      result.json           任务描述（发现路 / 目标 / 结论 / 时间等）
"""
import json
import os
import re
import shutil
import subprocess
import threading
import time

import config

try:
    import cjk_font
except ImportError:
    cjk_font = None

# 与 osd.py 一致的画框常量（这里按原图全尺寸画，不缩放）
_LABEL_H = 22        # 标签条高度（原图像素）；16px 字模 + 上下留白
_LINE_PX = 2         # 框线宽（原图像素）
_MAX_TEXT = 32
_FILE_LOCK = threading.Lock()

# 缺字占位：16x16 空心方框
_BOX_ROWS = tuple(0xFFFF if r in (0, 15) else 0x8001 for r in range(16))


def _text_pixels(text):
    """把标签文字渲染成 (总宽, ((x,y),...))；缺字画空心方框。"""
    pts = []
    x = 0
    for ch in text:
        g = cjk_font.glyph(ch) if cjk_font is not None else None
        adv, rows = (16, _BOX_ROWS) if g is None else g
        for row, bits in enumerate(rows):
            if not bits:
                continue
            for col in range(16):
                if bits & (1 << (15 - col)):
                    pts.append((x + col, row))
        x += adv + 1
    return x, tuple(pts)


def _rgba(color):
    if not (isinstance(color, str) and len(color) == 7 and color[0] == "#"):
        color = "#E24B4A"
    try:
        return (int(color[1:3], 16), int(color[3:5], 16), int(color[5:7], 16))
    except ValueError:
        return (0xE2, 0x4B, 0x4A)


def _make_overlay(w, h, boxes):
    """在 w*h 的 RGBA 画布上画框 + 中文标签，返回 bytes(RGBA)。"""
    buf = bytearray(w * h * 4)   # 全透明

    def hline(x1, x2, y, color):
        a, b = max(0, int(min(x1, x2))), min(w, int(max(x1, x2)) + 1)
        yy = int(y)
        if yy < 0 or yy >= h:
            return
        rgb = bytes(color) + b"\xff"
        for xx in range(a, b):
            off = (yy * w + xx) * 4
            buf[off:off + 4] = rgb

    def vline(x, y1, y2, color):
        xx = int(x)
        if xx < 0 or xx >= w:
            return
        a, b = max(0, int(min(y1, y2))), min(h, int(max(y1, y2)) + 1)
        rgb = bytes(color) + b"\xff"
        for yy in range(a, b):
            off = (yy * w + xx) * 4
            buf[off:off + 4] = rgb

    def rect(x1, y1, x2, y2, color):
        for i in range(_LINE_PX):
            hline(x1, x2, y1 + i, color)
            hline(x1, x2, y2 - i, color)
            vline(x1 + i, y1, y2, color)
            vline(x2 - i, y1, y2, color)

    white = b"\xff\xff\xff\xff"
    for b in boxes or []:
        try:
            x1, y1, x2, y2 = (float(b["x1"]), float(b["y1"]),
                              float(b["x2"]), float(b["y2"]))
        except (KeyError, TypeError, ValueError):
            continue
        color = _rgba(b.get("color"))
        rect(x1, y1, x2, y2, color)
        text = str(b.get("text") or b.get("zh") or b.get("label") or "?")[:_MAX_TEXT]
        tw, pts = _text_pixels(text)
        # 标签底色条（在框上方；越界则压到框内）
        x0 = max(0, int(x1))
        y0 = int(y1) - _LABEL_H
        if y0 < 0:
            y0 = int(y1)
        bar_w = min(w - x0, tw + 6)
        rgb = bytes(color) + b"\xff"
        for yy in range(y0, min(h, y0 + _LABEL_H)):
            for xx in range(x0, x0 + bar_w):
                off = (yy * w + xx) * 4
                buf[off:off + 4] = rgb
        px, py = x0 + 3, y0 + 3
        for dx, dy in pts:
            xx, yy = px + dx, py + dy
            if 0 <= xx < w and 0 <= yy < h:
                off = (yy * w + xx) * 4
                buf[off:off + 4] = white
    return bytes(buf)


def _safe_job_id(job_id):
    """job_id 只允许安全字符，避免路径穿越 / 文件名注入。"""
    s = re.sub(r"[^A-Za-z0-9_\-\u4e00-\u9fff]", "_", str(job_id or "")).strip("_")
    return (s or "job")[:80]


def job_dir(job_id):
    return os.path.join(config.DETECT_JOBS_DIR, _safe_job_id(job_id))


def save_frame(jpg_path, boxes, frame_w, frame_h, out_path):
    """把画框后的帧写到 out_path。失败退回原图复制，返回是否画框成功。"""
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    try:
        w = max(1, int(frame_w))
        h = max(1, int(frame_h))
    except (TypeError, ValueError):
        w, h = config.CAPTURE_WIDTH, config.CAPTURE_HEIGHT

    overlay = _make_overlay(w, h, boxes)
    if shutil.which("ffmpeg") and boxes:
        cmd = [
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
            "-i", jpg_path,
            "-f", "rawvideo", "-pix_fmt", "rgba",
            "-video_size", "%dx%d" % (w, h), "-framerate", "1",
            "-i", "pipe:0",
            "-filter_complex", "[0:v][1:v]overlay=0:0:shortest=1[out]",
            "-map", "[out]", "-frames:v", "1", "-q:v", "3", out_path,
        ]
        try:
            p = subprocess.run(cmd, input=overlay, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, timeout=30)
            if p.returncode == 0 and os.path.isfile(out_path):
                return True
        except Exception:
            pass
    # 回退：原图直接复制（没有框，但证据不丢）
    try:
        shutil.copyfile(jpg_path, out_path)
    except Exception:
        pass
    return False


def write_result(job_id, payload):
    """把任务结果描述写到 <job>/result.json（原子写）。"""
    d = job_dir(job_id)
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, "result.json")
    tmp = path + ".tmp"
    with _FILE_LOCK:
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(payload, f, ensure_ascii=False, indent=2)
            f.write("\n")
        os.replace(tmp, path)
    return path


def save_not_found(job_id, payload):
    """未命中任务也留一份 result.json（无证据图）。"""
    return write_result(job_id, payload)


def cleanup_old(days=7):
    """可选：清理超过 N 天的任务目录（默认不自动调用）。"""
    root = config.DETECT_JOBS_DIR
    if not os.path.isdir(root):
        return 0
    now = time.time()
    removed = 0
    for name in os.listdir(root):
        p = os.path.join(root, name)
        if not os.path.isdir(p):
            continue
        try:
            if now - os.path.getmtime(p) > days * 86400:
                shutil.rmtree(p, ignore_errors=True)
                removed += 1
        except OSError:
            pass
    return removed
