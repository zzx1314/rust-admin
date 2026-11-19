#!/usr/bin/env python3
"""四路摄像头「帧时间戳」采集与同步量化 —— 回答"图片间隔/延时到底是多少"

为什么要这个工具
----------------
先把前提说清楚（本项目已实测确认）：

  1. **摄像头自由运行** ~28.5fps，**触发不影响视频流出帧**
     （描述符 bTriggerTriggerSupport=1 但 bTriggerUsage=0 = 静图语义；
       帧数实验：不触发 278 / 2Hz 触发 284 / 30Hz 触发 285，各 10 秒）。
  2. Linux 侧**只有一条能出图的节点**（/dev/video121 类型），
     video122 是 **Metadata Capture**（不出图）。
  3. uvcvideo **没有实现 UVC still image 捕获**。

⇒ 所以：
  · **不存在"GPIO 触发出图"的因果链**。GPIO 拉低只是一个**软件时间标记**。
  · 拔掉 2P 线，本工具照样能测（图像照出，只是摄像头内部不会去采那张静图）。
  · 你测到的"延时"是 **取帧/时间戳延迟**，不是"触发响应延迟"。
  · 四路共用同一个 GPIO **不能带来硬件同步**，它只提供一个共同的时间参考点。

本工具做的事（这是当前精度最高、可落地的方式）
--------------------------------------------
  · 每路一个常驻 ffmpeg，`-f image2pipe -c:v copy` 直接吐 MJPEG 原始帧（不落盘、不重编码）
  · 按 JPEG 的 SOI/SOS/EOI 做**段解析分帧**（不是简单搜 FFD9 —— 那样会被 EXIF 内嵌缩略图误判）
  · 每切出一个完整帧，立刻记 `time.time()` → **微秒级帧到达时间戳**
  · 可选 `--gpio N`：周期性拉低并记录每次触发时刻
  · 结束后输出：每路帧率/帧间隔分布、**四路帧到达时刻的相互偏差**、**触发→首帧延迟**
  · 可导出 CSV（每帧一行），便于自己用 Excel/pandas 分析

用法
----
    # 四路采 10 秒（先小分辨率试，USB 带宽有限）
    python3 multi_cam_timeline.py \
        --cams /dev/video121,/dev/video122,/dev/video123,/dev/video124 \
        --seconds 10 --csv /root/timeline.csv

    # 带 GPIO 触发（每 2 秒拉低一次），并记录触发时刻
    python3 multi_cam_timeline.py --cams <4路> --seconds 20 \
        --gpio 46 --trigger-interval 2000 --csv /root/timeline.csv

    # 只测两路、720p
    python3 multi_cam_timeline.py --cams /dev/video121,/dev/video122 \
        --seconds 5 --size 1280x720

输出怎么读
----------
    · 每路「帧间隔」：min/avg/max/σ。正常应 ≈ 33.3ms；max 明显偏大 = 丢帧
    · 「四路互差」：以第 1 路为基准，各路的帧时刻差值分布 —— 这就是**同步误差**
    · 「触发→首帧延迟」：拉低后各路第一帧到达相对触发时刻的延迟
      ⚠️ 它只反映"取帧链路延迟"，**不代表"摄像头响应触发的延时"**（见上文前提 3）
"""
import argparse
import os
import statistics
import subprocess
import sys
import threading
import time

GPIO_ROOT = "/sys/class/gpio"


# ---------------------------------------------------------------- JPEG 分帧

def _jpeg_end(buf, start=0):
    """从 start 处的 SOI 开始解析 JPEG 段，返回帧结束偏移（EOI 之后），-1 表示数据不全。

    按段解析而不是直接搜 FFD9：JPEG 的 EXIF 段里可能内嵌完整缩略图（含 FFD9），
    直接搜会提前截断。正确做法是跳过所有带长度字段的段，到达 SOS(FFDA) 之后
    再找第一个 FFD9 —— 熵编码数据里 FF 只会是 FF00 / FFD0~FFD7 / FFD9，所以安全。
    """
    n = len(buf)
    i = start + 2                       # 跳过 SOI(FFD8)
    while i + 1 < n:
        if buf[i] != 0xFF:
            i += 1
            continue
        m = buf[i + 1]
        if m == 0xD9:                   # EOI（异常位置，但仍作为结束）
            return i + 2
        if m == 0xDA:                   # SOS：之后是熵编码数据
            j = buf.find(b"\xff\xd9", i)
            return j + 2 if j >= 0 else -1
        if m == 0x01 or 0xD0 <= m <= 0xD7:
            i += 2
            continue
        if i + 4 > n:
            return -1
        seg_len = (buf[i + 2] << 8) | buf[i + 3]
        if seg_len < 2:
            return -1
        i += 2 + seg_len
    return -1


