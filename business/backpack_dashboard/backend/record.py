"""航点录像 / 航段全程录像（板端落盘，客户端触发）

巡检路线里的两个录像动作最终都在这里落地：

```text
路线「航点动作 → 录像采集」  → 客户端到点 POST /api/record/start {scope:"point", seconds:15}
路线「行进任务 → 全程录像」  → 客户端随任务 POST /api/record/start {scope:"task"}
                               任务终态/取消  POST /api/record/stop
```

- **谁触发**：设备客户端（与 Agent 同机，`127.0.0.1:8000`）。平台/浏览器不直接调：
  录像属于任务执行的一部分，起停时机只有 5009 的状态机知道。
- **源**：本机 mediamtx 上的流，路线动作里按需选（`cameras: [...]`，多选 = 多路
  同时录）。可选 `dogcam`（机器狗前置 / 正前，按当前档位选 `dogcam`/`dogcam720`）、
  `cam0`~`cam3`（背包四路 USB：左前 / 右前 / 右后 / 左后）、`dog-ipcam`（红外），
  与航点拍照同一份相机名；不配就退到 `RECORD_SOURCE_CAM`（默认 `dogcam`）。
  **不碰 v4l2**，也不影响实时视频与 AI 抽帧。
- **多路**：每一路一个独立 ffmpeg 进程（`-c copy`，互不影响），文件名带相机名，
  任何一路起不来只丢那一路，其余照录。
- **编码**：`-c copy` 直接封装 H264（源已经是 H264），不重编码 —— RK3576 上多一路
  编码会跟实时流抢 MPP；只有确实没有关键帧时才由 `RECORD_TRANSCODE=1` 兜底。
- **落盘**：`RECORD_DIR/<planId>/<kind>-<相机>-<时间戳>.mp4` + 同名 `.json` 边车
  （planId / 航点 / 范围 / 相机 / 起止时间 / 时长）。客户端传完调 `/api/record/delete` 删。
  断电/断网没传走的由 `sweep()` 按 `RECORD_RETENTION_HOURS` 清掉，不会写满 SSD。

为什么要切片：全程录像一跑可能几十分钟，4G 上一次传几百 MB 必然断。`scope=task`
用 ffmpeg 的 segment 每 `RECORD_SEGMENT_SECS`（默认 300s）切一段，客户端逐段落盘后
一段一段传；任何一段失败只影响那一段。
"""
import json
import os
import re
import shutil
import signal
import subprocess
import threading
import time

import config
import relay

# 文件名 / 目录名白名单：计划号来自平台（`TK-1112-11`），只认字母数字汉字 - _
_SAFE_NAME = re.compile(r"^[0-9A-Za-z_\-\u4e00-\u9fff]+$")
# 边车里不需要对外的字段（源地址含凭据，绝不进接口 / 日志）
_SCRUB = ("cmd",)

_LOCK = threading.RLock()
# job key -> _Job；key = "<planId>|<scope>|<pointIndex>|<camera>"（每一路一个 job）
_JOBS = {}
# 已经在收尾（ffmpeg 退出但边车还没写）的文件名：`list` 不返回它们，避免传半截
_FINALIZING = set()
_SWEEP_STARTED = False


def _log(msg):
    print("[record] %s" % msg, flush=True)


def _ffmpeg_bin():
    return os.getenv("FFMPEG_BIN", "ffmpeg")


def _job_key(plan_id, scope, point_index, camera):
    return "%s|%s|%d|%s" % (plan_id, scope, point_index, camera)


def _safe_name(value):
    text = str(value or "").strip()
    if not text or len(text) > 120 or not _SAFE_NAME.match(text):
        return None
    return text


def _int(value, default=0, low=0, high=None):
    try:
        number = int(value)
    except (TypeError, ValueError):
        number = default
    if number < low:
        number = low
    if high is not None and number > high:
        number = high
    return number


def allowed_cameras():
    """可录的流：背包四路 + 机器狗前置（含 720p 档）+ 红外。

    与 `photo.py::allowed_cameras` 同一份口径（路线编辑器也用这套相机名），
    改这里记得同步那边。
    """
    names = set(getattr(config, "CAMERAS", {}).keys())
    names.update({"dogcam", "dogcam720", "dog-ipcam"})
    return names


