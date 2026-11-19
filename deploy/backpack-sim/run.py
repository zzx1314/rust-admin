#!/usr/bin/env python3
"""本地一键起 `.246` 背包机（+ 业务平台）的模拟环境。

```text
                     本机（模拟「现场」）                           本机（模拟「平台」）
┌───────────────────────────────────────────────────────┐   ┌──────────────────────┐
│ fake_cams.py ──RTSP──▶ mediamtx :8554/:8889           │   │                      │
│      (testsrc2)         ▲            ▲                │   │  business/server     │
│                         │            └──WHEP── 浏览器  │   │  :3100               │
│ mock_agent.py :8000 ────┘（cam0~cam3/preview 由 Agent 推）│   │  ├ /api/backpack/*   │
│      ▲   ▲                                               │   │  └ /ws/device/video  │
│      │   └──SSE──┐                                       │   │                      │
│  relay_push.py ──┼──WS /ws/device/video（FLV 分片）──────┼──▶│  VideoHub 房间 bp:*  │
│  （仓库原版脚本） │                                       │   │                      │
│                  │                                       │   │                      │
│ robodog-device-client --robot mock ──WS /ws/device──────┼──▶│  设备台账 / 任务 / AI │
│      （仓库原版二进制）  │                               │   └──────────────────────┘
│                        └──HTTP 127.0.0.1:8000（agent_relay 隧道）
└───────────────────────────────────────────────────────┘
```

用到的**都是仓库里的真实实现**（`deploy/backpack-relay/relay_push.py`、
`business/client`、`deploy/site-console/serve.py`、`business/server`），只有板端那三样
跑不进笔记本的件（真 Agent 的 NPU/相机依赖、mediamtx、相机）用模拟件顶替。真 Agent 的
源码已在仓库 `business/backpack_dashboard/`，用 `--agent real` 可直接跑它：

| `.246` 上的东西 | 本地模拟 |
| --- | --- |
| `backpack-agent.service`（FastAPI :8000 + RKNN） | 默认 `mock_agent.py`（纯标准库，假装检出/画框/落证据图）；`--agent real` 跑仓库里的真实现 |
| mediamtx（7 条 path） | `bin/mediamtx` + `mediamtx.yml`（下载的官方二进制） |
| 4 路 USB 相机 + 本体前置 `dogcam` | `fake_cams.py`（ffmpeg `testsrc2` 推 RTSP） |
| `backpack-relay.service` | 同一个脚本，`RELAY_PLATFORM` 指向本机平台 |
| `robodog-device-client.service` | 同一个二进制，`--robot mock`（进程内假本体） |
| `site-console.service`（:8080） | 同一个 `serve.py`（需要先 `pnpm build`） |

## 用法

```bash
cd deploy/backpack-sim

python3 run.py setup     # 一次性：下 mediamtx、建 venv(websockets)、编设备客户端
python3 run.py up        # 起全套（前台，Ctrl-C 全停）
python3 run.py status    # 另开一个终端看谁活着、平台/Agent 通不通
python3 run.py smoke     # 冒烟自测：隧道 + 中继起流 + AI 启停 + plan_hit/证据图
python3 run.py down      # 兜底：上一个 up 崩了，按 pid 文件收尸
```

`up` 起来之后：

| 入口 | 地址 | 账号 |
| --- | --- | --- |
| 业务平台（前端开发服务器） | <http://127.0.0.1:5273>（另开：`cd business/web && pnpm dev`） | `admin / admin123` |
| 业务平台（已构建时由后端托管） | <http://127.0.0.1:3100> | `admin / admin123` |
| 现场入口（同网段控制台，可选） | <http://127.0.0.1:8080> | `admin / admin123` |
| 背包 Agent（模拟） | <http://127.0.0.1:8000/api/detect/status> | — |
| mediamtx WHEP | `http://127.0.0.1:8889/<stream>/whep` | — |
| 设备 | `RD-MOCK-01`（在线，`--robot mock` 会真的把任务跑完） | — |

## 常用开关

```bash
python3 run.py up --mode direct          # 浏览器直连 Agent（:8000），而不是设备客户端隧道
python3 run.py up --platform http://127.0.0.1:3100   # 只起板端，连已有平台
python3 run.py up --no-platform          # 同上（等价写法）
python3 run.py up --no-relay --no-cams   # 只模拟 Agent / 客户端（不看视频）
python3 run.py up --with-console         # 额外起 :8080 现场入口（要 business/web/dist）
SIM_PUBLISH_STREAMS=cam0,cam1,cam2,cam3,dogcam,preview python3 run.py up   # 全 7 路
SIM_COUNT_RESULT=mismatch python3 run.py up   # 让数量任务少 1 个 → 验证 count 告警 + 证据图
SIM_EXIST_FOUND=0 python3 run.py up           # 存在性任务一直不命中 → 验证「未检出」告警
SIM_LATE_EVIDENCE=1 python3 run.py up         # 图比卡片晚到 → 验证 ai_evidence 只补图
```

## 与真实 `.246` 的差异（排障时别被它带偏）

1. **默认 Agent 没有 RKNN**：`mock_agent.py` 的两个模型包是写死的，框是编的，`osd.ready` 恒真。
   真机上的模型加载/NPU 忙/画框字体缺失，这里都复现不了；验这些要连真板子（或 `--agent real`
   在板子上跑，接口/SSE 是真实现，AI 仍要 NPU 与编译好的 `yolov8_detector`）。
2. **视频是 `testsrc2`**：只有一路「彩条」。相机没插、本体没推流、红外黑屏这类现象不模拟。
3. **网络是 localhost**：没有 Wi-Fi/4G 切换、没有 P2P、没有 `ip route` 出口。
   `relay_push.py` 的降档逻辑仍在跑，但 `RELAY_ROUTE_POLL=0`（本机没有 `enx*`/`wlp*` 出口，
   `ip route get` 也没意义）。想在本地演练降档，用
   `RELAY_SLOW_IFACES=lo RELAY_SLOW_IFACE_LEVEL=2 python3 run.py up` 之类的方式逼它。
4. **平台是本机 `business/server`**：没有中台（:3000）、没有官网入口、没有企业微信群。
   `run.py` 会主动剥掉继承来的 `ROBODOG_WECOM_*`，避免本机跑一次就往现场群里推消息。
   要验企业微信，显式 `SIM_KEEP_WECOM=1 python3 run.py up`（前提是你真配了 webhook）。
5. **设备号 `RD-MOCK-01`**：连真平台时它会被注册成一台新设备（`run.py` 不默认连真平台）。

## 平台配置（`up` 自动设置，手工跑平台时照抄）

```bash
# 隧道模式（默认，线上正式环境就是这条：平台在公网、板端在内网）
ROBODOG_PORT=3100
ROBODOG_DB=deploy/backpack-sim/data/robodog.db
ROBODOG_POINT_AI=client
ROBODOG_AI_JOBS_DIR=deploy/backpack-sim/data/detect_jobs   # 客户端读证据图用

# 直连模式（--mode direct）
ROBODOG_BACKPACK_BROWSER_URL=http://127.0.0.1:8000
ROBODOG_BACKPACK_WEBRTC_URL=http://127.0.0.1:8889
```

## 文件

```text
deploy/backpack-sim/
├── run.py           进程编排 + 依赖检查（up/down/status/smoke/setup）
├── mock_agent.py    模拟 Agent（:8000，契约见 docs/backpack-video-integration.md §2.1）
│                    真实现是仓库里的 business/backpack_dashboard/backend/agent.py（--agent real）
├── fake_cams.py     ffmpeg 假相机 → mediamtx
├── smoke.py         冒烟自测（run.py smoke 调它）
├── mediamtx.yml     7 条 path（全部 source: publisher，本地没有真 RTSP 源）
├── bin/             mediamtx 二进制（setup 下载，已 gitignore）
├── runtime/venv/    relay 的 websockets 依赖（setup 创建，已 gitignore）
├── data/            robodog.db + detect_jobs/（已 gitignore）
└── logs/            每个进程一份日志（已 gitignore）
```
"""