def iter_jpeg_frames(stream):
    """从字节流中切出完整 JPEG 帧，yield bytes。"""
    buf = bytearray()
    while True:
        chunk = stream.read(65536)
        if not chunk:
            break
        buf += chunk
        while True:
            i = buf.find(b"\xff\xd8\xff")
            if i < 0:
                if len(buf) > 2:
                    del buf[: len(buf) - 2]
                break
            if i > 0:
                del buf[:i]
            end = _jpeg_end(buf)
            if end < 0:
                break
            yield bytes(buf[:end])
            del buf[:end]


# ---------------------------------------------------------------- 帧采集

class CamStream(threading.Thread):
    """一路摄像头：常驻读流 + 分帧 + 记录每帧到达时间戳（可选把每帧存盘）。"""

    def __init__(self, name, dev, fmt, size, framerate,
                 save_dir=None, save_limit=100000):
        super().__init__(daemon=True)
        self.name = name
        self.dev = dev
        self.save_dir = save_dir
        self.save_limit = save_limit
        self.cmd = [
            "ffmpeg", "-hide_banner", "-nostdin", "-loglevel", "error",
            "-f", "v4l2", "-input_format", fmt, "-video_size", size,
            "-framerate", str(framerate), "-i", dev,
            "-c:v", "copy", "-f", "image2pipe", "-",
        ]
        self.stamps = []        # 每帧到达时刻（墙钟，秒）
        self.sizes = []         # 每帧字节数
        self.saved = 0
        self.error = None
        self._proc = None
        self._stop = threading.Event()

    def run(self):
        try:
            self._proc = subprocess.Popen(self.cmd, stdout=subprocess.PIPE,
                                          stderr=subprocess.PIPE)
        except Exception as e:
            self.error = f"启动 ffmpeg 失败: {e}"
            return
        try:
            idx = 0
            for frame in iter_jpeg_frames(self._proc.stdout):
                t = time.time()             # ★ 微秒级：整帧切出来的瞬间（在写盘之前取）
                self.stamps.append(t)
                self.sizes.append(len(frame))
                if self.save_dir and idx < self.save_limit:
                    p = os.path.join(self.save_dir, f"{self.name}_{idx:06d}.jpg")
                    try:
                        with open(p, "wb") as f:
                            f.write(frame)
                        self.saved += 1
                    except Exception:
                        pass
                idx += 1
        except Exception as e:
            self.error = f"读流异常: {e}"

    def stop(self):
        self._stop.set()
        if self._proc:
            try:
                self._proc.terminate()
                self._proc.wait(timeout=5)
            except Exception:
                try:
                    self._proc.kill()
                except Exception:
                    pass

    def stderr_text(self):
        try:
            if self._proc and self._proc.stderr:
                return (self._proc.stderr.read() or b"").decode("utf-8", "replace").strip()
        except Exception:
            pass
        return ""


# ---------------------------------------------------------------- GPIO

def gpio_prepare(gpio):
    path = f"{GPIO_ROOT}/gpio{gpio}"
    if not os.path.isdir(path):
        try:
            with open(f"{GPIO_ROOT}/export", "w") as f:
                f.write(str(gpio))
            time.sleep(0.05)
        except Exception as e:
            print(f"[x] export gpio{gpio} 失败: {e}")
            return False
    with open(path + "/direction", "w") as f:
        f.write("out")
    with open(path + "/value", "w") as f:
        f.write("1")            # 空闲高
    return True


def gpio_set(gpio, level):
    with open(f"{GPIO_ROOT}/gpio{gpio}/value", "w") as f:
        f.write(str(level))


# ---------------------------------------------------------------- 统计

def gaps(stamps):
    return [ (stamps[i + 1] - stamps[i]) * 1000.0 for i in range(len(stamps) - 1) ]


def fmt_gaps(gs):
    if not gs:
        return "无数据"
    return (f"min {min(gs):.1f} / avg {statistics.mean(gs):.1f} / "
            f"max {max(gs):.1f} ms  σ={statistics.pstdev(gs):.1f}ms")


