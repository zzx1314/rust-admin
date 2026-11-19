#!/usr/bin/env python3
"""Linux 侧获取"2P 端子触发拍照"那张图 —— 机制探测 + 抓帧 + 软快门

重要认知（本机实测结论，决定了本工具为什么这样设计）
--------------------------------------------------
1. 摄像头**自由运行** ~28.5fps，而**触发不影响视频流出帧**：
   帧数实验 —— 不触发 278 帧 / 2Hz 触发 284 帧 / 30Hz 触发 285 帧（各 10 秒），几乎不变。
2. 描述符里 `bTriggerSupport=1` 且 `bTriggerUsage=0`（标准 UVC 的"触发用于静图"语义）；
   另有 `STILL_IMAGE_FRAME` 描述符（9 档尺寸，含 1280x960，`bEndpointAddress=0` 即 EP0）。
3. 由此得出三条实操结论：

   · 要"某一瞬间的画面" → **直接抓帧，不需要发触发**。
   · 要"触发那一刻的画面" → 触发信号是我们自己发的（时刻已知），因此必须**保持流常开**，
     在拉低的同一瞬间取"最新帧"（本工具 --shutter 就是这么做的）。
     ⚠️ `v4l2-ctl --stream-count=1` 做不到 —— 它每次都要重开设备/协商/启流，
     启动延迟几百毫秒，拿到的是"读的那一刻恰好最新的一帧"，与触发毫无关联。
   · 要"固件认定的那张静图" → 走厂商机制（XU 控件 / SDK），本工具目前拿不到；
     它与流帧可能不同（本机静图 1280x960、流 1920x1080）。
     厂商 XU 已定位为 **entity 4，CS 0x0A / 0x0B**（读取需先解决内核驱动占用，
     见 uvc_ctrl_probe.py）。

用法
----
    # 1) 列全部控件（含 XU）找『触发模式 / 触发源』；或逐个取值试
    python3 uvc_snapshot.py --ctrls --video /dev/video121
    python3 uvc_snapshot.py --set trigger_mode=1 --video /dev/video121

    # 2) 探测机制；并逐个节点试抓一帧，判断哪个节点能出图
    python3 uvc_snapshot.py --probe --video /dev/video121
    python3 uvc_snapshot.py --grab-all --outdir /tmp/nodes
    #     判读：Metadata Capture 的节点不出图；都能抓到同尺寸 = 同一物理流的多个入口

    # 3) 从视频流抓一帧（不需要触发）
    python3 uvc_snapshot.py --grab --video /dev/video121 -o /tmp/snap.jpg
    python3 uvc_snapshot.py --grab --video /dev/video121 --size 1920x1080 -o /tmp/full.jpg

    # 4) 先发一次 GPIO 触发，再抓一帧（与上一张对比，看触发是否改变画面内容）
    python3 uvc_snapshot.py --grab --video /dev/video121 --gpio 46 --trigger -o /tmp/after.jpg

    # 5) ★软快门：常驻读流，"下降沿 → 取帧"紧贴（最接近"触发瞬间"）
    python3 uvc_snapshot.py --shutter --video /dev/video121 --gpio 46 --count 3 --outdir /tmp/shots
    #     时序：拉低(下降沿=触发) → 立刻取最新帧 → 保持低 --low ms → 回高
    #     会打印"下降沿 → 取帧 用了 N ms"；--delay 可额外加等待（默认 0）

    # 6) 不接 GPIO，按回车手动拍（可用板子上的 SOS/STOP 按键当快门，自行对接）
    python3 uvc_snapshot.py --shutter --video /dev/video121 --outdir /tmp/shots
"""
import argparse
import glob
import os
import re
import shutil
import signal
import subprocess
import sys
import time

GPIO_ROOT = "/sys/class/gpio"
IFACE_CLASS_NAME = {
    "01": "Audio", "02": "CDC/Comm", "03": "HID", "08": "Mass Storage",
    "0e": "Video (UVC)", "fe": "Application Specific", "ff": "Vendor Specific",
}


