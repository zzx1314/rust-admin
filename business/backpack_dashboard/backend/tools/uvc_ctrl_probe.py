#!/usr/bin/env python3
"""直发 UVC 控制请求 —— 找回内核没暴露的"厂商映射控件"

要解决的问题
------------
厂家 Android SDK 里有这么一段（读"GPIO7"引脚电平）：

    int v = USBCameraSDK.getITPropertyValue(IT_FOCUS_ABS);   // 0x000020
    // v == 0 → pressed（低电平）   v == 1 → released

但三方对照下来：

    厂家 Android SDK   ✅ 能读到（自己发 UVC 请求，绕过描述符）
    Windows AMCap      ❌ 属性页里"焦点"那行是空白（读不到）
    Linux v4l2         ❌ `v4l2-ctl --list-ctrls` 里根本没有 focus_absolute

原因推断：**摄像头固件响应这个控件，但它的 UVC 描述符里没有声明 Focus Absolute。**
内核 uvcvideo 按描述符创建控件，所以不给我们 `focus_absolute`。

> 注：SDK 的 `IT_FOCUS_ABS = 0x20` 是 **bmControls 的位掩码**（bit5）。
> UVC 里 CT（Input Terminal）的 **CS 选择子 = 位序号 + 1**，所以
> **bit5（Focus Absolute）→ CS = 0x06**；bit3（Exposure Time Absolute）→ CS = 0x04。

四个子命令
----------
    --descriptor   从 lsusb -v 解析 UVC 描述符：真实实体号 + 声明了哪些控件
                   （★ 先跑这个，不要靠猜 entity ID）
    --selftest     用 libusb 读一个"确定已声明"的控件，验证通道是否可用
    --read-all     ★★把每个实体「已声明」的控件全部读出（**包含**厂商 XU）；
                   纯只读，不改设备任何东西，也不用猜选择子 —— 优先用这个
    --scan         扫描 entity × selector（范围更广但会碰到大量无效组合）
    --get/--watch  单读 / 持续读（配合手动给 2P 端子拉低，看值是否跟着变）
    --put          ★写控件（SET_CUR）—— 用于试厂家说的「软件触发」：
                   厂家触发出示意图里那条 "USB → UVC 接口触发控制" 通路，
                   在 Linux 上就是往对应控件写值。首选目标：厂商 XU entity 4, CS 0x0A/0x0B。

典型流程
--------
    # 0) 找 VID/PID 与协商速率
    python3 uvc_ctrl_probe.py --list

    # 1) 读描述符：拿到真实的 CT / PU 实体号，并看 Focus 位是否声明
    python3 uvc_ctrl_probe.py --vid 0x0bda --pid 0x3035 --descriptor

    # 2) 验证 libusb 通道能不能读（读一个已声明的控件）
    python3 uvc_ctrl_probe.py --vid 0x0bda --pid 0x3035 --selftest

    # 3) 扫描
    python3 uvc_ctrl_probe.py --vid 0x0bda --pid 0x3035 --scan

    # 4) 对候选持续读，同时给端子拉低
    python3 uvc_ctrl_probe.py --vid 0x0bda --pid 0x3035 --watch --entity 1 --selector 0x06

依赖
----
    pip3 install pyusb          # 另需系统 libusb-1.0
    # 板端是 root，权限没问题；lsusb -v 也需要 root

说明
----
· 本工具以**只读**为主（GET_CUR）；只有 --raw 的写操作会改设备，默认不启用。
· 内核驱动占用设备时，libusb 通常仍可发控制传输（走 EP0，不需要 claim 接口）；
  若全部失败，用 --claim N 尝试 detach+claim 后再读（会导致视频流中断）。
"""
import argparse
import glob
import os
import re
import subprocess
import sys
import time

try:
    import usb.core
    import usb.util
    HAVE_PYUSB = True
except Exception:
    HAVE_PYUSB = False
    usb = None


def require_pyusb():
    """非 --usbfs 模式需要 pyusb；缺失时给清晰提示而不是崩掉。"""
    if HAVE_PYUSB:
        return True
    print("[x] 该模式需要 pyusb（pip3 install pyusb，另需系统 libusb-1.0）")
    print("    ★ 若板子上装不了 pyusb，可改用 **--usbfs** 旁路 —— 它只依赖内核 usbfs，")
    print("      通过 /sys/bus/usb/devices 解析总线/地址，不需要 pyusb。")
    return False


def sysfs_find_usb(vid, pid):
    """不依赖 pyusb：在 /sys/bus/usb/devices 下按 VID:PID 找设备，返回 (busnum, devnum)。"""
    for d in glob.glob("/sys/bus/usb/devices/*"):
        try:
            with open(os.path.join(d, "idVendor")) as f:
                v = int(f.read().strip(), 16)
            with open(os.path.join(d, "idProduct")) as f:
                p = int(f.read().strip(), 16)
        except Exception:
            continue
        if v == vid and p == pid:
            try:
                with open(os.path.join(d, "busnum")) as f:
                    bus = int(f.read().strip())
                with open(os.path.join(d, "devnum")) as f:
                    dev = int(f.read().strip())
                return bus, dev
            except Exception:
                continue
    return None

UVC_GET_CUR = 0x81
UVC_SET_CUR = 0x01

# PU（Processing Unit）的 CS 选择子 → 名称（与 CT 不同！同一 CS 在不同实体上含义不同）
PU_CS_NAMES = {
    0x01: "Backlight Compensation", 0x02: "Brightness", 0x03: "Contrast",
    0x04: "Gain", 0x05: "Power Line Frequency", 0x06: "Hue",
    0x07: "Saturation", 0x08: "Sharpness", 0x09: "Gamma",
    0x0A: "White Balance Temperature", 0x0B: "White Balance Component",
    0x0C: "Digital Multiplier", 0x0D: "Digital Multiplier Limit",
    0x0E: "Analog Video Standard", 0x0F: "Analog Video Lock Status",
    0x10: "Contrast, Auto", 0x11: "Hue, Auto", 0x12: "White Balance Temperature, Auto",
}


def cs_name(subtype, cs):
    """按实体类型解释 CS 选择子的含义（同一 CS 在不同实体上含义不同！）。"""
    st = (subtype or "").upper()
    if "INPUT_TERMINAL" in st:
        base = CT_CS_NAMES.get(cs)
        return f"CT {base}" if base else f"CT CS {cs:#04x}"
    if "PROCESSING_UNIT" in st:
        base = PU_CS_NAMES.get(cs)
        return f"PU {base}" if base else f"PU CS {cs:#04x}"
    if "EXTENSION_UNIT" in st:
        return f"XU 厂商自定义 CS {cs:#04x}"
    return f"{st or '?'} CS {cs:#04x}"


