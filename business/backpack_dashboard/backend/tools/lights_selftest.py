#!/usr/bin/env python3
"""计划灯光自检：夜间时间窗 + 补光灯 GPIO + LED 进程 + 看门狗。

覆盖的四个行为（对应 `control.py`）：

1. `is_night` 的边界与跨零点（默认 18:00-06:00，板端本地时间）；
2. 计划勾了「仅夜间自动开灯」时，白天到点**什么都不做**（不写 GPIO、不拉 LED 进程）；
3. 夜间到点开灯 = GPIO 拉高 + `test_rgbrun` 进程存活，`/api/lights/off` 把两者收掉；
4. 看门狗：计划灯超过 `LIGHT_MAX_ON_SECS` 自动关（客户端掉线时兜底）。

为什么单独一个工具：这条逻辑全是「配置 + 进程 + GPIO 文件」，现场排「灯为什么不亮 /
为什么一直亮」时必须能脱离硬件单独跑。这里把 GPIO 写入换成记录桩、把 LED 程序换成
一个 `sleep` 脚本，其余代码路径与板端完全一致。

用法（开发机即可，不需要板子）：

    python3 business/backpack_dashboard/backend/tools/lights_selftest.py
"""
import datetime as dt
import os
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
BACKEND = os.path.dirname(HERE)
sys.path.insert(0, BACKEND)

import config  # noqa: E402
import control  # noqa: E402

failures = []
SH_CALLS = []


def check(label, ok, detail=""):
    print(("  ok   " if ok else "  FAIL ") + label + (("  → " + detail) if detail else ""))
    if not ok:
        failures.append(label)


def install_stubs():
    """GPIO 写入只记账，不碰 /sys；LED 程序换成本目录的 sleep 脚本。"""
    def fake_sh(cmd):
        SH_CALLS.append(cmd)
        return True

    control._sh = fake_sh
    control.FILL_LIGHT_GPIO = 46
    control.CAM_TRIGGER_GPIO = -1

    root = tempfile.mkdtemp(prefix="lights-selftest-")
    led = os.path.join(root, "fake_led.sh")
    with open(led, "w", encoding="utf-8") as f:
        f.write("#!/bin/sh\nsleep 30\n")
    os.chmod(led, 0o755)
    control.LED_TEST_BIN = led
    return root


def at(hh, mm=0):
    return dt.datetime(2026, 7, 1, hh, mm, 0)


def reset():
    control.lights_off("自检重置")
    SH_CALLS.clear()


