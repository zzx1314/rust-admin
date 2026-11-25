"""控制类操作：LED 灯带 / 补光灯（手动按钮 + 航点灯光）

两套入口共用同一份状态，**别各写一份**：

1. 手动（Dashboard「风扇 / LED / 补光」面板）：`POST /api/filllight`、
   `POST /api/led/blink`；
2. 计划（客户端按路线动作调）：`POST /api/lights`（开）、`POST /api/lights/off`（关）。
   动作来自航点 `lightFill` / `lightLed`，计划级 `lightNightOnly` 决定「只在夜间开」。

手动灯与计划灯的唯一区别是**看门狗**：计划开的灯超过 `LIGHT_MAX_ON_SECS` 自动关
（客户端掉线 / 任务被强杀 / 板子重启后不会一直亮着），手动开的灯不管——现场按了
「补光 开」就是要求一直亮。

LED 灯带没有「常亮」接口，厂商程序都是「跑起来就一直刷颜色」，所以「开」= 让程序跑、
「关」= 让它停。**开关走 systemd unit**（`systemctl start/stop alarmlight`），
不再由本模块 Popen 二进制：

  1. 厂商程序要收到 **SIGINT** 才会在退出前写一帧「全 0 + reset」把灯珠熄掉；
     老逻辑按进程组发 SIGTERM 是默认终止 —— 进程没了，**灯珠停在最后一帧亮着**；
  2. `alarmlight.service` 里 `KillSignal=SIGINT` + `TimeoutStopSec=10`，`systemctl stop`
     才是真正的「熄灯」，而且进程归 systemd 管（谁在跑 / 跑了几份 `systemctl status` 一看就知道）；
  3. Agent 重启不会带走 Popen 出来的子进程（`start_new_session=True`），孤儿会继续抢 SPI；
     unit 归 systemd 管，`Restart=on-failure` 还能自己拉起来。

没有这个 unit 的板子（开发机 / 模拟器 / 老部署）自动退回「直接跑 `LED_TEST_BIN`」，
但关灯改成**先 SIGINT 再退 SIGTERM/SIGKILL**，同样能熄灯。
手动按钮与计划仍共用同一份状态，否则计划结束时收不掉手动拉起来的那一盏。
"""
import datetime as dt
import os
import shutil
import signal
import subprocess
import threading
import time

import config
from config import (CAM_TRIGGER_GPIO, FILL_LIGHT_GPIO, LED_CONTROL, LED_SERVICE,
                    LED_TEST_BIN)

_LOCK = threading.RLock()
_LED_PROC = None
_WATCHDOG = None
# `"systemd"` / `"proc"`：判定一次就缓存（unit 在 Agent 生命周期里不会凭空出现）
_LED_MODE = None
# 老版本 Agent 留下的厂家进程只需清一次
_LEGACY_LED_CLEANED = False

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
# LED 灯带：systemctl start/stop（首选） or 直接跑二进制（兜底）
# ======================================================================
def _systemctl(*args, timeout=15.0):
    """跑一次 `systemctl`，返回 `(ok, 输出)`（stdout + stderr 合并）。

    一律带超时：`systemctl` 卡在 dbus 上时不能把客户端的开灯 / 关灯请求拖死
    （客户端 HTTP 超时短，卡住就变成「客户端以为失败、灯其实亮了」）。
    """
    try:
        proc = subprocess.run(["systemctl"] + list(args), capture_output=True,
                              text=True, timeout=timeout)
    except Exception as e:
        return False, str(e)
    out = ((proc.stdout or "") + (proc.stderr or "")).strip()
    return proc.returncode == 0, out


def led_mode():
    """LED 走哪条路：`"systemd"`（首选）或 `"proc"`（兜底）。

    `LED_CONTROL=systemd|proc` 可强制指定；`auto`（默认）看板子上到底有没有这个
    unit（`systemctl show -p LoadState` = `loaded`）。判定结果缓存：每次开灯都 fork
    一个 systemctl 只为问「你的 unit 在不在」不值得。
    """
    global _LED_MODE
    if _LED_MODE is not None:
        return _LED_MODE
    forced = str(LED_CONTROL or "auto").strip().lower()
    if forced == "proc":
        _LED_MODE = "proc"
        return _LED_MODE
    if forced == "systemd":
        _LED_MODE = "systemd"
        return _LED_MODE
    usable = bool(LED_SERVICE) and os.path.isdir("/run/systemd/system") \
        and shutil.which("systemctl") is not None
    if usable:
        ok, out = _systemctl("show", "-p", "LoadState", "--value", LED_SERVICE,
                             timeout=5)
        usable = ok and out.strip() == "loaded"
    _LED_MODE = "systemd" if usable else "proc"
    if _LED_MODE == "proc":
        _log("没有 systemd unit `%s`，LED 退回直接运行 %s" % (LED_SERVICE, LED_TEST_BIN))
    return _LED_MODE


def _led_service_state():
    """unit 的原始状态串：active / activating / inactive / failed / unknown。"""
    ok, out = _systemctl("is-active", LED_SERVICE, timeout=5)
    lines = [line.strip() for line in out.splitlines() if line.strip()]
    return lines[0] if lines else "unknown"


def _led_service_active():
    """unit 现在是不是活着的（`activating` / `reloading` 也算亮）。"""
    return _led_service_state() in ("active", "activating", "reloading")