# UVC class 请求的 bmRequestType
RT_GET_CUR = 0xA1      # Device-to-Host, Class, Interface
RT_SET_CUR = 0x21      # Host-to-Device, Class, Interface

# UVC CS 选择子 → 名称（CT = Input Terminal 用的那套）
CT_CS_NAMES = {
    0x01: "Scanning Mode", 0x02: "Auto-Exposure Mode", 0x03: "Auto-Exposure Priority",
    0x04: "Exposure Time (Absolute)", 0x05: "Exposure Time (Relative)",
    0x06: "Focus (Absolute)", 0x07: "Focus (Relative)",
    0x08: "Iris (Absolute)", 0x09: "Iris (Relative)",
    0x0A: "Zoom (Absolute)", 0x0B: "Zoom (Relative)",
    0x0C: "Pan (Absolute)", 0x0D: "Pan (Relative)",
    0x0E: "Tilt (Absolute)", 0x0F: "Tilt (Relative)",
    0x10: "Roll (Absolute)", 0x11: "Roll (Relative)",
    0x12: "Reserved", 0x13: "Reserved", 0x14: "Reserved", 0x15: "Reserved",
    0x16: "Focus, Auto", 0x17: "Privacy",
}

# CT bmControls 位名（bit 下标；CS = bit + 1）
CT_BIT_NAMES = [
    "Scanning Mode", "Auto-Exposure Mode", "Auto-Exposure Priority",
    "Exposure Time (Absolute)", "Exposure Time (Relative)",
    "Focus (Absolute)", "Focus (Relative)", "Iris (Absolute)", "Iris (Relative)",
    "Zoom (Absolute)", "Zoom (Relative)", "Pan (Absolute)", "Pan (Relative)",
    "Tilt (Absolute)", "Tilt (Relative)", "Roll (Absolute)", "Roll (Relative)",
    "Reserved", "Reserved", "Reserved", "Reserved", "Reserved", "Focus, Auto", "Privacy",
]

# PU bmControls 位名（按 UVC 规范的真实位序，已用 lsusb -v 的解码结果核对）
PU_BIT_NAMES = [
    "Brightness", "Contrast", "Hue", "Saturation", "Sharpness", "Gamma",
    "White Balance Temperature", "White Balance Component",
    "Backlight Compensation", "Gain", "Power Line Frequency",
    "Hue, Auto", "White Balance Temperature, Auto",
    "Digital Multiplier", "Digital Multiplier Limit", "Analog Video Standard",
]

# UVC 规范化：**CS 选择子 = 声明位图的 bit 序号 + 1**
#   对 CT：bit5(Focus Abs) → CS 0x06；bit3(Exposure Time Abs) → CS 0x04
#   对 XU：bit9 → CS 10；bit10 → CS 11（XU 的 CS 从 1 开始按位递增）

# ★ VideoStreaming 接口（VC 之外的另一个接口）上的控制 —— 与 entity 控件的 wIndex 形式不同！
#   entity 控件：wIndex 高字节 = entity/unit ID，低字节 = 接口号（0）
#   VS 接口控件：wIndex 高字节 = 0，低字节 = **VS 接口号（本机为 1）**
#     —— 也就是说用 --entity 0 --iface 1 发（见 --vs-scan / --vs-trigger）
VS_CS_NAMES = {
    0x00: "VS_UNDEFINED",
    0x01: "VS_PROBE_CONTROL",                  # 格式探测
    0x02: "VS_COMMIT_CONTROL",                 # 格式提交
    0x03: "VS_STILL_PROBE_CONTROL",            # 静图参数探测
    0x04: "VS_STILL_COMMIT_CONTROL",           # 静图参数提交
    0x05: "VS_STILL_IMAGE_TRIGGER_CONTROL",    # ★ 触发一次静图采集！
    0x06: "VS_STREAM_ERROR_CONTROL",
    0x07: "VS_STREAMING_INTERFACE_ERROR_CONTROL",
    0x08: "VS_GENERATE_KEY_FRAME_CONTROL",
    0x09: "VS_UPDATE_FRAME_SEGMENT_CONTROL",
    0x0A: "VS_SYNCHRONIZATION_DELAY_CONTROL",
}

# 默认扫描的选择子：CT 标准 CS 0x01~0x12 + SDK 位掩码值（厂商可能偷懒拿位掩码当 CS）
DEFAULT_SELECTORS = (
    list(range(0x01, 0x13))
    + [0x10, 0x20, 0x21, 0x30, 0x40, 0x41, 0x80,
       0x100, 0x200, 0x400, 0x1000, 0x20000]
)

INTERESTING_SUBTYPES = ("INPUT_TERMINAL", "PROCESSING_UNIT", "EXTENSION_UNIT",
                        "OUTPUT_TERMINAL", "SELECTOR_UNIT", "ENCODING_UNIT")


