#!/usr/bin/env python3
"""本地模拟 `.246` 背包板上的 `backpack-agent`（FastAPI :8000）。

## 它解决什么问题

`192.168.41.246` 那台背包机上有三样东西跑不进笔记本：mediamtx、真 Agent（源码已在仓库
`business/backpack_dashboard/`，但启动要 `fastapi` + `evdev` + `librknnrt.so` + 相机）、
USB 相机 / RKNN。板子连不上时，背包页、航点 AI、企业微信证据图这些链路就没法在本地改/验。

模拟件的用途是「没有 NPU 也能跑通接口链路」；想要**真实现**的接口/SSE（比如在板子上
自测），用 `python3 run.py up --agent real`。

本文件用**纯标准库**复刻 Agent 对平台/客户端暴露的那一份契约
（`business/docs/backpack-video-integration.md` §2.1 / `business/web/src/api/backpack.ts`）：

```text
GET  /api/cameras           六路源（cam0~cam3 + dogcam + infrared）
GET  /api/relay/status      狗前置链路状态
GET  /api/wifi              WiFi 扫描 + 当前连接（SSID / 信号 / IP，客户端报心跳 `signal`）
GET  /api/detect/tasks      模型包目录（消防通道 / 环境卫生）
GET  /api/detect/status     运行态 + osd
POST /api/detect/start      {task,labels,conf,cams,fps,plan_id,point_index,check_type,expect_count}
POST /api/detect/stop
POST /api/detect/config
POST /api/detect/plan/clear
GET  /api/events            SSE：detect / plan_hit / detect_state / detect_error
```

差异（刻意的）：

| 真板子 | 这里 |
| --- | --- |
| RKNN 跑真模型 | 按固定节拍「假装检出」，框坐标是编出来的 |
| 4 路 USB 相机 / 本体 RTSP | 由 `fake_cams.py` 用 ffmpeg `testsrc2` 推进 mediamtx |
| 证据图是命中帧截图 | PIL 画一个带框的色块（没装 PIL 就用 ffmpeg，再不行用内置小图） |
| FastAPI + uvicorn | `http.server`（不需要装任何依赖，`python3` 就能跑） |

## 联调开关（环境变量）

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `SIM_AGENT_PORT` | `8000` | 监听端口 |
| `SIM_JOBS_DIR` | `./data/detect_jobs` | 证据图根目录，**要和设备客户端 `ROBODOG_AI_JOBS_DIR` 指同一个** |
| `SIM_RELAY_RUNNING` | `1` | `/api/relay/status` 是否报「狗前置在推流」 |
| `SIM_PLAN_HIT_DELAY` | `4.5` | 开窗后多久发 `plan_hit`（真板子实测 4.6s：先连采 3 帧再落盘） |
| `SIM_MEDIA_DIR` | `./data/media-data/picture` | 媒体归档目录（真板子 `/media/media-data/picture`），「同步媒体」从这里取图 |
| `SIM_PLAN_SAVE_FRAMES` | `3` | 每个航点写几帧到媒体归档目录（中台 `media_frames` 下发时以中台为准） |
| `SIM_COUNT_RESULT` | `ok` | 数量任务的结果：`ok` / `mismatch`（少 1 个）/ `none`（一个没找到）/ `random` |
| `SIM_EXIST_FOUND` | `1` | 存在性任务是否检出；`0` = 一直不命中（用来验证「未检出」告警） |
| `SIM_LATE_EVIDENCE` | `0` | 设 1 时图**不在 `plan_hit` 里给**，只在 `stop` 时由 `ai_evidence` 补（复现「卡片先发、图晚到」） |
| `SIM_TICK` | `1.2` | 普通（非计划）识别的检出节拍（秒） |
| `SIM_NIGHT` | 空 | 计划灯光的「现在算不算夜间」：`1` / `0` 钉死，空 = 按本地时间窗（`LIGHT_NIGHT_START`/`LIGHT_NIGHT_END`，与板端同规则） |
| `SIM_WIFI_SIGNAL` | `66` | `/api/wifi` 的当前连接信号（0-100）；设备客户端随心跳上报，平台「设备管理」的「信号」列靠它验，`0` = 没连 WiFi |

控制接口（只在本模拟里有，真 Agent 没有）：

```bash
curl -s 127.0.0.1:8000/api/sim/state
curl -s 127.0.0.1:8000/api/sim/relay -d '{"running":false}'
curl -s -XPOST 127.0.0.1:8000/api/sim/emit  -d '{"type":"telemetry","battery":88}'
curl -s 127.0.0.1:8000/api/lights          # 计划灯光（补光 / LED / 是否夜间）
curl -s -XPOST 127.0.0.1:8000/api/lights/off -d '{"reason":"manual"}'
curl -s 127.0.0.1:8000/api/speak/status   # 语音播报（计划到点 / 进段时客户端调的 /api/speak）
curl -s 127.0.0.1:8000/api/photo/status   # 航点拍照（到点抓 JPEG → 客户端回传平台）
```
"""

