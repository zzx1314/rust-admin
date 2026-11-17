#!/usr/bin/env python3
"""探测「2pin 硬触发产生的那张静态帧」是否可识别 —— 读 UVC metadata 节点

背景与目标
----------
现场要求：两个摄像头的 2P 端子都接到板子 GPIO46，做**硬触发**；
并且**拿到的必须是"由 2pin 触发产生的那张静态帧"**，还要它的时间戳来算延时。

已知事实（本项目已实测）：
  · 2P 硬触发（空闲高 → 拉低下降沿）→ 摄像头内部采一张静图（Windows amcap 已证实）
  · Linux 的 uvcvideo **不实现 UVC still image 捕获** ⇒ 标准 v4l2 API 拿不到那张图
  · 描述符：`bStillCaptureMethod = 2`（静图走**流端点**）、`bTriggerUsage = 0`（静图语义）、
    `STILL_IMAGE_FRAME.bEndpointAddress = 0x00`

★ 本工具的突破口
----------------
设备暴露了一个 **Metadata Capture** 节点（本机 `/dev/video122`，Device Caps 含
`Metadata Capture`）。其数据类型是 `V4L2_META_FMT_UVC`（fourcc `'UVCH'`），
内容是**每一帧的 UVC payload header**。而 payload header 首字节的位定义里

    bit0 FID   帧标识        bit4 RES  保留
    bit1 EOF   帧结束        bit5 STI  ★ Still Image（这一帧是静态帧）
    bit2 PTS   携带 PTS      bit6 ERR  错误
    bit3 SCR   时钟参考      bit7 EOH  头部结束

⇒ **若摄像头在「触发产生的那一帧」上把 STI 置 1，我们就能精确认出它**，
   再从视频节点取那一帧并记时间戳，即可满足"必须由 2pin 控制的静态帧"这一要求。

本工具先回答一个问题：**STI 位到底会不会被置起来？**
（这是整条路的开关。会 → 继续做精确版；不会 → 只能走厂家 SDK。）

用法
----
    # 0) 先看 metadata 节点支持什么格式
    python3 uvc_still_probe.py --formats --node /dev/video122

    # 1) 基线：只抓 metadata，不加触发（确认节点可读、看 STI 是否本来就为 1）
    python3 uvc_still_probe.py --node /dev/video122 --count 200

    # 2) ★ 一边抓一边用 GPIO46 硬触发（每 2 秒一次，共 20 秒）
    python3 uvc_still_probe.py --node /dev/video122 --count 600 \
        --seconds 20 --gpio 46 --trigger-interval 2000

    # 3) 自动推断记录长度失败时，手工指定
    python3 uvc_still_probe.py --node /dev/video122 --count 200 --stride 32

依赖
----
    · v4l2-ctl（v4l-utils）
    · 需要 root（或对 /dev/video* 有权限）
    · 注意：同一物理流不能被两个进程同时打开 —— 先 pkill -f agent.py; pkill -f ffmpeg
"""
import argparse
import os
import re
import subprocess
import sys
import time

GPIO_ROOT = "/sys/class/gpio"
DUMP = "/tmp/uvc_meta_dump.bin"

BIT_NAMES = [(0, "FID"), (1, "EOF"), (2, "PTS"), (3, "SCR"),
             (4, "RES"), (5, "STI"), (6, "ERR"), (7, "EOH")]


