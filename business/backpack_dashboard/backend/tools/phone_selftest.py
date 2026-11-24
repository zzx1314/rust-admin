"""模拟电话（phone 模式）自检 —— 不需要板子、声卡、网络

改 `phone.py` 的对端校验 / 起停 / 幂等逻辑后必跑：

    python3 business/backpack_dashboard/backend/tools/phone_selftest.py

怎么在没有聆思小板 / asr / 对端 PC 的机器上验：把 `telemetry.send_lingsi_cmd`
（真正写 FIFO 的那一步）、`telemetry.last_lingsi_cmd` / `last_lingsi_mode`
（读状态文件）、`telemetry.get_interfaces`、`speak.stop` 换成记录器，其余代码
（IPv4 / 端口白名单、幂等、asr 门禁、进电话模式前收播报）全走真实实现。
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import phone  # noqa: E402
import speak  # noqa: E402
import telemetry  # noqa: E402

CMDS = []
STOPPED = []
ASR = {"ok": True}
STATE = {"raw": "voice"}


def fake_send(cmd):
    CMDS.append(cmd)
    # 与 `telemetry._write_lingsi_cmd` 一致：stop 类指令后状态文件记 voice
    STATE["raw"] = "voice" if cmd in ("phone stop", "call stop") else cmd
    return {"ok": True, "cmd": cmd, "mode": mode_of(STATE["raw"])}


def mode_of(raw):
    if not raw:
        return None
    if raw == "voice" or raw.startswith("voice "):
        return "voice"
    if raw.startswith("phone"):
        return "phone"
    if raw == "call" or raw.startswith("call"):
        return "call"
    return None


def fake_last_cmd():
    return STATE["raw"]


def fake_last_mode():
    return mode_of(STATE["raw"])


def fake_asr():
    return ASR["ok"]


def fake_speak_stop(reason="计划结束"):
    STOPPED.append(reason)
    return {"ok": True, "changed": False, "reason": reason}


def check(name, condition, detail=""):
    if condition:
        print("  ok   %s" % name)
        return True
    print("  FAIL %s %s" % (name, detail))
    return False


def main():
    telemetry.send_lingsi_cmd = fake_send
    telemetry.last_lingsi_cmd = fake_last_cmd
    telemetry.last_lingsi_mode = fake_last_mode
    telemetry.ensure_asr = fake_asr
    telemetry.get_interfaces = lambda: [{"name": "eth0", "ip": "192.168.234.200"}]
    speak.stop = fake_speak_stop

    failures = 0

    # 1) 对端校验：IPv4 + 端口白名单（这条命令进的是 shell，必须严格）
    print("[1] 对端地址校验")
    ip, port, error = phone.normalize_peer("192.168.234.101", 5000)
    failures += not check("合法地址通过", ip == "192.168.234.101" and port == 5000 and not error, error)
    bad_cases = [
        ("192.168.1.1; rm -rf /", 5000, "分号注入"),
        ("$(reboot)", 5000, "命令替换"),
        ("1.2.3", 5000, "段数不足"),
        ("256.1.1.1", 5000, "段越界"),
        ("0.0.0.0", 5000, "0.0.0.0"),
        ("192.168.1.1", 0, "端口 0"),
        ("192.168.1.1", 70000, "端口越界"),
        ("192.168.1.1", "5000;ls", "端口注入"),
        ("", 5000, "空地址"),
    ]
    for peer, port_value, label in bad_cases:
        ip_value, port_value2, error_text = phone.normalize_peer(peer, port_value)
        failures += not check("拒绝 %s" % label, ip_value is None and bool(error_text), label)

    # 2) 发起通话：写 `phone <ip> <port>`，状态切到 phone
    print("[2] 发起通话")
    CMDS.clear()
    STATE["raw"] = "voice"
    r = phone.start({"peer": "192.168.234.101", "peerPort": 5000})
    failures += not check("返回 ok", r.get("ok") is True, r)
    failures += not check("写的是 phone <ip> <port>", CMDS == ["phone 192.168.234.101 5000"], CMDS)
    status = r.get("phone") or {}
    failures += not check("状态 active + 对端", status.get("active") is True
                          and status.get("peer") == "192.168.234.101"
                          and status.get("peerPort") == 5000, status)
    failures += not check("带上板端监听端口", status.get("localPort") == phone.LOCAL_PORT, status)

    # 3) 非法对端：不写 FIFO
    print("[3] 非法对端不写 FIFO")
    CMDS.clear()
    r = phone.start({"peer": "192.168.1.1; rm -rf /tmp/x", "peerPort": 5000})
    failures += not check("返回 ok=false", r.get("ok") is False, r)
    failures += not check("没有写 FIFO", CMDS == [], CMDS)

    # 4) 进电话模式前把循环播报收掉（FIFO 单通道）
    print("[4] 先收播报再进电话")
    STOPPED.clear()
    phone.start({"peer": "192.168.234.101", "peerPort": 5000})
    failures += not check("调用了 speak.stop", len(STOPPED) == 1, STOPPED)

    # 5) 挂断：写 phone stop，幂等
    print("[5] 挂断 + 幂等")
    CMDS.clear()
    r = phone.stop("挂断")
    failures += not check("返回 ok + changed", r.get("ok") is True and r.get("changed") is True, r)
    failures += not check("写的是 phone stop", CMDS == ["phone stop"], CMDS)
    failures += not check("状态回到非通话", r["phone"]["active"] is False, r.get("phone"))
    r = phone.stop("再挂一次")
    failures += not check("重复挂断 changed=false", r.get("changed") is False, r)
    failures += not check("重复挂断不写 FIFO", CMDS == ["phone stop"], CMDS)

    # 6) 不在通话中（voice）时挂断：什么都不写
    print("[6] voice 状态下挂断是空操作")
    STATE["raw"] = "voice"
    CMDS.clear()
    r = phone.stop("voice")
    failures += not check("changed=false 且不写 FIFO", r.get("changed") is False and CMDS == [], (r, CMDS))

    # 7) asr 起不来：只报错、不写 FIFO
    print("[7] asr 起不来时跳过")
    STATE["raw"] = "voice"
    CMDS.clear()
    ASR["ok"] = False
    r = phone.start({"peer": "192.168.234.101", "peerPort": 5000})
    failures += not check("返回 ok=false", r.get("ok") is False, r)
    failures += not check("不写 FIFO", CMDS == [], CMDS)
    ASR["ok"] = True

    # 8) 广播：板端网卡列表原样带上（页面据此拼 PC 端命令）
    print("[8] 状态带网卡")
    status = phone.status()
    failures += not check("interfaces 透传", status["interfaces"][0]["ip"] == "192.168.234.200", status)
    failures += not check("脚本下载路径", status["script"] == "/api/lingsi/voice_call_pc.py", status)

    print()
    if failures:
        print("失败 %d 项" % failures)
        return 1
    print("全部通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())