import json
import os
import queue
import random
import subprocess
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
PORT = int(os.environ.get("SIM_AGENT_PORT", "8000"))
HOST = os.environ.get("SIM_AGENT_HOST", "0.0.0.0")
JOBS_DIR = os.path.abspath(
    os.environ.get("SIM_JOBS_DIR", os.path.join(HERE, "data", "detect_jobs"))
)
# 媒体归档目录（真板子是 /media/media-data/picture）：中台「同步媒体」从这里取图。
# 模拟环境默认落在仓库 data/ 下，客户端用 ROBODOG_MEDIA_DIR 指同一处即可。
MEDIA_DIR = os.path.abspath(
    os.environ.get("SIM_MEDIA_DIR", os.path.join(HERE, "data", "media-data", "picture"))
)
PLAN_SAVE_FRAMES = max(1, min(10, int(os.environ.get("SIM_PLAN_SAVE_FRAMES", "3"))))
# 航点拍照（真板子是 /media/media-data/photo）：客户端上传成功后删，这里落到仓库 data/
PHOTO_DIR = os.path.abspath(
    os.environ.get("SIM_PHOTO_DIR", os.path.join(HERE, "data", "photo"))
)
# 假照片：只要 JPEG 的 SOI 头（平台 ingest 就按这个头判，不需要真图）
FAKE_JPEG = (
    b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00"
    b"\xff\xdb\x00C\x00" + b"\x08" * 64 + b"\xff\xc9\x00\x0b\x08\x00\x01\x00\x01"
    b"\x01\x01\x11\x00\xff\xcc\x00\x06\x00\x10\x10\x05\xff\xda\x00\x08\x01"
    b"\x01\x00\x00?\x00\xd2\xcf\x20\xff\xd9"
)
RELAY_RUNNING = os.environ.get("SIM_RELAY_RUNNING", "1") == "1"
PLAN_HIT_DELAY = float(os.environ.get("SIM_PLAN_HIT_DELAY", "4.5"))
COUNT_RESULT = os.environ.get("SIM_COUNT_RESULT", "ok").strip().lower()
EXIST_FOUND = os.environ.get("SIM_EXIST_FOUND", "1") == "1"
LATE_EVIDENCE = os.environ.get("SIM_LATE_EVIDENCE", "0") == "1"
TICK = float(os.environ.get("SIM_TICK", "1.2"))
# 计划灯光的夜间判定：空 = 按本地时间窗（与板端 `control.is_night` 同规则），
# `1` / `0` 钉死（联调「白天到达跳过开灯」时不用等到晚上）。
SIM_NIGHT = os.environ.get("SIM_NIGHT", "").strip()
LIGHT_NIGHT_START = os.environ.get("LIGHT_NIGHT_START", "18:00")
LIGHT_NIGHT_END = os.environ.get("LIGHT_NIGHT_END", "06:00")
# WiFi 信号（板端 `/api/wifi.current.signal`，0-100）：设备客户端读它跳心跳 `signal`
# 上报，业务平台「设备管理」的「信号」列靠这个验。`0` = 模拟没连 WiFi。
WIFI_SIGNAL = max(0, min(100, int(os.environ.get("SIM_WIFI_SIGNAL", "66"))))
WIFI_SSID = os.environ.get("SIM_WIFI_SSID", "POE-2.4G")


def wifi_quality(signal):
    """与板端 `telemetry._signal_label` 同分档（80 / 60 / 40）。"""
    if signal is None:
        return "未知"
    if signal >= 80:
        return "优"
    if signal >= 60:
        return "良"
    if signal >= 40:
        return "中"
    return "差"

# 与真板子一致的六路源（cam0~cam3 由 Agent 推给 mediamtx，dogcam 是本体前置）
CAMERAS = {
    "cam0": {"title": "左前", "stream": "cam0"},
    "cam1": {"title": "右前", "stream": "cam1"},
    "cam2": {"title": "左后", "stream": "cam2"},
    "cam3": {"title": "右后", "stream": "cam3"},
    "dogcam": {"title": "机器狗前置", "stream": "dogcam"},
}
ALLOWED_CAMS = ["dogcam", "cam0", "cam1", "cam2", "cam3"]

# 两个「模型包」：字段与板端 /api/detect/tasks 对齐（web 的 BackpackTask）
TASKS = {
    "firextinguisher": {
        "task": "firextinguisher",
        "name": "消防通道",
        "desc": "占用消防通道的目标（模拟模型包）",
        "nc": 2,
        "labels": [
            {"id": 0, "name": "fire", "zh": "火焰", "color": "#ff3b30", "ignored": False},
            {"id": 1, "name": "smoke", "zh": "烟雾", "color": "#ff9500", "ignored": False},
        ],
        "conf": 0.35,
        "alarm": ["火焰", "烟雾"],
        "target": "消防通道占用",
        "model_size": 8_388_608,
        "ok": True,
        "issues": [],
    },
    "garbage": {
        "task": "garbage",
        "name": "环境卫生",
        "desc": "垃圾桶 / 散落垃圾（模拟模型包，带数量核对）",
        "nc": 2,
        "labels": [
            {"id": 0, "name": "trash_bin", "zh": "垃圾桶", "color": "#34c759", "ignored": False},
            {"id": 1, "name": "trash", "zh": "垃圾", "color": "#0a84ff", "ignored": False},
        ],
        "conf": 0.35,
        "alarm": ["垃圾桶", "垃圾"],
        "target": "垃圾桶满溢",
        "model_size": 12_582_912,
        "ok": True,
        "issues": [],
    },
}

# 内置兜底图（8x8 灰色 JPEG）：没 PIL 也没 ffmpeg 时保证「有图可发」
FALLBACK_JPEG = bytes.fromhex(
    "ffd8ffe000104a46494600010100000100010000ffdb0043000302020302020303030304030304050805050404050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f171816141812141514ffc0000b080008000801011100ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffda0008010100003f00fbfeffd9"
)


