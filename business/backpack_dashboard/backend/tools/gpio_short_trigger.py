#!/usr/bin/env python3
"""摄像头 2P 端子触发工具 —— 已确认「电平型 / 高→低下降沿触发」

已确认的触发语义（2026-10-15 现场实测）
------------------------------------
    1. 端子悬空时摄像头**正常出流**，所以端子不是"控制出图的开关"
    2. 端子是**电平型输入**，不是干接点：GPIO 拉高/拉低都能参与触发
    3. 必须**先高电平、再拉低**才触发 → 即检测「高→低的下降沿」
       只 echo 1 不触发；只 echo 0 也不触发；echo 1 之后再 echo 0 才触发
    4. 由此推知端子**内部有上拉**，空闲电平为高
       —— 这同时解释了为什么"手动短接"也能触发（短接 = 从高拉到低）

    ⇒ 实现要点：
         空闲态 = **高**（推高 3.3V，或高阻靠内部上拉，两者都行）
         触发   = **拉低**（产生一个下降沿）
         连续触发 = 周期性 高→低，**每个下降沿 = 一次触发**

    推荐用法：`--mode low`（空闲推高 / 触发拉低）—— 正是上面这个语义。

一条重要的计算注意：触发周期是"高保持 + 低保持"之和。
只有高保持时间缩短到足够小，才可能跑到 30Hz 以上；
用 `--probe-high` 可以扫出「最小有效高电平保持时间」。

用法
----
    # 引脚现状（补光灯可能把它留在某个电平上）
    python3 gpio_short_trigger.py --gpio 46 --status
    python3 gpio_short_trigger.py --gpio 46 --release

    # 单次触发（推荐 --mode low：空闲高 / 触发拉低）
    python3 gpio_short_trigger.py --gpio 46 --mode low
    python3 gpio_short_trigger.py --gpio 46 --mode low --count 5 --interval 2000

    # 【关键实验】低频连续方波：判断摄像头是否处于"外触发出流"模式
    #   ⚠️ 必须用低频（1~2Hz）！30Hz 下"自由运行"和"被触发"都是 30fps，区分不出来
    python3 gpio_short_trigger.py --gpio 46 --wave --hz 2 --seconds 20

    # 【关键实验】扫出最小高电平保持时间：决定触发最高能跑多快
    python3 gpio_short_trigger.py --gpio 46 --probe-high

    # 只看计划不动硬件
    python3 gpio_short_trigger.py --gpio 46 --wave --hz 2 --dry-run

    # 其它：--list 列出已导出引脚；--release 恢复高阻；--sweep 依次试 short/low/high

安全提示
--------
1. `low` / `high` 会**推挽主动驱动**引脚。既然实测推高 3.3V 可用，说明端子是 3.3V 兼容域，
   但仍建议串一个 100Ω~1kΩ 保护电阻，避免误接短路时过流。
2. 必须**共地**（电平型输入依赖参考地）。
3. 退出时会把引脚恢复为空闲态；`--wave` / `--probe-high` 结束后回到高电平（空闲）。
4. 若摄像头一次触发存一张图，连续触发会产生大量文件，注意存储。
"""
import argparse
import os
import signal
import sys
import time

GPIO_ROOT = "/sys/class/gpio"
_state = {"gpio": None, "mode": None}

MODES = {
    "short": ("干接点模拟", "空闲=高阻(断开)", "触发=out+0(拉低=短接)"),
    "low":   ("高-低脉冲（实测推荐）", "空闲=out+1(推高3.3V)", "触发=out+0(拉低)"),
    "high":  ("高电平有效", "空闲=out+0(拉低)", "触发=out+1(推高3.3V)"),
}

# sweep 顺序：先把与原厂验证等价的 short 跑一遍作基准，再试两种电平极性
SWEEP_ORDER = ["short", "low", "high"]

# --probe-high 默认扫描的高电平保持时间（ms），从大到小
DEFAULT_HIGH_TIMES = "2000,1000,500,200,100,50,20,10,5,2,1"


def _write(path, text):
    with open(path, "w") as f:
        f.write(str(text))


def _read(path):
    try:
        with open(path) as f:
            return f.read().strip()
    except Exception:
        return ""


def _pin(gpio):
    return f"{GPIO_ROOT}/gpio{gpio}"


def load_project_config():
    """尝试读取项目 config.py，用于检测引脚冲突。失败则返回 None。"""
    here = os.path.dirname(os.path.abspath(__file__))
    parent = os.path.dirname(here)
    if parent not in sys.path:
        sys.path.insert(0, parent)
    try:
        import config
        return config
    except Exception:
        return None


