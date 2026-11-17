#!/usr/bin/env python3
"""触发 vs 帧率 客观测量 —— 用「数帧数」回答"触发是否控制出帧"

为什么需要它
------------
厂商软件只把**最新一帧**显示在窗口里，刷新很快，人眼分不出 2fps 和 30fps，
所以"看画面"这条判据不可靠（也数不出张数）。
换成**在固定时间窗口内统计相机实际交付了多少帧**，结论就是数字，不需要肉眼判断。

三段测量（见 DEVELOPER_GUIDE.md 13.14）
--------------------------------------
    ① 不触发（基线）   10 秒 ≈ 300 帧   → 相机自由运行 30fps
    ② 触发 2 Hz        10 秒 ≈  20 帧   → 出帧被触发控制 ✅ 外触发模式成立
                       10 秒仍 ≈300 帧 → 触发与出帧无关 ❌ 只是拍照功能
    ③ 触发 30 Hz       10 秒 ≈ 300 帧   → 配合 ② 即可确认「支持 30fps 外触发」

原理：ffmpeg 从 /dev/videoX 读流，加 `-progress pipe:1` 输出机读进度，
      其中 `frame=` 就是**实际交付的帧数**（不是按 -framerate 推算的），
      所以能真实反映"触发时相机到底给了几帧"。

用法
----
    # 三段全跑（推荐）：先测基线，再测低频触发，再测高频触发
    python3 trigger_frame_test.py --video /dev/video121 --gpio 46

    # 自定义频率与时长
    python3 trigger_frame_test.py --video /dev/video121 --gpio 46 \
        --hz 1 --hz2 30 --seconds 10

    # 只测某几段
    python3 trigger_frame_test.py --video /dev/video121 --gpio 46 --skip-high
    python3 trigger_frame_test.py --video /dev/video121 --skip-base --hz2 0 --hz 5

    # 只看计划不动硬件
    python3 trigger_frame_test.py --video /dev/video121 --gpio 46 --dry-run

前置要求
--------
1. 摄像头接在**板子**上（不是 Windows），设备节点用 `v4l2-ctl --list-devices` 确认
2. 该 video 节点当前没有被其他程序占用（先停掉 Agent / ffmpeg / mediamtx 对应推流）
3. 补光灯要让出 GPIO：`FILL_LIGHT_GPIO=-1 python3 agent.py`
"""
import argparse
import os
import re
import subprocess
import sys
import threading
import time

GPIO_ROOT = "/sys/class/gpio"


def _write(path, text):
    with open(path, "w") as f:
        f.write(str(text))


def gpio_set(gpio, level):
    _write(f"{GPIO_ROOT}/gpio{gpio}/value", level)


def gpio_prepare(gpio):
    """导出并把引脚置为空闲高电平（不触发状态）。"""
    path = f"{GPIO_ROOT}/gpio{gpio}"
    if not os.path.isdir(path):
        try:
            _write(f"{GPIO_ROOT}/export", gpio)
            time.sleep(0.05)
        except Exception as e:
            print(f"[x] export gpio{gpio} 失败: {e}")
            print("    'Device or resource busy' → 该脚被内核驱动占用，换一个空闲脚")
            return False
    _write(path + "/direction", "out")
    _write(path + "/value", 1)          # 空闲 = 高（无下降沿 = 不触发）
    return True


class WaveThread(threading.Thread):
    """后台按 hz 输出方波：每个周期产生 1 个下降沿 = 1 次触发。"""

    def __init__(self, gpio, hz):
        super().__init__(daemon=True)
        self.gpio = gpio
        self.hz = hz
        self._stop = threading.Event()
        self.pulses = 0

    def run(self):
        half = 1.0 / (self.hz * 2.0)
        while not self._stop.is_set():
            gpio_set(self.gpio, 1)
            if self._stop.wait(half):
                break
            gpio_set(self.gpio, 0)      # 下降沿 → 触发
            self.pulses += 1
            if self._stop.wait(half):
                break
        gpio_set(self.gpio, 1)          # 收尾回到空闲高

    def stop(self):
        self._stop.set()