# --------------------------------------------------------------------------- 计划灯光
#
# 真板子在 `backend/control.py`：夜间窗跑本地时间、`nightOnly` 白天的返回
# `skipped="daytime"`。模拟里只维持状态计数，不碰 GPIO / 进程。

def _hhmm_minutes(text):
    try:
        hh, mm = str(text).strip().split(":")
        hh, mm = int(hh), int(mm)
        if 0 <= hh <= 24 and 0 <= mm <= 59:
            return (hh % 24) * 60 + mm
    except Exception:
        pass
    return None


def sim_is_night(now=None):
    """与板端 `control.is_night` 同一规则（18:00-06:00，允许跨零点）。"""
    if SIM_NIGHT in ("1", "true", "on", "yes"):
        return True
    if SIM_NIGHT in ("0", "false", "off", "no"):
        return False
    start = _hhmm_minutes(LIGHT_NIGHT_START)
    end = _hhmm_minutes(LIGHT_NIGHT_END)
    if start is None or end is None or start == end:
        return False
    now = now or time.localtime()
    minutes = now.tm_hour * 60 + now.tm_min
    if start < end:
        return start <= minutes < end
    return minutes >= start or minutes < end


# --------------------------------------------------------------------------- 状态


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.running = False
        self.state = "stopped"
        self.task = ""
        self.labels = []
        self.conf = 0.35
        self.cams = ["dogcam"]
        self.fps = 6
        self.error = ""
        self.session = 0
        self.plan = None  # {plan_id, point_index, check_type, expect_count, started, hit}
        self.stats = {}
        self.frames = 0
        self.hits = 0
        self.subscribers = []  # list[queue.Queue]
        self.relay = {"enabled": True, "running": RELAY_RUNNING, "restarts": 0,
                      "last_error": "", "stream": "dogcam"}
        # 计划灯光（客户端在 5009 到点 / 离开航点时调的 /api/lights）
        self.lights = {"fill": False, "led": False, "managed": False,
                       "night_only": False, "source": "", "applied": 0,
                       "off": 0, "skipped": 0}
        # 语音播报（客户端在 5009 到点 / 进段时调的 /api/speak）
        self.speak = {"active": False, "text": "", "loop": False,
                      "seconds": 0, "plan_id": "", "point_index": -1,
                      "scope": "", "words": 0, "stops": 0}
        # 航点拍照（客户端在 5009 到点那一帧调的 /api/photo）
        self.photos = {}
        self.seen_events = 0

    # ---- 事件 ----
    def publish(self, event):
        event.setdefault("ts", time.time())
        with self.lock:
            self.seen_events += 1
            targets = list(self.subscribers)
        payload = "data: %s\n\n" % json.dumps(event, ensure_ascii=False)
        for q in targets:
            try:
                q.put_nowait(payload)
            except queue.Full:  # 页面卡住就别攒了，与平台 `Lagged` 一个策略
                pass

    def subscribe(self):
        q = queue.Queue(maxsize=256)
        with self.lock:
            self.subscribers.append(q)
        return q

    def unsubscribe(self, q):
        with self.lock:
            if q in self.subscribers:
                self.subscribers.remove(q)

    # ---- 运行态快照 ----
    def labels_of(self):
        task = TASKS.get(self.task) or {}
        names = {lab["name"] for lab in task.get("labels", [])}
        if not self.labels:
            return [lab for lab in task.get("labels", []) if not lab["ignored"]]
        return [lab for lab in task.get("labels", []) if lab["name"] in self.labels
                and lab["name"] in names]

    def status(self):
        with self.lock:
            task = TASKS.get(self.task) or {}
            cam = self.cams[0] if self.cams else "dogcam"
            stats = {c: dict(v) for c, v in self.stats.items()}
            running = self.running
            plan = dict(self.plan) if self.plan else None
            return {
                "running": running,
                "state": self.state,
                "task": self.task,
                "labels": list(self.labels) or None,
                "conf": self.conf,
                "cams": list(self.cams),
                "fps": self.fps,
                "warnings": [],
                "stats": stats,
                "grabbers": {c: {"has_frame": running, "error": ""} for c in self.cams},
                "error": self.error,
                "single_cam": True,
                "allowed_cams": ALLOWED_CAMS,
                "default_cam": "dogcam",
                "preview_stream": "preview",
                "osd": {
                    "enabled": True,
                    "running": running,
                    "ready": running,
                    "session": self.session,
                    "cam": cam if running else None,
                    "stream": "preview",
                    "error": "",
                    "frames": self.frames,
                    "write_fps": float(self.fps),
                    "render_ms": 6.5,
                    "missing_glyphs": [],
                    "log_tail": [
                        "sim: 用 fake_cams.py 与 PIL 生成的画框流（无 RKNN）",
                        "sim: 想看真模型请指向 .246 的 :8000",
                    ],
                },
                "plan": plan,
                "sim": {
                    "count_result": COUNT_RESULT,
                    "exist_found": EXIST_FOUND,
                    "late_evidence": LATE_EVIDENCE,
                    "jobs_dir": JOBS_DIR,
                },
            }

    def lights_status(self):
        """计划灯光快照（GET /api/lights）——字段与板端 `control.lights_status` 对齐。"""
        with self.lock:
            lights = dict(self.lights)
        window = {"start": LIGHT_NIGHT_START, "end": LIGHT_NIGHT_END,
                  "enabled": _hhmm_minutes(LIGHT_NIGHT_START) is not None
                             and _hhmm_minutes(LIGHT_NIGHT_END) is not None
                             and _hhmm_minutes(LIGHT_NIGHT_START)
                             != _hhmm_minutes(LIGHT_NIGHT_END)}
        lights.update({
            "night": sim_is_night(),
            "night_window": window,
            "fill_gpio": 46,
            "led_bin": "sim/no-led",
            "led_bin_exists": True,
            "fill_available": True,
            "max_on_secs": 900,
            "on_secs": None,
            "sim": True,
        })
        return lights

    def photo_status(self):
        """航点拍照快照（GET /api/photo/status）——字段与板端 `photo.status` 对齐。"""
        files = self.photo_list()
        return {
            "dir": PHOTO_DIR,
            "free_mb": 4096,
            "pending": len(files),
            "pending_bytes": sum(item["size"] for item in files),
            "sessions": sorted({item["planId"] for item in files})[:50],
            "allowed_cameras": ["cam0", "cam1", "cam2", "cam3", "dogcam", "dogcam720"],
            "max_frames": 5,
            "quality": "2",
            "last": self.photos.get("__last__") or {"at": None, "files": [], "error": ""},
            "retention_hours": 72,
            "sim": True,
        }

    def photo_list(self, plan_id=None):
        """内存清单（按 name 排序）：返回给客户端的字段与真板端 `photo.list_files` 一致。"""
        with self.lock:
            items = list(self.photos.items())
        files = []
        for name, meta in sorted(items):
            if name.startswith("__"):
                continue
            if plan_id and meta["planId"] != plan_id:
                continue
            path = os.path.join(PHOTO_DIR, *name.split("/"))
            files.append({
                "name": name,
                "path": path,
                "planId": meta["planId"],
                "pointIndex": meta["pointIndex"],
                "pointName": meta.get("pointName") or "",
                "camera": meta["camera"],
                "size": meta["size"],
                "capturedAt": meta["capturedAt"],
            })
        return files

    def speak_status(self):
        """语音播报快照（GET /api/speak/status）——字段与板端 `speak.status` 对齐。"""
        with self.lock:
            speak = dict(self.speak)
        speak["max_chars"] = 30
        speak["max_secs"] = 600
        speak["sim"] = True
        return speak

    def detect_state_event(self):
        snapshot = self.status()
        return {
            "type": "detect_state",
            "running": snapshot["running"],
            "state": snapshot["state"],
            "task": snapshot["task"],
            "labels": snapshot["labels"],
            "conf": snapshot["conf"],
            "cams": snapshot["cams"],
            "fps": snapshot["fps"],
            "error": snapshot["error"],
        }