def run(cmd, timeout=15):
    try:
        r = subprocess.run(cmd, shell=True, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
        return ((r.stdout or "") + (r.stderr or "")).strip()
    except Exception as e:
        return f"<执行失败: {e}>"


def echo_help():
    print("""
排查建议
--------
A. 错误里出现 Access / Busy / Not supported：
   说明 libusb 通道被内核驱动挡住。
   ① 先停掉占用视频流的进程：pkill -f agent.py; pkill -f ffmpeg
   ② 再试 --claim 0（会 detach 内核驱动并 claim 接口 0，视频流会中断）
   ③ 仍不行就 modprobe -r uvcvideo 后再试（读完 modprobe uvcvideo 恢复）

B. 错误里出现 Pipe / STALL：
   说明请求被设备拒绝了 —— 通常是「实体号不对」或「该控件不存在」。
   ★ 先跑 --descriptor 拿到真实的 CT / PU 实体号，再用正确的 entity 重试。

C. 已声明控件的自检（--selftest）能读到，但 Focus(CS 0x06) 读不到：
   说明固件不通过标准 UVC 请求暴露该映射 →
   厂商用的是 vendor-specific 请求，只能索取 Linux 版 SDK 或协议说明。

D. 自检本身就读不到：
   是通道问题（见 A），不是"控件不存在"，别急着下结论。
""")


# ---------------------------------------------------------------- 描述符解析

def parse_uvc_descriptor(vid, pid):
    """从 lsusb -v 里解析 UVC 描述符的实体列表（id / 类型 / bmControls 字节数组）。"""
    out = run(f"lsusb -v -d {vid:04x}:{pid:04x}")
    if "Device Descriptor" not in out and "Couldn't open" in out:
        print(f"[x] 读不到描述符（lsusb -v 需要 root）：{out[:200]}")
        return []
    entities, cur = [], None
    for raw in out.splitlines():
        s = raw.strip()
        m = re.match(r"bDescriptorSubtype\s+\d+\s*\((.+?)\)", s)
        if m:
            name = m.group(1).strip()
            cur = {"subtype": name, "id": None, "bm": None,
                   "bmbytes": {}, "ctrl_size": None}
            if any(k in name for k in INTERESTING_SUBTYPES):
                entities.append(cur)
            else:
                cur = None
            continue
        if cur is None:
            continue
        m = re.match(r"bTerminalID\s+(\d+)", s)
        if m:
            cur["id"] = int(m.group(1)); continue
        m = re.match(r"bUnitID\s+(\d+)", s)
        if m:
            cur["id"] = int(m.group(1)); continue
        m = re.match(r"bControlSize\s+(\d+)", s)
        if m:
            cur["ctrl_size"] = int(m.group(1)); continue
        m = re.match(r"bNumControls\s+(\d+)", s)
        if m:
            cur["num_ctrls"] = int(m.group(1)); continue
        m = re.match(r"guidExtensionCode\s*\{?([0-9a-fA-F\-]+)\}?", s)
        if m:
            cur["guid"] = m.group(1); continue
        # XU 形式：bmControls( 0) 0x00 / bmControls( 1) 0x06
        m = re.match(r"bmControls\s*\(\s*(\d+)\)\s*0x([0-9a-fA-F]+)", s)
        if m:
            cur["bmbytes"][int(m.group(1))] = int(m.group(2), 16); continue
        # CT/PU 形式：bmControls 0x00002a0e
        m = re.match(r"bmControls\s+0x([0-9a-fA-F]+)", s)
        if m:
            cur["bm"] = int(m.group(1), 16); continue
    return entities


def bm_bytes(cur):
    """把 bmControls 统一成字节数组（低位在前）。"""
    if cur.get("bmbytes"):
        n = max(cur["bmbytes"]) + 1
        return bytes(cur["bmbytes"].get(i, 0) for i in range(n))
    if cur.get("bm") is not None:
        size = cur.get("ctrl_size") or max(1, (cur["bm"].bit_length() + 7) // 8)
        return cur["bm"].to_bytes(size, "little")
    return b""


def declared_cs(cur):
    """返回该实体声明的 CS 选择子列表（bit 序号 + 1）。"""
    data = bm_bytes(cur)
    return [bit + 1 for bit in range(len(data) * 8) if (data[bit // 8] >> (bit % 8)) & 1]


def decode_bits(cur, names):
    data = bm_bytes(cur)
    return [names[bit] for bit in range(len(data) * 8)
            if ((data[bit // 8] >> (bit % 8)) & 1) and bit < len(names)]


def cmd_descriptor(vid, pid):
    print(f"===== UVC 描述符实体列表（{vid:#06x}:{pid:#06x}）=====")
    ents = parse_uvc_descriptor(vid, pid)
    if not ents:
        print("  （没解析到实体 —— 确认 VID/PID，或用 root 跑）")
        print(run(f"lsusb -v -d {vid:04x}:{pid:04x} 2>/dev/null | head -20"))
        return
    ct_ent = pu_ent = xu_ent = None
    xu_cs = []
    for e in ents:
        st, eid = e["subtype"], e["id"]
        cs = declared_cs(e)
        bm = bm_bytes(e)
        print(f"\n  entity {eid if eid is not None else '?':<4} {st:<18} "
              f"bmControls={'-' if not bm else bm.hex()}")
        if e.get("guid"):
            print(f"        guidExtensionCode = {{{e['guid']}}}")
        if st == "INPUT_TERMINAL":
            ct_ent = eid
            names = decode_bits(e, CT_BIT_NAMES)
            print(f"        声明控件({len(cs)}): {', '.join(names) if names else '无'}")
            print(f"        声明的 CS: {', '.join(hex(c) for c in cs) if cs else '无'}")
            print(f"        bit5 = Focus (Absolute) → CS 0x06 : "
                  f"{'✅ 已声明' if 0x06 in cs else '❌ 未声明'}")
        elif st == "PROCESSING_UNIT":
            pu_ent = eid
            names = decode_bits(e, PU_BIT_NAMES)
            print(f"        声明控件({len(cs)}): {', '.join(names) if names else '无'}")
            print(f"        声明的 CS: {', '.join(hex(c) for c in cs) if cs else '无'}")
        elif st == "EXTENSION_UNIT":
            xu_ent = eid
            xu_cs = cs
            print(f"        ★ 厂商 XU！声明控件数={e.get('num_ctrls', len(cs))}，"
                  f"声明的 CS: {', '.join(hex(c) for c in cs) if cs else '无'}")

    print("\n===== 结论与下一步 =====")
    if ct_ent is not None:
        print(f"  CT（Input Terminal）实体号 = {ct_ent}")
    if pu_ent is not None:
        print(f"  PU（Processing Unit）实体号 = {pu_ent}")
    print("  UVC 规则：**CS 选择子 = 声明位图的 bit 序号 + 1**")
    print("    CT: bit5 (Focus Abs) → CS 0x06 ； bit3 (Exposure Time Abs) → CS 0x04")

    cmds = []
    if xu_ent is not None and xu_cs:
        print(f"\n  ★★ **厂商 XU 是最值得先试的目标**（entity {xu_ent}，CS "
              f"{', '.join(hex(c) for c in xu_cs)}）")
        print("     为什么：uvcvideo 只为自己认识的 GUID 创建控件，未知的厂商 XU 会被跳过 ——")
        print("     所以 v4l2-ctl 看不到它，但用 libusb 直接发请求是能读到的。")
        print("     其中很可能就有一个是「软件触发」或「触发状态/模式」控制。")
        for c in xu_cs:
            cmds.append(f"python3 uvc_ctrl_probe.py --vid {vid:#06x} --pid {pid:#06x} "
                        f"--get --entity {xu_ent} --selector {c:#04x}")
            cmds.append(f"python3 uvc_ctrl_probe.py --vid {vid:#06x} --pid {pid:#06x} "
                        f"--watch --entity {xu_ent} --selector {c:#04x}")
    if ct_ent is not None:
        cmds.append(f"python3 uvc_ctrl_probe.py --vid {vid:#06x} --pid {pid:#06x} "
                    f"--get --entity {ct_ent} --selector 0x06   # Focus(未声明，可能被复用为 GPIO7)")

    print("\n  建议依次执行：")
    print(f"    python3 uvc_ctrl_probe.py --vid {vid:#06x} --pid {pid:#06x} --selftest")
    for c in cmds:
        print(f"    {c}")


# ---------------------------------------------------------------- 设备与请求

def build_dev(vid, pid, claim_list=None):
    dev = usb.core.find(idVendor=vid, idProduct=pid)
    if dev is None:
        print(f"[x] 找不到设备 {vid:#06x}:{pid:#06x}（用 --list 确认 VID/PID）")
        return None
    for iface in (claim_list or []):
        try:
            if dev.is_kernel_driver_active(iface):
                dev.detach_kernel_driver(iface)
                print(f"[!] 已 detach 内核驱动（interface {iface}）—— 该摄像头的视频流会中断")
        except Exception as e:
            print(f"[!] detach interface {iface} 失败：{e}")
        try:
            usb.util.claim_interface(dev, iface)
            print(f"[i] 已 claim interface {iface}")
        except Exception as e:
            print(f"[!] claim interface {iface} 失败：{e}")
    return dev


def get_cur(dev, entity, selector, length=2):
    """返回 (data_bytes, None) 或 (None, 错误描述)。"""
    if _USEFS["on"]:
        return _usbfs_get_cur(entity, selector, length)
    try:
        data = dev.ctrl_transfer(0xA1, UVC_GET_CUR, (selector << 8) | 0x00,
                                 (_IFACE["num"] if entity is None else (entity << 8) | _IFACE["num"]), length, timeout=1000)
        return bytes(data), None
    except usb.core.USBError as e:
        err = e.errno
        msg = str(e)
        if "Pipe" in msg or err == 32:
            kind = "STALL(设备拒绝)"
        elif "Access" in msg or err == 13:
            kind = "ACCESS(无权限/被占用)"
        elif "Busy" in msg or err == 16:
            kind = "BUSY(被内核驱动占用)"
        elif "timed out" in msg.lower() or err == 110:
            kind = "TIMEOUT"
        else:
            kind = f"ERR{err}"
        return None, f"{kind}: {msg}"
    except Exception as e:
        return None, f"EXC: {e}"


# ---- 旁路：不经 libusb，直接对 /dev/bus/usb/BBB/DDD 发 USBDEVFS_CONTROL ioctl ----
# 用途：区分「内核挡住请求」与「libusb 挡住请求」。
# 若此路能通，就说明可以**在不 detach 内核驱动、不中断视频流**的前提下读厂商 XU。
_USEFS = {"on": False, "bus": 0, "addr": 0}
# UVC 请求的 wIndex 低字节 = 接口号。VideoControl=0（entity 控件的默认），VideoStreaming=1
# （VS 接口控制必须指向接口 1，见 VS_CS_NAMES / --vs-scan / --vs-trigger）
_IFACE = {"num": 0}
USBDEVFS_CONTROL = 0xC0185500   # _IOWR('U', 0, struct usbdevfs_ctrltransfer)


def _usbfs_do(bm_request_type, b_request, entity, selector, payload, timeout=1000):
    """统一的 usbfs 控制传输实现（GET 时 payload=None，SET 时传字节）。

    返回 (data_or_None, err_or_None)。整个函数体都有保护：
    任何异常（缺模块、权限、ioctl 不认等）都转成可读的错误信息，不让工具崩掉。
    """
    try:
        import ctypes
        import fcntl
        import struct as _struct
    except Exception as e:                      # 理论上不会发生，但别让工具崩
        return None, f"usbfs 依赖缺失: {e}"
    path = f"/dev/bus/usb/{_USEFS['bus']:03d}/{_USEFS['addr']:03d}"
    length = 0 if payload is None else len(payload)
    try:
        if payload is None:
            data = ctypes.create_string_buffer(2)       # GET：读 2 字节
            length = 2
        else:
            data = ctypes.create_string_buffer(bytes(payload), length)
        req = _struct.pack("BBHHH I 4x P", bm_request_type, b_request,
                           (selector << 8) | 0x00, (_IFACE["num"] if entity is None else (entity << 8) | _IFACE["num"]),
                           length, timeout, ctypes.addressof(data))
    except Exception as e:
        return None, f"usbfs 组装请求失败: {e}"
    fd = None
    try:
        fd = os.open(path, os.O_RDWR)
    except OSError as e:
        return None, f"usbfs 打开 {path} 失败: [Errno {e.errno}] {e.strerror}"
    except Exception as e:
        return None, f"usbfs 打开 {path} 异常: {e}"
    try:
        fcntl.ioctl(fd, USBDEVFS_CONTROL, req)
        return (bytes(data.raw[:length]) if payload is None else b""), None
    except OSError as e:
        return None, f"usbfs 被拒: [Errno {e.errno}] {e.strerror}"
    except Exception as e:
        return None, f"usbfs 异常: {e}"
    finally:
        try:
            os.close(fd)
        except Exception:
            pass


def _usbfs_get_cur(entity, selector, length=2, timeout=1000):
    return _usbfs_do(RT_GET_CUR, UVC_GET_CUR, entity, selector, None, timeout)


def _usbfs_set_cur(entity, selector, payload, timeout=1000):
    data, err = _usbfs_do(RT_SET_CUR, UVC_SET_CUR, entity, selector,
                          bytes(payload), timeout)
    if err:
        return False, err
    return True, None


def to_int(b):
    return int.from_bytes(b, "little", signed=False)


def cmd_list():
    print("===== 当前 USB 设备 =====")
    print(f"  {'VID:PID':<14} {'速率':<14} {'总线:地址':<11} 名称")
    for d in usb.core.find(find_all=True):
        try:
            prod = usb.util.get_string(d, d.iProduct) if d.iProduct else ""
        except Exception:
            prod = ""
        spd = getattr(d, "speed", None)
        spd_txt = f"{spd} Mbps" if spd else "?"
        if spd == 5000:
            spd_txt += " (USB3)"
        elif spd in (480, 12, 1):
            spd_txt += " (USB2)"
        mark = "  ← 可能是 UVC 摄像头" if getattr(d, "bDeviceClass", None) == 0xEF else ""
        print(f"  {d.idVendor:#06x}:{d.idProduct:#06x} {spd_txt:<14} "
              f"{d.bus}:{d.address:<8} {prod}{mark}")
    print("\n也可用： lsusb   /   lsusb -t")
    print("查协商速率： cat /sys/bus/usb/devices/<设备>/speed   （5000=USB3, 480=USB2）")


def cmd_read_all(dev, vid, pid):
    """★ 读出所有实体「已声明」的控件 —— 自动包含厂商 XU，不用再猜选择子。

    为什么要按「已声明」读：每个实体的 CS 含义不同（CT 与 PU 完全不同），
    而且 XU 的选择子是厂商自定义的（本机是 0x0A/0x0B）。按描述符里的 bmControls 读，
    就不会漏、也不会读不存在的控件。
    """
    ents = parse_uvc_descriptor(vid, pid)
    if not ents:
        print("[!] 解析不到描述符（需 root 且 lsusb -v 可用），退化为读常用 CS")
        ents = [{"subtype": "INPUT_TERMINAL", "id": 1, "bm": 0x2A0E, "ctrl_size": 3},
                {"subtype": "PROCESSING_UNIT", "id": 2, "bm": 0x177F, "ctrl_size": 2},
                {"subtype": "EXTENSION_UNIT", "id": 4, "bmbytes": {0: 0x00, 1: 0x06},
                 "ctrl_size": 2}]
    print("===== 读取所有「已声明」的控件 =====")
    print("  （按 UVC 描述符的 bmControls 逐实体读；XU 也在内 —— 不用再猜选择子）\n")
    ok_cnt, fail_cnt = 0, 0
    for e in ents:
        st, eid = e["subtype"], e["id"]
        cs_list = declared_cs(e)
        if eid is None:
            continue
        if not cs_list:
            print(f"----- entity {eid} {st}：未声明任何控件，跳过 -----")
            continue
        print(f"----- entity {eid} {st}：声明 {len(cs_list)} 个控件 -----")
        for cs in cs_list:
            name = cs_name(st, cs)
            data, err = get_cur(dev, eid, cs)
            if err is None:
                print(f"  ✅ CS={cs:#04x} {name:<42} → {data.hex()} = {to_int(data)}")
                ok_cnt += 1
            else:
                print(f"  ✗  CS={cs:#04x} {name:<42} → {err}")
                fail_cnt += 1
        print()
    print(f"共成功 {ok_cnt} 个，失败 {fail_cnt} 个")
    if ok_cnt == 0:
        _print_failure_advice("BUSY")
        return
    print("\n下一步：")
    print("  · 上面 XU（EXTENSION_UNIT）的那几个 CS 就是厂商自定义控件 —— 其中很可能有「软触发」")
    print("  · 读出初值后，用 --put 试着写值（0/1/2/3）看是否触发拍照：")
    print("      python3 uvc_ctrl_probe.py --vid <VID> --pid <PID> --put \\")
    print("          --entity <XU实体号> --selector <CS> --value 1")
    print("  · 也可用 --watch 持续读某个 CS，同时改值观察行为变化")


def cmd_selftest(dev, entities, subtype_map=None):
    """读"描述符已声明"的控件，验证通道是否可用。"""
    subtype_map = subtype_map or {}
    print("===== 自检：通道是否可用 =====")
    print("  在每个实体上试读 CS 0x04 / CS 0x02（这些 CS 在不同实体上含义不同，见下面标签）")
    print("  意义：只要能读到一个**已声明**的控件，就说明通道可用；")
    print("        那么读不到的就是「该控件不存在/不可读」，而不是「通道被挡」。")
    print("  ★ 想看全部（含厂商 XU）请用 --read-all\n")
    ok_any = False
    errors = {}
    for ent in entities:
        st = subtype_map.get(ent, "")
        for sel in (0x04, 0x02):
            name = cs_name(st, sel) if st else f"CS {sel:#04x}"
            data, err = get_cur(dev, ent, sel)
            if err is None:
                print(f"  ✅ entity={ent} CS={sel:#04x} ({name}) → {data.hex()} = {to_int(data)}")
                ok_any = True
            else:
                print(f"  ✗  entity={ent} CS={sel:#04x} ({name}) → {err}")
                errors[err.split(":")[0]] = errors.get(err.split(":")[0], 0) + 1
    print()
    if ok_any:
        print("  ✅ **通道可用** —— 成功读到了已声明的控件。")
        print("     那么读不到的那些，就是设备真的不响应（而不是通道被挡）。")
        print("     ★ 下一步：用 --read-all 把每个实体**已声明**的控件全读一遍（会自动包含 XU）")
    else:
        print(f"  ❌ 全部失败，错误类型：{errors}")
        print("     这是**通道问题**，先用下面的办法解决，别急着判定控件不存在。")
    echo_help()


def cmd_scan(dev, entities, selectors):
    print(f"===== 扫描（entity × selector 的 GET_CUR）=====")
    print(f"  entity  : {entities}")
    print(f"  selector: {','.join(hex(s) for s in selectors)}  (共 {len(selectors)} 个)\n")
    found, errors = [], {}
    total = len(entities) * len(selectors)
    for ent in entities:
        for sel in selectors:
            data, err = get_cur(dev, ent, sel)
            if err is None:
                name = CT_CS_NAMES.get(sel, f"selector {sel:#04x}")
                print(f"  ✅ entity={ent:<3} CS={sel:#06x} ({name:<24}) → {data.hex()} = {to_int(data)}")
                found.append((ent, sel, name))
            else:
                kind = err.split(":")[0]
                errors[kind] = errors.get(kind, 0) + 1
                if kind != "STALL(设备拒绝)":      # STALL 是常态，不逐个刷屏
                    print(f"  ✗  entity={ent:<3} CS={sel:#06x} → {err}")
    print(f"\n  共 {total} 次请求：成功 {len(found)}，失败 {total - len(found)}")
    if errors:
        print(f"  失败原因分布：{errors}")
    if not found:
        print("\n  【没有任何成功响应】先判断是哪一类问题：")
        if errors and all(k.startswith("STALL") for k in errors):
            print("   · 全部是 STALL → 请求被设备拒绝。最可能是**实体号不对**。")
            print("     ★ 先跑 --descriptor 拿真实 CT 实体号，再用 --entities <号> 重扫。")
            print("     · 若实体号正确仍全 STALL → 固件不走标准 UVC 请求，")
            print("       厂商用的是 vendor-specific 请求 ⇒ 需要 Linux 版 SDK / 协议说明。")
        else:
            print("   · 出现 ACCESS / BUSY / TIMEOUT 等 → 这是**通道问题**，先解决它：")
        echo_help()
        return
    print(f"\n[i] 有响应的控件如下（其中**内核没暴露**的更值得关注）：")
    for ent, sel, name in found:
        print(f"      python3 uvc_ctrl_probe.py --vid <VID> --pid <PID> --watch "
              f"--entity {ent} --selector {sel:#04x}    # {name}")


def set_cur(dev, entity, selector, payload):
    """写控件（UVC SET_CUR）。返回 (ok, 错误描述)。"""
    if _USEFS["on"]:
        return _usbfs_set_cur(entity, selector, payload)
    try:
        dev.ctrl_transfer(RT_SET_CUR, UVC_SET_CUR, (selector << 8) | 0x00,
                          (_IFACE["num"] if entity is None else (entity << 8) | _IFACE["num"]), bytes(payload), timeout=1000)
        return True, None
    except usb.core.USBError as e:
        err = e.errno
        msg = str(e)
        if "Pipe" in msg or err == 32:
            kind = "STALL(设备拒绝)"
        elif "Access" in msg or err == 13:
            kind = "ACCESS(无权限/被占用)"
        elif "Busy" in msg or err == 16:
            kind = "BUSY(被内核驱动占用)"
        elif "timed out" in msg.lower() or err == 110:
            kind = "TIMEOUT"
        else:
            kind = f"ERR{err}"
        return False, f"{kind}: {msg}"
    except Exception as e:
        return False, f"EXC: {e}"


def cmd_put(dev, entity, selector, value, nbytes, yes=False):
    """写一个值到指定控件：先读旧值 → 写 → 再读回确认。

    用途：厂家示意图里的「软件触发 = USB → UVC 接口触发控制」——
    在 Linux 上就是往那个控件写值。厂商 XU（entity 4, CS 0x0A/0x0B）是首选目标。

    安全说明（重要）：
      · 写 UVC 控件**不会改固件** —— 固件只能通过专用烧录通道（.rfw 文件）修改，
        普通 UVC 控件请求在原理上够不到 flash。
      · 写标准控件（亮度/增益…）只改**运行时参数**，断电或重新插拔即恢复默认。
      · 写厂商 XU 的作用取决于厂商实现（可能是参数，也可能是"触发一次动作"）。
      · 万一有意外：**重新插拔摄像头 → 断电重启 → 厂商工具/固件恢复**。
    """
    name = CT_CS_NAMES.get(selector, f"selector {selector:#04x}")
    old, err = get_cur(dev, entity, selector, nbytes)
    print(f"===== 写控件 entity={entity} CS={selector:#04x} ({name}) =====")
    old_val = None
    if err is None:
        old_val = to_int(old)
        print(f"  写前读取: {old.hex()} = {old_val}")
    else:
        print(f"  写前读取失败({err}) —— 无法备份原值，建议先解决再写")
    payload = int(value).to_bytes(nbytes, "little", signed=False)
    print(f"  写入值  : {payload.hex()}  (= {value}, {nbytes} 字节)")
    print("\n  ⚠️ 安全提示：")
    print("    · 本操作只改**运行时控件值**，**不会修改摄像头固件**（固件只能靠 .rfw 专项烧录）")
    print("    · 写标准控件：断电/重新插拔即恢复默认")
    print("    · 写厂商 XU：用途取决于厂商实现；若有意外 → **重新插拔 → 断电重启** 即可恢复")
    if old_val is not None:
        print(f"    · 已备份原值 = {old_val} ；需要还原时用 "
              f"--put --entity {entity} --selector {selector:#04x} --value {old_val}")
    if not yes:
        print("\n  3 秒后写入，可按 Ctrl+C 中止（加 --yes 可跳过倒计时）…")
        try:
            for i in range(3, 0, -1):
                print(f"     {i}...", flush=True)
                time.sleep(1)
        except KeyboardInterrupt:
            print("\n[i] 已中止，未写入任何数据")
            return
    ok, werr = set_cur(dev, entity, selector, payload)
    if not ok:
        print(f"  ❌ 写入失败: {werr}")
        _print_failure_advice(werr)
        return
    print("  ✅ 写入成功")
    time.sleep(0.2)
    new, err = get_cur(dev, entity, selector, nbytes)
    if err is None:
        print(f"  写后读取: {new.hex()} = {to_int(new)}"
              + ("   ← 值已变化" if old_val is not None and to_int(new) != old_val
                 else "   （读回值与写前相同或无法比较）"))
    else:
        print(f"  写后读取失败: {err}")
    print("\n  下一步怎么看效果：")
    print("    · 若是「软触发」控件 → 写 1 之后摄像头应【拍一张】（与 2P 端子短接等效）；")
    print("      在 Windows 的 amcap 里观察，或用板端 dmesg 看有无 USB 事件")
    print("    · 若是「模式/开关」控件 → 用 --watch 读它，同时改值观察行为变化")
    print("    · 反复试不同值（0/1/2/3）能找出触发用的那一个：")
    print(f"      for v in 0 1 2 3; do python3 {os.path.basename(__file__)} "
          f"--vid <VID> --pid <PID> --put --entity {entity} --selector {selector:#04x} "
          f"--value $v --yes; sleep 1; done")
    print("    · 想还原：用上面打印的『原值』再写回去即可")


def cmd_vs_scan(dev, iface):
    """扫描 VideoStreaming 接口上的 UVC 控制（CS 0x00~0x0A）。

    为什么值得试：UVC 规范在 VS 接口上定义了 **VS_STILL_IMAGE_TRIGGER_CONTROL (CS 0x05)**
    —— 这就是"软件触发一次静图采集"的正规入口，对应厂家示意图里
    "USB → UVC 接口触发控制"那条通路。而 entity 控件扫不到它（wIndex 形式不同）。
    """
    print(f"===== 扫描 VideoStreaming 接口（interface {iface}）的 VS 控制 =====")
    print("  VS 接口控制用 wIndex = 接口号（高字节 0），与 entity 控件不同")
    print("  重点：0x03 STILL_PROBE / 0x04 STILL_COMMIT / 0x05 STILL_IMAGE_TRIGGER\n")
    ok = []
    for cs in range(0x00, 0x0B):
        name = VS_CS_NAMES.get(cs, f"CS {cs:#04x}")
        data, err = get_cur(dev, None, cs)          # entity=None → wIndex 只用接口号
        if err is None:
            print(f"  ✅ CS={cs:#04x} {name:<38} → {data.hex()} = {to_int(data)}")
            ok.append(cs)
        else:
            print(f"  ✗  CS={cs:#04x} {name:<38} → {err}")
    print()
    if not ok:
        print("  全部无响应。可能：")
        print("    · 该设备的 VS 控制不接受 GET_CUR（有些只能 SET）")
        print("    · 需要先 claim 接口 1：加 --claim 0,1 再试")
        print("    · 通道被挡（BUSY）：见 --claim / modprobe -r uvcvideo")
    else:
        print(f"  有响应的 CS：{', '.join(hex(c) for c in ok)}")
    print("\n  下一步：")
    print("    · ★ 直接试【软件触发静图】—— 写 CS 0x05（通常无数据载荷）：")
    print(f"      python3 {os.path.basename(__file__)} --vid <VID> --pid <PID> "
          f"--vs-trigger --claim 0,1")
    print("    · 若 0x01/0x02（PROBE/COMMIT）就有响应，说明设备确实走标准 VS 控制")


def cmd_vs_trigger(dev, iface, value):
    """★ 写 VS_STILL_IMAGE_TRIGGER_CONTROL（CS 0x05）触发一次静图采集。

    UVC 规范：向该控制写入即触发一次静图采集；静图如何上送取决于 bStillCaptureMethod
    （本机 = 2，走流端点）。
    """
    print(f"===== 软件触发静图：VS_STILL_IMAGE_TRIGGER_CONTROL"
          f"（iface={iface}, CS=0x05）=====")
    print("  UVC 规范：向该控制写入即触发一次静图采集（通常无数据载荷）")
    print("  ⚠️ 只改运行时行为，**不会改固件**；若有意外重新插拔/断电即恢复\n")
    payload = b"" if not value else int(value).to_bytes(1, "little")
    if payload:
        print(f"  本次带 1 字节载荷 {payload.hex()}")
    old, e0 = get_cur(dev, None, 0x05)
    print("  写前读取 CS 0x05: " + (f"{old.hex()} = {to_int(old)}" if e0 is None else f"失败({e0})"))
    ok, err = set_cur(dev, None, 0x05, payload)
    if not ok:
        print(f"  ❌ 触发失败: {err}")
        _print_failure_advice(err)
        return
    print("  ✅ 已发送触发请求（写入成功）")
    time.sleep(0.3)
    new, e1 = get_cur(dev, None, 0x05)
    if e1 is None:
        print(f"  写后读取 CS 0x05: {new.hex()} = {to_int(new)}")
    print("\n  怎么判断是否生效（依次试）：")
    print("    1) 看内核有无 USB 事件：      dmesg | tail -20")
    print("    2) 看视频流有无异常/多帧：    python3 multi_cam_timeline.py "
          "--cams /dev/video121 --seconds 5")
    print("    3) 触发前后各抓一帧比内容：   python3 uvc_snapshot.py --grab "
          "--video /dev/video121 -o /tmp/a.jpg")
    print("    4) 若摄像头也能接 Windows（不能同时），用 amcap 看是否出现新照片")
    print("\n  若 STALL / 毫无反应 ⇒ 该控制不被支持，只能向厂家索取静图取回协议")


def cmd_get(dev, entity, selector):
    data, err = get_cur(dev, entity, selector)
    if err:
        print(f"[x] 读取失败：{err}")
        echo_help()
        return
    name = CT_CS_NAMES.get(selector, f"selector {selector:#04x}")
    print(f"entity={entity} CS={selector:#04x} ({name}) → {data.hex()} = {to_int(data)}")


def cmd_watch(dev, entity, selector, interval):
    name = CT_CS_NAMES.get(selector, f"selector {selector:#04x}")
    print(f"===== 持续读取 entity={entity} CS={selector:#04x} ({name}) =====")
    print(">>> 现在请反复给 2P 端子拉低 / 释放，观察数值是否在变 <<<")
    print("    （值在 0/1 之间跳 = 找到触发状态通道；一直不变 = 不是这个控件）\n")
    last, n, fail, consec = None, 0, 0, 0
    last_err = None
    try:
        while True:
            data, err = get_cur(dev, entity, selector)
            n += 1
            if err:
                fail += 1
                consec += 1
                last_err = err
                if consec <= 3:
                    print(f"[{n:5d}] 读取失败：{err}")
                if consec == 5:
                    # 连续失败就别再空转了 —— 给出可执行的处置步骤
                    print(f"\n[x] 连续 5 次失败（共 {n} 次），停止轮询。")
                    print(f"    最后一次错误：{err}\n")
                    _print_failure_advice(err)
                    return
                time.sleep(interval)
                continue
            consec = 0
            v = to_int(data)
            flag = "  ← 变化!" if (last is not None and v != last) else ""
            print(f"[{n:5d}] {data.hex()} = {v}{flag}", flush=True)
            last = v
            time.sleep(interval)
    except KeyboardInterrupt:
        print(f"\n[i] 已停止（共 {n} 次，失败 {fail} 次）")
        if fail and last_err:
            _print_failure_advice(last_err)


def _print_failure_advice(err):
    kind = err.split(":")[0]
    if kind.startswith("BUSY") or kind.startswith("ACCESS"):
        print("  这类错误是「**请求被内核/libusb 挡住**」，不是「设备不响应」—— 属于通道问题。按顺序试：")
        print("  ① 停掉占用视频流的进程（最省事，先做这个）：")
        print("       pkill -f agent.py; pkill -f ffmpeg")
        print("  ② ★ 走 usbfs 旁路（不经 libusb，**不 detach 驱动、不影响视频流**）：")
        print("       在命令后面加 --usbfs   （如 --selftest --usbfs）")
        print("  ③ 让 libusb 接管接口（会中断该摄像头的视频流，重新插拔或重启 Agent 即可恢复）：")
        print("       在命令后面加 --claim 0 ；若失败再试 --claim 0,1")
        print("  ④ 仍不行就临时卸载内核驱动（最彻底，同样会中断视频流）：")
        print("       modprobe -r uvcvideo")
        print("       # 跑完读取后恢复：  modprobe uvcvideo")
        print("  ⑤ 全部失败 ⇒ 该请求格式不被内核允许，需要厂商的 Linux 版 SDK / XU 说明")
    elif kind.startswith("STALL"):
        print("  STALL = 设备拒绝了请求。最可能是**实体号或选择子不对**：")
        print("  ① 先跑 --descriptor 拿真实的 CT / PU / XU 实体号与被声明的 CS")
        print("  ② 再用正确的 --entity / --selector 重试")
        print("  ③ 若实体号正确仍 STALL ⇒ 该控件确实不存在，换下一个候选")
    else:
        print("  详细排查见 --help 或文档 DEVELOPER_GUIDE.md 13.16")


def main():
    ap = argparse.ArgumentParser(description="直发 UVC 控制请求，找回内核未暴露的厂商映射控件")
    ap.add_argument("--list", action="store_true", help="列出 USB 设备（VID/PID + 协商速率）")
    ap.add_argument("--descriptor", action="store_true", help="★解析 UVC 描述符：真实实体号 + 声明的控件")
    ap.add_argument("--selftest", action="store_true", help="★自检通道是否可用")
    ap.add_argument("--read-all", action="store_true",
                    help="★★把每个实体「已声明」的控件全部读出（**包含**厂商 XU；纯只读，不改任何东西）"
                         "—— 不用猜选择子，推荐用这个代替 --scan")
    ap.add_argument("--yes", action="store_true",
                    help="--put 时跳过 3 秒倒计时（默认会倒计时，给你 Ctrl+C 的机会）")
    ap.add_argument("--scan", action="store_true", help="扫描 entity × selector")
    ap.add_argument("--vs-scan", action="store_true",
                    help="★★扫描 VideoStreaming 接口上的 UVC 控制（CS 0x00~0x0A）"
                         "—— 含 VS_STILL_IMAGE_TRIGGER_CONTROL(0x05)，即规范里的软件触发静图入口")
    ap.add_argument("--vs-trigger", action="store_true",
                    help="★★写 CS 0x05 触发一次静图采集（配 --claim 0,1）")
    ap.add_argument("--vs-iface", type=int, default=1,
                    help="VS 接口号（默认 1；用 v4l2-ctl --list-devices 或 --descriptor 可确认）")
    ap.add_argument("--vs-value", type=int, default=0,
                    help="--vs-trigger 的写入值（默认 0 = 无载荷）")
    ap.add_argument("--iface", type=int, default=0,
                    help="wIndex 低字节=接口号（entity 控件默认 0；一般不用改）")
    ap.add_argument("--get", action="store_true", help="单次读取")
    ap.add_argument("--put", action="store_true",
                    help="★写控件（UVC SET_CUR）—— 用于试厂家的『软件触发』："
                         "往 XU entity 4 的 CS 0x0A/0x0B 写值看是否触发拍照")
    ap.add_argument("--value", type=lambda x: int(x, 0), default=1,
                    help="--put 要写入的值（默认 1，支持 0x 前缀）")
    ap.add_argument("--nbytes", type=int, default=2,
                    help="--put 写入字节数（默认 2，与控件长度一致）")
    ap.add_argument("--watch", action="store_true", help="持续读取（配合手动操作端子）")
    ap.add_argument("--vid", type=lambda x: int(x, 0), help="厂商 ID，如 0x0bda")
    ap.add_argument("--pid", type=lambda x: int(x, 0), help="产品 ID，如 0x3035")
    ap.add_argument("--entity", type=lambda x: int(x, 0), default=1,
                    help="实体号（用 --descriptor 拿到，CT 通常是 1；默认 1）")
    ap.add_argument("--selector", type=lambda x: int(x, 0), default=0x06,
                    help="CS 选择子，默认 0x06 = Focus (Absolute)")
    ap.add_argument("--entities", default=None,
                    help="--scan/--selftest 的实体号列表，逗号分隔（默认 1,2,3,4）")
    ap.add_argument("--selectors", default=None,
                    help="--scan 的选择子列表（支持 0x 前缀）；默认见 DEFAULT_SELECTORS")
    ap.add_argument("--claim", default=None,
                    help="传输前 detach 内核驱动并 claim 接口号（逗号分隔，如 0 或 0,1）"
                         "—— 会中断该摄像头视频流；用于排除『内核驱动占用(BUSY)』")
    ap.add_argument("--usbfs", action="store_true",
                    help="★旁路 libusb，直接对 /dev/bus/usb/BBB/DDD 发 USBDEVFS_CONTROL ioctl"
                         "—— 用来区分『内核挡住』还是『libusb 挡住』；"
                         "若此路能通，就不用 detach 驱动、也不影响视频流")
    ap.add_argument("--interval", type=float, default=0.2, help="--watch 轮询间隔秒（默认 0.2）")
    args = ap.parse_args()

    if args.list:
        if not require_pyusb():
            return
        cmd_list()
        return
    if args.vid is None or args.pid is None:
        ap.error("需要 --vid 和 --pid（先用 --list 或 lsusb 拿到）")

    if args.descriptor:
        cmd_descriptor(args.vid, args.pid)
        return

    if args.entities:
        try:
            ents = [int(x, 0) for x in str(args.entities).split(",") if x.strip()]
        except ValueError:
            ap.error("--entities 格式应为逗号分隔整数，如 1,2")
    else:
        ents = [1, 2, 3, 4]

    claim_list = None
    if args.claim:
        try:
            claim_list = [int(x, 0) for x in str(args.claim).split(",") if x.strip()]
        except ValueError:
            ap.error("--claim 格式应为逗号分隔整数，如 0 或 0,1")

    if args.usbfs:
        # ★ usbfs 旁路只需要 总线:地址 —— 优先用 pyusb 拿，没有 pyusb 就用 sysfs 解析
        bus = addr = None
        if HAVE_PYUSB:
            d = usb.core.find(idVendor=args.vid, idProduct=args.pid)
            if d is not None:
                bus, addr = d.bus, d.address
        if bus is None:
            r = sysfs_find_usb(args.vid, args.pid)
            if r:
                bus, addr = r
        if bus is None:
            print(f"[x] 找不到设备 {args.vid:#06x}:{args.pid:#06x}")
            print("    用 lsusb 或 --list 确认 VID/PID；也可能设备未插好")
            return
        _USEFS.update(on=True, bus=bus, addr=addr)
        print(f"★ 使用 usbfs 旁路：/dev/bus/usb/{bus:03d}/{addr:03d}"
              f"（不经 libusb；不 detach 驱动、不影响视频流）")
        print(f"  {'（pyusb 可用，仅用于定位设备）' if HAVE_PYUSB else '（无 pyusb，地址来自 sysfs）'}")
        dev = None
    else:
        if not require_pyusb():
            return
        dev = build_dev(args.vid, args.pid, claim_list)
        if dev is None:
            return
        print(f"[i] 已打开设备 {args.vid:#06x}:{args.pid:#06x}")

    _IFACE["num"] = args.iface          # wIndex 低字节 = 接口号
    if args.vs_scan:
        cmd_vs_scan(dev, args.vs_iface)
    elif args.vs_trigger:
        cmd_vs_trigger(dev, args.vs_iface, args.vs_value)
    elif args.read_all:
        cmd_read_all(dev, args.vid, args.pid)
    elif args.selftest:
        # 解析描述符，拿到 entity → 类型 的映射，这样标签才准确
        subtype_map = {}
        try:
            for e in parse_uvc_descriptor(args.vid, args.pid):
                if e.get("id") is not None:
                    subtype_map[e["id"]] = e["subtype"]
        except Exception:
            pass
        cmd_selftest(dev, ents, subtype_map)
    elif args.scan:
        if args.selectors:
            try:
                selectors = [int(x, 0) for x in str(args.selectors).split(",") if x.strip()]
            except ValueError:
                ap.error("--selectors 格式应为逗号分隔整数（支持 0x 前缀），如 0x04,0x06,0x20")
        else:
            selectors = list(DEFAULT_SELECTORS)
        cmd_scan(dev, ents, selectors)
    elif args.watch:
        cmd_watch(dev, args.entity, args.selector, args.interval)
    elif args.put:
        cmd_put(dev, args.entity, args.selector, args.value, args.nbytes, args.yes)
    else:
        cmd_get(dev, args.entity, args.selector)


if __name__ == "__main__":
    main()
