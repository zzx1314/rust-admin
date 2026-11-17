"""控制类操作：LED 灯带闪烁、补光灯开关（封装 yd_test_v4.0.sh 的逻辑）"""
import subprocess
import os
from config import LED_TEST_BIN, FILL_LIGHT_GPIO, CAM_TRIGGER_GPIO


def _sh(cmd: str):
    try:
        subprocess.run(cmd, shell=True, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=10)
        return True
    except Exception:
        return False


def led_blink():
    """触发 LED 灯带闪烁（复用脚本里的 test_rgbrun 程序）。"""
    if not os.path.exists(LED_TEST_BIN):
        return {"ok": False, "action": "blink",
                "error": f"未找到 {LED_TEST_BIN}，请确认 LED 测试程序已部署"}
    # 后台运行，避免阻塞 Agent
    subprocess.Popen([LED_TEST_BIN], stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL)
    return {"ok": True, "action": "blink",
            "detail": "已触发 LED 灯带闪烁（白/红/绿/蓝 循环）"}


def fill_light(state: bool):
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
                "error": f"GPIO{gpio} 已被指定为摄像头触发脚（CAM_TRIGGER_GPIO），"
                         f"补光灯不可用；请为两者分配不同的 GPIO"}
    _sh(f"echo {gpio} > /sys/class/gpio/export 2>/dev/null")
    _sh(f"echo out > /sys/class/gpio/gpio{gpio}/direction")
    _sh(f"echo {1 if state else 0} > /sys/class/gpio/gpio{gpio}/value")
    return {"ok": True, "state": "on" if state else "off",
            "gpio": gpio}
