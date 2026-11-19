#!/usr/bin/env python3
"""`run.py up` 之后的冒烟自测：把「.246 连不上时最想确认的那几条链路」跑一遍。

```text
[1] 模拟 Agent 契约      /api/cameras|relay/status|detect/tasks|detect/status|events
[2] 平台 → 客户端隧道    /api/backpack/status  reachable=true，mode=direct|tunnel|proxy
[3] 视频中继            浏览器 WS 拿到 FLV 分片（触发 relay 起 ffmpeg）
[4] 航点 AI 计划         detect/start(plan_id) → SSE plan_hit + 证据图落盘
[5] AI 启停走隧道        POST /api/backpack/detect/start|stop 真的改了 Agent 运行态
```

为什么值得单独跑：这五条里有三条（隧道、中继按需起流、证据图落盘）**只在有人看/有任务
时才动**，平时 `/api/health` 全绿也不代表它们是通的。

```bash
python3 run.py up            # 终端 1
python3 run.py smoke         # 终端 2（会自动用 runtime/venv 里的 python 跑本文件）
```

也可以单独跑：`SIM_AGENT_PORT=8000 SIM_PLATFORM_PORT=3100 python3 smoke.py`。
退出码 0 = 全通；1 = 有链路没通（每条会打印「为什么」）。
"""

import json
import os
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

AGENT_PORT = int(os.environ.get("SIM_AGENT_PORT", "8000"))
PLATFORM_PORT = int(os.environ.get("SIM_PLATFORM_PORT", "3100"))
STREAM = os.environ.get("SIM_STREAM", "dogcam")
AGENT = "http://127.0.0.1:%d" % AGENT_PORT
PLATFORM = os.environ.get("SIM_PLATFORM_URL", "http://127.0.0.1:%d" % PLATFORM_PORT).rstrip("/")
_platform = urllib.parse.urlparse(PLATFORM)
PLATFORM_HOST = _platform.hostname or "127.0.0.1"
PLATFORM_PORT = _platform.port or (443 if _platform.scheme == "https" else 80)
VIDEO_WS = PLATFORM.replace("https://", "wss://").replace("http://", "ws://")

PASS, FAIL = "\033[32mPASS\033[0m", "\033[31mFAIL\033[0m"
checks = []


def report(name, ok, detail=""):
    checks.append(ok)
    print("  [%s] %s%s" % (PASS if ok else FAIL, name, ("  — " + detail) if detail else ""),
          flush=True)
    return ok


def get_json(url, token=None, timeout=8):
    request = urllib.request.Request(url)
    if token:
        request.add_header("authorization", "Bearer " + token)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read())


def post_json(url, body, token=None, timeout=20):
    request = urllib.request.Request(
        url, method="POST", data=json.dumps(body).encode(),
        headers={"content-type": "application/json"},
    )
    if token:
        request.add_header("authorization", "Bearer " + token)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read())


def login():
    payload = post_json(PLATFORM + "/api/auth/login",
                        {"username": "admin", "password": "admin123"})
    return payload["data"]["token"]


# ------------------------------------------------------------------ [1] Agent


def check_agent():
    print("[1] 模拟 Agent 契约（对应 .246 的 backpack-agent）")
    try:
        cameras = get_json(AGENT + "/api/cameras")
        keys = list((cameras.get("cameras") or {}).keys())
        report("GET /api/cameras", bool(keys), "源：" + ",".join(keys))
    except Exception as error:  # noqa: BLE001
        report("GET /api/cameras", False, str(error))
        return False

    try:
        relay = get_json(AGENT + "/api/relay/status")
        report("GET /api/relay/status", "enabled" in relay,
               "enabled=%s running=%s" % (relay.get("enabled"), relay.get("running")))
    except Exception as error:  # noqa: BLE001
        report("GET /api/relay/status", False, str(error))

    try:
        tasks = get_json(AGENT + "/api/detect/tasks")
        names = [task.get("task") for task in tasks.get("tasks") or []]
        report("GET /api/detect/tasks", bool(names), "模型包：" + ",".join(names))
    except Exception as error:  # noqa: BLE001
        report("GET /api/detect/tasks", False, str(error))

    try:
        status = get_json(AGENT + "/api/detect/status")
        report("GET /api/detect/status", "osd" in status,
               "running=%s osd.ready=%s" % (status.get("running"), status.get("osd", {}).get("ready")))
    except Exception as error:  # noqa: BLE001
        report("GET /api/detect/status", False, str(error))
        return False
    return True


# -------------------------------------------------------- [2] 平台/隧道


def check_platform(token):
    print("[2] 平台聚合 + 设备客户端隧道（平台看不到板子的真实形态）")
    try:
        data = get_json(PLATFORM + "/api/backpack/status", token)["data"]
    except Exception as error:  # noqa: BLE001
        report("GET /api/backpack/status", False, str(error))
        return None
    mode = data.get("mode")
    reachable = bool((data.get("agent") or {}).get("reachable"))
    report("GET /api/backpack/status", reachable,
           "mode=%s agent=%s" % (mode, data.get("agent", {}).get("base")))
    report("设备客户端隧道已登记", bool((data.get("tunnel") or {}).get("available")),
           "device=%s" % (data.get("tunnel") or {}).get("device"))
    names = [task.get("task") for task in (data.get("detect", {}).get("tasks") or {}).get("tasks") or []]
    if mode == "direct":
        # 直连模式下平台只下发地址，模型目录由浏览器自己向 Agent 要
        api_base = (data.get("direct") or {}).get("apiBase")
        try:
            names = [task.get("task") for task in
                     (get_json(api_base.rstrip("/") + "/api/detect/tasks").get("tasks") or [])]
        except Exception as error:  # noqa: BLE001
            report("模型目录（直连）", False, str(error))
            return data
    report("模型目录可用", bool(names), ",".join(names))
    return data


