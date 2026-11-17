#!/usr/bin/env python3
"""摄像头 2P 端子「信号方向与作用」探测工具

为什么需要它
------------
摄像头在 2P 端子悬空时**本来就能正常出流**（v4l2-ctl --list-devices 能查到、画面正常）。
这说明端子**不是"控制出图的开关"**，所以「盯着画面看触发有没有生效」是无效的观测方法。

触发的作用可能是下面任意一种，每种的可观测现象都不同：
    A. 摄像头作为 HID 复合设备，短接 = 按下一个"快门键"
       → Linux 会出现一个 input 事件（KEY_CAMERA / KEY_SNAPSHOT 之类），能用 evdev 抓到
    B. 摄像头内部把这一帧存成静止图（UVC Still Image），视频流不受影响
       → Linux 侧可能什么都看不到，要在 Windows 的 amcap 里才看得到
    C. 摄像头内部把触发次数写进某个 UVC 扩展控件（XU）
       → 可对比触发前后 `v4l2-ctl --list-ctrls` 的控件值有没有变
    D. 端子其实是**输出**（帧同步 / 闪光同步输出），不是输入
       → 该测的是"端子有没有波形"，而不是"我能不能驱动它"

本工具用「前后快照差分」的方式把这四种可能一次筛一遍，找出真正有变化的那一项。

用法
----
    # 1) 环境指纹：USB 接口构成 / input 设备 / UVC 控件 / 内核日志尾部
    python3 trigger_probe.py --video /dev/video121

    # 2) 监听输入事件（重点测 A）：运行后**手动短接 2P 端子**，看有没有事件
    sudo python3 trigger_probe.py --watch-input --seconds 60

    # 3) 差分探测（推荐）：触发前后各采一次快照，打印差异
    python3 trigger_probe.py --diff --video /dev/video121 --gpio 46 --count 3
    python3 trigger_probe.py --diff --video /dev/video121          # 改为手动短接（回车继续）

判读规则
--------
    · 出现 input 事件（type 1 EV_KEY，非摄像头自带的按键）→ 是 HID 触发（可能 A）
    · UVC 控件值发生变化（如某个计数/状态控件）→ 有 XU 反馈，能做成闭环（可能 C）
    · USB 接口列表里除了 Video 还有 HID(03) → 基本可确认是"按键型"触发
    · 什么差异都没有 → 大概率是纯内部行为（B），或端子是输出（D）需要示波器/万用表
"""
import argparse
import os
import re
import subprocess
import sys
import time

VIDEO_CLASS = "0e"
IFACE_CLASS_NAME = {
    "01": "Audio",
    "02": "CDC/Comm",
    "03": "HID",
    "08": "Mass Storage",
    "0e": "Video (UVC)",
    "fe": "Application Specific",
    "ff": "Vendor Specific",
}