STATE = State()


# ------------------------------------------------------------------ 证据图


def _box(label):
    """编一个「像那么回事」的框（1280x720 坐标系）。"""
    x1 = random.randint(80, 620)
    y1 = random.randint(80, 420)
    return {
        "x1": x1,
        "y1": y1,
        "x2": min(1279, x1 + random.randint(160, 420)),
        "y2": min(719, y1 + random.randint(120, 260)),
        "label": label["name"],
        "zh": label["zh"],
        "color": label["color"],
    }


def render_jpeg(path, boxes=(), caption=""):
    """画一张带框的 1280x720 JPEG：PIL → ffmpeg → 内置小图。"""
    try:
        from PIL import Image, ImageDraw  # noqa: PLC0415 - 可选依赖

        image = Image.new("RGB", (1280, 720), (24, 28, 36))
        draw = ImageDraw.Draw(image)
        for box in boxes:
            draw.rectangle([box["x1"], box["y1"], box["x2"], box["y2"]],
                           outline=box["color"], width=6)
            draw.rectangle([box["x1"], max(0, box["y1"] - 26), box["x1"] + 220,
                            box["y1"]], fill=box["color"])
            draw.text((box["x1"] + 8, max(0, box["y1"] - 22)),
                      "%s %.2f" % (box["zh"], box.get("conf", 0.8)),
                      fill=(255, 255, 255))
        if caption:
            draw.text((24, 24), caption, fill=(200, 210, 220))
        image.save(path, "JPEG", quality=85)
        return True
    except Exception:  # noqa: BLE001 - 没装 PIL 就走 ffmpeg
        pass
    try:
        subprocess.run(
            ["ffmpeg", "-y", "-loglevel", "error", "-f", "lavfi",
             "-i", "color=c=0x1c2024:s=1280x720", "-frames:v", "1", "-q:v", "4", path],
            check=True,
            timeout=20,
        )
        return True
    except Exception:  # noqa: BLE001 - 再不行就用内置图，保证「有图」
        with open(path, "wb") as handle:
            handle.write(FALLBACK_JPEG)
        return True


