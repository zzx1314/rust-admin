"""航点拍照（板端抓拍，客户端触发）

路线动作「可见光拍照」→ 设备客户端在到点那一帧调本机：

```text
POST /api/photo       抓 1~N 张 JPEG（ffmpeg 从本机 mediamtx 拉一路）
GET  /api/photo/list  待回传的照片清单（客户端上传前调用）
POST /api/photo/delete 上传成功后删板端文件
GET  /api/photo/status 运行态（目录 / 剩余空间 / 待回传张数 / 最近一次）
```

- **谁触发**：设备客户端（同机 `127.0.0.1:8000`）。拍照属于任务执行的一部分，
  时机只有 5009 的状态机知道；平台/浏览器不直接调。
- **源**：本机 mediamtx 的流。`dogcam` = 机器狗前置可见光（默认，走当前档位），
  `cam0`~`cam3` = 背包四路 USB，`dog-ipcam` = 红外。想拍哪一路在路线动作里配
  （`photoCamera`）；视频那条链路怎么推的见 `stream.py` / `relay.py`。
- **编码参数**：`-frames:v N -q:v PHOTO_QUALITY`，多帧时用 `-vf fps=PHOTO_FPS` 拉开
  （默认 2fps，3 张 ≈ 1.5s 内的画面）；从 RTSP 拉起一路解码，不占 MPP 编码器。
- **落盘**：`PHOTO_DIR/<planId>/pt<航点>/<时间戳>-<相机>-<序号>.jpg`。客户端上传成功
  才调 `/api/photo/delete`；没传走的由 `sweep()` 按 `PHOTO_RETENTION_HOURS` 清。

三条约定（改这块先看）：

1. **拍照失败绝不影响巡检**：流没在推 / ffmpeg 缺失 / 磁盘不足都只回 `ok=false + error`，
   客户端只打日志，任务照跑。
2. **一次只抓一路**：抓拍是串行的（同一时刻一个 ffmpeg），客户端在到点那一帧 await，
   超时 `PHOTO_TIMEOUT_SECS` 兜底——不能让 5009 的处理链路卡死。
3. **不碰 AI 存证目录**：那些是 `detect_jobs/` + `MEDIA_DATA_DIR`（训练图 / 告警证据），
   这里是巡检结果照片，目录、留存、消费方都不一样。
"""
import json
import os
import re
import shutil
import subprocess
import threading
import time

import config

# 文件名 / 目录名白名单：计划号来自平台（`TK-1112-11`），只认字母数字汉字 - _
_SAFE_NAME = re.compile(r"^[0-9A-Za-z_\-\u4e00-\u9fff]+$")
# 照片文件名：`<YYYYmmdd-HHMMSS-mmm>-<相机>[-<序号>].jpg`（相机名从这里反解）
_FILE_NAME = re.compile(
    r"^(\d{8}-\d{6}-\d{3})-([0-9A-Za-z_\-]+?)(?:-(\d{3}))?\.jpg$", re.I
)

_LOCK = threading.RLock()
_SWEEP_STARTED = False
_LAST = {"at": None, "camera": None, "files": [], "error": ""}


def _log(msg):
    print("[photo] %s" % msg, flush=True)


def _ffmpeg_bin():
    return os.getenv("FFMPEG_BIN", "ffmpeg")


def _safe_name(value, limit=120):
    text = str(value or "").strip()
    if not text or len(text) > limit or not _SAFE_NAME.match(text):
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
    """可抓的流：背包四路 + 机器狗前置（含 720p 档）+ 红外。"""
    names = set(getattr(config, "CAMERAS", {}).keys())
    names.update({"dogcam", "dogcam720", "dog-ipcam"})
    return names


def stream_of(camera):
    """路线动作里的相机名 → mediamtx 上的 path（`dogcam` 按当前档位选流）。"""
    cam = str(camera or config.PHOTO_SOURCE_CAM or "dogcam").strip()
    if cam not in allowed_cameras():
        return None
    if cam == "dogcam":
        # 与录像 / AI 抽帧同一套解析：1080p60 档直接用 mediamtx 的 dogcam，
        # 720p30 档用 relay 推的 dogcam720
        import relay

        return relay.dogcam_stream()
    return cam