def check_conflict(gpio):
    cfg = load_project_config()
    if cfg is None:
        print("[i] 未读到项目 config.py，跳过引脚冲突检查")
        return
    fill = getattr(cfg, "FILL_LIGHT_GPIO", None)
    trig = getattr(cfg, "CAM_TRIGGER_GPIO", None)
    if fill is not None and fill == gpio and fill >= 0:
        print(f"[!] 冲突警告：gpio{gpio} 在 config.py 中仍是 FILL_LIGHT_GPIO（补光灯）。")
        print("    补光灯会反复改写该脚的方向与电平，和你设的触发逻辑互相打架，")
        print("    会造成误触发甚至持续触发，测试结果完全不可信。")
        print("    建议把 FILL_LIGHT_GPIO 设为 -1（禁用补光灯）后重启 Agent，")
        print("    测试期间也不要从网页点补光灯开关。")
    if trig is not None and trig >= 0 and trig != gpio:
        print(f"[i] config.py 里 CAM_TRIGGER_GPIO={trig}，与你本次测试的 {gpio} 不一致（仅提示）")


def list_exported():
    if not os.path.isdir(GPIO_ROOT):
        print(f"[!] {GPIO_ROOT} 不存在 —— 内核可能未启用 CONFIG_GPIO_SYSFS")
        print("    改用 libgpiod：gpioinfo / gpioset")
        return
    pins = sorted([d[len("gpio"):] for d in os.listdir(GPIO_ROOT)
                   if d.startswith("gpio")], key=lambda x: int(x))
    if not pins:
        print("当前没有已导出的 GPIO")
        return
    for p in pins:
        base = _pin(p)
        print(f"  gpio{p:<4} direction={_read(base+'/direction'):<4} "
              f"value={_read(base+'/value')}")


def ensure_export(gpio):
    path = _pin(gpio)
    if os.path.isdir(path):
        return True
    try:
        _write(f"{GPIO_ROOT}/export", gpio)
    except Exception as e:
        print(f"[✗] export gpio{gpio} 失败: {e}")
        print("    'Device or resource busy' → 该脚已被内核驱动占用，换一个空闲脚")
        print("    'Invalid argument'        → 该编号不存在或超出范围")
        return False
    time.sleep(0.05)
    print(f"[✓] gpio{gpio} 已导出")
    return True


def show_status(gpio):
    base = _pin(gpio)
    if not os.path.isdir(base):
        print(f"[i] gpio{gpio} 尚未导出（当前无人占用该脚）")
        return
    d, v = _read(base + "/direction"), _read(base + "/value")
    print(f"gpio{gpio}: direction={d}  value={v}")
    if d == "out" and v == "0":
        print("[!] 正处于『输出低电平』—— 若摄像头是短接型，此刻等于**持续触发中**。")
        print("    可用 --release 恢复为高阻（断开）。")
    elif d == "out" and v == "1":
        print("[!] 正处于『输出高电平 3.3V』—— 会向摄像头端子灌电压。")


def apply_idle(gpio, mode):
    base = _pin(gpio)
    if mode == "short":
        _write(base + "/direction", "in")          # 高阻 = 断开
    elif mode == "low":
        _write(base + "/direction", "out")
        _write(base + "/value", 1)                 # 推高
    elif mode == "high":
        _write(base + "/direction", "out")
        _write(base + "/value", 0)                 # 拉低


def apply_trigger(gpio, mode):
    base = _pin(gpio)
    if mode in ("short", "low"):
        _write(base + "/direction", "out")
        _write(base + "/value", 0)                 # 拉低
    else:  # high
        _write(base + "/direction", "out")
        _write(base + "/value", 1)                 # 推高


def release(gpio):
    """恢复高阻（断开），停止任何持续触发。"""
    _write(_pin(gpio) + "/direction", "in")


def cleanup(*_):
    g = _state.get("gpio")
    if g is not None:
        try:
            release(g)
            print(f"\n[i] 退出前已将 gpio{g} 恢复为 input（高阻 / 断开）")
        except Exception:
            pass
    sys.exit(0)