# ---------------------------------------------------------- [3] 视频中继


def check_video(token):
    print("[3] 视频中继（有人看才起 ffmpeg：只有这一条会真正压到板端上行）")
    try:
        import websockets  # noqa: PLC0415 - 只有这里需要
    except ImportError:
        report("导入 websockets", False,
               "用 `python3 run.py smoke` 跑（会用 runtime/venv 里的 python）")
        return
    import asyncio

    async def run():
        url = "%s/api/backpack/video/ws?token=%s&stream=%s" % (VIDEO_WS, token, STREAM)
        frames = 0
        payload = 0
        try:
            async with websockets.connect(url, max_size=None, open_timeout=8) as ws:
                deadline = time.time() + 25
                while time.time() < deadline and frames < 20:
                    try:
                        message = await asyncio.wait_for(ws.recv(), timeout=5)
                    except asyncio.TimeoutError:
                        continue
                    if isinstance(message, (bytes, bytearray)):
                        frames += 1
                        payload += len(message)
        except Exception as error:  # noqa: BLE001
            report("中继 FLV 分片", False, "%s（relay 起来了吗？看 logs/relay.log）" % error)
            return
        if frames:
            report("中继 FLV 分片", payload > 0,
                   "%d 片 / %.1f KB（stream=%s）" % (frames, payload / 1024, STREAM))
        else:
            report("中继 FLV 分片", False,
                   "25s 内没有二进制分片：检查 relay 是否在看板端 rtsp（logs/relay.log）")

    asyncio.run(run())


# ------------------------------------------------------------ [4][5] AI


def check_ai(token, plan_id="TK-SMOKE-01"):
    print("[4] 航点 AI 计划：detect/start(plan_id) → plan_hit + 证据图")
    events = []

    def reader():
        try:
            with urllib.request.urlopen(AGENT + "/api/events", timeout=25) as response:
                for raw in response:
                    line = raw.decode().strip()
                    if line.startswith("data: "):
                        events.append(json.loads(line[6:]))
        except Exception:  # noqa: BLE001 - 流断在退出时很正常
            pass

    thread = threading.Thread(target=reader, daemon=True)
    thread.start()
    time.sleep(0.5)
    try:
        post_json(AGENT + "/api/detect/stop", {})
        post_json(AGENT + "/api/detect/start", {
            "task": "garbage", "labels": ["trash_bin"], "conf": 0.35,
            "cams": [STREAM], "fps": 6,
            "plan_id": plan_id, "point_index": 0,
            "check_type": "count", "expect_count": 3,
        })
    except Exception as error:  # noqa: BLE001
        report("POST /api/detect/start", False, str(error))
        return
    deadline = time.time() + 12
    while time.time() < deadline and not any(e.get("type") == "plan_hit" for e in events):
        time.sleep(0.3)
    post_json(AGENT + "/api/detect/stop", {})
    hits = [e for e in events if e.get("type") == "plan_hit"]
    report("SSE 收到 plan_hit", bool(hits),
           "events=%s" % ",".join(sorted({e.get("type") for e in events})))
    if hits:
        hit = hits[0]
        path = hit.get("evidence") or ""
        exists = bool(path) and os.path.exists(path)
        report("证据图落盘", exists, path or "plan_hit 没带 evidence")
        if exists:
            size = os.path.getsize(path)
            with open(path, "rb") as handle:
                magic = handle.read(2)
            report("证据图是 JPG/PNG 且 ≤2MB",
                   size <= 2 * 1024 * 1024 and magic in (b"\xff\xd8", b"\x89P"),
                   "%.0f KB %s" % (size / 1024, os.path.basename(path)))

    print("[5] AI 启停经平台隧道下发（浏览器/平台不在板端网段时的唯一通道）")
    try:
        post_json(PLATFORM + "/api/backpack/detect/start",
                  {"task": "firextinguisher", "labels": None, "conf": 0.4,
                   "cams": [STREAM], "fps": 6}, token)
        status = get_json(AGENT + "/api/detect/status")
        report("平台 detect/start 到达板端", bool(status.get("running")),
               "running=%s task=%s" % (status.get("running"), status.get("task")))
        post_json(PLATFORM + "/api/backpack/detect/stop", {}, token)
        status = get_json(AGENT + "/api/detect/status")
        report("平台 detect/stop 到达板端", not status.get("running"))
    except Exception as error:  # noqa: BLE001
        report("平台 detect/start|stop", False, str(error))


def main():
    print()
    print("backpack-sim 冒烟自测：agent=%s platform=%s stream=%s" % (AGENT, PLATFORM, STREAM))
    print()
    if not check_agent():
        print()
        print("Agent 都不通，后面的链路没意义：python3 run.py up 起了吗？")
        return 1
    try:
        token = login()
    except Exception as error:  # noqa: BLE001
        print("登录失败（%s）：平台起了吗？" % error)
        return 1
    check_platform(token)
    check_video(token)
    check_ai(token)
    print()
    ok = all(checks)
    print("结果：%s（%d/%d 通过）" % ("全部通过" if ok else "有链路没通",
                                     sum(1 for item in checks if item), len(checks)))
    print()
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