def main():
    ap = argparse.ArgumentParser(description="四路摄像头帧时间戳采集与同步量化")
    ap.add_argument("--cams", required=True,
                    help="逗号分隔的 video 节点，如 /dev/video121,/dev/video122")
    ap.add_argument("--seconds", type=float, default=10.0, help="采集时长（默认 10 秒）")
    ap.add_argument("--format", default="mjpeg")
    ap.add_argument("--size", default="1280x720",
                    help="分辨率（默认 1280x720；USB2 HUB 上四路跑 1080p 会超带宽）")
    ap.add_argument("--framerate", type=int, default=30)
    ap.add_argument("--gpio", type=int, help="用于发触发的 GPIO（记录每次触发时刻）")
    ap.add_argument("--trigger-interval", type=int, default=2000,
                    help="触发周期 ms（默认 2000；需 --gpio）")
    ap.add_argument("--trigger-low", type=int, default=100,
                    help="触发低电平保持 ms（默认 100）")
    ap.add_argument("--csv", help="把每帧时间戳导出到该 CSV")
    ap.add_argument("--save-frames", metavar="DIR",
                    help="★把每帧存成 DIR/camN_000123.jpg —— 用于光学标定"
                         "（找'第一帧 LED 变亮'，再查 CSV 拿它的时间戳算真实端到端延迟）")
    ap.add_argument("--save-limit", type=int, default=200,
                    help="每路最多存多少帧（默认 200，约 6.7 秒@30fps）")
    ap.add_argument("--settle", type=float, default=2.0,
                    help="开始正式采集前先预热多少秒（默认 2s，让流稳定）")
    ap.add_argument("--dry-run", action="store_true", help="只打印计划")
    args = ap.parse_args()

    cams = [c.strip() for c in args.cams.split(",") if c.strip()]
    print("===== 采集计划 =====")
    print(f"  路数: {len(cams)}   节点: {', '.join(cams)}")
    print(f"  格式: {args.format} {args.size} @{args.framerate}")
    print(f"  时长: {args.seconds}s（另有 {args.settle}s 预热）")
    if args.gpio is not None:
        print(f"  触发: gpio{args.gpio} 每 {args.trigger_interval}ms 拉低 "
              f"{args.trigger_low}ms（仅作时间标记，不影响出图）")
    if args.dry_run:
        return

    for dev in cams:
        if not os.path.exists(dev):
            print(f"[x] {dev} 不存在")
            return

    print("\n>>> 提示：四路并发若在同一 USB2 HUB 上可能超带宽，"
          "先用 --size 640x480 试更稳\n")

    if args.save_frames:
        os.makedirs(args.save_frames, exist_ok=True)
        print(f"[i] 会把每路前 {args.save_limit} 帧存到 {args.save_frames}/"
              f"（⚠️ 写盘会略微影响读取，时间戳本身不受影响）\n")

    streams = [CamStream(f"cam{i}", dev, args.format, args.size, args.framerate,
                         save_dir=args.save_frames, save_limit=args.save_limit)
               for i, dev in enumerate(cams)]
    for s in streams:
        s.start()
    print(f"[i] 已启动 {len(streams)} 路读流，预热 {args.settle}s …")
    time.sleep(args.settle)

    for s in streams:
        if s.error:
            print(f"[x] {s.name} ({s.dev}) {s.error}")
            for x in streams:
                x.stop()
            return

    # 清掉预热期间的帧，正式计时
    for s in streams:
        s.stamps.clear()
        s.sizes.clear()

    if args.gpio is not None and not gpio_prepare(args.gpio):
        for x in streams:
            x.stop()
        return

    trig_times = []
    t0 = time.time()
    t_end = t0 + args.seconds
    next_trig = t0 + 0.1
    print(f"[i] 正式采集 {args.seconds}s …（Ctrl+C 可提前结束）")
    try:
        while time.time() < t_end:
            if args.gpio is not None and time.time() >= next_trig:
                t_edge = time.time()
                gpio_set(args.gpio, 0)
                trig_times.append(t_edge)
                # 低电平保持不阻塞采集：先记录，稍后恢复
                time.sleep(args.trigger_low / 1000.0)
                gpio_set(args.gpio, 1)
                next_trig = t_edge + args.trigger_interval / 1000.0
            else:
                time.sleep(0.01)
    except KeyboardInterrupt:
        print("\n[i] 提前结束")
    elapsed = time.time() - t0

    if args.gpio is not None:
        gpio_set(args.gpio, 1)
    for s in streams:
        s.stop()
    time.sleep(0.2)

    # ---------------- 结果 ----------------
    print(f"\n===== 每路统计（采集时长 {elapsed:.2f}s）=====")
    for s in streams:
        n = len(s.stamps)
        if n == 0:
            err = s.stderr_text()
            print(f"  {s.name} {s.dev}: ❌ 0 帧"
                  + (f"  [{err.splitlines()[0][:100]}]" if err else ""))
            continue
        gs = gaps(s.stamps)
        avg_kb = f"{statistics.mean(s.sizes)//1024} KB" if s.sizes else "?"
        print(f"  {s.name} {s.dev}: {n} 帧  {n/elapsed:.2f} fps  帧大小均 {avg_kb}")
        print(f"        帧间隔: {fmt_gaps(gs)}")
        if gs:
            drop = sum(1 for g in gs if g > 1.5 * (1000.0 / args.framerate))
            if drop:
                print(f"        ⚠️ 有 {drop} 个间隔 > 1.5 倍理论值（{(1000.0/args.framerate):.1f}ms）"
                      f" → 疑似丢帧/带宽不足")

    # 四路互差（以第 1 路为基准做最近邻匹配）
    ok = [s for s in streams if len(s.stamps) > 5]
    if len(ok) >= 2:
        base = ok[0]
        print(f"\n===== 四路帧到达时刻互差（基准 {base.name}，最近邻匹配）=====")
        for s in ok[1:]:
            diffs = []
            j = 0
            for t in base.stamps:
                while j + 1 < len(s.stamps) and abs(s.stamps[j + 1] - t) <= abs(s.stamps[j] - t):
                    j += 1
                diffs.append((s.stamps[j] - t) * 1000.0)
            if diffs:
                print(f"  {s.name} − {base.name}: 平均 {statistics.mean(diffs):+.1f} ms  "
                      f"σ={statistics.pstdev(diffs):.1f}ms  "
                      f"范围 {min(diffs):+.1f} ~ {max(diffs):+.1f} ms")
        print("  ⇒ σ 越小说明两路到达时刻越集中；但**这不代表同时曝光**，")
        print("    只反映【从各自流端点取到帧】的时刻散布（无硬件同步时典型为十几~几十 ms）。")

    # 触发 → 首帧延迟
    if trig_times and ok:
        interval_ms = 1000.0 / args.framerate
        print(f"\n===== 触发 → 该路下一帧到达的延迟（共 {len(trig_times)} 次触发）=====")
        for s in ok:
            lags = []
            for tt in trig_times:
                nxt = [x for x in s.stamps if x >= tt]
                if nxt:
                    lags.append((nxt[0] - tt) * 1000.0)
            if not lags:
                continue
            print(f"  {s.name}: 平均 {statistics.mean(lags):.1f} ms   "
                  f"min {min(lags):.1f} / max {max(lags):.1f} ms   "
                  f"σ={statistics.pstdev(lags):.1f}ms")
            # 直方图：把 [0, 帧间隔] 分 5 档 —— 形状是判断因果性的关键
            bins = [0] * 5
            for v in lags:
                k = min(4, max(0, int(v / interval_ms * 5)))
                bins[k] += 1
            mx = max(bins) or 1
            for i, c in enumerate(bins):
                lo, hi = interval_ms * i / 5.0, interval_ms * (i + 1) / 5.0
                print(f"      {lo:5.1f}~{hi:5.1f} ms | {c:3d} " + "#" * int(round(c / mx * 30)))
            print(f"      理论（触发与出帧无关时）：均匀分布于 0~{interval_ms:.1f}ms，"
                  f"均值 ≈ {interval_ms / 2:.1f}ms")
        print("\n  判读（★ 关键）：")
        print("    · min≈0、max≈帧间隔、均值≈半个帧间隔、直方图各档接近平均")
        print("      ⇒ **均匀分布** ⇒ 印证【触发与出帧无关】，它只是个软件时间标记")
        print("    · 若明显聚集在某值附近 ⇒ 才说明存在因果，需要另找解释")
        print("    ⚠️ 无论如何这个数**不是**「摄像头响应触发的延时」——")
        print("       触发不参与出帧（bTriggerUsage=0），两者之间没有硬件因果。")
        print("       要测真实端到端延迟：用 --save-frames 做 LED 光学标定（见 13.18）")

    # CSV
    if args.csv:
        try:
            with open(args.csv, "w", encoding="utf-8") as f:
                f.write("cam,dev,index,arrival_wallclock,bytes,dt_from_prev_ms\n")
                for s in streams:
                    prev = None
                    for i, (t, n) in enumerate(zip(s.stamps, s.sizes)):
                        dt = "" if prev is None else f"{(t-prev)*1000:.3f}"
                        f.write(f"{s.name},{s.dev},{i},{t:.6f},{n},{dt}\n")
                        prev = t
                for i, t in enumerate(trig_times):
                    f.write(f"TRIGGER,gpio{args.gpio},{i},{t:.6f},0,\n")
            print(f"\n[OK] 每帧时间戳已导出到 {args.csv}")
        except Exception as e:
            print(f"\n[x] 写 CSV 失败: {e}")

    print("\n判读要点：")
    print("  · 帧间隔 avg 应 ≈ 33.3ms（30fps）；max 偏大 = 丢帧；σ 大 = 到达时刻不稳定")
    print("  · 四路互差的 σ 才是你要的【同步散布】；无硬件同步时不可能接近 0")
    print("  · 想要真正的【同时曝光】，只能靠厂家提供 sensor 级同步（FSIN）或换支持同步的模组")


if __name__ == "__main__":
    main()