import argparse
import json
import os
import platform
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
BIN = os.path.join(HERE, "bin")
DATA = os.path.join(HERE, "data")
LOGS = os.path.join(HERE, "logs")
RUN = os.path.join(HERE, "run")
VENV = os.path.join(HERE, "runtime", "venv")
PID_FILE = os.path.join(RUN, "sim.json")

MEDIAMTX_VERSION = os.environ.get("SIM_MEDIAMTX_VERSION", "1.21.4")
MEDIAMTX_BIN = os.path.join(BIN, "mediamtx")


def mediamtx_url():
    return (
        "https://github.com/bluenviron/mediamtx/releases/download/v%s/"
        "mediamtx_v%s_%s.tar.gz"
        % (MEDIAMTX_VERSION, MEDIAMTX_VERSION, mediamtx_platform())
    )

PLATFORM_PORT = int(os.environ.get("SIM_PLATFORM_PORT", "3100"))
AGENT_PORT = int(os.environ.get("SIM_AGENT_PORT", "8000"))
CONSOLE_PORT = int(os.environ.get("SIM_CONSOLE_PORT", "8080"))
DEVICE_ID = os.environ.get("SIM_DEVICE_ID", "RD-MOCK-01")

COLORS = {"sim": "\033[36m", "warn": "\033[33m", "err": "\033[31m", "dim": "\033[2m"}
RESET = "\033[0m"


def _is_arm():
    return os.uname().machine.lower() in {"aarch64", "arm64"}