def stream_of(camera):
    """路线动作里的相机名 → mediamtx 上的 path（`dogcam` 按当前档位选流）。"""
    cam = str(camera or config.RECORD_SOURCE_CAM or "dogcam").strip()
    if cam not in allowed_cameras():
        return None
    if cam == "dogcam":
        # 与航点拍照 / AI 抽帧同一套解析：1080p60 档直接用 dogcam，
        # 720p30 档用 relay 推上来的 dogcam720
        return relay.dogcam_stream()
    return cam


def source_url(camera=None):
    """单路录像源：本机 mediamtx 上的流（与 AI 抽帧同一套解析）。

    源 URL 可能含账号密码（`DOG_RTSP_URL`），只用于 ffmpeg 参数，**不进接口/日志**。
    相机名不认识时返回 None（调用方据此拒绝这一路）。
    """
    stream = stream_of(camera or config.RECORD_SOURCE_CAM)
    if not stream:
        return None
    return "rtsp://%s:%d/%s" % (config.RECORD_RTSP_HOST, config.RTSP_PORT, stream)


def cameras_of(body):
    """请求里的相机列表：`cameras: [...]`，兼容单个 `camera`；去重 + 白名单 + 截断。

    返回 `(picked, rejected)`：不认识的相机名只丢这一路，并在回执里带上，
    不能让一个打字错误把整条录像废掉（现场改配置比改任务容易）。
    一个都没给 / 全都不认识时退到 `RECORD_SOURCE_CAM`（默认 dogcam）。
    """
    raw = body.get("cameras")
    if isinstance(raw, str):
        raw = [raw]
    if not isinstance(raw, list) or not raw:
        single = body.get("camera")
        raw = [single] if single else []
    allowed = allowed_cameras()
    picked = []
    rejected = []
    for item in raw:
        cam = str(item or "").strip()
        if not cam:
            continue
        if cam not in allowed:
            rejected.append(cam)
            continue
        if cam in picked:
            continue
        picked.append(cam)
    if not picked:
        fallback = str(config.RECORD_SOURCE_CAM or "dogcam").strip()
        picked = [fallback if fallback in allowed else "dogcam"]
    max_cams = max(1, int(getattr(config, "RECORD_MAX_CAMERAS", 4)))
    if len(picked) > max_cams:
        rejected.extend(picked[max_cams:])
        picked = picked[:max_cams]
    return picked, rejected


def _plan_dir(plan_id):
    return os.path.join(config.RECORD_DIR, plan_id)


def _disk_free_mb():
    try:
        usage = shutil.disk_usage(config.RECORD_DIR)
        return usage.free // (1024 * 1024)
    except OSError:
        # 目录还不存在（第一次录像）：看它的父目录
        parent = os.path.dirname(config.RECORD_DIR.rstrip("/")) or "/"
        try:
            return shutil.disk_usage(parent).free // (1024 * 1024)
        except OSError:
            return -1


def _ffmpeg_cmd(path_template, scope, seconds, segment_seconds, source):
    """构造一路 ffmpeg 命令。

    - `scope=point`：`-t seconds` 自动停，一个 mp4；
    - `scope=task`：`-f segment` 按 `segment_seconds` 切段（路线动作里的
      `recordSegmentSeconds`，缺省 `RECORD_SEGMENT_SECS`），总时长上限
      `RECORD_TASK_MAX_SECS`（客户端一定会发 stop，这里只是防跑飞）。
    """
    cmd = [
        _ffmpeg_bin(), "-hide_banner", "-loglevel", "warning", "-nostdin",
        # 4G / 弱网下 TCP 比 UDP 稳；实时视频那条路也是 TCP
        "-rtsp_transport", "tcp", "-i", source,
        "-t", str(seconds),
    ]
    if os.getenv("RECORD_TRANSCODE", "0") == "1":
        cmd += ["-c:v", "libx264", "-preset", "veryfast", "-crf", "26",
                "-c:a", "aac", "-b:a", "64k"]
    else:
        # 源已是 H264：只封装，不重编码（省 CPU / 不抢 MPP）
        cmd += ["-c", "copy", "-an"]
    if scope == "task":
        cmd += [
            "-f", "segment",
            "-segment_time", str(segment_seconds),
            "-reset_timestamps", "1",
            "-segment_format", "mp4",
            # 片段也要 faststart：客户端上传后浏览器边下边播
            "-segment_format_options", "movflags=+faststart",
            "-strftime", "1",
        ]
    else:
        cmd += ["-movflags", "+faststart"]
    cmd += ["-y", path_template]
    return cmd