def source_url(camera):
    stream = stream_of(camera)
    if not stream:
        return None
    return "rtsp://%s:%d/%s" % (config.PHOTO_RTSP_HOST, config.RTSP_PORT, stream)


def _plan_dir(plan_id):
    return os.path.join(config.PHOTO_DIR, plan_id)


def _point_dir(plan_id, point_index):
    return os.path.join(_plan_dir(plan_id), "pt%d" % max(0, point_index))


def _disk_free_mb():
    try:
        usage = shutil.disk_usage(config.PHOTO_DIR)
        return usage.free // (1024 * 1024)
    except OSError:
        parent = os.path.dirname(config.PHOTO_DIR.rstrip("/")) or "/"
        try:
            return shutil.disk_usage(parent).free // (1024 * 1024)
        except OSError:
            return -1


def _fmt(ts):
    return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(ts))


def _now_stamp():
    # 毫秒级：同一个航点连续抓两张也不会撞名
    return time.strftime("%Y%m%d-%H%M%S", time.localtime()) + "-%03d" % (
        int(time.time() * 1000) % 1000
    )


def _ffmpeg_cmd(url, out_template, frames):
    cmd = [
        _ffmpeg_bin(), "-hide_banner", "-loglevel", "error", "-nostdin",
        # 4G / 弱网下 TCP 比 UDP 稳；实时视频那条路也是 TCP
        "-rtsp_transport", "tcp", "-i", url,
        "-frames:v", str(frames),
    ]
    if frames > 1:
        # 多帧要拉开时间（连着抓 N 帧等于同一张图）：按 PHOTO_FPS 抽样
        cmd += ["-vf", "fps=%s" % config.PHOTO_FPS]
    cmd += ["-q:v", str(config.PHOTO_QUALITY), "-y", out_template]
    return cmd


def capture(body):
    """抓拍 → 返回 `{ok, files, error}`。

    `body`: `{planId, pointIndex, pointName, camera, frames, reason}`；
    `files[]`: `{name, path, planId, pointIndex, camera, size, capturedAt}`，
    `name` = `<planId>/pt<n>/<文件名>.jpg`（上传成功后原样回传给 delete）。
    """
    plan_id = _safe_name(body.get("planId") or body.get("taskCode"))
    if not plan_id:
        return {"ok": False, "error": "planId 不合法（只认字母数字汉字 - _）"}
    point_index = _int(body.get("pointIndex"), 0, 0, 999)
    camera = str(body.get("camera") or config.PHOTO_SOURCE_CAM or "dogcam").strip()
    frames = _int(body.get("frames"), 1, 1, config.PHOTO_MAX_FRAMES)
    url = source_url(camera)
    if not url:
        return {
            "ok": False,
            "error": "不支持的相机 %s（可选：%s）" % (camera, ",".join(sorted(allowed_cameras()))),
        }
    free = _disk_free_mb()
    if 0 <= free < config.PHOTO_MIN_FREE_MB:
        return {"ok": False, "error": "磁盘剩余 %dMB 低于下限 %dMB，跳过拍照" % (free, config.PHOTO_MIN_FREE_MB)}

    folder = _point_dir(plan_id, point_index)
    try:
        os.makedirs(folder, exist_ok=True)
    except OSError as error:
        return {"ok": False, "error": "创建照片目录失败: %s" % error}

    stamp = _now_stamp()
    if frames > 1:
        out_template = os.path.join(folder, "%s-%s-%%03d.jpg" % (stamp, camera))
    else:
        out_template = os.path.join(folder, "%s-%s.jpg" % (stamp, camera))
    cmd = _ffmpeg_cmd(url, out_template, frames)

    started = time.time()
    with _LOCK:
        try:
            result = subprocess.run(
                cmd, capture_output=True, timeout=config.PHOTO_TIMEOUT_SECS
            )
            output = (result.stderr or b"").decode("utf-8", "replace").strip()
            returncode = result.returncode
        except subprocess.TimeoutExpired:
            output = "ffmpeg 超时（%ds）" % config.PHOTO_TIMEOUT_SECS
            returncode = -1
        except Exception as error:  # pragma: no cover - ffmpeg 缺失等
            output = str(error)
            returncode = -1

        files = _collect(folder, stamp, plan_id, point_index, camera)
        if files:
            _remember_point(_plan_dir(plan_id), point_index, body.get("pointName"))
        ok = bool(files)
        error = ""
        if not ok:
            error = "抓拍失败（流没在推？）: %s" % (output[-300:] or "无输出")
            _log("航点 %d/%s 抓拍失败: %s" % (point_index, camera, error))
        else:
            _log("航点 %d/%s 抓拍 %d 张（%.1fs）" % (point_index, camera, len(files), time.time() - started))
        if output:
            try:
                with open(config.PHOTO_LOG, "a") as handle:
                    handle.write("=== %s %s\n%s\n" % (_fmt(started), " ".join(cmd), output))
            except OSError:
                pass
        _LAST.update({
            "at": _fmt(started),
            "camera": camera,
            "files": [item["name"] for item in files],
            "error": error,
        })
        return {
            "ok": ok,
            "planId": plan_id,
            "pointIndex": point_index,
            "pointName": str(body.get("pointName") or ""),
            "camera": camera,
            "seconds": round(time.time() - started, 2),
            "returncode": returncode,
            "files": files,
            "error": error,
        }


