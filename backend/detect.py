"""AI 识别：模型包扫描 / 契约校验 / 常驻推理调度（阶段 0 ~ 2）

分层：
  1) scan_tasks()         —— 阶段 0：只读 rknn/models/<task>/，静态契约校验，
                              不加载模型、不占 NPU、不进 telemetry.snapshot()。
  2) DetectManager        —— 阶段 1/2：按需启动 C++ --serve 常驻进程 + ffmpeg
                              RTSP 旁路抽帧。版本 2 起识别结果交给 osd.py 在
                              背包本地烧进视频；SSE 坐标仍保留（事件落盘/排错/回退）。
                              默认关闭，不启动任何线程，不影响首屏与 5s 自动刷新。

模型交付契约（由他人提供模型时遵守）：
    rknn/models/<task>/
        model.rknn    必须，target=rk3576、FP16、输入 [1,640,640,3] NHWC
        labels.txt    必须，每行一个类别名，顺序 = 模型输出通道顺序
        meta.json     可选，纯展示配置（中文名/颜色/忽略类/阈值/告警）

关键设计：
- 检测走「旁路」：不改 stream.py / ffmpeg 主链路 / RTSP / WebRTC，不重新编码视频。
  ffmpeg 从 rtsp://127.0.0.1:<RTSP_PORT>/<cam> 抽帧，避开 v4l2 设备被独占(EBUSY)。
- C++ 侧模型只 rknn_init 一次；网页上勾选类别/调阈值纯后端过滤，零模型重载。
- 一个推理队列串行访问 --serve，天然避免多进程抢 NPU 双核。
- 版本 2：AI 固定「五路单选」（默认 dogcam），NPU 独占一路；推理结果由
  osd.py 在板端画框后推 preview 流。osd 不可用时前端自动回退浏览器叠加。
- 版本 2.2：每个框的展示文字在 _emit() 里统一生成（中文名 + 同类数量，
  例「火焰 3处」），同一份 box["text"] 供板端 OSD、SSE、JSONL 和浏览器
  SVG 回退使用；事件另带 counts = {类别名: 本帧数量}。
"""
import os
import json
import time
import queue
import shutil
import threading
import subprocess

import config
import osd
import relay   # 只读 dogcam relay 状态，供前端一次轮询判断「狗前置是否有源」

MODELS_DIR = config.DETECT_MODELS_DIR
BACKEND_DIR = os.path.dirname(os.path.abspath(__file__))
BIN = config.DETECT_BIN
FRAME_DIR = config.DETECT_FRAME_DIR
EVENTS_FILE = config.DETECT_EVENTS_FILE
SERVE_LOG = os.path.join(FRAME_DIR, "serve.log")

_CACHE = {"sig": None, "data": None}
_LOCK = threading.Lock()
_FILE_LOCK = threading.Lock()

_WATCHED_FILES = ("model.rknn", "labels.txt", "meta.json")


# ======================================================================
# 阶段 0：模型包扫描与静态契约校验（原样保留）
# ======================================================================
def _signature():
    """目录签名：新增/删除/改动任一文件即变化，无需重启即可自动发现新场景。"""
    try:
        entries = []
        for name in sorted(os.listdir(MODELS_DIR)):
            d = os.path.join(MODELS_DIR, name)
            if not os.path.isdir(d):
                continue
            entries.append((name, "<dir>", 0, 0))
            for fn in _WATCHED_FILES:
                p = os.path.join(d, fn)
                if os.path.isfile(p):
                    st = os.stat(p)
                    entries.append((name, fn, st.st_size, int(st.st_mtime)))
        return tuple(entries)
    except Exception:
        return None


def _read_labels(path):
    """与 C++ init() 一致：只去掉行尾 CR/LF，保留类别名中的空格。"""
    with open(path, "r", encoding="utf-8") as f:
        raw = [line.rstrip("\r\n") for line in f]
    return [line for line in raw if line.strip() != ""]