class _Job:
    def __init__(self, key, plan_id, scope, point_index, point_name, camera,
                 path_template, prefix, before, started_at, cmd):
        self.key = key
        self.plan_id = plan_id
        self.scope = scope
        self.point_index = point_index
        self.point_name = point_name
        self.camera = camera
        self.path_template = path_template
        # 本路录像的文件名前缀（`pt<n>-` / `task-`）：同一计划可能同时有全程 + 航点两路，
        # 只按「开始后新出现的文件」认会互相抢对方的片段
        self.prefix = prefix
        # 开始前目录里已有的文件名：收尾时用「差集」认自己写出来的（segment 会生成多个，
        # 文件名带的是各段自己的时间戳，靠前缀认会把上一轮没传走的录像一起吞进来）
        self.before = set(before)
        self.started_at = started_at
        self.started_ts = time.time()
        self.cmd = cmd
        self.proc = None
        self.stopped_at = None
        self.finished = threading.Event()
        self.files = []

    def status(self):
        return {
            "planId": self.plan_id,
            "scope": self.scope,
            "pointIndex": self.point_index,
            "pointName": self.point_name,
            "camera": self.camera,
            "startedAt": _fmt(self.started_at),
            "seconds": round(time.time() - self.started_ts, 1),
            "running": self.proc is not None and self.proc.poll() is None,
            "files": list(self.files),
        }


def _fmt(ts):
    return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(ts))


def _now_stamp():
    return time.strftime("%Y%m%d-%H%M%S", time.localtime())


def _files_of(job):
    """这一轮写出来的 mp4：现在目录里的 - 开始前目录里的（segment 会有多个）。"""
    folder = os.path.dirname(job.path_template)
    try:
        names = sorted(os.listdir(folder))
    except OSError:
        return []
    found = []
    for name in names:
        if (
            name in job.before
            or not name.endswith(".mp4")
            or not name.startswith(job.prefix)
        ):
            continue
        path = os.path.join(folder, name)
        try:
            if os.path.getsize(path) <= 0:
                continue
        except OSError:
            continue
        found.append(path)
    return found


def _finalize(job, reason):
    """ffmpeg 退出后写边车：客户端按边车里的元数据回传，不用再猜文件名。"""
    with _LOCK:
        if job.stopped_at is not None:
            return
        job.stopped_at = time.time()
        job.files = _files_of(job)
        _FINALIZING.update(os.path.basename(path) for path in job.files)
    duration = max(0.0, job.stopped_at - job.started_ts)
    for path in job.files:
        meta = {
            "planId": job.plan_id,
            "scope": job.scope,
            "pointIndex": job.point_index,
            "pointName": job.point_name,
            "camera": job.camera,
            "startedAt": _fmt(job.started_at),
            "endedAt": _fmt(job.stopped_at),
            "durationSec": round(duration, 1),
            "size": os.path.getsize(path) if os.path.exists(path) else 0,
            "recordedOn": "backpack",
            "reason": reason,
        }
        try:
            with open(path + ".json", "w", encoding="utf-8") as handle:
                json.dump(meta, handle, ensure_ascii=False)
        except OSError as error:
            _log("写边车失败（%s）: %s" % (path, error))
    with _LOCK:
        _FINALIZING.difference_update(os.path.basename(path) for path in job.files)
        _JOBS.pop(job.key, None)
        job.finished.set()
    _log("录像结束 plan=%s scope=%s point=%s cam=%s 时长=%.1fs 文件=%d 原因=%s"
         % (job.plan_id, job.scope, job.point_index, job.camera, duration,
            len(job.files), reason))


def _watch(job):
    try:
        job.proc.wait()
    finally:
        reason = "到点自动停" if job.scope == "point" else "任务结束"
        _finalize(job, reason)