def _collect(folder, stamp, plan_id, point_index, camera):
    """这一轮写出来的 jpg：只认文件名带本次 stamp 的（别把上一轮没传走的算进来）。"""
    try:
        names = sorted(os.listdir(folder))
    except OSError:
        return []
    files = []
    for name in names:
        if not name.startswith(stamp) or not name.lower().endswith(".jpg"):
            continue
        path = os.path.join(folder, name)
        try:
            size = os.path.getsize(path)
            mtime = os.path.getmtime(path)
        except OSError:
            continue
        if size <= 0:
            continue
        files.append({
            "name": "%s/pt%d/%s" % (plan_id, point_index, name),
            "path": path,
            "planId": plan_id,
            "pointIndex": point_index,
            "camera": camera,
            "size": size,
            "capturedAt": _fmt(mtime),
        })
    return files


def _points_file(plan_dir):
    return os.path.join(plan_dir, "points.json")


def _remembered_points(plan_dir):
    """`points.json`：航点下标 → 航点名（抓拍时记，客户端回传时带上）。"""
    try:
        with open(_points_file(plan_dir)) as handle:
            data = json.load(handle)
        if isinstance(data, dict):
            return {str(key): str(value) for key, value in data.items()}
    except (OSError, ValueError):
        pass
    return {}


def _remember_point(plan_dir, point_index, point_name):
    name = str(point_name or "").strip()
    if not name:
        return
    with _LOCK:
        known = _remembered_points(plan_dir)
        known[str(point_index)] = name[:80]
        try:
            with open(_points_file(plan_dir), "w") as handle:
                json.dump(known, handle, ensure_ascii=False)
        except OSError as error:
            _log("记录航点名失败（不影响拍照）: %s" % error)


def list_files(plan_id=None):
    """待回传的照片（递归 `<planId>/pt<n>/*.jpg`）。"""
    root = config.PHOTO_DIR
    if not os.path.isdir(root):
        return []
    plans = [plan_id] if plan_id else sorted(os.listdir(root))
    files = []
    for plan in plans:
        plan_dir = os.path.join(root, plan)
        if not os.path.isdir(plan_dir) or plan.startswith("."):
            continue
        point_names = _remembered_points(plan_dir)
        for point in sorted(os.listdir(plan_dir)):
            point_dir = os.path.join(plan_dir, point)
            if not os.path.isdir(point_dir):
                continue
            match = re.match(r"pt(\d+)$", point)
            if not match:
                continue
            point_index = int(match.group(1))
            for name in sorted(os.listdir(point_dir)):
                if not name.lower().endswith(".jpg"):
                    continue
                path = os.path.join(point_dir, name)
                try:
                    size = os.path.getsize(path)
                    mtime = os.path.getmtime(path)
                except OSError:
                    continue
                if size <= 0:
                    continue
                # 文件名 `<stamp>-<相机>[-<序号>].jpg`：相机名从这里反解
                parsed = _FILE_NAME.match(name)
                camera = parsed.group(2) if parsed else config.PHOTO_SOURCE_CAM
                files.append({
                    "name": "%s/%s/%s" % (plan, point, name),
                    "path": path,
                    "planId": plan,
                    "pointIndex": point_index,
                    "camera": camera,
                    "pointName": point_names.get(str(point_index), ""),
                    "size": size,
                    "capturedAt": _fmt(mtime),
                })
    return files