def _scan_one(task_dir, task):
    issues = []
    model_path = os.path.join(task_dir, "model.rknn")
    labels_path = os.path.join(task_dir, "labels.txt")
    meta_path = os.path.join(task_dir, "meta.json")

    has_model = os.path.isfile(model_path)
    if not has_model:
        issues.append("缺少 model.rknn")
        if os.path.isfile(os.path.join(task_dir, "model_rk3588.rknn")):
            issues.append("只有 model_rk3588.rknn，疑似放错平台（当前板子为 rk3576）")

    labels = []
    if not os.path.isfile(labels_path):
        issues.append("缺少 labels.txt")
    else:
        try:
            labels = _read_labels(labels_path)
        except Exception as e:
            issues.append("labels.txt 读取失败: %s" % e)
        if os.path.isfile(labels_path) and not labels:
            issues.append("labels.txt 为空")
        if len(set(labels)) != len(labels):
            issues.append("labels.txt 存在重复行")

    meta = {}
    if os.path.isfile(meta_path):
        try:
            with open(meta_path, "r", encoding="utf-8") as f:
                meta = json.load(f)
            if not isinstance(meta, dict):
                raise ValueError("顶层必须是对象")
        except Exception as e:
            issues.append("meta.json 不是合法 JSON: %s" % e)
            meta = {}

    label_map = meta.get("labels") or {}
    if not isinstance(label_map, dict):
        issues.append("meta.json 的 labels 必须是对象")
        label_map = {}
    for key in label_map:
        if key not in labels:
            issues.append("meta.labels 引用了不存在的标签: %s" % key)

    for field in ("ignore", "alarm"):
        seq = meta.get(field) or []
        if not isinstance(seq, list):
            issues.append("meta.%s 必须是数组" % field)
            continue
        for item in seq:
            if item not in labels:
                issues.append("meta.%s 引用了不存在的标签: %s" % (field, item))

    target = meta.get("target", "rk3576")
    if target != "rk3576":
        issues.append("模型目标平台标记为 %s，当前板子为 rk3576" % target)

    ignored = set(meta.get("ignore") or [])
    out_labels = []
    for i, name in enumerate(labels):
        cfg = label_map.get(name) if isinstance(label_map.get(name), dict) else {}
        out_labels.append({
            "id": i,
            "name": name,
            "zh": cfg.get("zh") or name,
            "color": cfg.get("color"),
            "ignored": name in ignored,
        })

    nc = len(labels)
    return {
        "task": task,
        "name": meta.get("name") or task,
        "desc": meta.get("desc") or "",
        "nc": nc,
        "labels": out_labels,
        "conf": meta.get("conf", 0.25),
        "alarm": meta.get("alarm") or [],
        "target": target,
        "expected_n_elems": (4 + nc) * 8400,  # 首次加载模型时做张量级校验
        "model_size": os.path.getsize(model_path) if has_model else 0,
        "ok": len(issues) == 0,
        "issues": issues,
    }


def scan_tasks(force=False):
    """返回已发现场景清单；结果按目录签名缓存，读盘开销可忽略。"""
    sig = _signature()
    with _LOCK:
        if not force and sig is not None and _CACHE["sig"] == sig and _CACHE["data"] is not None:
            return _CACHE["data"]

    tasks = []
    error = None
    if sig is None:
        error = "模型目录不存在或不可读: %s" % MODELS_DIR
    else:
        names = sorted({e[0] for e in sig})
        for name in names:
            try:
                tasks.append(_scan_one(os.path.join(MODELS_DIR, name), name))
            except Exception as e:
                tasks.append({
                    "task": name, "name": name, "desc": "", "nc": 0,
                    "labels": [], "conf": 0.25, "alarm": [], "target": "rk3576",
                    "expected_n_elems": 0, "model_size": 0,
                    "ok": False, "issues": ["扫描异常: %s" % e],
                })

    data = {
        "models_dir": MODELS_DIR,
        "count": len(tasks),
        "ready": sum(1 for t in tasks if t["ok"]),
        "stage": 2,
        "note": ("选择场景与类别后点「开始识别」：五路中只能选一路（默认机器狗前置），"
                 "框在背包本地烧进视频后由该路画面直接显示；板端画框未就绪时自动回退浏览器叠加"),
        "tasks": tasks,
    }
    if error:
        data["error"] = error

    with _LOCK:
        _CACHE["sig"] = sig
        _CACHE["data"] = data
    return data


def _find_task(task):
    for t in scan_tasks()["tasks"]:
        if t["task"] == task:
            return t
    return None


