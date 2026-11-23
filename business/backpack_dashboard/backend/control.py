"""控制类操作：LED 灯带 / 补光灯（手动按钮 + 航点灯光）

两套入口共用同一份状态，**别各写一份**：

1. 手动（Dashboard「风扇 / LED / 补光」面板）：`POST /api/filllight`、
   `POST /api/led/blink`；
2. 计划（客户端按路线动作调）：`POST /api/lights`（开）、`POST /api/lights/off`（关）。
   动作来自航点 `lightFill` / `lightLed`，计划级 `lightNightOnly` 决定「只在夜间开」。

手动灯与计划灯的唯一区别是**看门狗**：计划开的灯超过 `LIGHT_MAX_ON_SECS` 自动关
（客户端掉线 / 任务被强杀 / 板子重启后不会一直亮着），手动开的灯不管——现场按了
「补光 开」就是要求一直亮。

LED 灯带只有厂商的 `test_rgbrun`（循环闪烁，没有常亮接口），所以「LED 开」= 拉起
这个进程、「LED 关」= 杀掉它。进程句柄由本模块持有（`_LED_PROC`），手动按钮与计划
共用，否则计划结束时杀不掉手动拉起来的那一个。
"""
import datetime as dt
import os
import signal
import subprocess
import threading
import time

import config
from config import CAM_TRIGGER_GPIO, FILL_LIGHT_GPIO, LED_TEST_BIN

_LOCK = threading.RLock()
_LED_PROC = None
_WATCHDOG = None

# 灯状态：`managed` = 这盏灯是计划点亮的（看门狗管），`on_since` = 最近一次点亮时刻
# （单调时钟，不受改系统时间影响）。
_STATE = {
    "fill": False,
    "led": False,
    "managed": False,
    "night_only": False,
    "on_since": None,
    "source": "",
    # 计划里自定义的夜间时段（最近一次 `POST /api/lights` 带下来的）；None = 用板端默认
    "plan_window": None,
}


def _sh(cmd: str):
    # 只用来写 /sys/class/gpio：正常是毫秒级，超时给 3s 足够，别让一次卡死把
    # 客户端的开灯请求拖过它的 HTTP 超时（那会变成「客户端以为失败、灯其实亮了」）。
    try:
        subprocess.run(cmd, shell=True, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=3)
        return True
    except Exception:
        return False


def _log(message: str):
    print("[lights] %s" % message, flush=True)


# ======================================================================
# 夜间时间窗（板端本地时间，允许跨零点）
# ======================================================================
def _hhmm_minutes(text):
    """`"18:00"` → 1080；非法值返回 None（当成「没配夜间窗」）。"""
    try:
        hh, mm = str(text).strip().split(":")
        hh, mm = int(hh), int(mm)
        if 0 <= hh <= 24 and 0 <= mm <= 59:
            return (hh % 24) * 60 + mm
    except Exception:
        pass
    return None


def night_window(start=None, end=None):
    """夜间时间窗。

    不带参数时读 `config`（`LIGHT_NIGHT_START`/`LIGHT_NIGHT_END`，板端默认，
    改的是所有计划的兜底值）；带 `start`/`end` 时用计划里自定义的时段
    （客户端随 `POST /api/lights` 的 `nightStart`/`nightEnd` 传下来）。
    读 `config` 而不是模块常量：自检脚本会临时改配置。
    """
    if start is None and end is None:
        start = getattr(config, "LIGHT_NIGHT_START", "18:00")
        end = getattr(config, "LIGHT_NIGHT_END", "06:00")
    start_minutes = _hhmm_minutes(start)
    end_minutes = _hhmm_minutes(end)
    # 起点 == 终点视为「没有夜间窗」：把「仅夜间」退化成「到点就开」，
    # 现场临时要白天也开灯时的开关，不用改代码。
    enabled = start_minutes is not None and end_minutes is not None and start_minutes != end_minutes
    return {
        "start": start or "",
        "end": end or "",
        "enabled": enabled,
    }