def write_evidence(plan, label, found):
    """落盘 `<jobs>/<plan_id>/pt<idx>/*.jpg` + `result.json`（真板子的目录布局）"""
    directory = os.path.join(JOBS_DIR, str(plan["plan_id"]), "pt%d" % plan["point_index"])
    os.makedirs(directory, exist_ok=True)
    stamp = int(time.time() * 1000)
    path = os.path.join(directory, "%d.jpg" % stamp)
    boxes = []
    for index in range(found):
        box = _box(label)
        box["conf"] = round(random.uniform(0.62, 0.93), 2)
        box["x1"] = min(1150, box["x1"] + index * 40)
        box["x2"] = min(1279, box["x2"] + index * 40)
        boxes.append(box)
    caption = "sim %s pt%s found=%s expect=%s" % (
        plan["plan_id"], plan["point_index"], found, plan.get("expect_count"))
    render_jpeg(path, boxes, caption)
    with open(os.path.join(directory, "result.json"), "w", encoding="utf-8") as handle:
        json.dump({
            "plan_id": plan["plan_id"],
            "point_index": plan["point_index"],
            "check_type": plan["check_type"],
            "label": label["name"],
            "found": found > 0,
            "found_count": found,
            "expect_count": plan.get("expect_count"),
            "count_alert": plan["check_type"] == "count"
            and plan.get("expect_count") is not None
            and found != plan["expect_count"],
            "evidence": path,
            "ts": time.time(),
        }, handle, ensure_ascii=False, indent=2)
    media_dir, media_frames = write_media_frames(plan, label)
    return path, boxes, media_dir, media_frames


def write_media_frames(plan, label):
    """再写一份媒体归档：`<MEDIA_DIR>/<plan>/pt<idx>/<cam>_<n>_<ts>.jpg`。

    真板子固定保存 N 帧（中台 `media_frames`），这里照抄布局，好让设备客户端的
    「同步媒体」指令在模拟环境里也能端到端跑通（上传成功会删掉这些文件）。
    ⚠ 媒体归档存的是**原始帧**（中台当训练数据用），**不画框也不加文字**；
    带框的那份只在 `JOBS_DIR`（临时存证 → 企业微信证据图）。
    """
    root = plan.get("media_dir") or MEDIA_DIR
    frames = max(1, min(10, int(plan.get("save_frames") or PLAN_SAVE_FRAMES)))
    directory = os.path.join(os.path.abspath(root), str(plan["plan_id"]),
                             "pt%d" % plan["point_index"])
    os.makedirs(directory, exist_ok=True)
    cam = (STATE.cams[0] if STATE.cams else "dogcam")
    stamp = time.strftime("%Y%m%d_%H%M%S")
    written = []
    for seq in range(1, frames + 1):
        name = "%s_%02d_%s.jpg" % (cam, seq, stamp)
        render_jpeg(os.path.join(directory, name))
        written.append(name)
    with open(os.path.join(directory, "result.json"), "w", encoding="utf-8") as handle:
        json.dump({
            "type": "plan_media",
            "plan_id": plan["plan_id"],
            "point_index": plan["point_index"],
            "label": label["name"],
            "media_dir": directory,
            "media_frames": written,
            "ts": time.time(),
        }, handle, ensure_ascii=False, indent=2)
    return directory, written


# ------------------------------------------------------------------ 识别引擎


def found_count_for(plan):
    expect = plan.get("expect_count")
    if plan["check_type"] != "count":
        return 1 if EXIST_FOUND else 0
    expect = expect or 1
    if COUNT_RESULT == "mismatch":
        return max(0, expect - 1)
    if COUNT_RESULT == "none":
        return 0
    if COUNT_RESULT == "random":
        return random.choice([0, max(1, expect - 1), expect, expect + 1])
    return expect


def emit_plan_hit(plan, with_evidence):
    labels = STATE.labels_of() or TASKS.get(STATE.task, {}).get("labels", [])
    if not labels:
        return
    label = labels[0]
    found = found_count_for(plan)
    expect = plan.get("expect_count")
    event = {
        "type": "plan_hit",
        "plan_id": plan["plan_id"],
        "point_index": plan["point_index"],
        "check_type": plan["check_type"],
        "label": label["name"],
        "zh": label["zh"],
        "found": found > 0,
        "found_count": found,
        "expect_count": expect,
        "count_alert": plan["check_type"] == "count" and expect is not None and found != expect,
        "evidence": "",
    }
    if found > 0 and with_evidence:
        path, boxes, media_dir, media_frames = write_evidence(plan, label, found)
        event["evidence"] = path
        event["boxes"] = boxes
        # 媒体归档（中台「同步媒体」从这里取图）
        event["media_dir"] = media_dir
        event["media_frames"] = media_frames
    plan["hit"] = dict(event)
    STATE.publish(event)


def emit_detect(label=None):
    status = STATE.status()
    if not status["running"]:
        return
    labels = STATE.labels_of()
    if not labels:
        return
    label = label or random.choice(labels)
    cam = status["cams"][0] if status["cams"] else "dogcam"
    count = random.randint(1, 3)
    boxes = []
    for _ in range(count):
        box = _box(label)
        box["conf"] = round(random.uniform(0.4, 0.95), 2)
        box["count"] = count
        box["text"] = "%s %d处" % (label["zh"], count)
        boxes.append(box)
    event = {
        "type": "detect",
        "cam": cam,
        "frame_w": 1280,
        "frame_h": 720,
        "task": STATE.task,
        "infer_ms": random.randint(12, 32),
        "boxes": boxes,
        "counts": {label["name"]: count},
    }
    with STATE.lock:
        STATE.frames += count
        STATE.hits += count
        stat = STATE.stats.setdefault(cam, {"frames": 0, "hits": 0, "fps": 0.0,
                                            "last_ms": 0, "ts": 0})
        stat["frames"] += 1
        stat["hits"] += count
        stat["fps"] = float(STATE.fps)
        stat["last_ms"] = event["infer_ms"]
        stat["ts"] = time.time()
    STATE.publish(event)