def run_wave(gpio, hz, seconds, idle_level):
    """连续方波输出 —— 用来判断摄像头是否处于「外触发出流」模式。

    为什么必须用**低频**（1~2Hz）而不是 30Hz：
        自由运行模式下相机自己按 30fps 出帧，10 秒 ≈ 300 帧；
        外触发模式下若也触发 30 次/秒，10 秒同样 ≈ 300 帧 —— 两种结果一样，测不出区别。
        但只要把触发降到 1~2Hz：
            自由运行 → 依旧 30fps，画面流畅
            外触发   → 变成每秒 1~2 帧的"幻灯片"
        数量级差距一眼可辨。

    顺带还能判断「两个沿都触发吗」：
        方波一个周期有 2 个沿（上升 + 下降）。设 --hz 1，若画面变成 2fps
        而不是 1fps，说明两个沿都会触发，实际触发率 = 2 × hz。
    """
    if hz <= 0:
        print("[x] --hz 必须大于 0")
        return
    period = 1.0 / hz
    half = period / 2.0
    edges_per_sec = hz * 2

    print(f"\n===== 连续方波：{hz} Hz，持续 {seconds}s =====")
    print(f"  半周期 {half*1000:.1f} ms；一个周期 2 个沿")
    print(f"  [!] 若摄像头『两个沿都触发』，实际触发率 = {edges_per_sec:.1f} 次/秒")
    print(f"  [!] 若摄像头一次触发存一张图，本次约会产生 {edges_per_sec*seconds:.0f} 张，注意存储")
    print(f"  [!] 3 秒后开始输出，请现在去另一个终端启动 ffmpeg / 播放器观察画面")
    for i in range(3, 0, -1):
        print(f"      {i}...", flush=True)
        time.sleep(1)

    base = _pin(gpio)
    _write(base + "/direction", "out")
    t0 = time.time()
    t_end = t0 + seconds
    n = 0
    try:
        while time.time() < t_end:
            _write(base + "/value", 1)
            t_hi = time.time() - t0
            time.sleep(half)
            _write(base + "/value", 0)
            t_lo = time.time() - t0
            n += 1
            print(f"  [周期 {n:3d}] 高 @ {t_hi:7.3f}s  低 @ {t_lo:7.3f}s", flush=True)
            time.sleep(half)
    except KeyboardInterrupt:
        print("\n[i] 已被 Ctrl+C 中断")
    print(f"[OK] 共输出 {n} 个周期（{n*2} 个跳变沿），理论 {seconds}s / {period*1000:.0f}ms = "
          f"{seconds/period:.0f} 个周期")

    # 收尾：优先恢复到用户指定的空闲电平；未指定则释放为高阻
    if idle_level is None:
        release(gpio)
        print(f"[i] gpio{gpio} 已释放为 input（高阻）。若发现卡在触发态，改用 "
              f"--wave-idle 0 或 --wave-idle 1 显式输出空闲电平。")
    else:
        _write(base + "/value", idle_level)
        print(f"[i] gpio{gpio} 已显式置为 {idle_level}（用户指定的 idle 电平）")

    print("\n判读：")
    print(f"  · 画面变成每秒约 {hz} 帧的『幻灯片』  → 真·外触发模式 ✅ 四路同步可行")
    print(f"  · 画面依旧流畅 30fps                → 触发与视频流无关 ❌ 只是拍照功能")
    print(f"  · 变成每秒约 {edges_per_sec:.0f} 帧          → 且说明『两个沿都触发』"
          f"（实际触发率 = 2 × hz）")


