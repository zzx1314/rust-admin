"""航点 / 航段语音播报自检（不需要板子、聆思硬件、网络）

改 `speak.py` 的内容校验 / 幂等 / 循环看门狗 / 收尾逻辑后必跑：

    python3 business/backpack_dashboard/backend/tools/speak_selftest.py

怎么在没有聆思小板 / TTS 的机器上验：把 `telemetry.send_lingsi_cmd`（真正写 FIFO 的
那一步）和 `telemetry.ensure_asr`（`systemctl start asr`）换成记录器，其余代码
（白名单净化、幂等、循环看门狗、单次播报收尾、stop 幂等）全部走真实实现。
"""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import config  # noqa: E402
import speak  # noqa: E402
import telemetry  # noqa: E402

CMDS = []
ASR = {"ok": True}


def fake_send(cmd):
    CMDS.append(cmd)
    return {"ok": True, "cmd": cmd, "mode": "call"}


def fake_ensure_asr():
    return ASR["ok"]


def check(name, condition, detail=""):
    if condition:
        print("  ok   %s" % name)
        return True
    print("  FAIL %s %s" % (name, detail))
    return False


def wait_idle(timeout=5.0):
    """等定时器把循环播报收掉（看门狗 / 单次播报收尾）。"""
    deadline = time.time() + timeout
    while time.time() < deadline:
        if not speak.status()["active"]:
            return True
        time.sleep(0.02)
    return False


def main():
    telemetry.send_lingsi_cmd = fake_send
    telemetry.ensure_asr = fake_ensure_asr
    # 看门狗压到毫秒级，单测不用等 10 分钟
    config.SPEAK_MAX_SECS = 1

    failures = 0

    # 1) 内容净化：空格去掉、非法字符丢弃、超长截断
    print("[1] 内容净化")
    text, error = speak.normalize("  请  注 意 避 让  ")
    failures += not check("去掉所有空白", text == "请注意避让", text)
    text, error = speak.normalize("巡检中，请避让 ABC 123")
    failures += not check(
        "非法字符丢弃、中文标点与数字保留", text == "巡检中，请避让123", text
    )
    text, error = speak.normalize("啊" * 50)
    failures += not check(
        "超长截断到 SPEAK_MAX_CHARS",
        text == "啊" * config.SPEAK_MAX_CHARS,
        "%d 字" % len(text or ""),
    )
    text, error = speak.normalize("   ")
    failures += not check("空内容报错", text is None and bool(error), error)
    text, error = speak.normalize("ABC")
    failures += not check("全是不支持字符时报错", text is None and bool(error), error)

    # 2) 单次播报：call 0 → 说完主动 call stop 回到 voice
    print("[2] 单次播报")
    CMDS.clear()
    r = speak.start({"text": "设备巡检，请勿靠近", "planId": "TK-1", "pointIndex": 2})
    failures += not check("返回 ok", r.get("ok") is True, r)
    failures += not check("写的是 call 0", CMDS == ["call 0 设备巡检，请勿靠近"], CMDS)
    failures += not check("状态标为播报中", speak.status()["active"] is True)
    failures += not check("单次播报不等客户端 stop 也会收尾", wait_idle(12.0))
    failures += not check("收尾写了 call stop", CMDS[-1] == "call stop", CMDS)

    # 3) 幂等：同一计划 / 航点 / 文本重复请求不再写 FIFO
    print("[3] 幂等")
    CMDS.clear()
    speak.start({"text": "请避让", "planId": "TK-1", "pointIndex": 3})
    before = len(CMDS)
    r = speak.start({"text": "请避让", "planId": "TK-1", "pointIndex": 3})
    failures += not check("重复请求返回 changed=false", r.get("changed") is False, r)
    failures += not check("重复请求不写 FIFO", len(CMDS) == before, CMDS)
    speak.stop("测试收尾")

    # 4) 循环播报：call 1，看门狗到点自动 stop
    print("[4] 循环播报 + 看门狗")
    CMDS.clear()
    r = speak.start(
        {
            "text": "巡检中，请注意避让",
            "loop": True,
            "seconds": config.SPEAK_MAX_SECS,
            "planId": "TK-2",
            "pointIndex": 0,
            "scope": "leg",
        }
    )
    failures += not check("返回 ok", r.get("ok") is True, r)
    failures += not check("写的是 call 1", CMDS == ["call 1 巡检中，请注意避让"], CMDS)
    failures += not check("状态标为循环", speak.status()["loop"] is True)
    failures += not check("看门狗到点自动停", wait_idle(3.0))
    failures += not check("看门狗写了 call stop", CMDS[-1] == "call stop", CMDS)

    # 5) 换下一条：先 stop 旧的再 call 新的（FIFO 单通道，不 stop 回不到 voice）
    print("[5] 换播报先收旧的")
    CMDS.clear()
    speak.start({"text": "第一条", "planId": "TK-3", "pointIndex": 0})
    speak.start({"text": "第二条", "planId": "TK-3", "pointIndex": 1})
    failures += not check(
        "依次是 call 0 第一条 / call stop / call 0 第二条",
        CMDS[:3] == ["call 0 第一条", "call stop", "call 0 第二条"],
        CMDS,
    )
    speak.stop("测试收尾")

    # 6) stop 幂等：没有播报时不写 FIFO
    print("[6] stop 幂等")
    CMDS.clear()
    r = speak.stop("任务终态")
    failures += not check("没有播报时 changed=false", r.get("changed") is False, r)
    failures += not check("没有播报时不写 FIFO", CMDS == [], CMDS)

    # 7) asr 起不来：只报错、不写 FIFO（TTS 在云端，没网就不喊）
    print("[7] asr 起不来时跳过")
    CMDS.clear()
    ASR["ok"] = False
    r = speak.start({"text": "没网不播", "planId": "TK-4", "pointIndex": 0})
    failures += not check("返回 ok=false", r.get("ok") is False, r)
    failures += not check("不写 FIFO", CMDS == [], CMDS)
    failures += not check("状态不是播报中", speak.status()["active"] is False)
    ASR["ok"] = True

    # 8) 手动「呼叫」：同一句话再喊一遍必须真的再写一条 FIFO
    print("[8] 手动呼叫不去重")
    CMDS.clear()
    speak.start({"text": "请避让", "manual": True, "planId": "TK-5"})
    speak.start({"text": "请避让", "manual": True, "planId": "TK-5"})
    failures += not check(
        "两次同文本各一条 call（中间夹一次 call stop）",
        CMDS[:3] == ["call 0 请避让", "call stop", "call 0 请避让"],
        CMDS,
    )
    failures += not check("手动呼叫 scope 记为 call", speak.status()["scope"] == "call")
    speak.stop("测试收尾")

    print()
    if failures:
        print("失败 %d 项" % failures)
        return 1
    print("全部通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())