def engine_loop(stop):
    while not stop.is_set():
        stop.wait(max(0.2, TICK))
        with STATE.lock:
            running = STATE.running
            plan = STATE.plan
        if not running:
            continue
        if plan:
            elapsed = time.time() - plan["started"]
            if plan["hit"] is None and elapsed >= PLAN_HIT_DELAY and not LATE_EVIDENCE:
                emit_plan_hit(plan, with_evidence=True)
            elif plan["hit"] is None:
                # 窗口刚开：先有检出（板端这时还在采样，图没落盘）
                if found_count_for(plan) > 0:
                    emit_detect()
            continue
        emit_detect()


# ------------------------------------------------------------------ HTTP


def parse_body(handler):
    length = int(handler.headers.get("Content-Length") or 0)
    raw = handler.rfile.read(length) if length else b""
    if not raw:
        return {}
    try:
        return json.loads(raw.decode("utf-8"))
    except ValueError:
        return {}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "backpack-sim-agent/1.0"

    # 静音默认访问日志（SSE 每 10s 一条心跳会把终端刷满）
    def log_message(self, fmt, *args):  # noqa: A003
        pass

    # ---- 工具 ----
    def cors(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type, Accept")

    def json(self, payload, code=200):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.cors()
        self.end_headers()
        self.wfile.write(body)

    # ---- 路由 ----
    def do_OPTIONS(self):  # noqa: N802
        self.send_response(204)
        self.cors()
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self):  # noqa: N802
        path = self.path.split("?", 1)[0].rstrip("/") or "/"
        if path == "/api/cameras":
            return self.json({"cameras": CAMERAS,
                              "infrared": {"title": "红外", "stream": "dog-ipcam"}})
        if path == "/api/relay/status":
            return self.json(dict(STATE.relay))
        if path == "/api/detect/tasks":
            tasks = list(TASKS.values())
            return self.json({
                "models_dir": os.path.join(HERE, "data", "rknn", "models"),
                "count": len(tasks),
                "ready": len(tasks),
                "note": "模拟模型包（deploy/backpack-sim/mock_agent.py），不是真的 RKNN",
                "tasks": tasks,
            })
        if path == "/api/detect/status":
            return self.json(STATE.status())
        if path == "/api/wifi":
            # 板端 `telemetry.get_wifi()` 的契约：扫描列表 + 当前连接。
            # 设备客户端只取 `current.signal`（0-100）随心跳上报。
            connected = WIFI_SIGNAL > 0
            return self.json({
                "iface": "wlan0",
                "total": 2,
                "networks": [
                    {"ssid": WIFI_SSID, "signal": WIFI_SIGNAL or None,
                     "quality": wifi_quality(WIFI_SIGNAL or None), "security": "WPA2"},
                    {"ssid": "邻居家WiFi", "signal": 32, "quality": "差", "security": "WPA2"},
                ],
                "current": {
                    "connected": connected,
                    "ssid": WIFI_SSID if connected else None,
                    "signal": WIFI_SIGNAL or None,
                    "quality": wifi_quality(WIFI_SIGNAL or None),
                    "ip": "192.168.41.246" if connected else None,
                },
            })
        if path == "/api/lights":
            return self.json(STATE.lights_status())
        if path == "/api/speak/status":
            return self.json(STATE.speak_status())
        if path == "/api/photo/status":
            return self.json(STATE.photo_status())
        if path == "/api/photo/list":
            query = self.path.split("?", 1)[1] if "?" in self.path else ""
            plan_id = ""
            for part in query.split("&"):
                if part.startswith("planId="):
                    plan_id = urllib.parse.unquote(part.split("=", 1)[1])
            return self.json({"files": STATE.photo_list(plan_id or None)})
        if path == "/api/events":
            return self.sse()
        if path == "/api/sim/state":
            return self.json({
                "running": STATE.running,
                "plan": STATE.plan,
                "subscribers": len(STATE.subscribers),
                "relay": STATE.relay,
                "lights": STATE.lights,
                "speak": STATE.speak,
                "photo": STATE.photo_status(),
                "events": STATE.seen_events,
            })
        if path == "/":
            return self.json({"ok": True, "hint": "backpack-sim mock agent", "docs": __doc__})
        return self.json({"ok": False, "error": "not found: %s" % path}, 404)

    def do_POST(self):  # noqa: N802
        path = self.path.split("?", 1)[0].rstrip("/") or "/"
        body = parse_body(self)

        if path == "/api/detect/start":
            return self.start(body)
        if path == "/api/detect/stop":
            return self.stop()
        if path == "/api/detect/config":
            with STATE.lock:
                if body.get("task") in TASKS:
                    STATE.task = body["task"]
                if "labels" in body:
                    STATE.labels = [str(x) for x in (body.get("labels") or [])]
                if "conf" in body:
                    STATE.conf = float(body["conf"])
                if body.get("cams"):
                    STATE.cams = [str(x) for x in body["cams"]]
                if "fps" in body:
                    STATE.fps = max(1, min(30, int(body["fps"])))
            STATE.publish(STATE.detect_state_event())
            return self.json({"ok": True, "status": STATE.status()})
        if path == "/api/detect/plan/clear":
            with STATE.lock:
                STATE.plan = None
            return self.json({"ok": True, "status": STATE.status()})

        # 计划灯光（客户端在 5009 到点 / 离开航点时调；与真板端同一份字段）
        if path == "/api/lights":
            night = sim_is_night()
            night_only = bool(body.get("nightOnly"))
            if night_only and not night:
                with STATE.lock:
                    STATE.lights["skipped"] += 1
                return self.json({"ok": True, "night": night, "skipped": "daytime",
                                  "detail": "当前不在夜间时段，跳过开灯",
                                  "status": STATE.lights_status()})
            with STATE.lock:
                for name in ("fill", "led"):
                    if body.get(name) is not None:
                        STATE.lights[name] = bool(body.get(name))
                STATE.lights.update({
                    "managed": STATE.lights["fill"] or STATE.lights["led"],
                    "night_only": night_only,
                    "source": "计划 %s 航点 %s" % (
                        body.get("planId") or body.get("taskCode") or "—",
                        body.get("pointIndex")),
                })
                STATE.lights["applied"] += 1
            return self.json({"ok": True, "night": night, "skipped": None,
                              "applied": {"fill": STATE.lights["fill"],
                                          "led": STATE.lights["led"]},
                              "errors": [], "status": STATE.lights_status()})
        if path == "/api/lights/off":
            reason = str(body.get("reason") or "计划结束")
            with STATE.lock:
                STATE.lights.update({"fill": False, "led": False, "managed": False,
                                     "night_only": False, "source": "off"})
                STATE.lights["off"] += 1
            return self.json({"ok": True, "reason": reason,
                              "status": STATE.lights_status()})

        # 语音播报（客户端在 5009 到点 / 进段时调；字段与真板端同一份）
        if path == "/api/speak":
            text = "".join(str(body.get("text") or "").split())
            if not text:
                return self.json({"ok": False, "error": "播报内容不能为空"}, 400)
            with STATE.lock:
                STATE.speak.update({
                    "active": True,
                    "text": text,
                    "loop": bool(body.get("loop")),
                    "seconds": int(body.get("seconds") or 0),
                    "plan_id": str(body.get("planId") or body.get("taskCode") or ""),
                    # `0 or -1` 会变成 -1：航点下标要单独判 None
                    "point_index": -1
                    if body.get("pointIndex") is None
                    else int(body.get("pointIndex")),
                    "scope": str(body.get("scope") or "point"),
                })
                STATE.speak["words"] += 1
            return self.json({"ok": True, "changed": True, "cmd": text,
                              "speak": STATE.speak_status()})
        # 航点拍照（客户端在 5009 到点那一帧调；字段与真板端同一份）
        if path == "/api/photo":
            plan_id = str(body.get("planId") or body.get("taskCode") or "")
            if not plan_id or "/" in plan_id or ".." in plan_id:
                return self.json({"ok": False, "error": "planId 不合法"}, 400)
            point_index = int(body.get("pointIndex") or 0)
            camera = str(body.get("camera") or "dogcam")
            frames = max(1, min(5, int(body.get("frames") or 1)))
            point_name = str(body.get("pointName") or "")
            stamp = time.strftime("%Y%m%d-%H%M%S", time.localtime())
            folder = os.path.join(PHOTO_DIR, plan_id, "pt%d" % point_index)
            os.makedirs(folder, exist_ok=True)
            files = []
            with STATE.lock:
                for index in range(frames):
                    name = "%s-%s%s.jpg" % (
                        stamp, camera, "" if frames == 1 else "-%03d" % (index + 1)
                    )
                    path = os.path.join(folder, name)
                    with open(path, "wb") as handle:
                        handle.write(FAKE_JPEG)
                    relative = "%s/pt%d/%s" % (plan_id, point_index, name)
                    STATE.photos[relative] = {
                        "planId": plan_id,
                        "pointIndex": point_index,
                        "pointName": point_name,
                        "camera": camera,
                        "size": len(FAKE_JPEG),
                        "capturedAt": time.strftime("%Y-%m-%d %H:%M:%S", time.localtime()),
                    }
                    files.append({
                        "name": relative,
                        "path": path,
                        "planId": plan_id,
                        "pointIndex": point_index,
                        "camera": camera,
                        "size": len(FAKE_JPEG),
                        "capturedAt": STATE.photos[relative]["capturedAt"],
                    })
                STATE.photos["__last__"] = {
                    "at": time.strftime("%Y-%m-%d %H:%M:%S", time.localtime()),
                    "camera": camera,
                    "files": [item["name"] for item in files],
                    "error": "",
                }
            if point_name:
                # 与真板端一样写 points.json：客户端重启后回传也能带上航点名
                points_file = os.path.join(PHOTO_DIR, plan_id, "points.json")
                known = {}
                try:
                    with open(points_file) as handle:
                        known = json.load(handle)
                except (OSError, ValueError):
                    known = {}
                known[str(point_index)] = point_name
                with open(points_file, "w") as handle:
                    json.dump(known, handle, ensure_ascii=False)
            return self.json({"ok": True, "planId": plan_id, "pointIndex": point_index,
                              "camera": camera, "files": files, "error": ""})
        if path == "/api/photo/delete":
            names = body.get("names") or []
            if isinstance(names, str):
                names = [names]
            deleted = []
            errors = []
            with STATE.lock:
                for name in names:
                    name = str(name or "").strip("/")
                    parts = name.split("/")
                    if len(parts) != 3 or parts[1][:2] != "pt" or "../" in name:
                        errors.append("%s: 路径不合法" % name)
                        continue
                    STATE.photos.pop(name, None)
                    target = os.path.join(PHOTO_DIR, *parts)
                    try:
                        os.remove(target)
                    except OSError:
                        pass
                    deleted.append(name)
                    # 空目录（含 points.json 的收口）与真板端一致
                    point_dir = os.path.dirname(target)
                    plan_dir = os.path.dirname(point_dir)
                    try:
                        os.rmdir(point_dir)
                    except OSError:
                        pass
                    try:
                        if os.listdir(plan_dir) == ["points.json"]:
                            os.remove(os.path.join(plan_dir, "points.json"))
                        os.rmdir(plan_dir)
                    except OSError:
                        pass
            return self.json({"ok": not errors, "deleted": deleted, "errors": errors})

        if path == "/api/speak/stop":
            reason = str(body.get("reason") or "计划结束")
            with STATE.lock:
                changed = STATE.speak["active"]
                STATE.speak.update({"active": False, "text": "", "loop": False,
                                    "seconds": 0, "plan_id": "", "point_index": -1,
                                    "scope": ""})
                if changed:
                    STATE.speak["stops"] += 1
            return self.json({"ok": True, "changed": changed, "reason": reason,
                              "speak": STATE.speak_status()})

        # 模拟专用
        if path == "/api/sim/relay":
            with STATE.lock:
                if "enabled" in body:
                    STATE.relay["enabled"] = bool(body["enabled"])
                if "running" in body:
                    STATE.relay["running"] = bool(body["running"])
                STATE.relay["last_error"] = str(body.get("last_error", ""))
                STATE.relay["restarts"] = int(body.get("restarts", STATE.relay["restarts"]))
            return self.json({"ok": True, "relay": STATE.relay})
        if path == "/api/sim/emit":
            if not isinstance(body, dict) or not body.get("type"):
                return self.json({"ok": False, "error": "需要 {type: ...}"}, 400)
            STATE.publish(body)
            return self.json({"ok": True})
        if path == "/api/sim/flush":
            # 复现「窗口关了才落盘」：把当前计划立刻收尾（写图 + plan_hit）
            with STATE.lock:
                plan = STATE.plan
            if plan and plan.get("hit") is None:
                emit_plan_hit(plan, with_evidence=True)
                return self.json({"ok": True, "plan_hit": plan.get("hit")})
            return self.json({"ok": True, "plan_hit": None})

        return self.json({"ok": False, "error": "not found: %s" % path}, 404)

    # ---- 动作 ----
    def start(self, body):
        task = body.get("task")
        if task not in TASKS:
            return self.json({"ok": False,
                              "error": "未知模型包 %r（模拟只提供 %s）"
                                       % (task, "/".join(TASKS))})
        labels = body.get("labels") or []
        cams = body.get("cams") or ["dogcam"]
        with STATE.lock:
            STATE.running = True
            STATE.state = "running"
            STATE.task = task
            STATE.labels = [str(x) for x in labels]
            STATE.conf = float(body.get("conf") or TASKS[task]["conf"])
            STATE.cams = [str(x) for x in cams][:1] or ["dogcam"]
            STATE.fps = max(1, min(30, int(body.get("fps") or 6)))
            STATE.error = ""
            STATE.session += 1
            plan_id = body.get("plan_id")
            if plan_id:
                STATE.plan = {
                    "plan_id": str(plan_id),
                    "point_index": int(body.get("point_index") or 0),
                    "check_type": str(body.get("check_type") or "").lower()
                    or ("count" if body.get("expect_count") else "exist"),
                    "expect_count": body.get("expect_count"),
                    # 中台参数配置随 ai_start 下发；缺省用模拟的 MEDIA_DIR / 帧数
                    "media_dir": str(body.get("media_dir") or MEDIA_DIR),
                    "save_frames": int(body.get("save_frames") or PLAN_SAVE_FRAMES),
                    "started": time.time(),
                    "hit": None,
                }
            else:
                STATE.plan = None
        STATE.publish(STATE.detect_state_event())
        return self.json({"ok": True, "status": STATE.status()})

    def stop(self):
        with STATE.lock:
            plan = STATE.plan
            STATE.running = False
            STATE.state = "stopped"
        # 板端 `_plan_flush`：停识别时才把采样中的命中画框落盘。图晚到时
        # 客户端正是靠这一步拿图（`ai_evidence`），所以模拟要照做。
        if plan and plan.get("hit") is None:
            emit_plan_hit(plan, with_evidence=True)
        with STATE.lock:
            STATE.plan = None
        STATE.publish(STATE.detect_state_event())
        return self.json({"ok": True, "status": STATE.status()})

    # ---- SSE ----
    def sse(self):
        q = STATE.subscribe()
        try:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream; charset=utf-8")
            self.send_header("Cache-Control", "no-cache")
            self.send_header("Connection", "close")
            self.cors()
            self.end_headers()
            self.wfile.write(b": connected backpack-sim\n\n")
            self.wfile.flush()
            while True:
                try:
                    chunk = q.get(timeout=10)
                except queue.Empty:
                    chunk = ": ping\n\n"
                self.wfile.write(chunk.encode("utf-8"))
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            STATE.unsubscribe(q)
            self.close_connection = True


def main():
    os.makedirs(JOBS_DIR, exist_ok=True)
    stop = threading.Event()
    threading.Thread(target=engine_loop, args=(stop,), daemon=True).start()
    server = ThreadingHTTPServer((HOST, PORT), Handler)
    server.daemon_threads = True
    print("[agent] 模拟背包 Agent 已启动: http://%s:%d  jobs=%s" % (HOST, PORT, JOBS_DIR),
          flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        stop.set()
        server.server_close()


if __name__ == "__main__":
    main()