def is_night(now=None, start=None, end=None):
    """当前是否落在夜间时间窗内。

    `start`/`end` 不传 = 板端配置的默认窗（`LIGHT_NIGHT_START` ~ `LIGHT_NIGHT_END`，
    18:00-06:00）；传了 = 计划里自定义的时段（客户端从计划带下来）。
    **判定永远在板端**：平台/客户端只把「几点到几点算夜间」交给这里，不自己算。

    窗口可以跨零点（默认 18:00-06:00）。未配置 / 非法 / 起止相同 → 一律 False
    （等于「白天」），配合 `night_only` 时就是「不自动开灯」。
    """
    if start is not None or end is not None:
        start = _hhmm_minutes(start)
        end = _hhmm_minutes(end)
        if start is None or end is None:
            # 自定义时段不完整/格式不对：退回板端默认，别把「仅夜间」变成「永远不开」
            _log("计划夜间时段不合法（%r / %r），退回板端默认 %s-%s" % (
                start, end, getattr(config, "LIGHT_NIGHT_START", ""),
                getattr(config, "LIGHT_NIGHT_END", "")))
            start = _hhmm_minutes(getattr(config, "LIGHT_NIGHT_START", "18:00"))
            end = _hhmm_minutes(getattr(config, "LIGHT_NIGHT_END", "06:00"))
    else:
        start = _hhmm_minutes(getattr(config, "LIGHT_NIGHT_START", "18:00"))
        end = _hhmm_minutes(getattr(config, "LIGHT_NIGHT_END", "06:00"))
    if start is None or end is None or start == end:
        return False
    current = now or dt.datetime.now()
    minutes = current.hour * 60 + current.minute
    if start < end:
        return start <= minutes < end
    return minutes >= start or minutes < end


# ======================================================================
# LED 灯带
# ======================================================================
def _mark(field, value, managed=None, source=None):
    """更新灯状态。`managed` / `source` 不传表示「保持原值」。"""
    with _LOCK:
        _STATE[field] = bool(value)
        if managed is not None:
            _STATE["managed"] = bool(managed)
        if source is not None:
            _STATE["source"] = source
        on = _STATE["fill"] or _STATE["led"]
        _STATE["on_since"] = time.monotonic() if on else None


def led_on():
    """点亮 LED 灯带（拉起厂商程序；它自己循环换色，没有常亮接口）。"""
    global _LED_PROC
    if not os.path.exists(LED_TEST_BIN):
        return {"ok": False, "action": "led",
                "error": "未找到 %s，请确认 LED 灯带程序已部署" % LED_TEST_BIN}
    with _LOCK:
        if _LED_PROC is not None and _LED_PROC.poll() is None:
            _mark("led", True)
            return {"ok": True, "action": "led", "state": "on",
                    "detail": "LED 灯带已在运行"}
        try:
            # 单独开一个 session：厂商程序可能还要 fork，关灯时按进程组一起收，
            # 免得只杀掉父进程、灯还在闪。
            _LED_PROC = subprocess.Popen([LED_TEST_BIN],
                                         stdout=subprocess.DEVNULL,
                                         stderr=subprocess.DEVNULL,
                                         start_new_session=True)
        except Exception as e:
            _LED_PROC = None
            return {"ok": False, "action": "led", "error": "LED 灯带启动失败: %s" % e}
        _mark("led", True)
    return {"ok": True, "action": "led", "state": "on",
            "detail": "已点亮 LED 灯带（厂商程序为循环闪烁）"}


def led_off():
    """熄灭 LED 灯带（杀掉厂商程序；手动 / 计划共用同一个进程句柄）。"""
    global _LED_PROC
    with _LOCK:
        proc, _LED_PROC = _LED_PROC, None
        _mark("led", False)
    if proc is None:
        return {"ok": True, "action": "led", "state": "off"}
    if proc.poll() is None:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        except Exception:
            try:
                proc.terminate()
            except Exception:
                pass
        try:
            proc.wait(timeout=3)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass
    return {"ok": True, "action": "led", "state": "off"}