def start(body):
    """开始一段录像（幂等：同一计划同一航点同一路相机重复调用返回正在跑的那一路）。

    `body`：`{planId, scope: "point"|"task", pointIndex, pointName, seconds,
    segmentSeconds, cameras: ["cam0", "dogcam"], camera}`。

    `seconds` 只对 `point` 有意义（上限 `RECORD_POINT_MAX_SECS`）。
    `cameras` 是多选：**每一路一个独立 ffmpeg + 一个独立 mp4**，任何一路起不来
    只丢那一路（回执里带 `errors`），其余照录；不配就退到 `RECORD_SOURCE_CAM`。
    """
    plan_id = _safe_name(body.get("planId"))
    if not plan_id:
        return {"ok": False, "error": "planId 缺失或不合法（只认字母数字汉字 - _）"}
    scope = "task" if str(body.get("scope") or "").lower() == "task" else "point"
    point_index = _int(body.get("pointIndex"), -1, -1)
    if scope == "point" and point_index < 0:
        return {"ok": False, "error": "航点录像必须带 pointIndex"}
    point_name = str(body.get("pointName") or "")[:60]
    if scope == "point":
        seconds = _int(body.get("seconds"), config.RECORD_POINT_DEFAULT_SECS,
                       3, config.RECORD_POINT_MAX_SECS)
        segment_seconds = seconds
    else:
        # 全程录像：`segmentSeconds` 是切片长度（路线动作可配），`maxSeconds` 是兜底总上限
        segment_seconds = _int(body.get("segmentSeconds"), config.RECORD_SEGMENT_SECS,
                               10, config.RECORD_TASK_MAX_SECS)
        seconds = _int(body.get("maxSeconds"), config.RECORD_TASK_MAX_SECS,
                       30, config.RECORD_TASK_MAX_SECS)
    cameras, rejected = cameras_of(body)

    started = []
    skipped = []
    errors = []
    jobs = []
    with _LOCK:
        # 磁盘护栏：宁可这段不录，也不要把 SSD 写满（写满会连 AI 存证一起丢）
        free = _disk_free_mb()
        if free >= 0 and free < config.RECORD_MIN_FREE_MB:
            return {
                "ok": False,
                "error": "板端剩余空间不足（%dMB < %dMB），跳过录像"
                         % (free, config.RECORD_MIN_FREE_MB),
            }

        out_dir = _plan_dir(plan_id)
        try:
            os.makedirs(out_dir, exist_ok=True)
        except OSError as error:
            return {"ok": False, "error": "创建录像目录失败: %s" % error}

        stamp = _now_stamp()
        kind = "pt%d" % point_index if scope == "point" else "task"
        # 一次快照：本轮所有相机的文件都在它之后落盘（各相机靠文件名前缀区分）
        try:
            before = set(os.listdir(out_dir))
        except OSError:
            before = set()

        for cam in cameras:
            source = source_url(cam)
            if not source:
                errors.append("%s: 不支持的相机" % cam)
                continue
            key = _job_key(plan_id, scope, point_index, cam)
            existing = _JOBS.get(key)
            if (existing is not None and existing.proc is not None
                    and existing.proc.poll() is None):
                skipped.append(cam)
                jobs.append(existing.status())
                continue

            template = os.path.join(out_dir, "%s-%s-%s.mp4" % (kind, cam, stamp))
            suffix = 0
            while os.path.exists(template) and suffix < 50:
                suffix += 1
                template = os.path.join(
                    out_dir, "%s-%s-%s-%d.mp4" % (kind, cam, stamp, suffix))
            cmd = _ffmpeg_cmd(template, scope, seconds, segment_seconds, source)
            job = _Job(key, plan_id, scope, point_index, point_name, cam,
                       template, "%s-%s-" % (kind, cam), before, time.time(), cmd)
            try:
                log = open(config.RECORD_LOG, "ab")
            except OSError:
                log = subprocess.DEVNULL
            try:
                job.proc = subprocess.Popen(
                    cmd,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    # stderr 落日志：起流失败（源没有、鉴权错、编码不支持）只有这里看得到
                    stderr=log,
                    # 独立进程组：stop 时按组杀，ffmpeg 的子进程不会漏
                    start_new_session=True,
                )
            except OSError as error:
                errors.append("%s: 启动 ffmpeg 失败: %s" % (cam, error))
                continue
            finally:
                if log is not subprocess.DEVNULL:
                    log.close()
            # 源不可用 / 参数不支持时 ffmpeg 会立刻退出：记一条可读原因，
            # 而不是「开始录了但一直没文件」。只丢这一路，别的相机照录。
            time.sleep(0.05)
            if job.proc.poll() is not None:
                errors.append(
                    "%s: ffmpeg 启动后立即退出（相机没在推流？见 %s）"
                    % (cam, config.RECORD_LOG))
                continue
            _JOBS[key] = job
            threading.Thread(target=_watch, args=(job,), daemon=True).start()
            started.append(cam)
            jobs.append(job.status())

    if started:
        _log("录像开始 plan=%s scope=%s point=%s 相机=%s 时长=%ss 切片=%ss"
             % (plan_id, scope, point_index, ",".join(started), seconds, segment_seconds))
    if errors:
        _log("录像部分相机没起来 plan=%s: %s" % (plan_id, " | ".join(errors)))
    if not started and not skipped:
        return {
            "ok": False,
            "error": "; ".join(errors) or "没有可用的录像源",
            "cameras": cameras,
            "rejected": rejected,
        }
    return {
        "ok": True,
        "created": bool(started),
        "cameras": started or cameras,
        "rejected": rejected,
        "jobs": jobs,
        "errors": errors[:5],
    }