def run(cmd, timeout=10):
    try:
        r = subprocess.run(cmd, shell=True, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
        return ((r.stdout or "") + (r.stderr or "")).strip()
    except Exception as e:
        return f"<执行失败: {e}>"


def read_file(path):
    try:
        with open(path) as f:
            return f.read().strip()
    except Exception:
        return ""


def find_usb_dir(video_dev):
    name = os.path.basename(video_dev)
    base = f"/sys/class/video4linux/{name}/device"
    if not os.path.exists(base):
        return None
    cur = os.path.realpath(base)
    for _ in range(8):
        if os.path.exists(os.path.join(cur, "idVendor")):
            return cur
        nxt = os.path.dirname(cur)
        if nxt == cur:
            break
        cur = nxt
    return None


def is_capture_node(dev):
    info = run(f"v4l2-ctl -d {dev} --all")
    caps = re.search(r"Device Caps\s*:\s*(.+)", info)
    caps = caps.group(1) if caps else ""
    has_fmt = "Video Capture" in info or "Video Capture Multiplanar" in info
    return has_fmt, caps or "(未读到 Device Caps)"


def probe(video_dev):
    print("===== 1. 所有 video 节点 =====")
    print(run("v4l2-ctl --list-devices"))

    roots = sorted(glob.glob("/dev/video*"),
                   key=lambda p: int(re.sub(r"\D", "", p) or 0))
    print(f"\n===== 2. 逐节点能力（共 {len(roots)} 个）=====")
    print("   重点看：哪个节点是 Video Capture、有没有非 capture 的节点（可能是 still/metadata）")
    for dev in roots:
        if not os.path.exists(dev):
            continue
        try:
            has_fmt, caps = is_capture_node(dev)
        except Exception:
            has_fmt, caps = False, "(探测失败)"
        link = ""
        for l in glob.glob("/dev/usbcam*"):
            if os.path.realpath(l) == os.path.realpath(dev):
                link = f"  ← {l}"
        print(f"  {dev:<16} capture={has_fmt!s:<5} {caps}{link}")

    if not video_dev:
        return
    if not os.path.exists(video_dev):
        print(f"\n[!] {video_dev} 不存在，跳过后续探测")
        return

    print(f"\n===== 3. {video_dev} 所属 USB 设备 =====")
    usb = find_usb_dir(video_dev)
    if not usb:
        print("  (未找到 USB 设备节点)")
        return
    vid, pid = read_file(f"{usb}/idVendor"), read_file(f"{usb}/idProduct")
    print(f"  {vid}:{pid}  {read_file(usb + '/product')}  (速度 {read_file(usb + '/speed')} Mbps)")

    print("\n===== 4. USB 接口构成（关键：找 HID / 额外 Video 接口）=====")
    parent, base = os.path.dirname(usb), os.path.basename(usb)
    try:
        entries = sorted(os.listdir(parent))
    except Exception:
        entries = []
    for e in entries:
        if not e.startswith(base + ":"):
            continue
        d = os.path.join(parent, e)
        cls = read_file(os.path.join(d, "bInterfaceClass"))
        drv = os.path.basename(os.path.realpath(os.path.join(d, "driver"))) \
            if os.path.exists(os.path.join(d, "driver")) else ""
        print(f"  {e:<22} class={cls:<4} {IFACE_CLASS_NAME.get(cls, '?'):<16} driver={drv}")

    print("\n===== 5. USB 描述符里的 Still Image / 中断端点 =====")
    out = run(f"lsusb -v -d {vid}:{pid} 2>/dev/null")
    hits = [l.strip() for l in out.splitlines()
            if re.search(r"still|Still|bNumStillImages|Interrupt", l)]
    if hits:
        for l in hits[:30]:
            print(f"  {l}")
    else:
        print("  (未发现 Still Image 相关描述符)")

    print("\n===== 6. 额外查询 =====")
    ctrl = run(f"v4l2-ctl -d {video_dev} --list-ctrls --all")
    interesting = [l for l in ctrl.splitlines()
                   if re.search(r"trigger|snapshot|still|button|count", l, re.I)]
    print("  含 trigger/snapshot/still/button/count 的控件：")
    for l in (interesting or ["  (无)"]):
        print(f"    {l.strip()}")

    print("\n===== 判读 =====")
    print("  · 存在第二个 capture 节点 或 有 Still Image 描述符 → 倾向机制 A/B（图由摄像头上传）")
    print("  · 只有标准 UVC 流 + 一个事件端点，无 still 描述符   → 倾向机制 C（图是主机从流里抓的）")
    print("  · 有 class=03 HID 接口                              → 触发事件走 HID，可用 evdev 监听")
    print("  · 什么都没有                                        → 强烈倾向 C，直接用 --shutter 自己拍")


# ---------------- GPIO 触发（与 gpio_short_trigger.py 的 --mode low 语义一致） ----------------

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
        f.write("1")            # 空闲 = 高
    return True


def gpio_set(gpio, level):
    with open(f"{GPIO_ROOT}/gpio{gpio}/value", "w") as f:
        f.write(str(level))


def gpio_pulse(gpio, low_ms=100):
    """发一次触发：空闲高 → 拉低（下降沿=触发）→ 回高。"""
    base = f"{GPIO_ROOT}/gpio{gpio}/value"
    with open(base, "w") as f:
        f.write("1")
    time.sleep(0.02)
    with open(base, "w") as f:
        f.write("0")
    time.sleep(low_ms / 1000.0)
    with open(base, "w") as f:
        f.write("1")


# ---------------- 抓帧 ----------------

def ffmpeg_input_args(video, fmt, size, framerate):
    return ["-f", "v4l2", "-input_format", fmt, "-video_size", size,
            "-framerate", str(framerate), "-i", video]


def grab_one(video, out, fmt, size, framerate, timeout=15):
    cmd = ["ffmpeg", "-hide_banner", "-nostdin", "-loglevel", "error", "-y"]
    cmd += ffmpeg_input_args(video, fmt, size, framerate)
    cmd += ["-frames:v", "1", out]
    r = subprocess.run(cmd, capture_output=True, text=True,
                       encoding="utf-8", errors="replace", timeout=timeout)
    ok = os.path.exists(out) and os.path.getsize(out) > 1000
    if not ok:
        print(f"[x] 抓帧失败: {(r.stderr or '').strip()[:300]}")
    return ok


def parse_v4l2_info(dev):
    """用一次 v4l2-ctl --all 拿到 (driver名, Device Caps 十六进制, Caps 名称列表)。"""
    info = run(f"v4l2-ctl -d {dev} --all 2>&1")
    drv = re.search(r"Driver name\s*:\s*(\S+)", info)
    caps_line = re.search(r"Device Caps\s*:\s*(0x[0-9a-fA-F]+)", info)
    names = []
    if caps_line:
        # v4l2-ctl 会把 Device Caps 后面的缩进行解码成名称
        tail = info[caps_line.end():].splitlines()
        for l in tail:
            if not l.strip():
                continue
            if re.match(r"^\s+\S", l):        # 缩进行 = caps 名称
                names.append(l.strip())
            else:
                break
    return (drv.group(1) if drv else "?",
            caps_line.group(1) if caps_line else "?",
            names)


def cmd_grab_all(fmt, size, framerate, outdir):
    """对每个 UVC 节点逐个尝试抓一帧，判断哪个能出图、出的是什么。

    只关注 uvcvideo 驱动的节点（板子上还有大量 Rockchip 多媒体节点，与摄像头无关，直接跳过）。
    """
    os.makedirs(outdir, exist_ok=True)
    nodes = sorted(glob.glob("/dev/video*"),
                   key=lambda p: int(re.sub(r"\D", "", p) or 0))
    if not nodes:
        print("[x] 没找到任何 /dev/video* 节点")
        return

    print(f"===== 扫描 {len(nodes)} 个 /dev/video* 节点，筛出 uvcvideo 的 =====")
    print("  注意：同一物理流不能被两个进程同时打开 —— 请先停掉推流/预览")
    print(f"  抓帧尺寸用 --size 指定（当前 {size}）；可用 --size 1920x1080 拿原生满分辨率\n")

    uvc_nodes, skipped = [], 0
    for dev in nodes:
        drv, caps, names = parse_v4l2_info(dev)
        if drv != "uvcvideo":
            skipped += 1
            continue
        uvc_nodes.append((dev, caps, names))

    print(f"  uvcvideo 节点 {len(uvc_nodes)} 个；其余 {skipped} 个非 UVC 节点已跳过\n")
    if not uvc_nodes:
        print("[!] 没找到 uvcvideo 节点 —— 确认摄像头已插好（lsusb / v4l2-ctl --list-devices）")
        return

    report = []
    for dev, caps, names in uvc_nodes:
        print(f"----- {dev} -----")
        print(f"  Device Caps : {caps}   [{' / '.join(names) if names else '?'}]")
        is_capture = any("Video Capture" in n for n in names)
        is_meta = any("Metadata" in n for n in names)
        if is_meta and not is_capture:
            print("  ⇒ 这是 **Metadata（元数据）节点，不出图**，跳过抓帧")
            report.append((dev, "元数据节点", "-", "-"))
            continue

        out = os.path.join(outdir, f"node_{os.path.basename(dev)}.jpg")
        if not grab_one(dev, out, fmt, size, framerate):
            report.append((dev, "抓帧失败", "", ""))
            continue
        dim = run(f"ffprobe -v error -select_streams v:0 "
                  f"-show_entries stream=width,height -of csv=p=0 '{out}'")
        dim = dim.strip().replace(",", "x") if dim else "?"
        kb = os.path.getsize(out) // 1024
        print(f"  ⇒ ✅ 抓帧成功 → {out}  ({kb} KB, {dim})")
        report.append((dev, "图像采集 ✅", dim, f"{kb} KB"))

    print("\n===== 汇总 =====")
    print(f"  {'节点':<18} {'结果':<14} {'尺寸':<14} 大小")
    for dev, res, dim, kb in report:
        print(f"  {dev:<18} {res:<14} {dim:<14} {kb}")

    print("\n===== 判读 =====")
    print("  · Device Caps 含 'Metadata Capture' 而无 'Video Capture' → **元数据节点，不出图**")
    print("  · 多个节点都能抓到同尺寸的图 → 它们是**同一物理流的多个入口**，不是独立静图通道")
    print("  · 抓帧报 'Device or resource busy' → 该节点与另一个**共享同一条流**")
    print("  · 只有 1 个节点能出图 → Linux 侧就只有这一条取图通路；")
    print("    想拿静图只能从这条流抓帧（本项目 uvc_snapshot.py 的 --grab / --shutter）")
    print(f"\n  抓到的文件都在 {outdir}/，可用图片查看器逐个比对内容")
    print(f"  看设备支持哪些尺寸： v4l2-ctl -d <能出图的节点> --list-formats-ext")


def jpeg_complete(path):
    """检查 JPEG 是否写入完整（以 FFD9 结尾）。"""
    try:
        with open(path, "rb") as f:
            f.seek(-2, os.SEEK_END)
            return f.read() == b"\xff\xd9"
    except Exception:
        return False


def probe_size(path):
    """用 ffprobe 读回图片实际尺寸（防止协商失败后静默降级）。"""
    out = run(f"ffprobe -v error -select_streams v:0 "
              f"-show_entries stream=width,height -of csv=p=0 '{path}'")
    out = out.strip().replace(",", "x") if out else ""
    return out or "尺寸未知"


class StreamTap:
    """常驻读流，始终把最新一帧写到 latest_path（-update 1 覆盖写）。"""

    def __init__(self, video, latest_path, fmt, size, framerate):
        self.latest = latest_path
        self.proc = None
        cmd = ["ffmpeg", "-hide_banner", "-nostdin", "-loglevel", "error", "-y"]
        cmd += ffmpeg_input_args(video, fmt, size, framerate)
        cmd += ["-update", "1", latest_path]
        self.cmd = cmd

    def start(self, wait=3.0):
        self.proc = subprocess.Popen(self.cmd, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL)
        t0 = time.time()
        while time.time() - t0 < wait:
            if os.path.exists(self.latest) and os.path.getsize(self.latest) > 1000:
                time.sleep(0.3)          # 再等一会儿让文件稳定
                return True
            time.sleep(0.1)
        return False

    def snapshot(self, out, retries=8):
        """把当前最新帧另存为 out（校验完整性，失败则稍等重试）。"""
        for i in range(retries):
            try:
                shutil.copyfile(self.latest, out)
                if jpeg_complete(out):
                    return True
            except Exception:
                pass
            time.sleep(0.02)
        return False

    def stop(self):
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except Exception:
                self.proc.kill()


def main():
    ap = argparse.ArgumentParser(description="获取 2P 端子触发拍照的图（机制探测 + 抓帧 + 软快门）")
    ap.add_argument("--video", help="video 节点，如 /dev/video121")
    ap.add_argument("--probe", action="store_true", help="探测静图机制（默认动作）")
    ap.add_argument("--ctrls", action="store_true",
                    help="打印全部 UVC 控件（含 XU 扩展控件）—— 用于找『触发模式/触发源』开关")
    ap.add_argument("--set", action="append", metavar="NAME=VALUE",
                    help="设置控件值，如 --set trigger_mode=1（可重复；常用来切到硬件触发模式）")
    ap.add_argument("--grab", action="store_true", help="抓一帧")
    ap.add_argument("--grab-all", action="store_true",
                    help="★对每个 /dev/video* 节点逐个试抓一帧，判断哪个节点能出图、出的是什么")
    ap.add_argument("--shutter", action="store_true", help="软快门：常驻读流，每次触发保存一张")
    ap.add_argument("--gpio", type=int, help="用于发触发的 GPIO")
    ap.add_argument("--trigger", action="store_true", help="--grab 时先发一次 GPIO 触发再抓")
    ap.add_argument("-o", "--out", default="/tmp/snap.jpg", help="--grab 的输出路径")
    ap.add_argument("--outdir", default="/tmp/shots", help="--shutter 的输出目录")
    ap.add_argument("--count", type=int, default=3, help="--shutter 拍几张（默认 3）")
    ap.add_argument("--delay", type=int, default=0,
                    help="从『下降沿』到『取帧』的等待 ms（默认 0 = 立刻取，最贴近触发瞬间）")
    ap.add_argument("--low", type=int, default=100,
                    help="触发时低电平保持 ms（默认 100，即下降沿后维持低电平这么久再回高）")
    ap.add_argument("--interval", type=int, default=2000, help="--shutter 每张之间的间隔 ms")
    ap.add_argument("--format", default="mjpeg")
    ap.add_argument("--size", default="1280x720")
    ap.add_argument("--framerate", type=int, default=30)
    ap.add_argument("--tap-dir", default=None,
                    help="常驻流『最新帧』的落盘目录。默认优先用内存文件系统 /dev/shm，"
                         "避免每秒 ~28 次写 eMMC（降低落盘延迟与磨损）；不可用时退回 --outdir")
    ap.add_argument("--dry-run", action="store_true", help="只打印计划，不操作硬件")
    args = ap.parse_args()

    if args.dry_run:
        print("[dry-run] 计划：")
        print(f"  video={args.video}  format={args.format}  size={args.size} @{args.framerate}")
        print(f"  gpio={args.gpio}  count={args.count}  delay={args.delay}ms  "
              f"low={args.low}ms  interval={args.interval}ms")
        print(f"  out={args.out}  outdir={args.outdir}")
        mods = [m for m, on in (("probe", args.probe), ("ctrls", args.ctrls),
                                ("set", bool(args.set)), ("grab", args.grab),
                                ("grab-all", args.grab_all), ("shutter", args.shutter)) if on]
        print(f"  模式={mods if mods else ['probe(默认)']}")
        return

    # 默认动作：探测
    if not (args.probe or args.grab or args.grab_all or args.shutter
            or args.ctrls or args.set):
        args.probe = True

    if args.grab_all:
        cmd_grab_all(args.format, args.size, args.framerate, args.outdir)
        return

    if args.ctrls:
        if not args.video:
            ap.error("--ctrls 需要 --video")
        print(f"===== {args.video} 的全部控件（v4l2-ctl --list-ctrls --all）=====")
        print(run(f"v4l2-ctl -d {args.video} --list-ctrls --all"))
        print("\n===== XU 扩展控件（uvcdynctrl，需 apt install uvcdynctrl）=====")
        out = run(f"uvcdynctrl -d {args.video} -l")
        print(out)
        print("\n===== 直接读取控件原始信息（get-ctrl 全量）=====")
        print(run(f"v4l2-ctl -d {args.video} --all | grep -A200 'User Controls'"))
        print("\n----- 找什么 -----")
        print("  名字里带 trigger / trigger_mode / trigger_source / snapshot / still /")
        print("  hardware / sync 的控件就是厂家说的『UVC PU 接口触发控制』。")
        print("  找到后用 --set NAME=VALUE 逐个取值试（例如 --set trigger_mode=1）。")
        print("  XU 控件若 v4l2-ctl 看不到，用 uvcdynctrl 的 SetControl 命令：")
        print("    uvcdynctrl -d " + str(args.video) + " -s '控件名' <值>")
        return

    if args.set:
        if not args.video:
            ap.error("--set 需要 --video")
        print(f"[i] 设置前的控件值：")
        print(run(f"v4l2-ctl -d {args.video} --list-ctrls"))
        for kv in args.set:
            print(f"\n[i] 尝试设置 {kv} …")
            print(run(f"v4l2-ctl -d {args.video} -c {kv}"))
            time.sleep(0.3)
        print(f"\n[i] 设置后的控件值：")
        print(run(f"v4l2-ctl -d {args.video} --list-ctrls"))
        print("\n[i] 接下来验证是否真的切到了硬件触发模式：")
        print(f"    python3 trigger_frame_test.py --video {args.video} --gpio 46")
        print("    · 若 2Hz 触发时实收帧数塌到 ≈20 帧 → ✅ 模式切换成功，四路同步有戏！")
        print("    · 若仍是 ≈280 帧            → ❌ 该控件不是触发源开关，换一个值/控件再试")
        return

    if args.probe:
        probe(args.video)
        return

    if not args.video:
        ap.error("--grab / --shutter 需要 --video")
    if not os.path.exists(args.video):
        print(f"[x] {args.video} 不存在，先用 v4l2-ctl --list-devices 确认")
        return

    if args.gpio is not None and not gpio_prepare(args.gpio):
        return

    if args.grab:
        if args.trigger and args.gpio is None:
            ap.error("--trigger 需要 --gpio")
        if args.trigger:
            print(f"[i] 先发一次 gpio{args.gpio} 触发（下降沿），等 {args.delay}ms")
            gpio_pulse(args.gpio)
            time.sleep(args.delay / 1000.0)
        print(f"[i] 从 {args.video} 抓一帧 → {args.out}")
        if grab_one(args.video, args.out, args.format, args.size, args.framerate):
            print(f"[OK] 已保存 {args.out}（{os.path.getsize(args.out)} 字节）")
        return

    if args.shutter:
        os.makedirs(args.outdir, exist_ok=True)
        # 常驻流"最新帧"放内存文件系统，避免每秒 ~28 次写 eMMC（降低落盘延迟与磨损）
        tap_dir = args.tap_dir
        if not tap_dir:
            if os.path.isdir("/dev/shm") and os.access("/dev/shm", os.W_OK):
                tap_dir = "/dev/shm"
            else:
                tap_dir = args.outdir
        os.makedirs(tap_dir, exist_ok=True)
        latest = os.path.join(tap_dir, f".uvc_latest_{os.getpid()}.jpg")
        print(f"[i] 常驻读流：最新帧维护在 {latest}（{'内存文件系统' if tap_dir == '/dev/shm' else '磁盘'}）")
        tap = StreamTap(args.video, latest, args.format, args.size, args.framerate)
        if not tap.start():
            print("[x] 常驻读流启动失败（设备被占用？换个节点试试）")
            tap.stop()
            return
        print("[OK] 读流已就绪\n")

        def _cleanup(*_):
            tap.stop()
            try:
                os.remove(latest)
            except Exception:
                pass
            print("\n[i] 读流已停止")
            sys.exit(0)

        signal.signal(signal.SIGINT, _cleanup)
        signal.signal(signal.SIGTERM, _cleanup)

        print(f">>> 共拍 {args.count} 张，输出到 {args.outdir}")
        if args.gpio is None:
            print(">>> 未指定 --gpio：请按回车拍一张")
        else:
            print(f">>> 时序：拉低(下降沿=触发) → 取帧 → 低电平保持 {args.low}ms → 回高")
        offsets = []
        for i in range(1, args.count + 1):
            out = os.path.join(args.outdir, f"shot_{i:03d}.jpg")
            if args.gpio is not None:
                # ★ 触发与取帧紧贴：先拉低产生下降沿，立刻取"当前最新帧"，再恢复高电平。
                t_edge = time.time()
                gpio_set(args.gpio, 0)                      # 下降沿 = 触发
                try:
                    # .latest.jpg 的"写入时刻"≈ ffmpeg 把该帧落盘的时间；
                    # 用它减去下降沿，才能看出取到的帧究竟偏新还是偏旧。
                    t_written = os.path.getmtime(tap.latest)
                except Exception:
                    t_written = None
                if args.delay > 0:
                    time.sleep(args.delay / 1000.0)
                ok = tap.snapshot(out)
                t_copy = time.time()
                time.sleep(max(0.0, args.low / 1000.0))
                gpio_set(args.gpio, 1)
                line = (f"[{i}/{args.count}] 拉低 → 取帧 耗时 "
                        f"{int((t_copy - t_edge) * 1000)} ms")
                if t_written:
                    off = (t_written - t_edge) * 1000.0
                    offsets.append(off)
                    who = "帧较新·触发后写入" if off >= 0 else "帧较旧·触发前写入"
                    line += f"；帧写入时刻相对下降沿 {off:+.0f} ms（{who}）"
                print(line, flush=True)
            else:
                try:
                    input(f"[{i}/{args.count}] 按回车拍一张…")
                except EOFError:
                    pass
                ok = tap.snapshot(out)
            if ok:
                print(f"        → 已保存 {out}（{os.path.getsize(out) // 1024} KB, "
                      f"{probe_size(out)}）")
            else:
                print("        → [x] 保存失败（取到的文件不完整）")
            if i < args.count and args.gpio is not None:
                time.sleep(args.interval / 1000.0)
        tap.stop()
        try:
            os.remove(latest)
        except Exception:
            pass
        print(f"\n[OK] 完成。共 {args.count} 张，位于 {args.outdir}/")
        if offsets:
            print(f"[i] 偏差统计（帧写入时刻 − 下降沿）：min {min(offsets):+.0f} ms / "
                  f"max {max(offsets):+.0f} ms / 平均 {sum(offsets)/len(offsets):+.0f} ms")
            print("    判读：落在 0 ~ -33ms（一个帧间隔）内属正常 —— 取到的是最近一帧，")
            print("          偏差是随机的（取决于触发落在帧周期的哪个位置）。")
            print("          想进一步收紧（到几 ms）需改用『内存环形缓冲 + 每帧打时间戳』，")
            print("          即直接读 MJPEG 流分帧、在内存里保留最近几帧，而不是靠落盘取帧。")
        print("[i] 关于「帧写入时刻相对下降沿」这个数：")
        print("    · 负数 = 取到的是**触发之前**写入的帧（偏旧）；正数 = 触发之后写入的帧（偏新）")
        print("    · 它只反映 ffmpeg 落盘时刻；**实际画面时刻还要再往前推最多一个帧间隔（~35ms）**，")
        print("      因为采样→编码→落盘本身有延迟。想进一步收紧，可提高帧率或减小 --size。")
        print("[i] 提示：这是**主机从视频流抓的帧**，不是摄像头上传的独立静图；")
        print("    因为触发信号是我们自己发的，所以触发时刻本来就知道，无需检测 USB 事件。")
        return


if __name__ == "__main__":
    main()
