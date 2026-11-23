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
- 版本 4：航点任务模式（plan mode）。除目标类别外再传一个**计划 id**
  （`plan_id` + `point_index` + `expect_count`）：命中就把证据图 + result.json
  落到 `detect_jobs/<plan_id>/pt<航点下标>/`，并按「检查目标数量」核对本帧
  检出的同类数量，不一致时 `plan_hit` 事件带 `count_alert=true`（调用方据此
  上报数量告警）。`expect_count` 为空 = 不核对数量。
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
import evidence   # 任务存证：命中帧画框 JPG + result.json
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


def _media_root(value):
    """中台下发的板端媒体目录：只接受绝对路径且不含 `..`，否则回退 config 兜底。"""
    raw = str(value or "").strip()
    if raw:
        if os.path.isabs(raw) and ".." not in raw.split(os.sep):
            return os.path.abspath(raw)
        print("[detect] 媒体目录不合法（%s），回退 %s"
              % (raw, config.MEDIA_DATA_DIR), flush=True)
    return os.path.abspath(config.MEDIA_DATA_DIR)


def _save_frames(value):
    """每航点固定保存帧数：1~10，非法值回退 config。"""
    try:
        frames = int(value)
    except (TypeError, ValueError):
        frames = 0
    if frames <= 0:
        frames = config.DETECT_PLAN_SAVE_FRAMES
    return max(1, min(10, frames))


def _capture_enabled(value):
    """中台下发的「到点自动采集」总开关（`media_capture_enabled`）。

    缺省 / 空串（老平台、老客户端、工具自测）一律当**开**，保持历史行为；
    显式 `false` / `0` / `"0"` / `"off"` 等才关——关时只写告警证据图，
    不再往媒体归档目录（`MEDIA_DATA_DIR`）攒训练图。
    """
    if value is None:
        return True
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return bool(value)
    text = str(value).strip().lower()
    if not text:
        return True
    return text not in ("0", "false", "off", "no", "否", "关", "关闭")


def _as_int(value):
    """安全转 int：None / 空串 / 非法值都返回 None（用于 plan 参数容错）。"""
    if value is None or (isinstance(value, str) and not value.strip()):
        return None
    try:
        return int(float(value))
    except (TypeError, ValueError):
        return None


def _count_by_label(boxes):
    """{类别名: 本帧数量}：存证 JSON 给平台一份可直接消费的各类数量。"""
    out = {}
    for b in boxes or []:
        key = b.get("label") or "?"
        out[key] = out.get(key, 0) + 1
    return out


# 任务类型：只判「有没有」，或判「数量对不对」
CHECK_EXIST = "exist"
CHECK_COUNT = "count"
_CHECK_TYPE_ALIASES = {
    "exist": CHECK_EXIST, "exists": CHECK_EXIST, "presence": CHECK_EXIST,
    "存在": CHECK_EXIST, "存在性": CHECK_EXIST, "检测是否存在": CHECK_EXIST,
    "count": CHECK_COUNT, "quantity": CHECK_COUNT,
    "数量": CHECK_COUNT, "数量校验": CHECK_COUNT, "检测数量是否正确": CHECK_COUNT,
}