def measure(video, seconds, fmt, size, framerate):
    """跑 ffmpeg 读流 seconds 秒（墙钟），返回实际交付的帧数。"""
    cmd = [
        "ffmpeg", "-hide_banner", "-nostdin", "-loglevel", "error",
        "-progress", "pipe:1", "-nostats",
        "-f", "v4l2", "-input_format", fmt,
        "-video_size", size, "-framerate", str(framerate),
        "-i", video,
        "-f", "null", "-",
    ]
    try:
        p = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True,
                             encoding="utf-8", errors="replace", bufsize=1)
    except Exception as e:
        print(f"[x] 无法启动 ffmpeg: {e}")
        return None, ""

    state = {"frames": 0}

    def reader():
        for line in p.stdout:
            m = re.match(r"frame=(\d+)", line.strip())
            if m:
                v = int(m.group(1))
                if v > state["frames"]:
                    state["frames"] = v

    th = threading.Thread(target=reader, daemon=True)
    th.start()

    t0 = time.time()
    while time.time() - t0 < seconds:
        time.sleep(0.2)
        if p.poll() is not None:        # ffmpeg 自己退出了（设备打不开等）
            break
    p.terminate()
    try:
        p.wait(timeout=5)
    except Exception:
        p.kill()
    th.join(timeout=3)

    err = ""
    try:
        err = (p.stderr.read() or "").strip()
    except Exception:
        pass
    return state["frames"], err