def led_blink():
    """Dashboard「LED 闪烁」按钮：与 `led_on` 同一个进程，返回旧版文案。"""
    result = led_on()
    if not result.get("ok"):
        return {"ok": False, "action": "blink", "error": result.get("error")}
    return {"ok": True, "action": "blink",
            "detail": "已触发 LED 灯带闪烁（白/红/绿/蓝 循环）"}


# ======================================================================
# 补光灯
# ======================================================================
def fill_light(state: bool, managed=None, source=None):
    """补光灯开关：GPIO 拉高=亮，拉低=灭（沿用 test_fill_light）。

    两种情况会拒绝执行，避免和摄像头 2P 端子触发抢同一个脚：
      1. FILL_LIGHT_GPIO = -1 → 补光灯已在配置中禁用（现场测试摄像头触发时用这个）
      2. CAM_TRIGGER_GPIO 与 FILL_LIGHT_GPIO 同号 → 一定会互相干扰，直接拒绝

    为什么同号一定出问题：补光灯会反复改写该脚的方向与电平（开灯推高、关灯持续拉低），
    与摄像头触发逻辑互相打架 —— 可能造成误触发甚至持续触发，测试结果完全不可信。
    """
    gpio = FILL_LIGHT_GPIO
    if gpio < 0:
        return {"ok": False, "action": "filllight",
                "error": "补光灯已禁用（config: FILL_LIGHT_GPIO=-1），如需启用请改回 GPIO 编号"}
    if CAM_TRIGGER_GPIO >= 0 and CAM_TRIGGER_GPIO == gpio:
        return {"ok": False, "action": "filllight",
                "error": "GPIO%d 已被指定为摄像头触发脚（CAM_TRIGGER_GPIO），"
                         "补光灯不可用；请为两者分配不同的 GPIO" % gpio}
    _sh("echo %d > /sys/class/gpio/export 2>/dev/null" % gpio)
    _sh("echo out > /sys/class/gpio/gpio%d/direction" % gpio)
    _sh("echo %d > /sys/class/gpio/gpio%d/value" % (1 if state else 0, gpio))
    _mark("fill", bool(state), managed=managed, source=source)
    return {"ok": True, "state": "on" if state else "off", "gpio": gpio}


# ======================================================================
# 计划灯光（航点动作 / 计划夜间开关）—— 客户端调这两个
# ======================================================================
def _begin_watchdog():
    global _WATCHDOG
    with _LOCK:
        if _WATCHDOG is not None and _WATCHDOG.is_alive():
            return
        _WATCHDOG = threading.Thread(target=_watchdog_loop, name="light-watchdog",
                                     daemon=True)
        _WATCHDOG.start()


def _watchdog_loop():
    while True:
        time.sleep(5)
        try:
            _watchdog_tick()
        except Exception as e:  # 看门狗自己不能把 Agent 拖垮
            _log("看门狗异常: %s" % e)


def _watchdog_tick(now=None):
    """计划灯超时兜底（自检脚本直接调它，不用真等 15 分钟）。"""
    limit = max(0, int(getattr(config, "LIGHT_MAX_ON_SECS", 900)))
    now = time.monotonic() if now is None else now
    with _LOCK:
        since = _STATE["on_since"]
        managed = _STATE["managed"]
    if not managed or since is None or now - since < limit:
        return None
    _log("计划灯已亮 %ds（上限 %ds），自动关灯" % (int(now - since), limit))
    return lights_off("超时自动关灯")