def main():
    install_stubs()
    config.LIGHT_NIGHT_START = "18:00"
    config.LIGHT_NIGHT_END = "06:00"
    config.LIGHT_MAX_ON_SECS = 900

    print("1) 夜间时间窗（18:00-06:00，跨零点）")
    check("18:00 整算夜间", control.is_night(at(18, 0)))
    check("19:30 算夜间", control.is_night(at(19, 30)))
    check("23:59 算夜间", control.is_night(at(23, 59)))
    check("00:00 算夜间", control.is_night(at(0, 0)))
    check("05:59 算夜间", control.is_night(at(5, 59)))
    check("06:00 不算夜间（右开区间）", not control.is_night(at(6, 0)))
    check("12:00 不算夜间", not control.is_night(at(12, 0)))
    check("17:59 不算夜间", not control.is_night(at(17, 59)))
    window = control.night_window()
    check("时间窗回报 18:00-06:00",
          (window["start"], window["end"], window["enabled"]) == ("18:00", "06:00", True),
          str(window))

    print("2) 不跨零点的窗口（08:00-17:00）")
    config.LIGHT_NIGHT_START = "08:00"
    config.LIGHT_NIGHT_END = "17:00"
    check("08:00 算夜间", control.is_night(at(8, 0)))
    check("16:59 算夜间", control.is_night(at(16, 59)))
    check("07:59 不算", not control.is_night(at(7, 59)))
    check("17:00 不算", not control.is_night(at(17, 0)))
    check("20:00 不算", not control.is_night(at(20, 0)))

    print("3) 起止相同 / 非法配置 = 关闭夜间判定")
    config.LIGHT_NIGHT_START = "18:00"
    config.LIGHT_NIGHT_END = "18:00"
    check("起止相同 → 任何时刻都不算夜间", not control.is_night(at(19, 0)))
    check("起止相同 → enabled=false", control.night_window()["enabled"] is False)
    config.LIGHT_NIGHT_START = "晚上六点"
    config.LIGHT_NIGHT_END = "06:00"
    check("非法配置 → 不算夜间", not control.is_night(at(19, 0)))
    config.LIGHT_NIGHT_START = "18:00"
    config.LIGHT_NIGHT_END = "06:00"

    print("4) 仅夜间 + 白天到达 → 跳过（不写 GPIO、不拉 LED）")
    reset()
    r = control.lights_apply(True, True, True, "计划 P1 航点 0", now=at(12, 0))
    check("返回 skipped=daytime", r.get("skipped") == "daytime", str(r.get("skipped")))
    check("没有写 GPIO", not SH_CALLS, str(SH_CALLS))
    status = control.lights_status()
    check("补光 / LED 都还是关的", not status["fill"] and not status["led"], str(status))
    check("没有进入计划灯管理态", status["managed"] is False)

    print("5) 夜间到点开灯 → GPIO 46 拉高 + LED 进程存活")
    reset()
    r = control.lights_apply(True, True, False, "计划 P1 航点 1", now=at(20, 0))
    check("返回 ok", r.get("ok") is True, str(r.get("errors")))
    check("GPIO46 写 out", any("gpio46/direction" in c for c in SH_CALLS), str(SH_CALLS))
    check("GPIO46 写 1", any(c.strip().endswith("gpio46/value") and c.strip().startswith("echo 1")
                          for c in SH_CALLS), str(SH_CALLS))
    status = control.lights_status()
    check("状态：补光开 + LED 亮", status["fill"] is True and status["led"] is True, str(status))
    check("状态：计划管理态 + 来源", status["managed"] is True and "P1" in status["source"],
          str(status["source"]))
    check("状态：on_secs 有值", status["on_secs"] is not None)

    print("6) 离开航点关灯 → GPIO 46 拉低 + LED 进程被杀")
    reset_marker = len(SH_CALLS)
    r = control.lights_off("已离开航点")
    check("返回 ok", r.get("ok") is True)
    off_calls = SH_CALLS[reset_marker:]
    check("GPIO46 写 0", any(c.strip().startswith("echo 0") and "gpio46/value" in c
                            for c in off_calls), str(off_calls))
    status = control.lights_status()
    check("状态：补光 / LED 都关", status["fill"] is False and status["led"] is False, str(status))
    check("状态：退出计划管理态", status["managed"] is False)

    print("7) 看门狗：计划灯超时自动关")
    reset()
    config.LIGHT_MAX_ON_SECS = 60
    control.lights_apply(True, True, False, "计划 P2 航点 0", now=at(21, 0))
    check("未到时长不关灯", control._watchdog_tick(now=time.monotonic() + 30) is None)
    check("灯还亮着", control.lights_status()["fill"] is True)
    control._watchdog_tick(now=time.monotonic() + 61)
    status = control.lights_status()
    check("超时后自动关灯", status["fill"] is False and status["led"] is False, str(status))
    config.LIGHT_MAX_ON_SECS = 900

    print("8) 补光灯硬件不可用时，LED 仍然照常开")
    reset()
    control.FILL_LIGHT_GPIO = -1
    r = control.lights_apply(True, True, False, "计划 P3 航点 0", now=at(21, 0))
    check("整体 ok=false（有错误）", r.get("ok") is False)
    check("错误里说明补光灯被禁用",
          any("FILL_LIGHT_GPIO=-1" in e for e in r.get("errors", [])), str(r.get("errors")))
    check("LED 仍然点亮", control.lights_status()["led"] is True)
    check("补光灯保持关", control.lights_status()["fill"] is False)
    control.FILL_LIGHT_GPIO = 46
    control.lights_off("自检清理")

    print("9) 补光灯与摄像头触发脚同号 → 直接拒绝")
    reset()
    control.CAM_TRIGGER_GPIO = 46
    r = control.fill_light(True)
    check("拒绝执行", r.get("ok") is False and "CAM_TRIGGER_GPIO" in (r.get("error") or ""),
          str(r))
    check("没有写 GPIO", not SH_CALLS, str(SH_CALLS))
    control.CAM_TRIGGER_GPIO = -1

    print()
    if failures:
        print("FAILED: %d 项（%s）" % (len(failures), "; ".join(failures)))
        return 1
    print("ALL OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