def _legacy_led_procs():
    """清掉老版本 Agent 留下的厂商进程（`LED_TEST_BIN`），整个生命周期只清一次。

    老逻辑是 `Popen(..., start_new_session=True)`：Agent 重启带不走它，它就成了孤儿，
    继续按自己的节奏抢 SPI。systemd 模式下留着它 = 两个写者抢同一条灯带，
    「关灯」会被它覆盖回去。只按完整路径匹配，不会误杀别的进程。
    """
    global _LEGACY_LED_CLEANED
    if _LEGACY_LED_CLEANED:
        return
    _LEGACY_LED_CLEANED = True
    path = str(LED_TEST_BIN or "")
    if not os.path.isabs(path):
        # 相对路径 / 名字无法精确匹配，宁可不杀（`pkill -f` 会误伤同 substring 的进程）
        return
    try:
        subprocess.run(["pkill", "-f", path], stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=3)
    except Exception:
        pass


def led_state():
    """LED 现在到底亮没亮 —— 问 systemd / 问进程，不做「我以为」的乐观判断。"""
    if led_mode() == "systemd":
        return _led_service_active()
    with _LOCK:
        proc = _LED_PROC
    return proc is not None and proc.poll() is None


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
    """点亮 LED 灯带（systemd 模式：`systemctl start <unit>`）。"""
    global _LED_PROC
    if led_mode() == "systemd":
        _legacy_led_procs()
        if _led_service_active():
            _mark("led", True)
            return {"ok": True, "action": "led", "state": "on",
                    "detail": "LED 灯带已在运行（%s.service）" % LED_SERVICE}
        ok, out = _systemctl("start", LED_SERVICE)
        if not ok:
            _mark("led", False)
            return {"ok": False, "action": "led",
                    "error": "systemctl start %s 失败：%s" % (LED_SERVICE, out or "未知原因")}
        state = _led_service_state()
        if state not in ("active", "activating", "reloading"):
            _mark("led", False)
            return {"ok": False, "action": "led",
                    "error": "%s.service 已 start 但状态是 %s（查 systemctl status %s / "
                             "journalctl -u %s）" % (LED_SERVICE, state, LED_SERVICE,
                                                     LED_SERVICE)}
        _mark("led", True)
        return {"ok": True, "action": "led", "state": "on",
                "detail": "已点亮 LED 灯带（%s.service）" % LED_SERVICE}

    # ---- 兜底：板子上没有 unit（开发机 / 模拟器 / 老部署）就直接跑二进制 ----
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
            "detail": "已点亮 LED 灯带（直接运行 %s）" % os.path.basename(LED_TEST_BIN)}


def _stop_proc(proc):
    """兜底模式关灯：SIGINT → 等 2s → SIGTERM → 等 1s → SIGKILL（都按进程组）。

    **先 SIGINT** 是这段的关键：厂商程序只在 SIGINT 里写了「全 0 + reset」的收尾，
    直接 SIGTERM 等于默认终止，灯珠会停在最后一帧亮着。
    """
    if proc is None or proc.poll() is not None:
        return
    for sig, wait in ((signal.SIGINT, 2), (signal.SIGTERM, 1)):
        try:
            os.killpg(os.getpgid(proc.pid), sig)
        except Exception:
            try:
                proc.send_signal(sig)
            except Exception:
                pass
        try:
            proc.wait(timeout=wait)
            return
        except Exception:
            continue
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    except Exception:
        try:
            proc.kill()
        except Exception:
            pass


def led_off():
    """熄灭 LED 灯带（systemd 模式：`systemctl stop <unit>`）。

    unit 里 `KillSignal=SIGINT`，厂商程序收到后先写全 0 + reset 再退出 —— 这才是
    真正的「熄灯」，不是把进程干掉、灯珠停在最后一帧。
    """
    global _LED_PROC
    if led_mode() == "systemd":
        _legacy_led_procs()
        # unit 的 TimeoutStopSec=10，留点余量；程序本身 100ms 内就退干净了
        ok, out = _systemctl("stop", LED_SERVICE, timeout=20)
        state = _led_service_state()
        _mark("led", False)
        if state in ("active", "activating", "reloading"):
            return {"ok": False, "action": "led",
                    "error": "systemctl stop %s 没停掉（状态 %s%s）" % (
                        LED_SERVICE, state,
                        "：" + out if out else "; 查 systemctl status " + LED_SERVICE)}
        if not ok:
            # 状态已经是 inactive/failed：目的达到了，只记日志，不把客户端搞成「关灯失败」
            _log("systemctl stop %s 返回非零但已是 %s：%s" % (LED_SERVICE, state, out))
        return {"ok": True, "action": "led", "state": "off"}

    with _LOCK:
        proc, _LED_PROC = _LED_PROC, None
        _mark("led", False)
    _stop_proc(proc)
    return {"ok": True, "action": "led", "state": "off"}


def led_blink():
    """Dashboard「LED 闪烁」按钮：与 `led_on` 同一个进程，返回旧版文案。"""
    result = led_on()
    if not result.get("ok"):
        return {"ok": False, "action": "blink", "error": result.get("error")}
    return {"ok": True, "action": "blink",
            "detail": result.get("detail") or "已触发 LED 灯带闪烁"}


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
    # LED 的真实状态：systemd 模式问 unit，兜底模式看进程（锁外用 systemctl，别把
    # `_mark` / 计划开灯卡在 fork 上）
    mode = led_mode()
    service_state = None
    if mode == "systemd":
        service_state = _led_service_state()
        led_running = service_state in ("active", "activating", "reloading")
    else:
        with _LOCK:
            proc = _LED_PROC
        led_running = proc is not None and proc.poll() is None
    with _LOCK:
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
            # LED 走哪条路：systemd = `systemctl start/stop <led_service>`，proc = 直接跑二进制
            "led_control": mode,
            "led_service": LED_SERVICE if mode == "systemd" else None,
            "led_service_state": service_state,
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

    LED 的开关全部是幂等的（`systemctl start/stop` 打在已在跑的 unit 上是空操作），
    所以客户端 300s 一次的重发 / 看门狗重入都不会出问题。
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