def run(cmd, timeout=8):
    try:
        r = subprocess.run(cmd, shell=True, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
        return (r.stdout or "") + (r.stderr or "")
    except Exception as e:
        return f"<执行失败: {e}>"


def _read(path):
    try:
        with open(path) as f:
            return f.read().strip()
    except Exception:
        return ""


def find_usb_device(video_dev):
    """由 /dev/videoN 反查所属 USB 设备目录（返回 (目录, vid, pid, product)）。"""
    name = os.path.basename(video_dev)
    base = f"/sys/class/video4linux/{name}/device"
    if not os.path.exists(base):
        return None
    cur = os.path.realpath(base)
    for _ in range(8):
        vid_path = os.path.join(cur, "idVendor")
        if os.path.exists(vid_path):
            return (cur, _read(vid_path), _read(os.path.join(cur, "idProduct")),
                    _read(os.path.join(cur, "product")))
        nxt = os.path.dirname(cur)
        if nxt == cur:
            break
        cur = nxt
    return None


def usb_interfaces(usb_dir):
    """列出该 USB 设备的所有接口及其 class（用来判断有没有 HID 接口）。"""
    out = []
    parent = os.path.dirname(usb_dir)
    base = os.path.basename(usb_dir)
    try:
        entries = os.listdir(parent)
    except Exception:
        return out
    for e in sorted(entries):
        if not e.startswith(base + ":"):
            continue
        d = os.path.join(parent, e)
        cls = _read(os.path.join(d, "bInterfaceClass"))
        sub = _read(os.path.join(d, "bInterfaceSubClass"))
        drv = os.path.basename(os.path.realpath(os.path.join(d, "driver"))) \
            if os.path.exists(os.path.join(d, "driver")) else ""
        out.append((e, cls, IFACE_CLASS_NAME.get(cls, f"class={cls}"), sub, drv))
    return out


def list_input_devices():
    """解析 /proc/bus/input/devices，返回 [{name, handlers}]。"""
    devs, cur = [], None
    try:
        with open("/proc/bus/input/devices") as f:
            for line in f:
                line = line.rstrip("\n")
                if line.startswith("N: Name="):
                    cur = {"name": line.split("=", 1)[1].strip('"'), "handlers": ""}
                elif line.startswith("H: Handlers=") and cur is not None:
                    cur["handlers"] = line.split("=", 1)[1].strip()
                elif not line.strip() and cur is not None:
                    devs.append(cur)
                    cur = None
        if cur is not None:
            devs.append(cur)
    except Exception:
        pass
    return devs


def uvc_controls(video_dev):
    """取控件列表（含值），用于差分。"""
    out = run(f"v4l2-ctl -d {video_dev} --list-ctrls")
    low = out.lower()
    if (not out.strip()) or "cannot open" in low or "not found" in low \
            or "no such" in low or "failed" in low:
        return f"(无法读取控件: {out.strip()[:160]})"
    return "\n".join(sorted(l.strip() for l in out.splitlines() if l.strip()))


def snapshot(video_dev):
    """采集一次环境指纹。"""
    snap = {}
    snap["input_devices"] = [f"{d['name']}  [{d['handlers']}]" for d in list_input_devices()]
    snap["uvc_ctrls"] = uvc_controls(video_dev) if video_dev else "(未指定 --video)"
    usb = find_usb_device(video_dev) if video_dev else None
    if usb:
        d, vid, pid, prod = usb
        snap["usb_id"] = f"{vid}:{pid}  {prod}"
        snap["usb_interfaces"] = [
            f"{name:<22} class={cls:<4} {cname:<16} drv={drv}"
            for name, cls, cname, sub, drv in usb_interfaces(d)
        ]
        snap["usb_raw"] = run(f"lsusb -v -d {vid}:{pid} 2>/dev/null | "
                              f"grep -i -E 'bInterfaceClass|bInterfaceSubClass|bNumEndpoints'")
    else:
        snap["usb_id"] = "(未找到 USB 设备节点)"
        snap["usb_interfaces"] = []
    snap["usb_tree"] = run("lsusb -t")
    snap["dmesg_tail"] = run("dmesg | tail -n 30")
    return snap


def print_snapshot(s):
    print("\n===== 环境指纹 =====")
    print(f"USB 设备      : {s['usb_id']}")
    print("\nUSB 接口构成（重点看有没有 class=03 HID）:")
    for l in s["usb_interfaces"] or ["  (无)"]:
        print(f"  {l}")
    print("\n输入设备（短接若是 HID 触发，这里可能多出一个）:")
    for l in s["input_devices"] or ["  (无)"]:
        print(f"  {l}")
    print("\nUVC 控件:")
    for l in (s["uvc_ctrls"] or "").splitlines() or ["  (无)"]:
        print(f"  {l}")
    print("\n内核日志尾部:")
    print(s["dmesg_tail"])


def diff_snapshots(a, b):
    changed = False
    print("\n===== 差分结果（触发前 → 触发后）=====")
    for key, label in (("input_devices", "输入设备列表"),
                       ("uvc_ctrls", "UVC 控件/值"),
                       ("usb_interfaces", "USB 接口构成"),
                       ("usb_id", "USB 设备标识")):
        va, vb = a.get(key), b.get(key)
        if va == vb:
            print(f"\n[{key}] {label}: 无变化")
            continue
        changed = True
        print(f"\n[!] [{key}] {label}: **有变化**")
        if isinstance(va, list):
            sa, sb = set(va), set(vb)
            for x in sorted(sb - sa):
                print(f"    + 新增: {x}")
            for x in sorted(sa - sb):
                print(f"    - 消失: {x}")
        else:
            print(f"    前: {va}")
            print(f"    后: {vb}")

    # dmesg 增量
    da = set(a.get("dmesg_tail", "").splitlines())
    db = b.get("dmesg_tail", "").splitlines()
    new_lines = [l for l in db if l not in da]
    if new_lines:
        changed = True
        print("\n[!] dmesg 新增行:")
        for l in new_lines:
            print(f"    + {l}")
    else:
        print("\n[dmesg] 无新增行")

    print("\n===== 判读 =====")
    if not changed:
        print("  四项全部无变化。可能：")
        print("   1) 触发是摄像头内部行为（存静止图），Linux 侧看不到 —— 回 Windows 用 amcap 确认")
        print("   2) 端子其实是**输出**，不是输入 —— 用万用表/示波器量端子有没有波形")
        print("   3) 触发模式未激活（需要先用 XU 控件切到外触发模式）")
        print("   4) 根本没接对线 / 未共地 / 电流不足")
    else:
        print("  上面标 [+] 的就是线索：")
        print("   · 输入设备多了一个 → 摄像头是 HID 复合设备，短接等于按键（可用 evdev 监听）")
        print("   · UVC 控件值变了   → 有 XU 反馈，可做成闭环控制")
        print("   · 接口构成变了     → 触发会让设备重新枚举，需重新确认 /dev 节点")


def watch_input(seconds):
    """用 evdev 监听所有输入设备，打印事件来源与键码。"""
    try:
        import evdev
        from evdev import ecodes
    except Exception as e:
        print(f"[x] 无法导入 evdev：{e}")
        print("    pip3 install evdev   （本项目 requirements 里已有）")
        return
    devs = []
    for path in evdev.list_devices():
        try:
            devs.append(evdev.InputDevice(path))
        except Exception:
            pass
    if not devs:
        print("[!] 没有可读的输入设备。请用 sudo 或把你的用户加入 input 组：")
        print("    sudo usermod -aG input $USER   （重新登录生效）")
        return
    print(f"[i] 正在监听 {len(devs)} 个输入设备，持续 {seconds} 秒：")
    for d in devs:
        print(f"    {d.path}  {d.name}")
    print("\n>>> 现在请**手动短接摄像头 2P 端子**，观察下面是否出现事件 <<<\n")

    import selectors
    sel = selectors.DefaultSelector()
    for d in devs:
        try:
            sel.register(d, selectors.EVENT_READ)
        except Exception:
            pass
    t_end = time.time() + seconds
    hit = False
    while time.time() < t_end:
        for key, _ in sel.select(timeout=1.0):
            d = key.fileobj
            try:
                for ev in d.read():
                    if ev.type == ecodes.EV_KEY:
                        name = ecodes.KEY.get(ev.code, f"code{ev.code}")
                        if isinstance(name, list):
                            name = name[0] if name else f"code{ev.code}"
                        state = {0: "松开", 1: "按下", 2: "长按"}.get(ev.value, ev.value)
                        print(f"  [事件] {d.path} <{d.name}>  KEY {name}  {state}")
                        hit = True
            except Exception:
                pass
    if hit:
        print("\n[✓] 捕获到按键事件 —— 端子是 HID/按键型触发，可在 Linux 侧用 evdev 检测。")
    else:
        print("\n[!] 未捕获任何按键事件。可能是：不在 HID 触发模式 / 该端子不是按键型 / 未接对线。")


def do_trigger(gpio, count, hold, interval):
    """调用同目录的 gpio_short_trigger.py 做触发（保持逻辑单一来源）。"""
    here = os.path.dirname(os.path.abspath(__file__))
    script = os.path.join(here, "gpio_short_trigger.py")
    if not os.path.exists(script):
        print(f"[x] 未找到 {script}")
        return
    cmd = (f"python3 {script} --gpio {gpio} --mode short "
           f"--count {count} --hold {hold} --interval {interval}")
    print(f"[i] 执行触发：{cmd}")
    print(run(cmd, timeout=120))


def main():
    ap = argparse.ArgumentParser(description="摄像头 2P 端子信号方向与作用探测")
    ap.add_argument("--video", help="要探测的 video 节点，如 /dev/video121")
    ap.add_argument("--gpio", type=int, help="用于自动触发的 GPIO（不填则改为手动短接）")
    ap.add_argument("--count", type=int, default=3, help="自动触发次数（默认 3）")
    ap.add_argument("--hold", type=int, default=200, help="触发保持 ms（默认 200）")
    ap.add_argument("--interval", type=int, default=2000, help="触发间隔 ms（默认 2000）")
    ap.add_argument("--diff", action="store_true", help="差分探测：触发前后各采一次快照")
    ap.add_argument("--watch-input", action="store_true", help="监听输入事件（测 HID 型触发）")
    ap.add_argument("--seconds", type=int, default=60, help="--watch-input 的监听时长")
    args = ap.parse_args()

    if args.watch_input:
        watch_input(args.seconds)
        return

    if args.diff:
        if not args.video:
            ap.error("--diff 需要 --video 指定 video 节点")
        print(">>> 第 1 步：采集触发前的环境指纹")
        before = snapshot(args.video)
        if args.gpio is not None:
            print(f"\n>>> 第 2 步：用 gpio{args.gpio} 自动触发 {args.count} 次")
            do_trigger(args.gpio, args.count, args.hold, args.interval)
        else:
            print("\n>>> 第 2 步：请手动短接摄像头 2P 端子几次，然后按回车继续")
            try:
                input("    （完成后按回车）")
            except EOFError:
                pass
        time.sleep(1.0)
        print(">>> 第 3 步：采集触发后的环境指纹")
        after = snapshot(args.video)
        diff_snapshots(before, after)
        return

    if not args.video:
        ap.error("请用 --video 指定 video 节点（如 /dev/video121），"
                 "或用 --watch-input / --diff")
    print_snapshot(snapshot(args.video))
    print("\n提示：")
    print("  · 上面 USB 接口构成里若出现 class=03 HID，说明端子很可能是『按键型』触发")
    print("  · 想测按键事件 → sudo python3 trigger_probe.py --watch-input --seconds 60")
    print("  · 想一次筛完四种可能 → python3 trigger_probe.py --diff --video <dev> [--gpio N]")


if __name__ == "__main__":
    main()
