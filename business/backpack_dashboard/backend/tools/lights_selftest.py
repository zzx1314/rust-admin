#!/usr/bin/env python3
"""计划灯光自检：夜间时间窗 + 补光灯 GPIO + LED 开关 + 看门狗。

覆盖的行为（对应 `control.py`）：

1. `is_night` 的边界与跨零点（默认 18:00-06:00，板端本地时间）；
2. 计划勾了「仅夜间自动开灯」时，白天到点**什么都不做**（不写 GPIO、不碰 LED）；
3. 夜间到点开灯 = GPIO 拉高 + LED 亮，`/api/lights/off` 把两者收掉；
4. 看门狗：计划灯超过 `LIGHT_MAX_ON_SECS` 自动关（客户端掉线时兜底）；
5. **LED 走 systemctl**：开 = `systemctl start <unit>`、关 = `systemctl stop <unit>`，
   状态取 `systemctl is-active`（板端 `alarmlight.service`，`KillSignal=SIGINT` 才能真正熄灯）；
6. 没有 unit 时的兜底：直接跑 `LED_TEST_BIN`，关灯先 SIGINT（厂商程序只在 SIGINT
   里熄灯）再退 SIGTERM/SIGKILL。

为什么单独一个工具：这条逻辑全是「配置 + 进程 + GPIO 文件」，现场排「灯为什么不亮 /
为什么一直亮」时必须能脱离硬件单独跑。这里把 GPIO 写入换成记录桩、把 systemctl 换成
一个内存状态机、把 LED 程序换成一个 `sleep` 脚本，其余代码路径与板端完全一致。

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
        f.write("#!/bin/sh\ntrap 'exit 0' INT TERM\nsleep 30\n")
    os.chmod(led, 0o755)
    control.LED_TEST_BIN = led
    # 前 10 节验的是「板子上没有 unit」的兜底路径（开发机跑自检就是这种情况）
    force_proc_mode()
    return root


def force_proc_mode():
    control.LED_CONTROL = "proc"
    control._LED_MODE = None
    control._LEGACY_LED_CLEANED = True   # 别在开发机上跑 pkill


class FakeSystemd:
    """`systemctl` 的内存替身：只实现 start / stop / is-active / show。"""

    def __init__(self):
        self.calls = []
        self.state = "inactive"
        self.fail_start = False
        self.fail_stop = False

    def systemctl(self, *args, timeout=15.0):
        cmd = list(args)
        self.calls.append(cmd)
        if cmd[0] == "start":
            if self.fail_start:
                return False, "Job for alarmlight.service failed"
            self.state = "active"
            return True, ""
        if cmd[0] == "stop":
            if not self.fail_stop:
                self.state = "inactive"
            return not self.fail_stop, "" if not self.fail_stop else "Job failed"
        if cmd[0] == "is-active":
            return self.state == "active", self.state
        if cmd[0] == "show":
            return True, "loaded"
        return True, ""


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

    print("10) 计划里自定义夜间时段（板端默认 18:00-06:00 不动）")
    config.LIGHT_NIGHT_START = "18:00"
    config.LIGHT_NIGHT_END = "06:00"
    check("自定义 20:00-23:00：20:00 算夜间", control.is_night(at(20, 0), "20:00", "23:00"))
    check("自定义 20:00-23:00：23:00 不算（右开）",
          not control.is_night(at(23, 0), "20:00", "23:00"))
    check("自定义 20:00-23:00：18:30 不算（板端默认算夜间，说明确实是自定义在起作用）",
          control.is_night(at(18, 30)) and not control.is_night(at(18, 30), "20:00", "23:00"))
    check("自定义跨零点 22:00-02:00：01:00 算夜间",
          control.is_night(at(1, 0), "22:00", "02:00"))
    check("自定义时段不合法 → 退回板端默认",
          control.is_night(at(19, 0), "晚上", "23:00"))
    reset()
    r = control.lights_apply(True, True, True, "计划 P9 航点 0", now=at(18, 30),
                             night_start="19:00", night_end="23:00")
    check("18:30 不在计划时段内 → skipped=daytime", r.get("skipped") == "daytime", str(r))
    check("跳过时没有写 GPIO", not SH_CALLS, str(SH_CALLS))
    r = control.lights_apply(True, True, True, "计划 P9 航点 0", now=at(19, 30),
                             night_start="19:00", night_end="23:00")
    check("19:30 在计划时段内 → 真的开灯", r.get("ok") is True and r.get("skipped") is None,
          str(r))
    status = control.lights_status()
    check("状态里的夜间窗 = 计划时段",
          status["night_window"]["start"] == "19:00"
          and status["night_window"]["end"] == "23:00", str(status["night_window"]))
    check("状态里单独露出计划时段",
          status["plan_night_window"] == {"start": "19:00", "end": "23:00"},
          str(status["plan_night_window"]))
    check("状态里的 night 按计划时段（而不是板端默认）算",
          status["night"] is control.is_night(None, "19:00", "23:00"),
          str(status["night"]))
    control.lights_off("自检清理")
    check("关灯后计划时段清空", control.lights_status()["plan_night_window"] is None)

    print("11) 兜底模式：没有 unit 时直接跑二进制，关灯先 SIGINT")
    force_proc_mode()
    reset()
    r = control.led_on()
    check("led_on 拉起进程", r.get("ok") is True and control.led_state() is True, str(r))
    check("状态里 led_control=proc", control.lights_status()["led_control"] == "proc")
    r = control.led_off()
    check("led_off 收掉进程", r.get("ok") is True and control.led_state() is False, str(r))

    print("12) systemd 模式：开 = systemctl start，关 = systemctl stop")
    fake = FakeSystemd()
    control._systemctl = fake.systemctl
    control.LED_CONTROL = "systemd"
    control._LED_MODE = None
    control.LED_SERVICE = "alarmlight"
    reset()
    check("led_mode 判定为 systemd", control.led_mode() == "systemd")
    r = control.led_on()
    check("led_on 成功", r.get("ok") is True, str(r))
    check("真的调了 systemctl start alarmlight", ["start", "alarmlight"] in fake.calls,
          str(fake.calls))
    check("led_state 读 is-active = 亮", control.led_state() is True)
    status = control.lights_status()
    check("状态：led=true / led_control=systemd / led_service=alarmlight",
          status["led"] is True and status["led_control"] == "systemd"
          and status["led_service"] == "alarmlight"
          and status["led_service_state"] == "active", str(status))
    starts = [c for c in fake.calls if c and c[0] == "start"]
    r = control.led_on()
    check("重复开灯幂等（不再 start 一次，返回已在运行）",
          r.get("ok") is True and "已在运行" in (r.get("detail") or "")
          and len([c for c in fake.calls if c and c[0] == "start"]) == len(starts), str(r))
    r = control.lights_apply(True, True, False, "计划 P7 航点 0", now=at(20, 0))
    check("计划开灯也走 systemd", r.get("ok") is True and control.led_state() is True, str(r))
    r = control.lights_off("已离开航点")
    check("关灯成功", r.get("ok") is True, str(r))
    check("真的调了 systemctl stop alarmlight", ["stop", "alarmlight"] in fake.calls,
          str(fake.calls))
    check("关灯后 led_state=false", control.led_state() is False)

    print("13) systemd 模式：stop 没停掉 → 如实报错（不能假报已关）")
    fake.fail_stop = True
    control.led_on()
    r = control.led_off()
    check("led_off 返回失败", r.get("ok") is False, str(r))
    check("错误里带 unit 名", "alarmlight" in (r.get("error") or ""), str(r.get("error")))
    fake.fail_stop = False
    control.led_off()

    print("14) systemd 模式：start 失败 → 报错而不是乐观标亮")
    fake.fail_start = True
    r = control.led_on()
    check("led_on 返回失败", r.get("ok") is False, str(r))
    check("错误里带 systemctl 输出", "failed" in (r.get("error") or ""), str(r.get("error")))
    check("状态里 led=false", control.lights_status()["led"] is False)
    fake.fail_start = False
    force_proc_mode()

    print()
    if failures:
        print("FAILED: %d 项（%s）" % (len(failures), "; ".join(failures)))
        return 1
    print("ALL OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