def run_probe_high(gpio, times_ms, pulses, low_ms, gap_s):
    """扫描「高电平需要先保持多久才被认」—— 决定触发能跑多快。

    实测已确认：触发 = 高→低的下降沿，而"先 echo 1"的作用是先建立高电平。
    于是剩下一个关键未知量：高电平要保持多久摄像头才认？
      · 若 1ms 就够 → 可以跑到几百 Hz，30fps 同步毫无压力
      · 若必须保持 100ms 以上 → 最高只有 ~10Hz，做不了 30fps 同步

    本模式从大到小扫一遍，每档触发 N 次并留出间隔，你盯着厂商软件数"多了几张照片"，
    数量对得上的最小档位就是答案。
    """
    n_per = pulses
    print(f"\n===== 扫描「最小高电平保持时间」 =====")
    print(f"  每档触发 {n_per} 次，低电平保持 {low_ms}ms，档间停顿 {gap_s}s")
    print("  >>> 请盯住厂商软件，数每档之后『照片多了几张』，对得上 = 该档有效 <<<")
    base = _pin(gpio)
    _write(base + "/direction", "out")
    _write(base + "/value", 1)          # 起始空闲 = 高
    time.sleep(0.5)

    results = []
    for idx, t_ms in enumerate(times_ms, 1):
        print(f"\n----- 第 {idx}/{len(times_ms)} 档：高电平保持 {t_ms} ms "
              f"（等效最高频率约 {1000.0/(t_ms+low_ms):.1f} Hz）-----", flush=True)
        time.sleep(1.0)
        ok = 0
        for k in range(1, n_per + 1):
            _write(base + "/value", 1)          # 建立高电平
            time.sleep(t_ms / 1000.0)
            _write(base + "/value", 0)          # 拉低 = 产生下降沿 = 触发
            time.sleep(low_ms / 1000.0)
            ok += 1
            print(f"    第 {k}/{n_per} 次触发已发出", flush=True)
        _write(base + "/value", 1)              # 回到空闲高
        print(f"    本档共发出 {n_per} 次触发，请记录照片数量", flush=True)
        results.append((t_ms, n_per))
        if idx < len(times_ms):
            print(f"    停顿 {gap_s}s …", flush=True)
            time.sleep(gap_s)

    _write(base + "/value", 1)
    print("\n===== 扫描结束 =====")
    print("  把你观察到的『最小有效高电平保持时间』记下来：")
    print("    · ≤ 5ms   → 最高可跑 ~100Hz 以上，30fps 同步轻松")
    print("    · ~20ms   → 最高约 30~40Hz，30fps 同步刚好够用")
    print("    · ≥ 100ms → 最高约 10Hz，做不了 30fps 同步")
    print("  引脚已回到空闲高电平。")


def run_mode(gpio, mode, count, interval, hold):
    name, idle_desc, trig_desc = MODES[mode]
    print(f"\n===== 方式 [{mode}] {name} =====")
    print(f"  空闲态：{idle_desc}")
    print(f"  触发态：{trig_desc}")
    if mode in ("low", "high"):
        print("  [!] 电平方式会主动推挽驱动引脚，3 秒后开始，可 Ctrl+C 中止")
        for i in range(3, 0, -1):
            print(f"      {i}...", flush=True)
            time.sleep(1)

    apply_idle(gpio, mode)
    print("  [i] 已置为空闲态，等待 0.5s 稳定")
    time.sleep(0.5)

    t0 = time.time()
    for i in range(1, count + 1):
        apply_trigger(gpio, mode)
        t_trig = time.time() - t0
        time.sleep(hold / 1000.0)
        apply_idle(gpio, mode)
        t_idle = time.time() - t0
        print(f"  [{i}/{count}] 触发 @ {t_trig:6.3f}s → 恢复空闲 @ {t_idle:6.3f}s"
              f"（触发保持 {hold}ms）", flush=True)
        if i < count:
            time.sleep(max(0, interval - hold) / 1000.0)
    apply_idle(gpio, mode)
    print(f"  [OK] 方式 [{mode}] 完成 {count} 次")