def mediamtx_platform():
    system = platform.system().lower()
    arch = "arm64" if _is_arm() else "amd64"
    if system == "darwin":
        return "darwin_%s" % arch
    return "linux_%s" % arch


def say(message, kind="sim"):
    prefix = {"sim": "[sim] ", "warn": "[sim][warn] ", "err": "[sim][err] "}[kind]
    color = COLORS[kind] if sys.stdout.isatty() else ""
    reset = RESET if sys.stdout.isatty() else ""
    print("%s%s%s%s" % (color, prefix, message, reset), flush=True)


def port_busy(port, host="127.0.0.1"):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.settimeout(0.3)
        return sock.connect_ex((host, port)) == 0


# --------------------------------------------------------------------- 进程


class Proc:
    def __init__(self, name, cmd, env=None, cwd=None, critical=False):
        self.name = name
        self.cmd = cmd
        self.env = env
        self.cwd = cwd
        # 关键组件（mediamtx / 平台）死了整场就没意义，直接收队；
        # 其他组件（Agent / 相机 / 客户端 / relay / 控制台）允许手动 kill 去演练故障场景
        self.critical = critical
        self.reported = False
        self.process = None
        self.thread = None
        self.log_path = os.path.join(LOGS, "%s.log" % name)

    def start(self):
        os.makedirs(LOGS, exist_ok=True)
        env = dict(os.environ if self.env is None else self.env)
        env.setdefault("PYTHONUNBUFFERED", "1")
        handle = open(self.log_path, "wb")
        self.process = subprocess.Popen(
            self.cmd,
            cwd=self.cwd or ROOT,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        self.thread = threading.Thread(target=self._pump, args=(handle,), daemon=True)
        self.thread.start()
        return self.process.poll() is None

    def _pump(self, handle):
        prefix = "[%s] " % self.name
        for raw in iter(self.process.stdout.readline, b""):
            line = raw.decode("utf-8", "replace")
            handle.write(raw)
            handle.flush()
            sys.stdout.write(prefix + line)
            sys.stdout.flush()
        self.process.stdout.close()
        handle.close()

    def alive(self):
        return self.process is not None and self.process.poll() is None

    def stop(self, timeout=8):
        if not self.alive():
            return
        try:
            os.killpg(os.getpgid(self.process.pid), signal.SIGTERM)
        except Exception:  # noqa: BLE001
            return
        deadline = time.time() + timeout
        while time.time() < deadline and self.alive():
            time.sleep(0.2)
        if self.alive():
            try:
                os.killpg(os.getpgid(self.process.pid), signal.SIGKILL)
            except Exception:  # noqa: BLE001
                pass


def http_ok(url, timeout=2.0):
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:
            return 200 <= response.status < 400
    except (urllib.error.URLError, OSError, ValueError):
        return False


def wait_for(name, url, seconds=90):
    deadline = time.time() + seconds
    while time.time() < deadline:
        if http_ok(url):
            say("%s 就绪：%s" % (name, url))
            return True
        time.sleep(1)
    say("%s 在 %ss 内没就绪（%s），继续启动其余组件" % (name, seconds, url), "warn")
    return False


def write_pids(procs, settings=None):
    os.makedirs(RUN, exist_ok=True)
    payload = {
        "started_at": time.time(),
        "settings": settings or {},
        "processes": [
            {"name": proc.name, "pid": proc.process.pid if proc.process else None,
             "cmd": proc.cmd}
            for proc in procs
        ],
    }
    with open(PID_FILE, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, ensure_ascii=False, indent=2)


# --------------------------------------------------------------------- setup


def download_mediamtx():
    if os.path.exists(MEDIAMTX_BIN):
        say("mediamtx 已存在：%s（v%s 由 SIM_MEDIAMTX_VERSION 控制）"
            % (MEDIAMTX_BIN, MEDIAMTX_VERSION), "sim")
        return True
    os.makedirs(BIN, exist_ok=True)
    if not shutil.which("curl") or not shutil.which("tar"):
        say("需要 curl 与 tar 才能下载 mediamtx", "err")
        return False
    say("下载 mediamtx v%s …" % MEDIAMTX_VERSION)
    say("（GitHub release 在国内可能需要代理：export https_proxy=http://127.0.0.1:7890）",
        "warn")
    archive = os.path.join(BIN, "mediamtx.tar.gz")
    url = mediamtx_url()
    steps = [
        ["curl", "-fSL", "--retry", "2", "-o", archive, url],
        ["tar", "-xzf", archive, "-C", BIN, "mediamtx"],
        ["chmod", "+x", MEDIAMTX_BIN],
    ]
    for step in steps:
        if subprocess.run(step).returncode != 0:
            say("下载失败：%s" % " ".join(step), "err")
            say("可以手动下 %s 解到 %s" % (url, BIN), "warn")
            return False
    try:
        os.remove(archive)
    except OSError:
        pass
    say("mediamtx 就绪：%s" % MEDIAMTX_BIN)
    return True


def ensure_venv():
    python = os.path.join(VENV, "bin", "python")
    if os.path.exists(python) and subprocess.run(
        [python, "-c", "import websockets"], capture_output=True
    ).returncode == 0:
        say("relay 依赖已就绪：%s" % python)
        return python
    say("创建 venv 并安装 websockets（relay_push.py 连平台要用）…")
    if subprocess.run([sys.executable, "-m", "venv", VENV]).returncode != 0:
        say("创建 venv 失败（python3-venv 装了吗？）", "err")
        return None
    if subprocess.run(
        [python, "-m", "pip", "install", "--quiet", "websockets"]
    ).returncode != 0:
        say("pip install websockets 失败（需要外网/代理）", "err")
        return None
    say("relay 依赖就绪：%s" % python)
    return python


def ensure_client_binary(build=True):
    candidates = [
        os.path.join(ROOT, "business/client/target/debug/robodog-device-client"),
        os.path.join(ROOT, "business/client/target/release/robodog-device-client"),
    ]
    for path in candidates:
        if os.path.exists(path):
            say("设备客户端已就绪：%s" % path)
            return path
    if not build:
        return None
    say("编译设备客户端（cargo build，首次要几分钟）…", "warn")
    result = subprocess.run(
        ["cargo", "build", "--manifest-path", "business/client/Cargo.toml"],
        cwd=ROOT,
    )
    if result.returncode != 0:
        return None
    return candidates[0] if os.path.exists(candidates[0]) else None


def cmd_setup(args):
    say("== 依赖检查 ==")
    ok = True
    if not shutil.which("ffmpeg"):
        say("缺少 ffmpeg：假相机与 relay 都要它（apt install ffmpeg / brew install ffmpeg）", "err")
        ok = False
    if not shutil.which("cargo"):
        say("缺少 cargo：设备客户端与业务平台都要编译", "err")
        ok = False
    ok = download_mediamtx() and ok
    ok = bool(ensure_venv()) and ok
    ok = bool(ensure_client_binary(build=not args.no_build)) and ok
    if ok:
        say("准备完毕。下一步：python3 run.py up")
    else:
        say("有依赖没装好，先解决上面的问题", "err")
    return 0 if ok else 1


# --------------------------------------------------------------------- up


def platform_env(mode):
    """业务平台的环境变量：默认不继承任何 ROBODOG_*（避免连到正式环境）。"""
    keep_wecom = os.environ.get("SIM_KEEP_WECOM") == "1"
    env = {}
    for key, value in os.environ.items():
        if key.startswith("ROBODOG_"):
            if keep_wecom and key.startswith("ROBODOG_WECOM_"):
                env[key] = value
            continue
        env[key] = value
    env.update({
        "ROBODOG_PORT": str(PLATFORM_PORT),
        "ROBODOG_DB": os.path.join(DATA, "robodog.db"),
        # cargo run 的工作目录是仓库根，迁移脚本在 business/server/migrations
        "ROBODOG_MIGRATIONS_DIR": os.path.join(ROOT, "business/server/migrations"),
        "ROBODOG_JWT_SECRET": "backpack-sim-jwt-secret",
        "ROBODOG_POINT_AI": "client",
        # 本机上浏览器/平台都能直连 mediamtx：两种模式都给 WHEP 基址，
        # 页面想用「直连看流」时不用再去猜 8889 端口
        "ROBODOG_BACKPACK_WEBRTC_URL": "http://127.0.0.1:8889",
        "ROBODOG_AUTO_DEGRADE": os.environ.get("SIM_VIDEO_AUTO_DEGRADE", "true"),
    })
    if mode == "direct":
        env["ROBODOG_BACKPACK_BROWSER_URL"] = "http://127.0.0.1:%d" % AGENT_PORT
    return env


def client_env(device_id, agent_port, jobs_dir, server_url):
    env = {}
    for key, value in os.environ.items():
        if key.startswith("ROBODOG_"):
            continue
        env[key] = value
    env.update({
        "ROBODOG_SERVER_URL": server_url,
        "ROBODOG_DEVICE_ID": device_id,
        "ROBODOG_ROBOT": "mock",
        "ROBODOG_AI_ENABLED": "1",
        "ROBODOG_AI_AGENT_URL": "http://127.0.0.1:%d" % agent_port,
        "ROBODOG_AI_JOBS_DIR": jobs_dir,
        "RUST_LOG": os.environ.get("SIM_RUST_LOG", "info"),
    })
    return env


REAL_AGENT = os.path.join(ROOT, "business/backpack_dashboard/backend/agent.py")


def real_agent_missing_deps():
    """真 Agent 启动依赖：板端有，开发机一般没有（只在选 --agent real 时检查）。"""
    missing = []
    for module in ("fastapi", "uvicorn", "evdev"):
        try:
            __import__(module)
        except Exception:  # noqa: BLE001 - 缺系统库也可能是 OSError
            missing.append(module)
    return missing


def make_agent_proc(mode, jobs_dir):
    """mock = 接口契约的替身；real = 仓库里 `business/backpack_dashboard` 的真实现。"""
    if mode == "real":
        if not os.path.exists(REAL_AGENT):
            say("找不到真 Agent：%s" % REAL_AGENT, "err")
            return None
        missing = real_agent_missing_deps()
        if missing:
            say("真 Agent 缺依赖：%s。它只能在板子上跑（pip install -r "
                "business/backpack_dashboard/backend/requirements.txt，再装 evdev 系统库与 "
                "librknnrt.so）；开发机请用默认 --agent mock" % ", ".join(missing), "err")
            return None
        env = dict(os.environ)
        env.pop("SIM_JOBS_DIR", None)
        env.update({
            "AGENT_PORT": str(AGENT_PORT),
            "AGENT_CORS_ORIGINS": "http://127.0.0.1:5273,http://localhost:5273,"
                                  "http://127.0.0.1:%d,http://127.0.0.1:8080" % PLATFORM_PORT,
            # 证据图要和设备客户端的 ROBODOG_AI_JOBS_DIR 指同一处
            "DETECT_JOBS_DIR": jobs_dir,
            # sim 把「假本体前置」推到本地 mediamtx 的 dogcam path，真 Agent 的 relay 去拉它
            "DOG_RTSP_URL": "rtsp://127.0.0.1:8554/dogcam",
            "DOG_RELAY_STREAM": "dogcam",
            "MEDIAMTX_WEBRTC_HOST": "127.0.0.1",
            "PYTHONUNBUFFERED": "1",
        })
        say("使用仓库里的真 Agent：%s（无 NPU / 相机时四路与 AI 会报错，属预期；"
            "接口与 SSE 仍是真实现）" % REAL_AGENT)
        return Proc("agent", [sys.executable, REAL_AGENT], env=env,
                    cwd=os.path.dirname(REAL_AGENT))
    return Proc("agent", [sys.executable, os.path.join(HERE, "mock_agent.py")],
                env=dict(os.environ, SIM_JOBS_DIR=jobs_dir))


def platform_ws(platform_url):
    """`http(s)://host:port` → `ws(s)://host:port`（客户端自己会补 `/ws/device`）。"""
    if platform_url.startswith("https://"):
        return "wss://" + platform_url[len("https://"):].rstrip("/")
    if platform_url.startswith("http://"):
        return "ws://" + platform_url[len("http://"):].rstrip("/")
    return platform_url.rstrip("/")


def relay_env(platform_url, streams):
    env = {}
    for key, value in os.environ.items():
        if key.startswith("RELAY_"):
            continue
        env[key] = value
    env.update({
        "RELAY_PLATFORM": platform_url,
        "RELAY_STREAMS": streams,
        "RELAY_RTSP_BASE": "rtsp://127.0.0.1:8554",
        "RELAY_ROOM_PREFIX": "bp:",
        "RELAY_PROFILE": os.environ.get("SIM_RELAY_PROFILE", "light"),
        "RELAY_WIDTH": os.environ.get("SIM_RELAY_WIDTH", "1280"),
        "RELAY_HEIGHT": os.environ.get("SIM_RELAY_HEIGHT", "720"),
        "RELAY_FPS": os.environ.get("SIM_RELAY_FPS", "15"),
        "RELAY_BITRATE": os.environ.get("SIM_RELAY_BITRATE", "1000k"),
        "RELAY_GOP": os.environ.get("SIM_RELAY_GOP", "15"),
        # 本机没有 4G dongle / Wi-Fi 出口切换，`ip route get` 也拿不到有意义的结果
        "RELAY_ROUTE_POLL": "0",
        "RELAY_SLOW_IFACES": os.environ.get("RELAY_SLOW_IFACES", ""),
        # x86 上没有 rkmpp / v4l2m2m，直接钉 libx264，省掉每次起流的编码器探测
        "RELAY_ENCODER": os.environ.get("RELAY_ENCODER", "libx264"),
        "RELAY_DEBUG": os.environ.get("RELAY_DEBUG", "0"),
    })
    env.pop("PYTHONUNBUFFERED", None)
    return env


def cmd_up(args):
    os.makedirs(DATA, exist_ok=True)
    os.makedirs(LOGS, exist_ok=True)
    streams = [
        item.strip()
        for item in os.environ.get("SIM_PUBLISH_STREAMS", "dogcam,cam0,preview").split(",")
        if item.strip()
    ]
    jobs_dir = os.path.join(DATA, "detect_jobs")

    # ---- 依赖 ----
    if not os.path.exists(MEDIAMTX_BIN):
        say("还没装 mediamtx：先跑 python3 run.py setup", "err")
        return 1
    if not shutil.which("ffmpeg"):
        say("缺少 ffmpeg（假相机 / relay 用）", "err")
        return 1
    relay_python = ensure_venv()
    if not relay_python:
        return 1
    client_bin = ensure_client_binary(build=not args.no_build)
    if not client_bin:
        say("没有可用的设备客户端二进制：先跑 python3 run.py setup", "err")
        return 1

    # ---- 端口 ----
    if port_busy(PLATFORM_PORT) and not args.no_platform and not args.platform:
        say("端口 %d 已被占用（已经有平台在跑？）：用 --platform 复用它，"
            "或 SIM_PLATFORM_PORT 换端口" % PLATFORM_PORT, "err")
        return 1
    if port_busy(AGENT_PORT) and not args.no_agent:
        say("端口 %d 已被占用（可能真板子的 Agent 在本机？）：SIM_AGENT_PORT 换端口"
            % AGENT_PORT, "err")
        return 1

    procs = []
    if args.platform:
        platform_url = args.platform.rstrip("/")
        say("复用已有平台：%s（不再启动本地 business/server）" % platform_url)
        if not http_ok(platform_url + "/api/health"):
            say("%s/api/health 现在不通：确认地址 / 端口 / 网络" % platform_url, "warn")
    elif args.no_platform:
        platform_url = "http://127.0.0.1:%d" % PLATFORM_PORT
        say("--no-platform：假定 %s 上已有平台" % platform_url)
    else:
        platform_url = "http://127.0.0.1:%d" % PLATFORM_PORT
    server_ws = platform_ws(platform_url)

    # ---- 1. mediamtx ----
    if not args.no_cams:
        procs.append(Proc("mediamtx", [MEDIAMTX_BIN, os.path.join(HERE, "mediamtx.yml")],
                          critical=True))
        if not procs[-1].start():
            say("mediamtx 启动失败，看 logs/mediamtx.log", "err")
            return 1
        time.sleep(1.5)
        procs.append(Proc("cams", [sys.executable, os.path.join(HERE, "fake_cams.py")],
                          env=dict(os.environ, SIM_PUBLISH_STREAMS=",".join(streams))))
        procs[-1].start()

    # ---- 2. Agent（模拟 / 仓库里的真实现）----
    if not args.no_agent:
        agent_proc = make_agent_proc(args.agent, jobs_dir)
        if agent_proc is None:
            for proc in reversed(procs):
                proc.stop()
            return 1
        procs.append(agent_proc)
        procs[-1].start()
        wait_for("背包 Agent", "http://127.0.0.1:%d/api/cameras" % AGENT_PORT, seconds=20)

    # ---- 3. 平台 ----
    if not args.platform and not args.no_platform:
        procs.append(Proc(
            "platform",
            ["cargo", "run", "--manifest-path", "business/server/Cargo.toml"],
            env=platform_env(args.mode),
            critical=True,
        ))
        procs[-1].start()
        wait_for("业务平台", "%s/api/health" % platform_url, seconds=180)

    # ---- 4. 设备客户端（隧道 + 假本体跑任务） ----
    if not args.no_client:
        procs.append(Proc(
            "client",
            [client_bin,
             "--server", server_ws,
             "--device-id", args.device_id,
             "--robot", "mock",
             "--ai-agent-url", "http://127.0.0.1:%d" % AGENT_PORT],
            env=client_env(args.device_id, AGENT_PORT, jobs_dir, server_ws),
        ))
        procs[-1].start()

    # ---- 5. 中继 ----
    if not args.no_relay and streams:
        procs.append(Proc(
            "relay",
            [relay_python, os.path.join(ROOT, "deploy/backpack-relay/relay_push.py")],
            env=relay_env(platform_url, ",".join(streams)),
        ))
        procs[-1].start()

    # ---- 6. 现场控制台（可选） ----
    if args.with_console:
        dist = os.path.join(ROOT, "business/web/dist/index.html")
        if not os.path.exists(dist):
            say("--with-console 需要先 cd business/web && pnpm build（%s 不存在）" % dist, "warn")
        else:
            procs.append(Proc(
                "console",
                [sys.executable, os.path.join(ROOT, "deploy/site-console/serve.py")],
                env=dict(
                    os.environ,
                    CONSOLE_DIR=os.path.join(ROOT, "business/web/dist"),
                    CONSOLE_API_BASE="%s/api" % platform_url,
                    CONSOLE_VIDEO_WS_BASE=platform_url.replace("http", "ws", 1),
                    CONSOLE_PORT=str(CONSOLE_PORT),
                ),
            ))
            procs[-1].start()

    write_pids(procs, {
        "agent_port": AGENT_PORT,
        "platform_port": PLATFORM_PORT,
        "console_port": CONSOLE_PORT,
        "mode": args.mode,
        "platform_url": platform_url,
        "streams": streams,
        "device_id": args.device_id,
    })

    # ---- 摘要 ----
    print()
    say("=" * 72)
    say("模拟环境已启动（Ctrl-C 全部停止）")
    say("  业务平台      %s   （admin / admin123）" % platform_url)
    say("  背包 Agent    http://127.0.0.1:%d  （%s，接入模式：%s）"
        % (AGENT_PORT, "mock 模拟" if args.agent == "mock" else "仓库真实现",
           args.mode if not args.platform else "由远端平台配置决定"))
    say("  设备          %s（--robot mock：任务会真的跑完）" % args.device_id)
    say("  视频流        %s" % ", ".join(streams) if streams else "  视频流        未推流")
    say("  现场入口      %s" % ("http://127.0.0.1:%d" % CONSOLE_PORT if args.with_console
                                else "（未启用，加 --with-console）"))
    if not os.path.exists(os.path.join(ROOT, "business/web/dist/index.html")):
        say("  前端          cd business/web && pnpm dev → http://127.0.0.1:5273")
    say("  日志          %s/<组件>.log" % LOGS)
    say("  排障          python3 run.py status")
    say("=" * 72)
    print()

    stopping = threading.Event()

    def shutdown(*_):
        if stopping.is_set():
            return
        stopping.set()
        say("正在停止…")
        for proc in reversed(procs):
            proc.stop()

    signal.signal(signal.SIGINT, shutdown)
    signal.signal(signal.SIGTERM, shutdown)

    while not stopping.is_set():
        for proc in procs:
            if proc.alive() or stopping.is_set():
                continue
            if proc.critical:
                say("%s 退出了（logs/%s.log）：整场模拟没意义，收队" % (proc.name, proc.name), "err")
                shutdown()
                break
            if not proc.reported:
                proc.reported = True
                say("%s 退出了（logs/%s.log）—— 不影响其他组件，"
                    "想演练「%s 挂掉」就保持现状，Ctrl-C 后重跑 up 可恢复"
                    % (proc.name, proc.name, proc.name), "warn")
        time.sleep(1)

    return 0


# --------------------------------------------------------------------- down


def cmd_down(_args):
    if not os.path.exists(PID_FILE):
        say("没有找到 %s：可能已经停过了" % PID_FILE)
        return 0
    with open(PID_FILE, encoding="utf-8") as handle:
        payload = json.load(handle)
    for item in payload.get("processes", []):
        pid = item.get("pid")
        if not pid:
            continue
        try:
            os.killpg(os.getpgid(pid), signal.SIGTERM)
            say("已停止 %s（pid %s）" % (item.get("name"), pid))
        except ProcessLookupError:
            say("%s（pid %s）已经不在" % (item.get("name"), pid))
        except PermissionError:
            say("没有权限停止 %s（pid %s）" % (item.get("name"), pid), "warn")
    time.sleep(2)
    for item in payload.get("processes", []):
        pid = item.get("pid")
        if not pid:
            continue
        try:
            os.killpg(os.getpgid(pid), signal.SIGKILL)
        except Exception:  # noqa: BLE001
            pass
    os.remove(PID_FILE)
    return 0


# --------------------------------------------------------------------- status


def cmd_smoke(_args):
    settings = load_settings()
    env = dict(os.environ)
    env["SIM_AGENT_PORT"] = str(settings.get("agent_port", AGENT_PORT))
    env["SIM_PLATFORM_PORT"] = str(settings.get("platform_port", PLATFORM_PORT))
    if settings.get("platform_url"):
        env["SIM_PLATFORM_URL"] = settings["platform_url"]
    streams = settings.get("streams") or []
    if streams and not env.get("SIM_STREAM"):
        env["SIM_STREAM"] = streams[0]
    python = os.path.join(VENV, "bin", "python")
    if not os.path.exists(python):
        python = sys.executable
    return subprocess.run([python, os.path.join(HERE, "smoke.py")], env=env).returncode


def load_settings():
    if not os.path.exists(PID_FILE):
        return {}
    try:
        with open(PID_FILE, encoding="utf-8") as handle:
            return json.load(handle).get("settings") or {}
    except (OSError, ValueError):
        return {}


def cmd_status(_args):
    settings = load_settings()
    entries = []
    if os.path.exists(PID_FILE):
        try:
            with open(PID_FILE, encoding="utf-8") as handle:
                payload = json.load(handle)
            for item in payload.get("processes", []):
                pid = item.get("pid")
                alive = False
                if pid:
                    try:
                        os.kill(pid, 0)
                        alive = True
                    except OSError:
                        alive = False
                entries.append("%s=%s%s" % (item.get("name"), pid, "" if alive else "(dead)"))
        except (OSError, ValueError):
            entries = ["pid 文件读不出来"]

    agent_port = int(settings.get("agent_port", AGENT_PORT))
    platform_port = int(settings.get("platform_port", PLATFORM_PORT))
    console_port = int(settings.get("console_port", CONSOLE_PORT))
    platform_url = settings.get("platform_url") or "http://127.0.0.1:%d" % platform_port
    parsed = urllib.parse.urlparse(platform_url)
    platform_host = parsed.hostname or "127.0.0.1"
    platform_port = parsed.port or (443 if parsed.scheme == "https" else 80)

    print()
    say("端口：")
    for name, host, port in [("mediamtx RTSP", "127.0.0.1", 8554),
                             ("mediamtx WHEP", "127.0.0.1", 8889),
                             ("背包 Agent", "127.0.0.1", agent_port),
                             ("业务平台", platform_host, platform_port),
                             ("现场入口", "127.0.0.1", console_port)]:
        print("  %-16s %s:%d  %s" % (name, host, port,
                                    "LISTEN" if port_busy(port, host) else "—"))

    print()
    if settings:
        say("上一轮 up：mode=%s  platform=%s  streams=%s"
            % (settings.get("mode"), settings.get("platform_url"),
               ",".join(settings.get("streams") or [])))
    say("进程：%s" % ("  ".join(entries) if entries else "无（run.py up 没跑或已退出）"))

    print()
    say("连通性：")
    checks = [
        ("背包 Agent /api/cameras", "http://127.0.0.1:%d/api/cameras" % agent_port),
        ("业务平台 /api/health", platform_url.rstrip("/") + "/api/health"),
    ]
    for label, url in checks:
        say("  %-24s %s" % (label, "OK" if http_ok(url) else "不通"))
    try:
        with urllib.request.urlopen(
            "http://127.0.0.1:%d/api/sim/state" % agent_port, timeout=2
        ) as response:
            state = json.load(response)
        say("  %-24s running=%s plan=%s subscribers=%s"
            % ("Agent 运行态", state.get("running"),
               bool(state.get("plan")), state.get("subscribers")))
    except Exception:  # noqa: BLE001 - 只是展示
        pass
    print()
    return 0


# --------------------------------------------------------------------- main


def main():
    parser = argparse.ArgumentParser(
        description="本地模拟 .246 背包机（Agent / mediamtx / 相机 / relay / 客户端）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""常用：
  python3 run.py setup            # 一次性：mediamtx / venv(websockets) / 设备客户端
  python3 run.py up               # 起全套（tunnel 模式，Ctrl-C 全停）
  python3 run.py up --mode direct --with-console
  python3 run.py status / smoke / down
详见 deploy/backpack-sim/README.md。""",
    )
    sub = parser.add_subparsers(dest="command")

    up = sub.add_parser("up", help="起全套（前台；Ctrl-C 停止）")
    up.add_argument("--platform", help="复用已有业务平台（如 http://39.107.55.13:3008）")
    up.add_argument("--no-platform", action="store_true", help="不起本地平台")
    up.add_argument("--mode", choices=["tunnel", "direct"], default="tunnel",
                    help="tunnel=平台经设备客户端代调 Agent（默认，线上形态）；"
                         "direct=浏览器直连 :8000")
    up.add_argument("--device-id", default=DEVICE_ID, help="设备号（默认 %s）" % DEVICE_ID)
    up.add_argument("--with-console", action="store_true", help="额外起 :8080 现场入口")
    up.add_argument("--agent", choices=["mock", "real"], default="mock",
                    help="mock=模拟 Agent（默认，契约一致、不需要 NPU）；"
                         "real=business/backpack_dashboard 里的真实现（需要板端依赖）")
    up.add_argument("--no-agent", action="store_true", help="不起 Agent")
    up.add_argument("--no-cams", action="store_true", help="不起 mediamtx / 假相机")
    up.add_argument("--no-relay", action="store_true", help="不起视频中继")
    up.add_argument("--no-client", action="store_true", help="不起设备客户端")
    up.add_argument("--no-build", action="store_true", help="不自动 cargo build")

    setup = sub.add_parser("setup", help="下载 mediamtx、建 venv、编译设备客户端")
    setup.add_argument("--no-build", action="store_true", help="不自动 cargo build")

    sub.add_parser("down", help="按 pid 文件停掉上一轮 up")
    sub.add_parser("status", help="端口 / 进程 / 连通性")
    sub.add_parser("smoke", help="跑一遍隧道 / 中继 / AI / 证据图冒烟自测")

    args = parser.parse_args()
    if args.command == "up":
        return cmd_up(args)
    if args.command == "setup":
        return cmd_setup(args)
    if args.command == "down":
        return cmd_down(args)
    if args.command == "status":
        return cmd_status(args)
    if args.command == "smoke":
        return cmd_smoke(args)
    parser.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(main())