def delete(body):
    """上传成功后删板端文件：只认 `<planId>/pt<n>/<文件名>.jpg`。"""
    names = body.get("names")
    if isinstance(names, str):
        names = [names]
    if not isinstance(names, list) or not names:
        return {"ok": False, "error": "names 不能为空"}
    deleted = []
    errors = []
    for name in names:
        relative = str(name or "").strip().strip("/")
        parts = relative.split("/")
        if (
            len(parts) != 3
            or not re.match(r"^[0-9A-Za-z_\-\u4e00-\u9fff]+$", parts[0])
            or not re.match(r"^pt\d+$", parts[1])
            or not re.match(r"^[0-9A-Za-z_\-\u4e00-\u9fff\.]+\.jpg$", parts[2], re.I)
        ):
            errors.append("%s: 路径不合法" % relative)
            continue
        path = os.path.join(config.PHOTO_DIR, *parts)
        try:
            os.remove(path)
            deleted.append(relative)
        except FileNotFoundError:
            deleted.append(relative)  # 已经没了也算成功：客户端重试是常态
        except OSError as error:
            errors.append("%s: %s" % (relative, error))
            continue
        # 空目录顺手收掉（一次巡检每个航点一个目录，不收会攒几千个空壳）
        point_dir = os.path.dirname(path)
        plan_dir = os.path.dirname(point_dir)
        try:
            os.rmdir(point_dir)
        except OSError:
            pass
        try:
            # 计划目录里只剩 points.json 也算「传完了」
            for left in os.listdir(plan_dir):
                if left != "points.json":
                    raise OSError("还有别的文件")
            os.remove(_points_file(plan_dir))
            os.rmdir(plan_dir)
        except OSError:
            pass
    return {"ok": not errors, "deleted": deleted, "errors": errors}


def status():
    files = list_files()
    sessions = sorted({item["planId"] for item in files})
    return {
        "dir": config.PHOTO_DIR,
        "free_mb": _disk_free_mb(),
        "pending": len(files),
        "pending_bytes": sum(item["size"] for item in files),
        "sessions": sessions[:50],
        "allowed_cameras": sorted(allowed_cameras()),
        "max_frames": config.PHOTO_MAX_FRAMES,
        "quality": config.PHOTO_QUALITY,
        "last": dict(_LAST),
        "retention_hours": config.PHOTO_RETENTION_HOURS,
        "sim": False,
    }


def sweep():
    """删掉超过 `PHOTO_RETENTION_HOURS` 的照片（断电 / 长期断网没传走的兜底）。"""
    hours = config.PHOTO_RETENTION_HOURS
    if hours <= 0:
        return 0
    cutoff = time.time() - hours * 3600
    removed = 0
    root = config.PHOTO_DIR
    if not os.path.isdir(root):
        return 0
    for plan in os.listdir(root):
        plan_dir = os.path.join(root, plan)
        if not os.path.isdir(plan_dir) or plan.startswith("."):
            continue
        for point in os.listdir(plan_dir):
            point_dir = os.path.join(plan_dir, point)
            if not os.path.isdir(point_dir):
                continue
            for name in os.listdir(point_dir):
                path = os.path.join(point_dir, name)
                try:
                    if os.path.getmtime(path) < cutoff:
                        os.remove(path)
                        removed += 1
                except OSError:
                    pass
            try:
                os.rmdir(point_dir)
            except OSError:
                pass
        try:
            os.rmdir(plan_dir)
        except OSError:
            pass
    if removed:
        _log("清理超期照片 %d 张（保留 %d 小时）" % (removed, hours))
    return removed


def start_sweeper(interval_secs=6 * 3600):
    """启动时清一次，之后每 `interval_secs` 再清一次（守护线程）。"""
    global _SWEEP_STARTED
    with _LOCK:
        if _SWEEP_STARTED:
            return
        _SWEEP_STARTED = True

    def loop():
        while True:
            try:
                sweep()
            except Exception as error:  # pragma: no cover
                _log("清理超期照片失败: %s" % error)
            time.sleep(interval_secs)

    threading.Thread(target=loop, daemon=True).start()