def main():
    ap = argparse.ArgumentParser(description="用数帧数判断触发是否控制出帧")
    ap.add_argument("--video", required=True, help="板子上的 video 节点，如 /dev/video121")
    ap.add_argument("--gpio", type=int, help="触发用 GPIO（不填则只测基线）")
    ap.add_argument("--hz", type=float, default=2.0, help="低频触发频率 Hz（默认 2）")
    ap.add_argument("--hz2", type=float, default=30.0, help="高频触发频率 Hz（默认 30，设 0 跳过）")
    ap.add_argument("--seconds", type=float, default=10.0, help="每段测量时长（默认 10 秒）")
    ap.add_argument("--format", default="mjpeg", help="输入格式（默认 mjpeg）")
    ap.add_argument("--size", default="1280x720", help="分辨率（默认 1280x720）")
    ap.add_argument("--framerate", type=int, default=30, help="请求帧率（默认 30）")
    ap.add_argument("--skip-base", action="store_true", help="跳过第①段基线")
    ap.add_argument("--skip-high", action="store_true", help="跳过第③段高频触发")
    ap.add_argument("--settle", type=float, default=2.0, help="每段之间的间隔秒数（默认 2）")
    ap.add_argument("--dry-run", action="store_true", help="只打印计划")
    args = ap.parse_args()

    phases = []
    if not args.skip_base:
        phases.append((f"① 不触发（基线）", None))
    if args.gpio is not None and args.hz > 0:
        phases.append((f"② 低频触发 {args.hz:g} Hz", args.hz))
    if args.gpio is not None and args.hz2 > 0 and not args.skip_high:
        phases.append((f"③ 高频触发 {args.hz2:g} Hz", args.hz2))

    if not phases:
        print("[x] 没有任何测量阶段，检查参数（--skip-base / --hz / --hz2）")
        return

    print("===== 测量计划 =====")
    print(f"  设备: {args.video}   格式: {args.format} {args.size} @{args.framerate}")
    for name, hz in phases:
        tail = f"（GPIO{args.gpio} 输出 {hz:g} Hz 方波）" if hz else "（不触发）"
        print(f"  {name} 测 {args.seconds:g}s {tail}")
    if args.dry_run:
        return

    if not os.path.isdir("/sys/class/gpio"):
        print("[!] /sys/class/gpio 不存在，GPIO 触发不可用（改动仍需实测）")

    if not os.path.exists(args.video):
        print(f"[x] 设备 {args.video} 不存在，先用 v4l2-ctl --list-devices 确认")
        return

    if args.gpio is not None:
        if not gpio_prepare(args.gpio):
            return
        print(f"\n[i] gpio{args.gpio} 已置为空闲高电平（不触发）")

    results = []
    for name, hz in phases:
        print(f"\n----- {name} -----")
        print("  3 秒后开始…", flush=True)
        time.sleep(3)
        wave = None
        if hz:
            wave = WaveThread(args.gpio, hz)
            wave.start()
        print(f"  测量中（{args.seconds:g}s）…", flush=True)
        frames, err = measure(args.video, args.seconds, args.format, args.size, args.framerate)
        if wave:
            wave.stop()
            gpio_set(args.gpio, 1)      # 确保回到空闲高
        if frames is None:
            print("  [x] 测量失败")
            return
        fps = frames / args.seconds
        print(f"  实际收到 {frames} 帧 → 平均 {fps:.1f} fps")
        if wave:
            print(f"  期间发出约 {wave.pulses} 个下降沿（{wave.pulses/args.seconds:.1f}/秒）")
        if err:
            print(f"  [ffmpeg] {err[:300]}")
        results.append({"name": name, "hz": hz, "frames": frames, "fps": fps})
        time.sleep(args.settle)

    if args.gpio is not None:
        gpio_set(args.gpio, 1)
        print(f"\n[i] gpio{args.gpio} 保持在空闲高电平")

    # ---------- 结论 ----------
    print("\n===== 结果汇总 =====")
    print(f"  {'阶段':<22} {'触发频率':<10} {'实收帧数':<10} {'平均fps':<10}")
    for r in results:
        hz_txt = f"{r['hz']:g} Hz" if r["hz"] else "—"
        print(f"  {r['name']:<22} {hz_txt:<10} {r['frames']:<10} {r['fps']:<10.1f}")

    base = next((r for r in results if r["hz"] is None), None)
    low = next((r for r in results if r["hz"] == args.hz and r["hz"] > 0), None)
    high = next((r for r in results if r["hz"] == args.hz2 and r["hz"] > 0), None)

    print("\n===== 判读 =====")
    if base and base["frames"] == 0:
        print("  [x] 连基线都是 0 帧 —— 设备可能被占用、格式/分辨率不支持，或没接好。")
        print("      先单独确认：ffmpeg -f v4l2 -input_format mjpeg -video_size "
              f"{args.size} -framerate {args.framerate} -i {args.video} -f null -")
        return

    if base:
        print(f"  ① 基线：{base['fps']:.1f} fps —— 相机自由运行的帧率")
    if low:
        if base and low["fps"] < base["fps"] * 0.6:
            if abs(low["fps"] - args.hz) <= max(2.0, args.hz * 0.6):
                print(f"  ② 低频触发：{low['fps']:.1f} fps ≈ 触发频率 {args.hz:g} Hz")
                print("     ✅ **出帧被触发控制** —— 外触发模式成立，四路同步可行！")
            else:
                print(f"  ② 低频触发：帧率掉到 {low['fps']:.1f} fps，与触发频率 {args.hz:g} Hz 不完全对应")
                print("     出帧确实受触发影响，但关系不是 1 触发 = 1 帧，需要再确认固件行为")
        else:
            print(f"  ② 低频触发：{low['fps']:.1f} fps，与基线基本相同")
            print("     ❌ **触发与出帧无关** —— 只是拍照功能，对实时视频无价值")
            print("        四路同步需要改用别的方案（软件时间对齐 / 服务器端合流）")
    if high:
        print(f"  ③ 高频触发 {args.hz2:g} Hz：{high['fps']:.1f} fps")
        if low and base and low["fps"] < base["fps"] * 0.6 and high["fps"] > low["fps"] * 2:
            if high["fps"] >= args.hz2 * 0.7:
                print(f"     ✅ 能跟上 {args.hz2:g} Hz 触发 → **支持 {args.hz2:g} fps 外触发**")
            else:
                print(f"     ⚠️ 跟不上 {args.hz2:g} Hz（只到 {high['fps']:.1f} fps）"
                      f" → 触发上限约 {high['fps']:.0f} Hz，30fps 同步可能不够")
    print("\n  把上面这几行结论回传，即可确定四路同步的落地方案。")


if __name__ == "__main__":
    main()