def _terminate(job, reason):
    proc = job.proc
    if proc is not None and proc.poll() is None:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGINT)
        except OSError:
            try:
                proc.terminate()
            except OSError:
                pass
        try:
            # ffmpeg 收到 SIGINT 会写完 moov 再退；最多等 8s，超时就硬杀
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            except OSError:
                pass
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                pass
    _finalize(job, reason)
    # `_watch` 线程可能已经在写边车了（`_finalize` 有幂等门，先到的那一方继续写）：
    # 必须等它写完再返回，否则客户端紧接着 `list` 会看不到刚停的这段录像。
    job.finished.wait(5)


def stop(body):
    """停止录像。可按 `planId` + `scope` / `pointIndex` 过滤，不带就停该计划全部。

    同步等 ffmpeg 收尾（写完边车）再返回：客户端紧接着 `list` 时必须看得到文件。
    """
    plan_id = _safe_name(body.get("planId"))
    if not plan_id:
        return {"ok": False, "error": "planId 缺失或不合法"}
    scope = str(body.get("scope") or "").strip().lower()
    point_index = body.get("pointIndex")
    point_index = _int(point_index, -1, -1) if point_index is not None else None
    reason = str(body.get("reason") or "客户端请求停止")[:60]

    with _LOCK:
        targets = [
            job for job in _JOBS.values()
            if job.plan_id == plan_id
            and (not scope or job.scope == scope)
            and (point_index is None or job.point_index == point_index)
        ]
    for job in targets:
        _terminate(job, reason)
    return {
        "ok": True,
        "stopped": len(targets),
        "files": [os.path.basename(path) for job in targets for path in job.files],
    }


def stop_all(reason="Agent 退出"):
    with _LOCK:
        jobs = list(_JOBS.values())
    for job in jobs:
        _terminate(job, reason)


def status():
    """运行态：正在录的 job + 目录 / 源 / 剩余空间（不含源 URL）。"""
    with _LOCK:
        jobs = [job.status() for job in _JOBS.values()]
    return {
        "enabled": True,
        "dir": config.RECORD_DIR,
        "source": config.RECORD_SOURCE_CAM,
        "cameras": sorted(allowed_cameras()),
        "maxCameras": max(1, int(getattr(config, "RECORD_MAX_CAMERAS", 4))),
        "segmentSecs": config.RECORD_SEGMENT_SECS,
        "pointDefaultSecs": config.RECORD_POINT_DEFAULT_SECS,
        "pointMaxSecs": config.RECORD_POINT_MAX_SECS,
        "freeMb": _disk_free_mb(),
        "jobs": jobs,
    }


def list_files(plan_id=None):
    """待回传的录像清单（客户端上传前调用，成功后按返回的 `name` 删除）。

    `name` 是 `<planId>/<文件名>`：客户端拿它回调 `/api/record/delete`，
    服务端只认这个形状，杜绝任意路径删除。
    """
    plan_filter = _safe_name(plan_id) if plan_id else None
    root = config.RECORD_DIR
    entries = []
    try:
        plans = sorted(os.listdir(root))
    except OSError:
        return entries
    for plan in plans:
        if plan.startswith(".") or not _SAFE_NAME.match(plan):
            continue
        if plan_filter and plan != plan_filter:
            continue
        folder = os.path.join(root, plan)
        if not os.path.isdir(folder):
            continue
        with _LOCK:
            busy = set(_FINALIZING)
            # 正在录的文件不返回：客户端可能在上传半截 mp4
            active = {
                name
                for job in _JOBS.values() if job.plan_id == plan
                for name in {
                    path[len(folder) + 1:] for path in _files_of(job)
                }
            }
        for name in sorted(os.listdir(folder)):
            if not name.endswith(".mp4") or name in busy or name in active:
                continue
            path = os.path.join(folder, name)
            if not os.path.isfile(path):
                continue
            meta = {}
            try:
                with open(path + ".json", "r", encoding="utf-8") as handle:
                    meta = json.load(handle)
            except (OSError, ValueError):
                # 边车丢了（老文件 / 被手工删）：按文件名兜底推范围，元数据留空
                pass
            size = os.path.getsize(path) if os.path.exists(path) else 0
            if size <= 0:
                continue
            entries.append({
                "name": "%s/%s" % (plan, name),
                "path": path,
                "planId": meta.get("planId") or plan,
                "scope": meta.get("scope") or ("task" if name.startswith("task-") else "point"),
                "pointIndex": meta.get("pointIndex", -1),
                "pointName": meta.get("pointName") or "",
                # 相机名：边车里有就用它（老文件没有 → 解析不出来就退到默认源）
                "camera": meta.get("camera") or config.RECORD_SOURCE_CAM,
                "size": size,
                "startedAt": meta.get("startedAt") or "",
                "endedAt": meta.get("endedAt") or "",
                "durationSec": meta.get("durationSec") or 0,
            })
    entries.sort(key=lambda item: item["name"])
    return entries