def main():
    ap = argparse.ArgumentParser(
        description="摄像头 2P 端子触发测试（短接型 / 电平型逐种试）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="触发方式说明：\n"
               "  short  空闲=高阻(断开)   触发=拉低      <- 完全模拟手动短接\n"
               "  low    空闲=推高 3.3V    触发=拉低      <- 电平型，低有效\n"
               "  high   空闲=拉低         触发=推高 3.3V <- 电平型，高有效")
    ap.add_argument("--gpio", type=int, help="GPIO 全局编号（测试期用 46，注意补光灯冲突）")
    ap.add_argument("--mode", choices=list(MODES.keys()), help="触发方式")
    ap.add_argument("--sweep", action="store_true",
                    help="依次跑 short → low → high，人工观察哪种有效（推荐）")
    ap.add_argument("--count", type=int, default=3, help="每种方式的触发次数（默认 3）")
    ap.add_argument("--interval", type=int, default=3000, help="两次触发间隔 ms（默认 3000）")
    ap.add_argument("--hold", type=int, default=200, help="触发态保持时长 ms（默认 200）")
    ap.add_argument("--status", action="store_true", help="只看当前引脚状态后退出")
    ap.add_argument("--release", action="store_true", help="把引脚恢复为高阻后退出")
    ap.add_argument("--list", action="store_true", help="列出已导出的 GPIO 后退出")
    ap.add_argument("--dry-run", action="store_true", help="只打印计划，不操作硬件")
    ap.add_argument("--wave", action="store_true",
                    help="连续方波模式：判断摄像头是否处于『外触发出流』模式。"
                         "⚠️ 必须配低频（--hz 1 或 2），用 30Hz 区分不出来")
    ap.add_argument("--hz", type=float, default=2.0, help="方波频率 Hz（默认 2，测出图请用 1~2）")
    ap.add_argument("--seconds", type=float, default=10.0, help="方波持续秒数（默认 10）")
    ap.add_argument("--wave-idle", type=int, choices=[0, 1], default=None,
                    help="方波结束后显式输出的空闲电平；不指定则释放为高阻")
    ap.add_argument("--probe-high", action="store_true",
                    help="扫描『最小高电平保持时间』：决定触发最高能跑多快（决定 30fps 同步是否可行）")
    ap.add_argument("--probe-times", default=DEFAULT_HIGH_TIMES,
                    help=f"扫描档位（ms，逗号分隔），默认 {DEFAULT_HIGH_TIMES}")
    ap.add_argument("--probe-pulses", type=int, default=3, help="每档触发次数（默认 3）")
    ap.add_argument("--probe-low", type=int, default=100, help="扫描时低电平保持 ms（默认 100）")
    ap.add_argument("--probe-gap", type=float, default=3.0, help="档间停顿秒数（默认 3）")
    args = ap.parse_args()

    if args.list:
        list_exported()
        return
    if args.gpio is None:
        ap.error("需要 --gpio（或用 --list 查看现有引脚）")

    gpio = args.gpio
    print(f"目标引脚：gpio{gpio}")
    check_conflict(gpio)

    if args.status:
        show_status(gpio)
        return

    if args.dry_run:
        if args.wave:
            print(f"[dry-run] wave: {args.hz} Hz 方波持续 {args.seconds}s"
                  f"（半周期 {1000/args.hz/2:.1f}ms，每周期 1 个下降沿 = 1 次触发）")
            return
        if args.probe_high:
            print(f"[dry-run] probe-high: 依次用高电平保持 {args.probe_times} ms，"
                  f"每档触发 {args.probe_pulses} 次（低 {args.probe_low}ms，档间停 {args.probe_gap}s）")
            return
        modes = SWEEP_ORDER if args.sweep else [args.mode or "short"]
        for m in modes:
            _, i_desc, t_desc = MODES[m]
            print(f"[dry-run] {m}: {i_desc} / {t_desc} x{args.count} 次"
                  f"（保持 {args.hold}ms，间隔 {args.interval}ms）")
        return

    if not os.path.isdir(GPIO_ROOT):
        print(f"[x] {GPIO_ROOT} 不存在 —— 内核未启用 GPIO sysfs 接口")
        print("    改用 libgpiod：gpioset <chip> <line>=0 / gpioset <chip> <line>=1")
        return

    if not ensure_export(gpio):
        return

    _state["gpio"] = gpio
    signal.signal(signal.SIGINT, cleanup)
    signal.signal(signal.SIGTERM, cleanup)

    if args.release:
        release(gpio)
        print(f"[OK] gpio{gpio} 已恢复高阻（断开），不会有持续触发")
        return

    if args.wave:
        run_wave(gpio, args.hz, args.seconds, args.wave_idle)
        return

    if args.probe_high:
        try:
            times = [int(x) for x in str(args.probe_times).split(",") if x.strip()]
        except ValueError:
            print("[x] --probe-times 格式应为逗号分隔的整数，如 1000,100,10,1")
            return
        run_probe_high(gpio, times, args.probe_pulses, args.probe_low, args.probe_gap)
        return

    if args.sweep:
        print(f"\n>>> 扫测模式：依次尝试 {len(SWEEP_ORDER)} 种方式，每种 {args.count} 次")
        print(">>> 请让摄像头处于可观察状态（画面/指示灯），记下哪种方式有效")
        for m in SWEEP_ORDER:
            run_mode(gpio, m, args.count, args.interval, args.hold)
        print("\n===== 扫测结束 =====")
        print("结果判读：")
        print("  · 只有 short 有效      -> 干接点/光耦输入型，只能模拟短接（建议加 MOS 管）")
        print("  · short 和 low 都有效  -> 电平型且低有效；空闲态用高阻更安全，不必推高")
        print("  · 只有 high 有效       -> 电平型且高有效，注意空闲时这个脚是拉低的")
        print("  · 三种都无效           -> 查共地 / 触发模式未切换 / 电流不足 / 脚未引到端子")
    else:
        run_mode(gpio, args.mode or "short", args.count, args.interval, args.hold)

    release(gpio)
    print(f"[OK] gpio{gpio} 已恢复高阻（断开）状态")


if __name__ == "__main__":
    main()