# ======================================================================
# 阶段 1/2：常驻检测管理器
# ======================================================================
class DetectManager:
    """默认关闭。start() 后才创建进程/线程；stop() 全部回收。

    C++ --serve 行协议（stdin 一行命令 -> stdout 恰好一行 JSON）：
        READY                    进程启动后主动输出，含模型契约信息
        PING                     心跳
        SWITCH <task>            热切换模型
        DETECT <id> <jpg_path>   推理一帧
        QUIT                     退出
    """

    def __init__(self):
        self._lock = threading.RLock()
        self._io_lock = threading.Lock()
        self._proc = None
        self._log_fh = None
        self._resp_q = queue.Queue()
        self._reader = None
        self._grabbers = {}
        self._sched = None
        self._stop_evt = threading.Event()
        self._publish = None
        self._seq = 0
        self.state = "stopped"           # stopped / starting / running / error
        self.error = ""
        self.warnings = []
        self.model = {}
        self._task_info = None
        self.cfg = {
            "task": "",
            "labels": None,             # None = 全部未忽略类别；否则为英文标签名列表
            "conf": config.DETECT_CONF,
            "cams": self._norm_cams(config.DETECT_CAMS),
            "fps": config.DETECT_FPS,
        }
        self.stats = {}

    # ---------- 对外：注入 SSE 发布函数 ----------
    def set_publisher(self, fn):
        self._publish = fn

    def _emit_sse(self, msg):
        fn = self._publish
        if fn:
            try:
                fn(msg)
            except Exception:
                pass

    # ---------- C++ --serve 通信 ----------
    def _reader_loop(self, proc):
        while True:
            try:
                line = proc.stdout.readline()
            except Exception:
                break
            if not line:
                self._resp_q.put({"ok": False, "error": "推理服务已退出"})
                return
            line = line.strip()
            if not line:
                continue
            try:
                self._resp_q.put(json.loads(line))
            except Exception:
                self._resp_q.put({"ok": False, "error": "非法响应: %s" % line[:120]})

    def _request(self, line, timeout=15):
        """串行访问 serve：写一行命令 -> 从读取线程队列取一行 JSON（带超时）。"""
        with self._io_lock:
            if self._proc is None or self._proc.poll() is not None:
                return {"ok": False, "error": "推理服务未运行"}
            while True:
                try:
                    self._resp_q.get_nowait()
                except queue.Empty:
                    break
            try:
                self._proc.stdin.write(line + "\n")
                self._proc.stdin.flush()
            except Exception as e:
                return {"ok": False, "error": "写命令失败: %s" % e}
            try:
                return self._resp_q.get(timeout=timeout)
            except queue.Empty:
                return {"ok": False, "error": "推理服务 %ss 无响应" % timeout}

    @staticmethod
    def _check_contract(model, task_info):
        """张量级校验：只在首次加载/切换模型时执行一次（不拖慢启动）。"""
        warnings = []
        exp = int(task_info["expected_n_elems"] or 0)
        got = int(model.get("n_elems") or 0)
        if exp and got != exp:
            nc = int(task_info["nc"] or 0)
            anchors = got // (4 + nc) if (4 + nc) else 0
            return False, (
                "模型输出 n_elems=%d，但 labels.txt 有 %d 类，期望 %d（(4+%d)×8400）；"
                "按实际输出反推 anchors=%d，标准 YOLOv8@640 应为 8400。"
                "若 anchors≠8400，多半是模型输入尺寸不是 640（需同步 --imgsz）；"
                "若 anchors=8400 但 n_elems 不符，多半是类别数与 labels.txt 不一致"
                % (got, nc, exp, nc, anchors)), warnings
        fmt = str(model.get("in_fmt") or "").upper()
        typ = str(model.get("in_type") or "").upper()
        if fmt and fmt != "NHWC":
            warnings.append("模型输入布局为 %s（期望 NHWC）" % fmt)
        if typ and "FP16" not in typ and "FLOAT16" not in typ:
            warnings.append("模型输入类型为 %s（期望 FP16）" % typ)
        return True, "", warnings

    def _fail(self, msg):
        with self._lock:
            self.state = "error"
            self.error = msg
        self._emit_sse({"type": "detect_error", "msg": msg})

    # ---------- 单路约束（版本 2）----------
    @staticmethod
    def _norm_cams(cams):
        """无论前端传几路，只保留第一个合法项；非法/空则回落到默认 dogcam。

        NPU 独占一路，避免四路分时把每路帧率摊薄；也为板端 OSD 只需要
        一条预览流提供前提。红外 dog-ipcam 不在允许列表里。
        """
        seq = cams if isinstance(cams, (list, tuple)) else [cams]
        for c in seq:
            if c in config.DETECT_ALLOWED_CAMS and c in config.CAMERAS:
                return [c]
        return [config.DETECT_DEFAULT_CAM
                if config.DETECT_DEFAULT_CAM in config.DETECT_ALLOWED_CAMS
                else config.DETECT_ALLOWED_CAMS[0]]

    # ---------- 抓帧：RTSP 旁路，常驻 ffmpeg 输出 MJPEG 管道 ----------
    def _rtsp_url(self, cam):
        info = config.CAMERAS.get(cam)
        stream = info["stream"] if info else cam
        return "rtsp://%s:%d/%s" % (config.DETECT_RTSP_HOST, config.RTSP_PORT, stream)

    def _grab_loop(self, cam):
        """每路一个线程：ffmpeg 抽帧 -> 按 JPEG SOI/EOI 切出完整帧 -> 存最新帧。

        不走磁盘轮询，避免读写半张图；mediamtx 未就绪时自动重连。
        """
        slot = self._grabbers.get(cam)
        if not slot:
            return
        # 每路自带停止标志：configure() 改帧率/摄像头时只停本路，
        # 不影响调度线程与 C++ --serve 进程。
        my_stop = slot["stop"]
        while not my_stop.is_set():
            # 抓帧与推理节拍解耦：ffmpeg 持续输出「最新帧」，调度线程按自己的
            # 节拍取 slot["frame"]。抓帧只需略快于实际推理速率即可；
            # 原先取 2×目标帧率（最高 24fps × 4 路 = 96 次/秒 MJPEG 编码）
            # 会和 C++ 的 letterbox / 后处理抢 CPU，把 infer_ms 一起拖高。
            # 实测四路分时推理上限约 2~4fps/路，12fps 抓帧已把滞后压到 ~80ms。
            grab_fps = max(8, min(12, int(self.cfg["fps"])))
            cmd = [
                "ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin",
                # RTSP 低延迟：不做输入缓冲，不等待长时间流探测
                "-fflags", "nobuffer", "-flags", "low_delay",
                "-analyzeduration", "0", "-probesize", "32768",
                "-rtsp_transport", "tcp", "-i", self._rtsp_url(cam),
                "-an", "-vf", "fps=%d" % grab_fps,
                "-c:v", "mjpeg", "-q:v", "6",
                "-f", "image2pipe", "-vcodec", "mjpeg", "pipe:1",
            ]
            try:
                p = subprocess.Popen(cmd, stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL, bufsize=0)
            except Exception as e:
                with self._lock:
                    s = self._grabbers.get(cam)
                    if s:
                        s["error"] = "ffmpeg 启动失败: %s" % e
                if my_stop.wait(2.0):
                    break
                continue
            with self._lock:
                if slot:
                    slot["proc"] = p
            buf = b""
            fd = p.stdout.fileno()
            while not my_stop.is_set():
                try:
                    chunk = os.read(fd, 65536)
                except Exception:
                    break
                if not chunk:
                    break
                buf += chunk
                while True:
                    s = buf.find(b"\xff\xd8")
                    if s < 0:
                        buf = b""
                        break
                    e = buf.find(b"\xff\xd9", s + 2)
                    if e < 0:
                        buf = buf[s:]
                        break
                    jpg = buf[s:e + 2]
                    buf = buf[e + 2:]
                    with self._lock:
                        if slot:
                            slot["frame"] = jpg
                            slot["ts"] = time.time()
            try:
                p.terminate()
                p.wait(timeout=2)
            except Exception:
                try:
                    p.kill()
                except Exception:
                    pass
            try:
                p.stdout.close()
            except Exception:
                pass
            if my_stop.wait(1.0):
                break

    def _start_grabbers(self):
        os.makedirs(FRAME_DIR, exist_ok=True)
        with self._lock:
            self._grabbers = {
                cam: {"proc": None, "thread": None, "frame": None, "ts": 0,
                      "error": "", "stop": threading.Event()}
                for cam in self.cfg["cams"]
            }
            for cam, slot in self._grabbers.items():
                t = threading.Thread(target=self._grab_loop, args=(cam,),
                                     name="detect-grab-" + cam, daemon=True)
                slot["thread"] = t
                t.start()

    def _stop_grabbers(self):
        with self._lock:
            slots = list(self._grabbers.values())
            procs = [s["proc"] for s in slots if s.get("proc")]
            threads = [s["thread"] for s in slots if s.get("thread")]
        # 先置各自的停止标志，避免线程在 terminate 后又重连一个新 ffmpeg
        for s in slots:
            s["stop"].set()
        for p in procs:
            try:
                p.terminate()
            except Exception:
                pass
        for t in threads:
            if t is not threading.current_thread():
                t.join(timeout=2)
        with self._lock:
            self._grabbers = {}

    @staticmethod
    def _write_frame(path, jpg):
        tmp = path + ".tmp"
        with open(tmp, "wb") as f:
            f.write(jpg)
        os.replace(tmp, path)

    def _grab_once(self, cam):
        """「立即识别」用：抓一帧到文件（不依赖常驻抓帧线程）。"""
        os.makedirs(FRAME_DIR, exist_ok=True)
        path = os.path.join(FRAME_DIR, cam + "_once.jpg")
        cmd = [
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-y",
            "-rtsp_transport", "tcp", "-i", self._rtsp_url(cam),
            "-frames:v", "1", "-q:v", "3", "-f", "image2", path,
        ]
        try:
            r = subprocess.run(cmd, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, timeout=15)
        except Exception as e:
            return None, "抓帧失败: %s" % e
        if r.returncode != 0 or not os.path.isfile(path):
            return None, "抓帧失败（mediamtx 是否已启动？RTSP: %s）" % self._rtsp_url(cam)
        return path, ""

    # ---------- 调度：单线程串行推理，四路分时复用 ----------
    def _sched_loop(self):
        i = 0
        # 上一轮「写帧 + DETECT + 拿到响应」的墙钟耗时。
        # 推理本身已经占用了这一轮的时间，节拍里必须把它减掉；
        # 若再按「推理耗时」额外 sleep 一次，等于把耗时算了两遍：
        # 单次 120ms 时每轮变成 120+126=246ms，四路只剩约 1fps/路。
        last_cost = 0.0
        while not self._stop_evt.is_set():
            with self._lock:
                cams = list(self.cfg["cams"])
                fps = max(1, int(self.cfg["fps"]))
            if not cams:
                self._stop_evt.wait(0.3)
                continue
            # 补偿式节拍：目标间隔减去上一轮已花掉的时间，只 sleep 剩余部分。
            # 推理慢于目标节拍时 interval=0，直接连跑，不会积压请求。
            interval = max(0.0, 1.0 / (fps * len(cams)) - last_cost)
            if interval and self._stop_evt.wait(interval):
                break
            if self._stop_evt.is_set():
                break
            cam = cams[i % len(cams)]
            i += 1
            with self._lock:
                slot = self._grabbers.get(cam)
                jpg = slot["frame"] if slot else None
                ts = slot["ts"] if slot else 0
            if not jpg:
                self._stop_evt.wait(0.1)   # 没帧时别空转烧 CPU
                continue
            path = os.path.join(FRAME_DIR, cam + ".jpg")
            t0 = time.monotonic()
            try:
                self._write_frame(path, jpg)
            except Exception:
                continue
            self._seq += 1
            resp = self._request("DETECT %d %s" % (self._seq, path), timeout=15)
            if not resp.get("ok"):
                self._fail("推理中断: %s" % resp.get("error"))
                self.stop()
                return
            # 用墙钟耗时（含 C++ 的 imread / letterbox / 后处理 / 行协议），
            # 而不是只覆盖 detect() 的 infer_ms，否则节拍会被低估。
            last_cost = min(0.5, max(0.0, time.monotonic() - t0))
            self._emit(cam, resp, ts or time.time())

    def _emit(self, cam, resp, ts):
        """过滤 + 中文映射 -> SSE detect 事件；有检出才落盘。"""
        with self._lock:
            info = self._task_info or {}
            conf = float(self.cfg["conf"])
            enabled = self.cfg["labels"]
        lmap = {l["name"]: l for l in (info.get("labels") or [])}
        boxes = []
        for b in resp.get("boxes") or []:
            meta = lmap.get(b.get("label"), {})
            if meta.get("ignored"):
                continue
            if enabled is not None and b.get("label") not in enabled:
                continue
            try:
                score = float(b.get("score") or 0)
            except Exception:
                continue
            if score < conf:
                continue
            color = meta.get("color") or "#E24B4A"
            if not (isinstance(color, str) and len(color) == 7 and color[0] == "#"):
                color = "#E24B4A"
            boxes.append({
                "label": b.get("label"),
                "zh": meta.get("zh") or b.get("label"),
                "color": color,
                "conf": round(score, 3),
                "x1": b.get("x1"), "y1": b.get("y1"),
                "x2": b.get("x2"), "y2": b.get("y2"),
            })
            if len(boxes) >= config.DETECT_MAX_BOXES:
                break

        # ---- 同类计数 + 统一标签文字（版本 2.2）----
        # 数量 = 当前这一帧「通过类别/阈值过滤且实际画出」的同类框数。
        # 只在这里算一次并写进每个 box，板端 OSD、SSE、JSONL 落盘、浏览器
        # SVG 回退都直接取 box["text"]，四处显示必然一致。
        # 注意：受 DETECT_MAX_BOXES 截断影响，是「可见框数」而非模型原始检出数。
        counts = {}
        for b in boxes:
            key = b["label"]
            counts[key] = counts.get(key, 0) + 1
        for b in boxes:
            n = counts[b["label"]]
            b["count"] = n
            parts = [str(b["zh"] or b["label"])]
            if config.DETECT_SHOW_COUNT:
                parts.append("%d%s" % (n, config.DETECT_LABEL_UNIT))
            if config.DETECT_SHOW_CONF:
                parts.append("%.2f" % float(b["conf"]))
            b["text"] = " ".join(parts)

        frame = resp.get("frame") or {}
        event = {
            "type": "detect", "cam": cam, "ts": ts,
            "frame_w": frame.get("w") or config.CAPTURE_WIDTH,
            "frame_h": frame.get("h") or config.CAPTURE_HEIGHT,
            "task": resp.get("task") or self.cfg["task"],
            "infer_ms": resp.get("infer_ms"),
            "boxes": boxes,
            # {模型原始类别名: 本帧数量}，平台可直接消费，无需自己再数框
            "counts": counts,
        }
        self._emit_sse(event)
        # 版本 2：把框交给板端 OSD 烧进视频；osd 未启用/未就绪时它自己忽略，
        # SSE 事件仍然发出（落盘、排错、以及浏览器回退叠加都还要用）。
        osd.update(cam, boxes, event["frame_w"], event["frame_h"])

        now = time.time()
        with self._lock:
            st = self.stats.setdefault(cam, {"frames": 0, "hits": 0, "fps": 0.0,
                                             "last_ms": 0, "last": 0.0, "ts": 0})
            if st["last"]:
                inst = 1.0 / max(0.001, now - st["last"])
                st["fps"] = round(st["fps"] * 0.7 + inst * 0.3, 1) if st["fps"] else round(inst, 1)
            st["last"] = now
            st["frames"] += 1
            st["hits"] += len(boxes)
            st["last_ms"] = event["infer_ms"] or 0
            st["ts"] = now

        if boxes:
            rec = {"ts": ts, "cam": cam, "task": event["task"],
                   "infer_ms": event["infer_ms"], "boxes": boxes}
            try:
                with _FILE_LOCK:
                    with open(EVENTS_FILE, "a", encoding="utf-8") as f:
                        f.write(json.dumps(rec, ensure_ascii=False) + "\n")
            except Exception:
                pass

    # ---------- 对外：启停 / 配置 / 立即识别 ----------
    def start(self, task=None, labels=None, conf=None, cams=None, fps=None):
        with self._lock:
            if self.state in ("starting", "running"):
                return {"ok": False, "error": "识别已在运行，请先停止"}
            self.state = "starting"
            self.error = ""
            self.warnings = []

        if not os.path.isfile(BIN) or not os.access(BIN, os.X_OK):
            self._fail("找不到可执行程序 %s（需在板端 cmake --build build 编译）" % BIN)
            self.state = "stopped"
            return {"ok": False, "error": self.error}
        if shutil.which("ffmpeg") is None:
            self._fail("ffmpeg 不可用，无法从 RTSP 旁路抽帧")
            self.state = "stopped"
            return {"ok": False, "error": self.error}

        task = task or self.cfg["task"] or ""
        info = _find_task(task)
        if not info:
            self._fail("未找到场景 %s（检查 rknn/models/ 目录）" % task)
            self.state = "stopped"
            return {"ok": False, "error": self.error}
        if not info["ok"]:
            self._fail("场景 %s 未通过校验: %s" % (task, "；".join(info["issues"])))
            self.state = "stopped"
            return {"ok": False, "error": self.error}

        with self._lock:
            self.cfg.update({
                "task": task,
                "labels": list(labels) if labels else None,
                "conf": float(conf if conf is not None else (info.get("conf") or config.DETECT_CONF)),
                # 单路约束：传多个也只取第一个；默认 dogcam（机器狗前置 RTSP）
                "cams": self._norm_cams(cams if cams else self.cfg["cams"]),
                "fps": max(1, min(config.DETECT_FPS_MAX, int(fps or self.cfg["fps"]))),
            })
            self._task_info = info
            self.stats = {}

        os.makedirs(FRAME_DIR, exist_ok=True)
        cmd = [BIN, "--serve", "--task", task, "--models-root", MODELS_DIR]
        try:
            self._log_fh = open(SERVE_LOG, "ab", buffering=0)
            proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=self._log_fh, text=True, bufsize=1,
                                    cwd=os.path.dirname(BIN) or None)
        except Exception as e:
            self._fail("启动推理服务失败: %s" % e)
            self.state = "stopped"
            return {"ok": False, "error": self.error}

        with self._lock:
            self._proc = proc
            self._stop_evt = threading.Event()
        # 清掉上一轮 stop 时读取线程可能残留的「服务已退出」消息，
        # 否则下面的 READY 等待会误读旧消息。
        while True:
            try:
                self._resp_q.get_nowait()
            except queue.Empty:
                break
        self._reader = threading.Thread(target=self._reader_loop, args=(proc,),
                                        name="detect-serve-reader", daemon=True)
        self._reader.start()

        # READY 由进程主动输出，直接等它（此时还没有其它请求占用响应队列）
        try:
            ready = self._resp_q.get(timeout=30)
        except queue.Empty:
            ready = {"ok": False, "error": "模型加载超时（30s）"}
        if not ready.get("ok"):
            self._cleanup_proc()
            self._fail("模型加载失败: %s" % ready.get("error"))
            self.state = "stopped"
            return {"ok": False, "error": self.error}

        model = ready.get("model") or {}
        ok, err, warns = self._check_contract(model, info)
        if not ok:
            self._cleanup_proc()
            self._fail(err)
            self.state = "stopped"
            return {"ok": False, "error": err}

        with self._lock:
            self.model = model
            self.warnings = warns
            self.state = "running"

        self._start_grabbers()
        # 版本 2：板端本地画框。失败只记日志，不中断识别 —— 前端看到
        # status.osd.running=false 会自动回退到浏览器 SVG 叠加。
        try:
            if not osd.start(self.cfg["cams"][0]):
                print("[detect] 板端画框未就绪，回退浏览器叠加: %s"
                      % osd.status().get("error", ""), flush=True)
        except Exception as e:
            print("[detect] 板端画框启动异常（不影响识别）: %s" % e, flush=True)
        self._sched = threading.Thread(target=self._sched_loop,
                                       name="detect-sched", daemon=True)
        self._sched.start()
        self._emit_sse({"type": "detect_state", "running": True, "task": task,
                        "cam": self.cfg["cams"][0], "osd": osd.status()})
        return {"ok": True, "status": self.status()}

    def _cleanup_proc(self):
        proc = self._proc
        if proc:
            try:
                proc.stdin.write("QUIT\n")
                proc.stdin.flush()
            except Exception:
                pass
            try:
                proc.wait(timeout=3)
            except Exception:
                try:
                    proc.kill()
                except Exception:
                    pass
        if self._log_fh:
            try:
                self._log_fh.close()
            except Exception:
                pass
            self._log_fh = None
        with self._lock:
            self._proc = None

    def stop(self):
        was = self.state
        with self._lock:
            self.state = "stopped"
            self.error = ""
        self._stop_evt.set()
        self._stop_grabbers()
        # 关掉板端画框编码器，preview 流随之消失，前端把卡片切回干净流
        try:
            osd.stop()
        except Exception:
            pass
        # 调度线程内部出错时也会调用 stop()，不能 join 自己
        if (self._sched and self._sched.is_alive()
                and self._sched is not threading.current_thread()):
            self._sched.join(timeout=2)
        self._cleanup_proc()
        with self._lock:
            self.model = {}
            self._task_info = None
        # 发一条全局清空事件，前端立即擦掉所有残留框并切回干净流
        self._emit_sse({"type": "detect_clear", "osd": osd.status()})
        if was != "stopped":
            self._emit_sse({"type": "detect_state", "running": False})
        return {"ok": True, "status": self.status()}

    def configure(self, task=None, labels=None, conf=None, cams=None, fps=None):
        """运行中热更新：类别/阈值零成本；换场景走 SWITCH；改摄像头/帧率重启抓帧。"""
        if self.state != "running":
            return {"ok": False, "error": "识别未运行，请先点「开始识别」"}
        with self._lock:
            old = dict(self.cfg)
        switched = False

        if task and task != old["task"]:
            info = _find_task(task)
            if not info or not info["ok"]:
                return {"ok": False, "error": "场景 %s 不可用" % task}
            resp = self._request("SWITCH %s" % task, timeout=30)
            if not resp.get("ok"):
                self._fail("切换场景失败: %s" % resp.get("error"))
                self.stop()
                return {"ok": False, "error": self.error}
            ok, err, warns = self._check_contract(resp.get("model") or {}, info)
            if not ok:
                self._fail(err)
                self.stop()
                return {"ok": False, "error": err}
            with self._lock:
                self.cfg["task"] = task
                self.cfg["labels"] = None      # 换场景后类别勾选重置为全选
                self._task_info = info
                self.model = resp.get("model") or {}
                self.warnings = warns
                self.stats = {}
            switched = True

        with self._lock:
            # 换场景当次忽略前端传来的 labels：它可能属于旧场景，套到新场景会全部过滤掉
            if labels is not None and not switched:
                self.cfg["labels"] = list(labels) if labels else None
            if conf is not None:
                self.cfg["conf"] = float(conf)
            new_cams = self._norm_cams(cams if cams is not None else old["cams"])
            new_fps = max(1, min(config.DETECT_FPS_MAX,
                                 int(fps if fps is not None else old["fps"])))
            self.cfg["cams"] = new_cams
            self.cfg["fps"] = new_fps
            changed = (new_cams != old["cams"] or new_fps != old["fps"])
            cam_changed = (new_cams != old["cams"])

        if changed:
            self._stop_grabbers()
            self._start_grabbers()
        # 运行中换 AI 目标路：preview 编码器要换输入源，重启一次即可，
        # 前端仍盯着同一个 preview path，不需要重新协商 WHEP。
        if cam_changed:
            try:
                osd.stop()
                osd.start(new_cams[0])
            except Exception as e:
                print("[detect] 切换 AI 路时板端画框重启失败: %s" % e, flush=True)
        return {"ok": True, "status": self.status()}

    def once(self, cam=None, task=None, conf=None, labels=None):
        """[调试接口，网页已不暴露] 抓当前帧识别一次。
        检测未开启时走一次性 CLI，不启动常驻服务；场景/类别/阈值临时套用，
        不改运行配置。仅供排错（例如不启常驻验证模型是否能加载）。
        """
        cam = self._norm_cams(cam or config.DETECT_DEFAULT_CAM)[0]
        if cam not in config.CAMERAS:
            return {"ok": False, "error": "不支持的摄像头 %s" % cam}
        path, err = self._grab_once(cam)
        if err:
            return {"ok": False, "error": err}
        if not os.path.isfile(BIN) or not os.access(BIN, os.X_OK):
            return {"ok": False, "error": "找不到可执行程序 %s" % BIN}

        with self._lock:
            cur_task = self.cfg["task"] or (scan_tasks()["tasks"][0]["task"]
                                            if scan_tasks()["tasks"] else "")
        task = task or cur_task
        info = _find_task(task)
        if not info:
            return {"ok": False, "error": "未找到场景 %s" % task}
        if not info["ok"]:
            return {"ok": False, "error": "场景 %s 未通过校验" % task}

        if self.state == "running" and self._proc is not None:
            self._seq += 1
            resp = self._request("DETECT %d %s" % (self._seq, path), timeout=20)
            if not resp.get("ok"):
                return {"ok": False, "error": resp.get("error") or "推理失败"}
            self._emit(cam, resp, time.time())
            return {"ok": True, "boxes": len(resp.get("boxes") or []),
                    "infer_ms": resp.get("infer_ms")}

        # 常驻服务未运行：一次性 CLI。--conf 给很低的值做粗筛，
        # 精确阈值由 _emit 按前端选择过滤，避免 C++ 提前把低分框丢掉。
        cmd = [BIN, "--task", task, "--models-root", MODELS_DIR,
               "--conf", "0.05", "--input", path, "--json"]
        try:
            r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               text=True, timeout=40, cwd=os.path.dirname(BIN) or None)
        except Exception as e:
            return {"ok": False, "error": "一次性推理失败: %s" % e}
        lines = (r.stdout or "").strip().splitlines()
        try:
            resp = json.loads(lines[-1]) if lines else {}
        except Exception:
            return {"ok": False, "error": "推理输出不是合法 JSON: %s" % (r.stdout or "")[:120]}
        if not resp.get("frame"):
            return {"ok": False, "error": "推理未返回有效结果（检查 serve.log）"}

        with self._lock:
            saved = dict(self.cfg)
            saved_info = self._task_info
            self.cfg["task"] = task
            if conf is not None:
                self.cfg["conf"] = float(conf)
            self.cfg["labels"] = list(labels) if labels else None
            self._task_info = info
        try:
            self._emit(cam, resp, time.time())
        finally:
            with self._lock:
                self.cfg = saved
                self._task_info = saved_info
        return {"ok": True, "boxes": len(resp.get("boxes") or []),
                "infer_ms": resp.get("infer_ms")}

    def status(self):
        with self._lock:
            proc_alive = self._proc is not None and self._proc.poll() is None
            grab = {c: {"has_frame": bool(s.get("frame")),
                        "error": s.get("error") or ""}
                    for c, s in self._grabbers.items()}
            return {
                "running": self.state == "running" and proc_alive,
                "state": self.state,
                "task": self.cfg["task"],
                "labels": self.cfg["labels"],
                "conf": self.cfg["conf"],
                "cams": list(self.cfg["cams"]),
                "fps": self.cfg["fps"],
                "model": dict(self.model),
                "warnings": list(self.warnings),
                "stats": {k: dict(v) for k, v in self.stats.items()},
                "grabbers": grab,
                "bin": BIN,
                "bin_exists": os.path.isfile(BIN) and os.access(BIN, os.X_OK),
                "models_dir": MODELS_DIR,
                "events_file": EVENTS_FILE,
                "serve_log": SERVE_LOG,
                "error": self.error,
                # 版本 2：单路约束与板端画框状态，前端据此决定切 preview 还是回退 SVG
                "single_cam": bool(config.DETECT_SINGLE_CAM),
                "allowed_cams": list(config.DETECT_ALLOWED_CAMS),
                "default_cam": config.DETECT_DEFAULT_CAM,
                "preview_stream": config.OSD_STREAM,
                "osd": osd.status(),
                # 框上文字的格式，平台 / 前端可据此解释 box["text"]
                "label_format": {
                    "lang": "zh",
                    "show_count": bool(config.DETECT_SHOW_COUNT),
                    "show_conf": bool(config.DETECT_SHOW_CONF),
                    "unit": config.DETECT_LABEL_UNIT,
                    "example": "火焰 3处",
                },
                # 狗前置链路：前端据此解释「第 6 张卡为什么没画面 / AI 目标为何被改」
                "relay": relay.status(),
            }


manager = DetectManager()