def _norm_check_type(check_type, expect_count=None):
    """归一化任务类型：兼容旧调用（只传 expect_count 就按数量任务算）。

    - 显式 `check_type`（`exist` / `count`，也收中文与常见英文别名）优先；
    - 没传时按「有没有 expect_count」推断：给了数量就是数量任务，否则存在性任务。
    """
    raw = str(check_type or "").strip().lower()
    if raw:
        mode = _CHECK_TYPE_ALIASES.get(raw) or _CHECK_TYPE_ALIASES.get(str(check_type).strip())
        if mode:
            return mode
    return CHECK_COUNT if expect_count is not None else CHECK_EXIST


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
        # ---- 版本 3：任务化识别状态 ----
        # job = None 表示普通常驻识别（网页"开始识别"）；有值时是一次带 ID 的任务。
        self.job = None
        self._job_hits = {}             # {cam: 连续命中次数}，任一路达标即结束
        self._job_seq_lock = threading.Lock()
        # ---- 版本 4：航点计划模式 ----
        # plan = None 表示普通识别；有值时命中即按计划目录存证 + 数量核对。
        self.plan = None

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

    # ---------- 摄像头归一化（版本 3：恢复多路分时）----------
    @staticmethod
    def _norm_cams(cams):
        """保留所有合法路（去重、按 allowed_cams 顺序），红外不参与。

        NPU 单模型串行，多路在 _sched_loop 里分时轮询：每路实际帧率约等于
        DETECT_FPS / 路数。红外 dog-ipcam 不在允许列表里。空/全非法时回落默认路。
        """
        seq = cams if isinstance(cams, (list, tuple)) else [cams]
        # 保序去重，只留合法路
        seen = []
        for c in seq:
            if (c in config.DETECT_ALLOWED_CAMS and c in config.CAMERAS
                    and c not in seen):
                seen.append(c)
        if seen:
            return seen
        return [config.DETECT_DEFAULT_CAM
                if config.DETECT_DEFAULT_CAM in config.DETECT_ALLOWED_CAMS
                else config.DETECT_ALLOWED_CAMS[0]]

    # ---------- 抓帧：RTSP 旁路，常驻 ffmpeg 输出 MJPEG 管道 ----------
    def _rtsp_url(self, cam):
        info = config.CAMERAS.get(cam)
        stream = info["stream"] if info else cam
        # 前置分辨率档位：dogcam 实际拉 dogcam（直拉 1080p60）或
        # dogcam720（relay 重编码），逻辑路名 dogcam 不变。
        if cam == "dogcam":
            stream = relay.dogcam_stream()
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

    # ---------- 调度：单线程串行推理，多路分时复用 ----------
    def _sched_loop(self):
        i = 0
        # 上一轮「写帧 + DETECT + 拿到响应」的墙钟耗时。
        # 推理本身已经占用了这一轮的时间，节拍里必须把它减掉；
        # 若再按「推理耗时」额外 sleep 一次，等于把耗时算了两遍：
        # 单次 120ms 时每轮变成 120+126=246ms，四路只剩约 1fps/路。
        last_cost = 0.0
        while not self._stop_evt.is_set():
            # 任务模式：先检查是否超过识别时长（超时即结束并通知未命中）
            with self._lock:
                job = self.job
            if job and time.monotonic() - job["t_start"] >= job["duration"]:
                self._finish_job(found=False)
                return

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
                err = resp.get("error")
                # 任务模式：推理中断也作为任务失败结束，通知前端
                if job:
                    self._finish_job(found=False, error="推理中断: %s" % err)
                    return
                self._fail("推理中断: %s" % err)
                self.stop()
                return
            # 用墙钟耗时（含 C++ 的 imread / letterbox / 后处理 / 行协议），
            # 而不是只覆盖 detect() 的 infer_ms，否则节拍会被低估。
            last_cost = min(0.5, max(0.0, time.monotonic() - t0))
            boxes, counts = self._emit(cam, resp, ts or time.time())

            # 航点计划模式：命中即采样存证 + 按「检查目标数量」核对（见 _plan_frame）
            if boxes and self.plan:
                self._plan_frame(cam, jpg, boxes, counts, resp.get("frame") or {})

            # 任务模式：按路维护"连续命中"计数（同一路连续 N 帧命中才算找到，
            # 任一路未命中就清零该路计数），达标即结束并画框存证。
            if job:
                hit = bool(boxes)
                with self._lock:
                    cur = 0 if not hit else self._job_hits.get(cam, 0) + 1
                    self._job_hits[cam] = cur
                    streak = int(job["streak"])
                if hit:
                    self._emit_sse({"type": "job_progress", "job_id": job["id"],
                                    "cam": cam, "streak": cur, "need": streak,
                                    "boxes": len(boxes)})
                    if cur >= streak:
                        self._finish_job(found=True, cam=cam, boxes=boxes,
                                         jpg=jpg, frame=resp.get("frame") or {})
                        return

    def _finish_job(self, found, cam=None, boxes=None, jpg=None, frame=None, error=None):
        """任务收尾：命中则画框存证，未命中/失败只写 result.json + SSE 通知，然后停推理。"""
        with self._lock:
            job = dict(self.job) if self.job else None
        if not job:
            return
        jid = job["id"]
        elapsed = round(time.monotonic() - job["t_start"], 3)
        ts = time.strftime("%Y-%m-%dT%H:%M:%S%z")
        found_count = len(boxes or []) if found else 0
        count_mode = job.get("check_type") == CHECK_COUNT
        expect = job.get("expect_count") if count_mode else None
        # 数量任务里「没找到」= 数量不一致（0 ≠ N），所以未命中也要判
        count_ok = None if expect is None else (found_count == expect)
        # 存证目录：带计划 id 时落到 detect_jobs/<plan_id>/pt<下标>/（与航点窗口一致），
        # 否则仍用 detect_jobs/<job_id>/。
        out_dir = evidence.target_dir(job.get("plan_id"), job.get("point_index"), jid)
        result = {
            "job_id": jid,
            "plan_id": job.get("plan_id"),
            "point_index": job.get("point_index"),
            "check_type": job.get("check_type"),
            "found": bool(found),
            "task": job.get("task"),
            "labels": job.get("labels"),
            "cams": job.get("cams"),
            "conf": job.get("conf"),
            "duration": job["duration"],
            "streak": job["streak"],
            "elapsed_s": elapsed,
            "finished_at": ts,
            "found_cam": cam if found else None,
            "found_cam_title": config.CAMERAS.get(cam, {}).get("title") if found else None,
            "boxes": boxes if found else [],
            "counts": dict(_count_by_label(boxes or [])) if found else {},
            "found_count": found_count,
            "expect_count": expect,
            "count_ok": count_ok,
            "count_alert": count_ok is False,
        }
        result["dir"] = out_dir
        evidence_path = None
        if found and cam and jpg:
            fname = "%s_%s.jpg" % (cam, evidence._safe_job_id(jid))
            out = os.path.join(out_dir, fname)
            try:
                evidence.save_frame(
                    os.path.join(FRAME_DIR, cam + ".jpg"), boxes,
                    (frame.get("w") or config.CAPTURE_WIDTH) if frame else config.CAPTURE_WIDTH,
                    (frame.get("h") or config.CAPTURE_HEIGHT) if frame else config.CAPTURE_HEIGHT,
                    out)
                evidence_path = out
            except Exception as e:
                result["evidence_error"] = "画框存证失败: %s" % e
        result["evidence"] = evidence_path
        if error:
            result["error"] = error
        try:
            evidence.write_json(out_dir, result)
            # 按次留档：`result.json` 只保留「最新一份」，同一个航点每跑一轮都会被覆盖，
            # 现场想回看历史结论就只能靠这些按完成时间命名的副本（图片本来就是一图一次）。
            evidence.write_json(
                out_dir, result, "result_%s.json" % time.strftime("%Y%m%d_%H%M%S"))
        except Exception as e:
            self._emit_sse({"type": "job_error", "job_id": jid,
                            "msg": "写 result.json 失败: %s" % e})
        # 先清任务再 stop，避免 stop() 的 detect_clear 混淆任务状态
        with self._lock:
            self.job = None
            self._job_hits = {}
        self._emit_sse({
            "type": "job_done", "job_id": jid, "found": bool(found),
            "cam": cam, "cam_title": result["found_cam_title"],
            "plan_id": result["plan_id"], "point_index": result["point_index"],
            "check_type": result["check_type"],
            "found_count": found_count, "expect_count": expect,
            "count_ok": count_ok, "count_alert": result["count_alert"],
            "evidence": evidence_path, "elapsed_s": elapsed,
            "result_json": os.path.join(out_dir, "result.json"),
            "error": error,
        })
        # 任务结束后停止本轮推理（复用 stop，会关抓帧/OSD/推理进程并清空画面）
        self.stop()

    # ---------- 版本 4：航点计划模式（plan_id + 检查目标数量）----------
    def set_plan(self, plan_id=None, point_index=None, expect_count=None,
                 check_type=None, media_dir=None, save_frames=None,
                 capture_enabled=None):
        """登记航点计划（运行中也能调，不必重启识别）。

        `check_type` 两类任务：

        - `exist`（检测是否存在）：命中即存证；`expect_count` 不参与判定；
        - `count`（检测数量是否正确）：命中存证后把同类目标数与 `expect_count` 比对，
          不一致就发 `plan_hit.count_alert`；窗口结束仍未命中也算不一致（0 ≠ N），
          发 `plan_miss`。

        换计划/清计划时会把上一个还在采样的命中先收尾（不丢证据）。
        """
        plan = self._plan_set(plan_id, point_index, expect_count, check_type,
                              media_dir=media_dir, save_frames=save_frames,
                              capture_enabled=capture_enabled)
        return {"ok": True, "plan": None if not plan else {
            "plan_id": plan["id"],
            "point_index": plan["point_index"],
            "check_type": plan["check_type"],
            "expect_count": plan["expect_count"],
            "dir": evidence.plan_dir(plan["id"], plan["point_index"]),
            # 媒体归档目录（中台「同步媒体」按它扫描回传）
            "media_dir": evidence.media_plan_dir(
                plan["id"], plan["point_index"], root=plan["media_dir"]),
            "save_frames": plan["save_frames"],
            "capture_enabled": plan["capture_enabled"],
        }}

    def clear_plan(self):
        """清掉航点计划（窗口结束但识别继续跑时用，避免后续命中写错目录）。"""
        with self._lock:
            prev = self.plan
        self._plan_set(None)
        return {"ok": True, "cleared": prev["id"] if prev else None}

    def _plan_set(self, plan_id=None, point_index=None, expect_count=None,
                  check_type=None, media_dir=None, save_frames=None,
                  capture_enabled=None):
        pid = "" if plan_id is None else str(plan_id).strip()
        if not pid:
            self._plan_flush("计划结束")
            with self._lock:
                self.plan = None
            return None
        # 换计划前先把上一个还在采样的命中收尾（证据不丢）
        self._plan_flush("计划切换")
        expect = _as_int(expect_count)
        if expect is not None:
            expect = max(1, min(999, expect))
        # 没传 check_type 时用旧调用口径推断：给了数量就是数量任务
        mode = _norm_check_type(check_type, expect)
        if mode != CHECK_COUNT:
            # 存在性任务：即使误传了数量也不报数量告警
            expect = None
        count_missing = mode == CHECK_COUNT and expect is None
        if count_missing:
            print("[detect] 计划 %s 是数量任务但没填检查目标数量，按存在性判定" % pid[:80],
                  flush=True)
        plan = {
            "id": pid[:80],
            "point_index": _as_int(point_index),
            "check_type": mode,
            "expect_count": expect,
            "count_missing": count_missing,
            # 媒体归档目录与固定帧数：中台参数配置随 ai_start 下发
            # （客户端转成 media_dir / save_frames），缺省用板端 config 兜底。
            "media_dir": _media_root(media_dir),
            "save_frames": _save_frames(save_frames),
            # 到点自动采集总开关（中台 `media_capture_enabled`，默认关）：关时
            # 只写 detect_jobs 告警证据图，不攒媒体归档（训练数据）。
            "capture_enabled": _capture_enabled(capture_enabled),
            "sampling": None,
            "done": False,
            "t_start": time.monotonic(),
        }
        with self._lock:
            self.plan = plan
        return plan

    def _plan_frame(self, cam, jpg, boxes, counts, frame):
        """计划模式的一帧命中：采样 `DETECT_PLAN_COUNT_SAMPLE` 帧后收尾。

        为什么采样：数量核对只看一帧时，遮挡 / 运动模糊少画一个框就会误报
        「数量不一致」。这里在同一路连采几帧，取检出数量最大的那一帧作为结论
        （存证图也用那一帧），单帧少框不会影响结论。
        """
        total = len(boxes)
        final = None
        with self._lock:
            plan = self.plan
            if not plan or plan["done"]:
                return
            save_frames = max(1, min(10, int(plan.get("save_frames") or
                                              config.DETECT_PLAN_SAVE_FRAMES)))
            # 采样帧数取「数量核对需要的帧数」与「媒体归档要留的帧数」的较大者：
            # 存 3 帧就不能只采 1 帧，否则媒体资源里永远只有一张图。
            need = max(1, int(config.DETECT_PLAN_COUNT_SAMPLE), save_frames)
            record = {
                "jpg": jpg,
                "boxes": boxes,
                "counts": dict(counts or {}),
                "total": total,
                "frame": dict(frame or {}),
            }
            st = plan["sampling"]
            if st is None:
                st = {
                    "cam": cam,
                    "left": need - 1,
                    # 本次窗口内看到的命中帧（最多 save_frames 张），媒体归档用
                    "frames": [record],
                    "best_total": total,
                    "best_counts": dict(counts or {}),
                    "best_boxes": boxes,
                    "best_jpg": jpg,
                    "best_frame": dict(frame or {}),
                }
                plan["sampling"] = st
                if st["left"] > 0:
                    return
            elif st["cam"] != cam:
                # 另一路的采样还没采完：以先命中的那一路为准，不抢同一份存证
                return
            else:
                st["frames"].append(record)
                del st["frames"][:-save_frames]
                if total >= st["best_total"]:
                    st.update(best_total=total, best_counts=dict(counts or {}),
                              best_boxes=boxes, best_jpg=jpg,
                              best_frame=dict(frame or {}))
                st["left"] -= 1
                if st["left"] > 0:
                    return
            plan["sampling"] = None
            plan["done"] = True
            final = (plan, st)
        self._plan_write(final[0], final[1], "命中目标")

    def _plan_flush(self, reason="窗口结束"):
        """把采样中的命中收尾；没命中也要收尾（未命中同样写 result.json）。"""
        final = None
        miss = None
        with self._lock:
            plan = self.plan
            if plan and not plan.get("done"):
                st = plan.get("sampling")
                plan["sampling"] = None
                plan["done"] = True
                if st:
                    final = (plan, st)
                else:
                    miss = plan
        if final:
            self._plan_write(final[0], final[1], reason)
        elif miss:
            self._plan_miss(miss, reason)

    def _plan_write_media(self, plan, st, result):
        """把本轮命中帧写进媒体归档目录（中台「同步媒体」取图）。

        固定保存 `save_frames` 帧（取采样窗口里看到的命中帧，按检出数量降序，
        第一张就是结论帧），**一律存原始帧、不画框**：媒体归档是中台「媒体资源」
        里的训练数据，叠加层（框 + 文字）会污染标注；画框的那份留在
        `detect_jobs/<plan>/pt<n>/` 当告警证据（`evidence.save_frame`）。

        采集总开关（中台 `media_capture_enabled`）关着时直接跳过：告警证据图与
        数量核对不受影响，只是不攒训练数据。

        返回 `(媒体航点目录, 文件名列表)`；一张都没写成时目录视为不存在。
        """
        if not plan.get("capture_enabled", True):
            return None, []
        frames = list(st.get("frames") or [])
        if not frames:
            return None, []
        root = plan.get("media_dir") or config.MEDIA_DATA_DIR
        out_dir = evidence.media_plan_dir(plan["id"], plan.get("point_index"), root=root)
        limit = max(1, min(10, int(plan.get("save_frames") or
                                  config.DETECT_PLAN_SAVE_FRAMES)))
        # 结论帧（检出数最大）排最前：观看端第一张就是检出最全的那帧
        frames.sort(key=lambda item: int(item.get("total") or 0), reverse=True)
        frames = frames[:limit]
        cam = st.get("cam") or "cam"
        stamp = time.strftime("%Y%m%d_%H%M%S")
        written = []
        for seq, item in enumerate(frames, start=1):
            jpg = item.get("jpg") or b""
            if not jpg:
                continue
            name = "%s_%02d_%s.jpg" % (cam, seq, stamp)
            out = os.path.join(out_dir, name)
            try:
                evidence.save_raw(jpg, out)
                written.append(name)
            except Exception as e:
                print("[detect] 媒体归档写图失败 %s: %s" % (out, e), flush=True)
        if not written:
            return None, []
        try:
            evidence.write_json(
                out_dir,
                dict(result, media_dir=out_dir, media_frames=written,
                     type="plan_media"))
        except Exception as e:
            print("[detect] 媒体归档 result.json 写入失败: %s" % e, flush=True)
        return out_dir, written

    def _plan_miss(self, plan, reason):
        """窗口结束仍未命中：写 found:false 的结果；数量任务另发 `plan_miss`。

        数量任务（检测数量是否正确）里「一个都没找到」等于数量不一致（0 ≠ N），
        所以 `plan_miss` 带 `count_alert=true`，由调用方决定要不要转成告警
        （窗口被任务结束/新航点提前打断时不该算账，那是调用方判断的事）。
        """
        expect = plan.get("expect_count")
        count_mode = plan.get("check_type") == CHECK_COUNT
        warned = bool(count_mode and expect is not None)
        out_dir = evidence.plan_dir(plan["id"], plan.get("point_index"))
        result = {
            "type": "plan_miss",
            "plan_id": plan["id"],
            "point_index": plan.get("point_index"),
            "check_type": plan.get("check_type"),
            "found": False,
            "found_count": 0,
            "counts": {},
            "expect_count": expect,
            "count_ok": (False if warned else None),
            "count_alert": warned,
            "task": self.cfg["task"],
            "labels": self.cfg["labels"],
            "conf": self.cfg["conf"],
            "cams": list(self.cfg["cams"]),
            "elapsed_s": round(time.monotonic() - plan["t_start"], 3),
            "finished_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "reason": reason,
            "dir": out_dir,
            "evidence": None,
        }
        try:
            result["result_json"] = evidence.write_json(out_dir, result)
        except Exception as e:
            result["write_error"] = "写 result.json 失败: %s" % e
        self._emit_sse(dict(result))
        return result

    def _plan_write(self, plan, st, reason):
        """命中帧画框存证到 `<jobs>/<plan_id>/pt<下标>/`，按数量核对出结论并发 SSE。"""
        cam = st["cam"]
        boxes = st.get("best_boxes") or []
        counts = st.get("best_counts") or {}
        found_count = int(st.get("best_total") or 0)
        count_mode = plan.get("check_type") == CHECK_COUNT
        expect = plan.get("expect_count") if count_mode else None
        count_ok = None if expect is None else (found_count == expect)
        frame = st.get("best_frame") or {}
        out_dir = evidence.plan_dir(plan["id"], plan.get("point_index"))
        fname = "%s_%s.jpg" % (cam, time.strftime("%Y%m%d_%H%M%S"))
        out = os.path.join(out_dir, fname)
        evidence_path = None
        evidence_error = ""
        try:
            # 存证用的是「检出数量最大的那一帧」的原始 JPEG，可能已经不是最新的
            # /tmp 帧文件，所以先把它落成临时文件再交给画框流程。
            src = os.path.join(FRAME_DIR, "_plan_best.jpg")
            os.makedirs(FRAME_DIR, exist_ok=True)
            with open(src, "wb") as f:
                f.write(st.get("best_jpg") or b"")
            evidence.save_frame(
                src, boxes,
                frame.get("w") or config.CAPTURE_WIDTH,
                frame.get("h") or config.CAPTURE_HEIGHT,
                out)
            evidence_path = out
        except Exception as e:
            evidence_error = "画框存证失败: %s" % e
        result = {
            "type": "plan_hit",
            "plan_id": plan["id"],
            "point_index": plan.get("point_index"),
            "check_type": plan.get("check_type"),
            "found": True,
            "task": self.cfg["task"],
            "labels": self.cfg["labels"],
            "conf": self.cfg["conf"],
            "cams": list(self.cfg["cams"]),
            "found_cam": cam,
            "found_cam_title": config.CAMERAS.get(cam, {}).get("title"),
            "found_count": found_count,
            "counts": dict(counts),
            "expect_count": expect,
            "count_ok": count_ok,
            "count_alert": count_ok is False,
            "boxes": boxes,
            "elapsed_s": round(time.monotonic() - plan["t_start"], 3),
            "finished_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "reason": reason,
            "dir": out_dir,
            "evidence": evidence_path,
        }
        # 媒体归档：本次固定保存的 N 帧写进 MEDIA_DATA_DIR/<plan>/pt<n>/，
        # 中台下发「同步媒体」后由设备客户端逐张回传并删除。这一步失败不影响
        # 企业微信证据图那条主链路（只是媒体资源里少几张）。
        try:
            media_dir, media_frames = self._plan_write_media(plan, st, result)
        except Exception as e:
            media_dir, media_frames = None, []
            result["media_error"] = "媒体归档失败: %s" % e
        result["media_dir"] = media_dir
        result["media_frames"] = media_frames
        if evidence_error:
            result["evidence_error"] = evidence_error
        try:
            result["result_json"] = evidence.write_json(out_dir, result)
        except Exception as e:
            result["write_error"] = "写 result.json 失败: %s" % e
        # SSE：`plan_hit` 只发一次（每个计划/航点一份存证），调用方据此上报
        # 「找到目标」与「数量不一致告警」。框数据也带上，页面可以直接回放。
        self._emit_sse({k: v for k, v in result.items() if k != "boxes"})
        return result

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
        # 返回（过滤后的框, 本帧各类数量）：任务/计划状态机据此判断命中与数量
        return boxes, counts

    # ---------- 对外：启停 / 配置 / 立即识别 ----------
    def start(self, task=None, labels=None, conf=None, cams=None, fps=None,
              plan_id=None, point_index=None, expect_count=None, check_type=None,
              media_dir=None, save_frames=None, capture_enabled=None):
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

        # 航点任务：登记计划（计划 id / 航点下标 / 任务类型 / 检查目标数量）。
        # 命中即按计划目录存证并核对数量；传空即清掉旧计划（普通「开始识别」与计划模式互斥）。
        self._plan_set(plan_id, point_index, expect_count, check_type,
                       media_dir=media_dir, save_frames=save_frames,
                       capture_enabled=capture_enabled)

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

    def submit_job(self, job_id=None, task=None, labels=None, conf=None,
                   cams=None, fps=None, duration=None, streak=None,
                   plan_id=None, point_index=None, expect_count=None,
                   check_type=None):
        """版本 3：下发一次带 ID 的识别任务。

        复用 start() 的推理链路，但记录任务参数与开始时间；调度线程按"同一路
        连续 N 帧命中即结束"或"超时未命中"自动收尾并通知前端。

        入参校验：job_id 为空时自动生成；duration/streak 限幅防呆。
        """
        with self._lock:
            if self.state in ("starting", "running"):
                return {"ok": False, "error": "识别已在运行，请先停止或等待当前任务结束"}

        jid = str(job_id or "").strip() or ("job_%s" % time.strftime("%Y%m%d_%H%M%S"))
        try:
            dur = int(duration if duration is not None else config.DETECT_JOB_DURATION_DEFAULT)
        except (TypeError, ValueError):
            dur = config.DETECT_JOB_DURATION_DEFAULT
        dur = max(1, min(config.DETECT_JOB_DURATION_MAX, dur))
        try:
            sk = int(streak if streak is not None else config.DETECT_JOB_HIT_STREAK)
        except (TypeError, ValueError):
            sk = config.DETECT_JOB_HIT_STREAK
        sk = max(1, min(config.DETECT_JOB_HIT_STREAK_MAX, sk))
        expect = _as_int(expect_count)
        if expect is not None:
            expect = max(1, min(999, expect))
        mode = _norm_check_type(check_type, expect)
        if mode != CHECK_COUNT:
            expect = None      # 存在性任务不判数量
        pid = "" if plan_id is None else str(plan_id).strip()

        norm = self._norm_cams(cams if cams else config.DETECT_CAMS)
        task = task or (scan_tasks()["tasks"][0]["task"] if scan_tasks()["tasks"] else "")
        # 先登记任务，再启动推理（调度线程起来后就能看到 job）
        with self._lock:
            self.job = {
                "id": jid, "task": task,
                "labels": list(labels) if labels else None,
                "cams": list(norm), "conf": conf,
                "duration": dur, "streak": sk,
                "plan_id": pid[:80] or None,
                "point_index": _as_int(point_index),
                "expect_count": expect,
                "check_type": mode,
                "t_start": time.monotonic(),
            }
            self._job_hits = {}
        r = self.start(task=task, labels=labels, conf=conf, cams=norm, fps=fps)
        if not r.get("ok"):
            with self._lock:
                self.job = None
                self._job_hits = {}
            return r
        self._emit_sse({"type": "job_started", "job_id": jid,
                        "task": task, "cams": list(norm),
                        "plan_id": pid or None, "point_index": _as_int(point_index),
                        "expect_count": expect, "check_type": mode,
                        "duration": dur, "streak": sk})
        return {"ok": True, "job": {
            "job_id": jid, "task": task, "cams": list(norm),
            "duration": dur, "streak": sk,
            "plan_id": pid or None, "point_index": _as_int(point_index),
            "expect_count": expect, "check_type": mode,
            "result_dir": evidence.target_dir(pid, point_index, jid),
        }, "status": self.status()}

    def cancel_job(self):
        """手动取消当前任务（等同 stop，但发一条 job 取消事件）。"""
        with self._lock:
            jid = self.job.get("id") if self.job else None
        r = self.stop()
        if jid:
            self._emit_sse({"type": "job_cancelled", "job_id": jid})
        return r

    @staticmethod
    def list_jobs():
        """列出已落盘的任务/计划结果（所有 result.json），按修改时间倒序。

        目录可能有一层（`<job_id>/`）或两层（`<plan_id>/pt<下标>/`），
        所以递归走一遍而不是只列一级目录。
        """
        root = config.DETECT_JOBS_DIR
        out = []
        if not os.path.isdir(root):
            return {"count": 0, "jobs": out}
        for dirpath, _dirnames, filenames in os.walk(root):
            if "result.json" not in filenames:
                continue
            rp = os.path.join(dirpath, "result.json")
            try:
                with open(rp, "r", encoding="utf-8") as f:
                    data = json.load(f)
                data["_mtime"] = os.path.getmtime(rp)
                data["_dir"] = dirpath
                out.append(data)
            except Exception:
                continue
        out.sort(key=lambda x: x.get("_mtime", 0), reverse=True)
        return {"count": len(out), "jobs": out}

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
        # 航点计划：先把还在采样的命中收尾（存证 + 数量核对 + plan_hit），再关流。
        # 必须放在 stop_grabbers 之前：此时推理配置（场景/类别/相机）还没被清。
        self._plan_flush("识别结束")
        was = self.state
        with self._lock:
            self.state = "stopped"
            self.error = ""
            self.plan = None
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

    def configure(self, task=None, labels=None, conf=None, cams=None, fps=None,
                  plan_id=None, point_index=None, expect_count=None, check_type=None,
                  media_dir=None, save_frames=None, capture_enabled=None):
        """运行中热更新：类别/阈值零成本；换场景走 SWITCH；改摄像头/帧率重启抓帧。

        `plan_id` 不为 None 时顺便登记/清掉航点计划（空串 = 清），这样「操作员
        已经开着识别」时也能用 `/api/detect/config` 带上计划，不必重启识别。
        """
        if self.state != "running":
            return {"ok": False, "error": "识别未运行，请先点「开始识别」"}
        if plan_id is not None:
            self._plan_set(plan_id, point_index, expect_count, check_type,
                           media_dir=media_dir, save_frames=save_frames,
                           capture_enabled=capture_enabled)
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
                # 多路分时（版本 3）：single_cam=false 表示前端允许多选
                "single_cam": bool(config.DETECT_SINGLE_CAM),
                "multi_cam": not config.DETECT_SINGLE_CAM,
                "allowed_cams": list(config.DETECT_ALLOWED_CAMS),
                "default_cam": config.DETECT_DEFAULT_CAM,
                "preview_stream": config.OSD_STREAM,
                "osd": osd.status(),
                # 框上文字的格式，平台 / 前端可据此解释 box["text"]
                # 示例与 _emit() 的拼接规则保持一致，开关/单位变了状态栏也跟着变
                "label_format": {
                    "lang": "zh",
                    "show_count": bool(config.DETECT_SHOW_COUNT),
                    "show_conf": bool(config.DETECT_SHOW_CONF),
                    "unit": config.DETECT_LABEL_UNIT,
                    "example": " ".join(x for x in (
                        "火焰",
                        ("3" + config.DETECT_LABEL_UNIT) if config.DETECT_SHOW_COUNT else "",
                        "0.90" if config.DETECT_SHOW_CONF else "",
                    ) if x),
                },
                # 狗前置链路：前端据此解释「机器狗前置卡片为什么没画面 / AI 目标为何被改」
                "relay": relay.status(),
                # 版本 3：当前任务快照（job=None 表示普通常驻识别）
                "job": ({
                    "id": self.job["id"],
                    "task": self.job.get("task"),
                    "cams": list(self.job.get("cams") or []),
                    "duration": self.job["duration"],
                    "streak": self.job["streak"],
                    "elapsed_s": round(time.monotonic() - self.job["t_start"], 2),
                    "hits": dict(self._job_hits),
                    "plan_id": self.job.get("plan_id"),
                    "point_index": self.job.get("point_index"),
                    "expect_count": self.job.get("expect_count"),
                    "result_dir": evidence.target_dir(
                        self.job.get("plan_id"), self.job.get("point_index"),
                        self.job["id"]),
                } if self.job else None),
                # 版本 4：航点计划快照（plan=None 表示当前不是计划模式）
                "plan": ({
                    "plan_id": self.plan["id"],
                    "point_index": self.plan.get("point_index"),
                    "check_type": self.plan.get("check_type"),
                    "expect_count": self.plan.get("expect_count"),
                    "done": bool(self.plan.get("done")),
                    "sampling": bool(self.plan.get("sampling")),
                    "elapsed_s": round(time.monotonic() - self.plan["t_start"], 2),
                    "dir": evidence.plan_dir(self.plan["id"], self.plan.get("point_index")),
                } if self.plan else None),
            }


manager = DetectManager()
