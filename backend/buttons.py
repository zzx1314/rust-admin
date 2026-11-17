"""SOS / STOP 按钮实时监听（gpio-keys 输入设备，evdev）

脚本 test_buttons 仅提示用 evtest 手动测；这里改为常驻监听，
按键事件通过 publish 回调实时推给前端（SSE）。

使用同步 InputDevice.read_loop() 跑在后台线程，兼容所有 evdev 版本
（旧版无 AsyncInputDevice，故不依赖该 API）。
"""
from evdev import InputDevice, list_devices, ecodes
from config import GPIO_KEYS_NAME, BUTTON_MAP


def find_gpio_keys_path():
    for p in list_devices():
        try:
            d = InputDevice(p)
            if d.name == GPIO_KEYS_NAME:
                return p
        except Exception:
            continue
    return None


def read_buttons(publish):
    """阻塞式监听，应在独立线程中调用。

    publish: 接收 dict 的回调（需线程安全，由 Agent 的 Broadcaster 保证）。
    """
    path = find_gpio_keys_path()
    if not path:
        publish({"type": "button", "error":
                 f"未找到名为 '{GPIO_KEYS_NAME}' 的 evdev 设备，请用 evtest 确认设备名/keycode"})
        return
    try:
        dev = InputDevice(path)
    except Exception as e:
        publish({"type": "button", "error": f"打开 {path} 失败: {e}"})
        return

    for ev in dev.read_loop():
        if ev.type == ecodes.EV_KEY:
            label = BUTTON_MAP.get(ev.code, f"key{ev.code}")
            state = {1: "pressed", 0: "released"}.get(ev.value, "held")
            publish({"type": "button", "label": label,
                     "state": state, "code": ev.code})