def delete(body):
    """删除已回传的录像（`names` 支持 `plan/file.mp4` 或纯文件名 + `planId`）。"""
    plan_id = _safe_name(body.get("planId"))
    raw = body.get("names") or ([body.get("name")] if body.get("name") else [])
    if isinstance(raw, str):
        raw = [raw]
    deleted = 0
    errors = []
    touched_plans = set()
    for item in raw:
        text = str(item or "").strip()
        if "/" in text:
            plan_part, name = text.split("/", 1)
        else:
            plan_part, name = plan_id, text
        plan_part = _safe_name(plan_part)
        if not plan_part or not _SAFE_NAME.match(os.path.splitext(name)[0] or "-") \
                or not name.endswith(".mp4") or os.path.basename(name) != name:
            errors.append("不认识的录像文件名: %s" % text)
            continue
        folder = _plan_dir(plan_part)
        touched_plans.add(plan_part)
        path = os.path.join(folder, name)
        try:
            os.remove(path)
            deleted += 1
        except FileNotFoundError:
            pass
        except OSError as error:
            errors.append("删除 %s 失败: %s" % (name, error))
            continue
        try:
            os.remove(path + ".json")
        except OSError:
            pass
    # 计划目录空了就一起收掉（与 AI 存证同样的收尾习惯）
    for plan in touched_plans:
        _prune_empty(_plan_dir(plan))
    result = {"ok": not errors, "deleted": deleted}
    if errors:
        result["errors"] = errors[:5]
    return result


def _prune_empty(folder):
    try:
        if os.path.isdir(folder) and not os.listdir(folder):
            os.rmdir(folder)
    except OSError:
        pass


def sweep():
    """清掉「本该传走却一直没传」的残留录像（默认 72 小时）。

    客户端上传成功会立刻删；断电 / 长期断网 / 客户端被换掉时这些文件只能靠超期回收。
    正在录的 job 与刚结束（`RECORD_RETENTION_HOURS` 内）的文件不动。
    """
    root = config.RECORD_DIR
    cutoff = time.time() - config.RECORD_RETENTION_HOURS * 3600
    removed = 0
    try:
        plans = os.listdir(root)
    except OSError:
        return 0
    with _LOCK:
        busy = set(_FINALIZING)
    for plan in plans:
        if plan.startswith(".") or not _SAFE_NAME.match(plan):
            continue
        folder = os.path.join(root, plan)
        if not os.path.isdir(folder):
            continue
        for name in os.listdir(folder):
            if not name.endswith(".mp4") or name in busy:
                continue
            path = os.path.join(folder, name)
            try:
                if os.path.getmtime(path) >= cutoff:
                    continue
                os.remove(path)
                try:
                    os.remove(path + ".json")
                except OSError:
                    pass
                removed += 1
            except OSError:
                continue
        _prune_empty(folder)
    if removed:
        _log("清理超期录像 %d 个（保留 %d 小时）" % (removed, config.RECORD_RETENTION_HOURS))
    return removed


def start_sweeper():
    """启动一次（幂等）+ 每小时兜一次。"""
    global _SWEEP_STARTED
    with _LOCK:
        if _SWEEP_STARTED:
            return
        _SWEEP_STARTED = True

    def loop():
        while True:
            try:
                sweep()
            except Exception as error:  # 清盘失败不能影响录像本身
                _log("清盘失败: %s" % error)
            time.sleep(3600)

    threading.Thread(target=loop, daemon=True).start()