def run(cmd, timeout=60):
    try:
        r = subprocess.run(cmd, shell=True, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
        return ((r.stdout or "") + (r.stderr or "")).strip()
    except Exception as e:
        return f"<执行失败: {e}>"


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


def decode_payload_header(b):
    """解码 UVC payload header 首字节的位含义。"""
    flags = [name for bit, name in BIT_NAMES if (b >> bit) & 1]
    return "/".join(flags) if flags else "(全 0)"


def main():
    ap = argparse.ArgumentParser(description="探测 UVC metadata 里的 Still Image(STI) 标记")
    ap.add_argument("--node", default="/dev/video122", help="metadata 节点（默认 /dev/video122）")
    ap.add_argument("--formats", action="store_true", help="只列出该节点支持的格式")
    ap.add_argument("--count", type=int, default=200, help="抓多少条 metadata 记录（默认 200）")
    ap.add_argument("--stride", type=int, default=None, help="每条记录字节数（默认自动推断）")
    ap.add_argument("--seconds", type=float, default=None,
                    help="抓取时长秒数（给了会配合 --gpio 一起触发；不给则按条数抓）")
    ap.add_argument("--gpio", type=int, help="硬触发用的 GPIO（如 46）")
    ap.add_argument("--trigger-interval", type=int, default=2000, help="触发周期 ms")
    ap.add_argument("--trigger-low", type=int, default=100, help="低电平保持 ms")
    ap.add_argument("--show", type=int, default=40, help="逐条打印前多少条（默认 40，0=全打）")
    args = ap.parse_args()

    if args.formats or not os.path.exists(args.node):
        print(f"===== {args.node} 支持的格式 =====")
        print(run(f"v4l2-ctl -d {args.node} --list-formats"))
        print("\n----- 设备能力 -----")
        print(run(f"v4l2-ctl -d {args.node} --all | head -30"))
        if not os.path.exists(args.node):
            print(f"\n[x] {args.node} 不存在 —— 用 v4l2-ctl --list-devices 确认节点名")
        return

    # 组合 v4l2-ctl 抓取命令
    cmd = ["v4l2-ctl", "-d", args.node,
           "--stream-mmap", "--stream-count", str(args.count),
           f"--stream-to={DUMP}"]
    print(f"===== 抓取 metadata =====")
    print(f"  {args.node}  →  {DUMP}")
    print(f"  条数 {args.count}" + (f"，时长约 {args.seconds}s" if args.seconds else ""))
    if args.gpio is not None:
        print(f"  ★ 同时用 gpio{args.gpio} 硬触发（每 {args.trigger_interval}ms 拉低 "
              f"{args.trigger_low}ms）")
    print("  注意：同一物理流不能被两个进程同时打开 —— 请先 pkill -f agent.py; pkill -f ffmpeg\n")

    try:
        os.remove(DUMP)
    except Exception:
        pass

    if args.gpio is not None and not gpio_prepare(args.gpio):
        return

    trig_times = []
    t0 = time.time()
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    timed_out = False
    try:
        if args.gpio is None:
            try:
                proc.wait(timeout=max(60, args.count // 10))
            except subprocess.TimeoutExpired:
                timed_out = True
                print(f"\n[!] 等待 {max(60, args.count // 10)}s 后**没有任何数据到达** —— 该节点可能不支持 metadata 上送")
                try:
                    proc.kill()
                except Exception:
                    pass
        else:
            t_end = t0 + (args.seconds or (args.count / 30.0))
            next_trig = t0 + 0.3
            while time.time() < t_end:
                if time.time() >= next_trig:
                    te = time.time()
                    gpio_set(args.gpio, 0)          # 下降沿 = 硬触发
                    trig_times.append(te)
                    time.sleep(args.trigger_low / 1000.0)
                    gpio_set(args.gpio, 1)
                    next_trig = te + args.trigger_interval / 1000.0
                else:
                    time.sleep(0.005)
            try:
                proc.terminate()
                proc.wait(timeout=5)
            except Exception:
                try:
                    proc.kill()
                except Exception:
                    pass
    except KeyboardInterrupt:
        print("\n[i] 提前结束")
        try:
            proc.terminate()
        except Exception:
            pass
    t1 = time.time()
    if args.gpio is not None:
        gpio_set(args.gpio, 1)

    if not os.path.exists(DUMP):
        print("[x] 没有生成 dump 文件 —— 该节点可能不支持 --stream-to，或设备被占用")
        print(run(f"v4l2-ctl -d {args.node} --all | head -20"))
        return
    size = os.path.getsize(DUMP)
    print(f"[i] dump 大小 {size} 字节，用掉 {t1 - t0:.2f}s")

    # ★ 0 字节 / 超时 ⇒ 节点不产数据，直接给出明确结论（不再继续瞎解析）
    if size == 0:
        print("\n===== 结论 =====")
        print("  ❌ dump 为 0 字节 —— **该 metadata 节点没有数据送达**。")
        print("     原因（对照描述符）：VideoStreaming INPUT_HEADER 里")
        print("       bControlSize = 1，bmaControls(0) = 0")
        print("     ⇒ 设备**没有启用 payload-header metadata 上送**，")
        print("       所以 uvcvideo 虽然建了 metadata 节点，队列永远是空的。")
        print("\n  ⇒ **STI（Still Image 标记）这条路排除**，不要再在这个节点上花时间。")
        print("\n  转去做这两件事：")
        print("    ① ★ 试 UVC 规范的 **VS_STILL_IMAGE_TRIGGER_CONTROL（CS 0x05, 接口1）** ——")
        print("       这才是【软件触发静图】的正规入口：")
        print("         python3 uvc_ctrl_probe.py --vid 0x0bda --pid 0x3035 --vs-scan --claim 0,1")
        print("         python3 uvc_ctrl_probe.py --vid 0x0bda --pid 0x3035 --vs-trigger --claim 0,1")
        print("    ② 向厂家索取静图取回协议 / Linux 版 SDK（见文档 13.21 第六节）")
        if timed_out:
            print("\n  （本次是等待超时后中止，说明确实一直没有数据）")
        return

    # 推断 stride
    stride = args.stride
    if not stride:
        for cand in (32, 64, 16, 24, 8, 44, 60, 128):
            if size % cand == 0:
                stride = cand
                break
    if not stride:
        stride = 32
        print(f"[!] 无法均分，按默认 stride={stride} 解析（可用 --stride 指定）")
    nrec = size // stride
    print(f"[i] 推断每条记录 {stride} 字节，共 {nrec} 条")

    with open(DUMP, "rb") as f:
        data = f.read()

    sti_idx = []
    print(f"\n===== 逐条解码（UVC payload header 首字节）=====")
    show = nrec if args.show == 0 else min(args.show, nrec)
    for i in range(show):
        rec = data[i * stride:(i + 1) * stride]
        if not rec:
            break
        b0 = rec[0]
        flags = decode_payload_header(b0)
        mark = "  ★★★ STI=1（静态帧！）" if (b0 >> 5) & 1 else ""
        print(f"  [{i:5d}] b0=0x{b0:02x}  {flags:<28}  {rec[:16].hex()}{mark}")
    for i in range(nrec):
        rec = data[i * stride:(i + 1) * stride]
        if rec and ((rec[0] >> 5) & 1):
            sti_idx.append(i)

    print(f"\n===== 结论 =====")
    if sti_idx:
        print(f"  ★★★ 发现 {len(sti_idx)} 条 STI=1 的记录，索引: "
              f"{sti_idx[:40]}{' …' if len(sti_idx) > 40 else ''}")
        print(f"  记录平均间隔 ≈ {(t1 - t0) / max(1, nrec) * 1000:.1f} ms")
        if trig_times:
            print(f"\n  触发时刻（相对抓取起点，共 {len(trig_times)} 次）：")
            for i, tt in enumerate(trig_times[:20]):
                est = (tt - t0) / ((t1 - t0) / max(1, nrec))   # 估算对应的记录索引
                print(f"    第 {i+1} 次触发 @ {tt - t0:7.3f}s → 估算记录索引 ≈ {est:.0f}")
            print("\n  ⇒ 若上面 STI=1 的索引与这些估算索引对应，就说明"
                  "**STI 标记 = 硬触发产生的那一帧**")
            print("     ⇒ 这条路走通！可以进一步把该帧 JPEG 从视频节点取出并打时间戳")
        else:
            print("  （本次没加触发，可加上 --gpio 46 再跑一次看 STI 是否与触发对应）")
    else:
        print("  ❌ 所有记录 STI 均为 0 —— 该节点没有标记静态帧。")
        print("     可能原因：")
        print("       ① metadata 节点传的不是 payload header（看 --formats 的 fourcc 是否为 UVCH）")
        print("       ② stride 推断错误，解析错位（用 --stride 手工指定再试）")
        print("       ③ 固件确实不打 STI 标记 ⇒ 只能向厂家索取静图取回协议 / Linux 版 SDK")
    print("\n提示：")
    print("  · 若 --formats 显示 fourcc 不是 'UVCH'，请把输出贴回来，我按实际格式改解析")
    print("  · 想同时看视频帧时间戳，用 multi_cam_timeline.py（它读的是 Video Capture 节点）")


if __name__ == "__main__":
    main()