def lights_status():
    """当前灯光状态（`GET /api/lights`；Dashboard 也用它显示计划灯）。

    `night_window` / `night` 反映**当前生效**的窗口：计划带了自己的时段就用它，
    否则用板端默认；`plan_night_window` 单独把计划时段露出来（排查用）。
    """
    with _LOCK:
        plan_window = _STATE.get("plan_window")
    plan_start = (plan_window or {}).get("start")
    plan_end = (plan_window or {}).get("end")
    window = night_window(plan_start, plan_end)
    with _LOCK:
        proc = _LED_PROC
        led_running = proc is not None and proc.poll() is None
        since = _STATE["on_since"]
        status = {
            "fill": _STATE["fill"],
            "led": led_running,
            "managed": _STATE["managed"],
            "night_only": _STATE["night_only"],
            "source": _STATE["source"],
            "on_secs": round(time.monotonic() - since, 1) if since else None,
            "max_on_secs": max(0, int(getattr(config, "LIGHT_MAX_ON_SECS", 900))),
            "night": is_night(None, plan_start, plan_end),
            "night_window": window,
            "plan_night_window": plan_window,
            "fill_gpio": FILL_LIGHT_GPIO,
            "led_bin": LED_TEST_BIN,
            "led_bin_exists": os.path.exists(LED_TEST_BIN),
            "fill_available": FILL_LIGHT_GPIO >= 0
                              and not (CAM_TRIGGER_GPIO >= 0 and CAM_TRIGGER_GPIO == FILL_LIGHT_GPIO),
        }
    return status


def lights_apply(fill=None, led=None, night_only=False, source="plan", now=None,
                 night_start=None, night_end=None):
    """按航点动作开灯。`fill` / `led` 为 None 表示「这次不动这盏灯」。

    `night_only=True`（计划勾了「仅夜间自动开灯」）且当前不在夜间窗内 → **什么都不做**
    并返回 `skipped="daytime"`。注意是「跳过」而不是「关灯」：白天把上一轮手动开的灯
    顺手关掉会很突然，而且这个航点本来就没要求开灯。

    `night_start` / `night_end`（计划里自定义的夜间时段）非空时用它判夜间，
    否则用板端默认（`LIGHT_NIGHT_START`/`LIGHT_NIGHT_END`）。
    """
    with _LOCK:
        _STATE["plan_window"] = (
            {"start": night_start, "end": night_end}
            if (night_start or night_end) else None
        )
    night = is_night(now, night_start, night_end)
    if night_only and not night:
        window = night_window(night_start, night_end)
        _log("跳过开灯（%s）：当前不在夜间时段 %s-%s" % (
            source, window["start"], window["end"]))
        return {"ok": True, "night": night, "skipped": "daytime",
                "detail": "当前不在夜间时段，跳过开灯", "status": lights_status()}

    results = {}
    errors = []
    if fill is not None:
        result = fill_light(bool(fill), managed=True, source=source)
        results["fill"] = result
        if not result.get("ok"):
            errors.append(result.get("error") or "补光灯不可用")
    if led is not None:
        result = led_on() if led else led_off()
        results["led"] = result
        if not result.get("ok"):
            errors.append(result.get("error") or "LED 灯带不可用")

    with _LOCK:
        _STATE["managed"] = bool(_STATE["fill"] or _STATE["led"])
        _STATE["night_only"] = bool(night_only)
        _STATE["source"] = source
        managed = _STATE["managed"]
        if managed:
            # 重新计时：换点再点灯是新一轮，不该继承上一个点的已亮时长
            _STATE["on_since"] = time.monotonic()
        else:
            _STATE["on_since"] = None
    if managed:
        _begin_watchdog()
    _log("开灯 %s → 补光=%s LED=%s%s" % (
        source, _STATE["fill"], _STATE["led"],
        ("；错误：" + "；".join(errors)) if errors else ""))
    return {
        "ok": not errors,
        "night": night,
        "skipped": None,
        "applied": {name: bool(result.get("ok")) for name, result in results.items()},
        "errors": errors,
        "status": lights_status(),
    }


def lights_off(reason="计划结束"):
    """关掉补光灯 + LED（离开航点 / 任务终态 / 取消 / 看门狗）。"""
    results = {
        "fill": fill_light(False),
        "led": led_off(),
    }
    with _LOCK:
        _STATE.update(managed=False, night_only=False, source="off", on_since=None,
                      plan_window=None)
    _log("关灯（%s）" % reason)
    return {
        "ok": True,
        "reason": reason,
        "applied": {name: bool(result.get("ok")) for name, result in results.items()},
        "errors": [result.get("error") for result in results.values()
                   if not result.get("ok") and result.get("error")],
        "status": lights_status(),
    }
