"""航点录像 / 航段录像（板端落盘，客户端触发）

巡检路线里的两个录像动作最终都在这里落地：

```text
路线「航点动作 → 录像采集」  → 客户端到点 POST /api/record/start {scope:"point", seconds:15}
                               离开该航点  POST /api/record/stop {scope:"point"}
路线「行进任务 → 录像采集」  → 客户端起步 POST /api/record/start {scope:"leg", segmentSeconds:300}
                               到下一个航点 POST /api/record/stop {scope:"leg", pointIndex:N}
                               任务终态/取消 POST /api/record/stop（不带 scope，全停）
```

> `scope="task"`（旧版「全程录像」：整条任务一条流）**已不用**，只为兼容老客户端保留：
> 现在航段动作按**段级**录（起步开、下一个航点停），一段一个 mp4。

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
- **落盘**：`RECORD_DIR/<planId>/<kind>-<相机>-<时间戳>-<片段>.mp4` + 同名 `.json` 边车
  （planId / 航点 / 范围 / 相机 / 起止时间 / 时长）。客户端传完调 `/api/record/delete` 删。
  断电/断网没传走的由 `sweep()` 按 `RECORD_RETENTION_HOURS` 清掉，不会写满 SSD。

为什么要切片：一段路 / 一趟任务可能几十分钟，4G 上一次传几百 MB 必然断。`scope=leg`
（以及兼容保留的老 `scope=task`）用 ffmpeg 的 segment 每 `RECORD_SEGMENT_SECS`
（默认 300s，路线动作可配）切一段，客户端逐段落盘后一段一段传；任何一段失败只影响那一段。

⚠️ **切片文件名必须带 `-%03d`**（`leg0-cam1-20261126-110421-000.mp4`、`-001.mp4`…）：
只用开始时间戳命名时，每一片都写同一个文件、后一片把前一片**覆盖**掉，一段 8 分钟的
航段最后只剩最后一片（现场表现：「航段录像只有最后几秒」）。边车写的是**这一片**的
时长，不是整个 job 的——否则平台「任务录像」里每一片都显示成整段航段。

⚠️ **mp4 必须关掉 edit list**（`use_editlist=0`，2026-11-26 起）：ffmpeg 从 RTSP 拉流时
第一帧的 DTS 不是 0（要等源的下一个关键帧：`dogcam` 约 2s、USB 相机 `cam0`~`cam3`
约 1s——见下面的「多路对齐」）。mov 封装器把这段「开头的空白」写成**空 edit**（`elst`
`{duration: <DTS>, media_time: -1}`），于是 `mvhd.duration = 内容 + 那段偏移`：
现场 `leg3-cam1-20261126-144815-000.mp4` 的 `mvhd` 是 65.35s，而真正能解的只有
52.4s。播放器按 `mvhd` 画进度条、从第一帧开始播，**最后那段偏移（最多十几秒）
只有进度条、没有画面**——就是「视频最后还有进度条，但后面几秒不能播放」。
`use_editlist=0` 之后 `mvhd.duration == 内容时长`，各播放器（浏览器 / 手机相册 /
微信）都对得上。ffmpeg 内部本来就按 0 写 sample 时间戳，关掉 edit list 不丢内容。

> **多路对齐**（2026-11-26 定位并修掉）：`-c copy` 会丢掉首个关键帧之前的包
> （`-copyinkf` 关着），所以每个文件的第一帧就是**那一路自己的下一帧关键帧**——同一时刻
> 起的多路，内容起点差一个 GOP。以前 `cam0`~`cam3`（`stream.py`）没设 `-g`，走编码器
> 默认（keyint 250，板端 15fps ≈ 16.7s 一个关键帧）：前置 ~2s 就出画面、左前 / 右前要等
> 8~13s，两路内容差 ~10s；`-t seconds` 的窗口还会被等关键帧吃掉（配 15s 那一路只录到
> 7.8s，用真 ffmpeg + mediamtx 复现过）。现在四路推流都是 `CAPTURE_GOP`（默认 =
> 输出帧率 = 1s），起点 / 时长差 ≤1s。**别用 `-copyinkf` 硬补片头**：关键帧之前的 P 帧
> 没法解码，补进来只是把「空白」挪到片头（而且 `mvhd` 那套问题会一起回来）。

⚠️ **空壳 mp4 不能当录像**（2026-11-26 17:44 `TK-1126-20` P1 现场）：`-c copy` 在窗口内
等不到本路的关键帧时，ffmpeg 仍然会正常退出（码 0），只留一个 `ftyp` + 空 `moov`——
`pt0-cam1-20261126-174456.mp4` 只有 **293 B、`mvhd.duration = 0`**。以前这种空壳会被
写成「正常录像」（`mvhd` 读不到就退回**墙钟时长**，于是它显示成 5.3s），回传到平台就是
一段打不开、时长还不对的视频，现场只看到「录像没了」，真凶（推流 GOP 太大）也被它掩住。
现在 `_finalize` 用 `_is_empty_capture` 认出来：删空壳、不写边车、不进 `list`，原因写进
`record.log`，并在 `/api/record/stop` 回执的 `discarded` 里回给客户端
（`client/src/record.rs` 落一行 warn）。**别改成「宁可留着」**：留一个 0 帧的 mp4 比没有
更坑人（平台上有进度条但播不动），回传还会白占 4G 带宽。
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
# 读 mp4 头部找 `mvhd` 用多少字节：`+faststart` 之后 moov 一定在最前面，
# 几秒的切片 moov 才几 KB；留 2MB 兜住「一次录很久、千帧以上」的 stsz。
_MP4_HEAD_BYTES = 2 * 1024 * 1024
# 一段**真的录到了画面**的 mp4 至少有多大：`-c copy` 一帧都没写出来时，ffmpeg 只留一个
# `ftyp` + 空 `moov`（现场 2026-11-26 `pt0-cam1-20261126-174456.mp4` = **293 B**，
# `mvhd.duration = 0`）；而写到画面的文件就算 320x180@8fps 也有几十 KB。
# 小于它**且**读不出 `mvhd` 时判「这一路没录到画面」（见 `_finalize`）。
_MIN_PLAYABLE_BYTES = 4096

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

    返回 `(picked, rejected, asked)`：不认识的相机名只丢这一路，并在回执里带上，
    不能让一个打字错误把整条录像废掉（现场改配置比改任务容易）。

    `asked` = 请求里**明确点了相机**（哪怕全是不认识的名字）。只有「一个都没点」
    才退到 `RECORD_SOURCE_CAM`（历史路线 / 老客户端的行为）；点了却全都不可用时
    **绝不拿默认源顶替**——那会录出一段和配置不符的画面，现场还以为录对了
    （2026-11-25 现场：航段配了右前 `cam1`，录下来是正前 `dogcam`）。
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
    asked = False
    for item in raw:
        cam = str(item or "").strip()
        if not cam:
            continue
        asked = True
        if cam not in allowed:
            rejected.append(cam)
            continue
        if cam in picked:
            continue
        picked.append(cam)
    if not picked and not asked:
        fallback = str(config.RECORD_SOURCE_CAM or "dogcam").strip()
        picked = [fallback if fallback in allowed else "dogcam"]
    max_cams = max(1, int(getattr(config, "RECORD_MAX_CAMERAS", 4)))
    if len(picked) > max_cams:
        rejected.extend(picked[max_cams:])
        picked = picked[:max_cams]
    return picked, rejected, asked


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


def _ffmpeg_cmd(path_template, scope, seconds, segment_seconds, source,
                segment_list=None):
    """构造一路 ffmpeg 命令。

    - `scope=point`：`-t seconds` 自动停，一个 mp4；
    - `scope=leg` / `task`（老）：`-f segment` 按 `segment_seconds` 切段（路线动作里的
      `recordSegmentSeconds`，缺省 `RECORD_SEGMENT_SECS`），总时长上限
      `RECORD_TASK_MAX_SECS`（客户端一定会发 stop，这里只是防跑飞）。
      `leg` = 一段路（起步 → 下一个航点），`task` = 旧版「整条任务一条流」。

    ⚠️ `path_template` 里必须带 `-%03d`（片段序号）：只靠开始时间戳时每一片都写同一个
    文件，segment 复用器会**覆盖**上一条，一段 8 分钟的航段最后只剩最后一片
    （2026-11-26 `leg0-cam1-20261126-110421.mp4` 现场）。
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
    # ⚠️ `use_editlist=0` 不能去掉：不关掉时 mov 封装器会给「第一帧 DTS ≠ 0」写一段
    # **空 edit**，`mvhd.duration` 就变成「内容 + 这段偏移」，播放器画出的进度条比
    # 真实画面长（最多十几秒），尾巴上只有进度条没有画面（2026-11-26 现场
    # `leg3-cam1-20261126-144815-000.mp4`：mvhd 65.35s / 实际 52.4s）。
    if scope in ("leg", "task"):
        cmd += [
            "-f", "segment",
            "-segment_time", str(segment_seconds),
            "-reset_timestamps", "1",
            "-segment_format", "mp4",
            # 片段也要 faststart：客户端上传后浏览器边下边播。
            # `-segment_format_options` 的多个选项用 `:` 分隔（整串交给内层 mp4 封装器）。
            "-segment_format_options", "movflags=+faststart:use_editlist=0",
            # ⚠️ 别再加 `-strftime 1`：时间戳（`<kind>-<cam>-<开始时间>`）已经在模板里，
            # 它还会把 `%03d` 当日期格式吃掉（glibc 给 "026"），于是每片写同一个文件、
            # 后一片覆盖前一片。片段序号只交给 segment 复用器的 `%03d`。
        ]
        if segment_list:
            # 每片的真实时长只有 ffmpeg 知道（`-segment_time` 是「按关键帧就近切」）：
            # 写进边车，平台「任务录像」才不会每片都显示成整段航段的时长。
            cmd += ["-segment_list", segment_list, "-segment_list_type", "csv"]
    else:
        cmd += ["-movflags", "+faststart", "-use_editlist", "0"]
    cmd += ["-y", path_template]
    return cmd


class _Job:
    def __init__(self, key, plan_id, scope, point_index, point_name, camera,
                 path_template, prefix, before, started_at, cmd, segment_list=None):
        self.key = key
        # 停止原因：`stop()` 知道真实原因（离开航点 / 到达下一个航点 / 任务终态），
        # 但 `_watch` 的 `proc.wait()` 往往先返回、抢先写边车，所以原因先记在 job 上
        self.reason = None
        self.plan_id = plan_id
        self.scope = scope
        self.point_index = point_index
        self.point_name = point_name
        self.camera = camera
        self.path_template = path_template
        # 这一路「起了 ffmpeg 但一帧都没录到」的记录（`_finalize` 里填）：
        # 空壳 mp4 会被删掉，原因随 `/api/record/stop` 回执回给客户端写日志
        self.discarded = []
        # 本路录像的文件名前缀（`pt<n>-` / `task-`）：同一计划可能同时有全程 + 航点两路，
        # 只按「开始后新出现的文件」认会互相抢对方的片段
        self.prefix = prefix
        # 开始前目录里已有的文件名：收尾时用「差集」认自己写出来的（segment 会生成多个，
        # 文件名带的是 start 时的时间戳 + 片段序号，靠前缀认会把上一轮没传走的录像一起吞进来）
        self.before = set(before)
        # segment 复用器写的切片清单（`<kind>-<cam>-<时间戳>.csv`）：
        # 每片的真实时长从这里取，写完就删（`_finalize`）
        self.segment_list = segment_list
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


def _segment_durations(list_path):
    """读 `-segment_list csv`（`文件名,起点秒,终点秒`）→ `{basename: 时长}`。

    一个 job 会切好几片（`recordSegmentSeconds`，默认 300s），每片的真实时长只有
    ffmpeg 知道：`-segment_time` 是「按关键帧就近切」，不是精确的 300s。
    边车里写整个 job 的时长会让平台「任务录像」里每一片都显示成整段航段
    （2026-11-26：航段只剩最后一片 + 每片时长都不对，一并修）。

    现在只当**兜底**：首选 [`_mp4_duration`]（读文件自己的 `mvhd`——播放器看到的就是它，
    而 csv 的起止是输入时间轴上的窗口，会把等关键帧那几秒也算进去）。
    读不到（老 ffmpeg / 清单被清掉 / 还没写）就返回空，调用方退回 job 时长。
    """
    if not list_path:
        return {}
    durations = {}
    try:
        with open(list_path, "r", encoding="utf-8", errors="replace") as handle:
            for line in handle:
                parts = [item.strip() for item in line.strip().split(",")]
                if len(parts) < 3:
                    continue
                try:
                    start = float(parts[1])
                    end = float(parts[2])
                except ValueError:
                    continue
                if end > start:
                    durations[os.path.basename(parts[0])] = end - start
    except OSError:
        return {}
    return durations


def _mp4_duration(path):
    """读 mp4 的 `mvhd.duration`（秒）；读不到返回 `None`。

    边车里的 `durationSec` 要跟**播放器看到的**一致：`mvhd` 正是播放器画进度条用的
    那个值。以前一律拿 job 的墙钟时长（`started→stopped`），ffmpeg 等源关键帧的那
    十几秒也被算进去，平台「任务录像」就写成 65.4s 而实际只有 52.4s（比进度条还
    让人以为是视频坏了）。

    只读文件头部（`+faststart` 之后 `moov` 一定在最前面），不扫整个文件：
    13MB 的切片全读一遍会在板子上多一份内存 / IO。

    > 老文件（还没升级 `use_editlist=0`）的 `mvhd` 包含了那段空 edit，比内容长；
    > 这里取到的就是「播放器认为的长度」，与新文件一个口径。
    """
    try:
        with open(path, "rb") as handle:
            data = handle.read(_MP4_HEAD_BYTES)
    except OSError:
        return None

    def walk(start, end):
        pos = start
        while pos + 8 <= end:
            size = int.from_bytes(data[pos:pos + 4], "big")
            kind = data[pos + 4:pos + 8]
            header = 8
            if size == 1:
                # 64 位长度（大 box 用得上）
                if pos + 16 > end:
                    return None
                size = int.from_bytes(data[pos + 8:pos + 16], "big")
                header = 16
            elif size == 0:
                size = end - pos
            if size < header or pos + size > end:
                return None
            body = pos + header
            if kind == b"mvhd":
                version = data[body] if body < len(data) else 0
                if version == 0:
                    timescale = int.from_bytes(data[body + 12:body + 16], "big")
                    duration = int.from_bytes(data[body + 16:body + 20], "big")
                else:
                    timescale = int.from_bytes(data[body + 20:body + 24], "big")
                    duration = int.from_bytes(data[body + 24:body + 32], "big")
                if not timescale or duration <= 0:
                    return None
                return duration / float(timescale)
            if kind == b"moov":
                found = walk(body, pos + size)
                if found:
                    return found
            pos += size
        return None

    return walk(0, len(data))


def _is_empty_capture(path):
    """这个 mp4 是不是「一帧都没有」的空壳（ffmpeg 只写了 `ftyp` + 空 `moov`）。

    两个条件都要满足才算：体积小于 [`_MIN_PLAYABLE_BYTES`]（空壳约 300 B，真的写过
    画面的文件至少几十 KB），**并且**读不出 `mvhd` 时长。只按体积判会误删「刚好很短」
    的正常片段（宁可留下一段能播的，也不要吞掉现场唯一的证据）。
    """
    try:
        if os.path.getsize(path) >= _MIN_PLAYABLE_BYTES:
            return False
    except OSError:
        return False
    return _mp4_duration(path) is None


def _finalize(job, reason):
    """ffmpeg 退出后写边车：客户端按边车里的元数据回传，不用再猜文件名。"""
    with _LOCK:
        if job.stopped_at is not None:
            return
        job.stopped_at = time.time()
        job.files = _files_of(job)
        _FINALIZING.update(os.path.basename(path) for path in job.files)
    # `stop()` 知道真实原因，「自行退出」才用 `_watch` 的理由（谁先到用谁的消息）
    reason = job.reason or reason
    duration = max(0.0, job.stopped_at - job.started_ts)
    segments = _segment_durations(job.segment_list)
    if job.segment_list:
        # 切片清单只是中间产物：内容已经落到每片边车，留着只会让回传目录变脏
        try:
            os.remove(job.segment_list)
        except OSError:
            pass
    kept = []
    for path in job.files:
        name = os.path.basename(path)
        # 片长优先级：文件自己的 `mvhd`（播放器看到的就是它）> 切片清单 > job 墙钟时长
        seconds = _mp4_duration(path)
        if seconds is None and _is_empty_capture(path):
            # 空壳：ffmpeg 起了、窗口也到了，但一帧都没写成（`-c copy` 在窗口内等不到
            # 本路的关键帧 / 源根本没在推流）。以前它会被当成正常录像：边车拿不到
            # `mvhd` 就退回**墙钟时长**，于是 293 B 的空壳在平台「任务录像」里变成
            # 一段「5.3s 但打不开」的假视频，客户端回传核对也发现不了
            # （2026-11-26 17:44 `TK-1126-20` P1：狗头 3.1s、右前 `cam1` 只有 293 B）。
            # 现在直接丢：不写边车、不进 `list_files`，原因报到日志 + `stop` 回执。
            job.discarded.append({
                "camera": job.camera,
                "name": name,
                "size": os.path.getsize(path) if os.path.exists(path) else 0,
                "windowSec": round(duration, 1),
            })
            try:
                os.remove(path)
            except OSError as error:
                _log("删除空壳录像失败（%s）: %s" % (path, error))
            continue
        kept.append(path)
        if seconds is None:
            seconds = segments.get(name, duration)
        meta = {
            "planId": job.plan_id,
            "scope": job.scope,
            "pointIndex": job.point_index,
            "pointName": job.point_name,
            "camera": job.camera,
            "startedAt": _fmt(job.started_at),
            "endedAt": _fmt(job.stopped_at),
            "durationSec": round(seconds, 1),
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
        job.files = kept
        job.finished.set()
    _log("录像结束 plan=%s scope=%s point=%s cam=%s 窗口=%.1fs 文件=%d 原因=%s"
         % (job.plan_id, job.scope, job.point_index, job.camera, duration,
            len(kept), reason))
    if job.discarded:
        # 现场排障就靠这一行：是「相机没在推流」还是「推流的关键帧间隔比录像窗口还长」
        _log("⚠️ 未录到画面，已丢弃空壳 plan=%s scope=%s point=%s cam=%s：窗口 %.1fs 内没有关键帧"
             "（源 GOP 太大 / 相机没在推流 / `-c copy` 等不到 I 帧），"
             "检查 `CAPTURE_GOP`（应 = 输出帧率 = 1s）与 mediamtx 上该路是否在推流"
             % (job.plan_id, job.scope, job.point_index, job.camera, duration))


def _watch(job):
    try:
        job.proc.wait()
    finally:
        # 客户端的 stop 会带真实原因（离开航点 / 到达下一个航点）；这里只处理
        # 「ffmpeg 自己退出」（源断了 / 撞上 `-t` 上限）——以前这里写「到上限自动停」，
        # 结果每条边车的 reason 都是它，现场排障时把 `-t` 都当成了凶手。
        _finalize(job, "ffmpeg 自行退出")


def start(body):
    """开始一段录像（幂等：同一计划同一航点同一路相机重复调用返回正在跑的那一路）。

    `body`：`{planId, scope: "point"|"leg"|"task", pointIndex, pointName, seconds,
    segmentSeconds, cameras: ["cam0", "dogcam"], camera}`。

    `seconds` 只对 `point` 有意义（上限 `RECORD_POINT_MAX_SECS`）；`leg` / `task`
    只看 `segmentSeconds`（切片长度，总上限 `RECORD_TASK_MAX_SECS`）。
    `pointIndex` 在 `leg` 下是**起点航点下标**（`legs[i]` 从 P{i+1} 出发）。
    `cameras` 是多选：**每一路一个独立 ffmpeg + 一个独立 mp4**，任何一路起不来
    只丢那一路（回执里带 `errors`），其余照录；没点相机才退到 `RECORD_SOURCE_CAM`。
    点了相机但**全都不可用**时直接 `ok=false`（回执带 `rejected`），**不拿默认源顶替**
    ——否则会录出一段与配置不符的画面，现场以为录对了（2026-11-25 现场事故）。
    """
    plan_id = _safe_name(body.get("planId"))
    if not plan_id:
        return {"ok": False, "error": "planId 缺失或不合法（只认字母数字汉字 - _）"}
    scope = str(body.get("scope") or "").strip().lower()
    if scope not in ("point", "leg", "task"):
        scope = "point"
    point_index = _int(body.get("pointIndex"), -1, -1)
    if scope in ("point", "leg") and point_index < 0:
        return {"ok": False, "error": "航点 / 航段录像必须带 pointIndex（航段用起点航点下标）"}
    point_name = str(body.get("pointName") or "")[:60]
    if scope == "point":
        seconds = _int(body.get("seconds"), config.RECORD_POINT_DEFAULT_SECS,
                       3, config.RECORD_POINT_MAX_SECS)
        segment_seconds = seconds
    else:
        # 航段 / 全程（老）：`segmentSeconds` 是切片长度（路线动作可配），`maxSeconds` 是兜底总上限
        segment_seconds = _int(body.get("segmentSeconds"), config.RECORD_SEGMENT_SECS,
                               10, config.RECORD_TASK_MAX_SECS)
        seconds = _int(body.get("maxSeconds"), config.RECORD_TASK_MAX_SECS,
                       30, config.RECORD_TASK_MAX_SECS)
    cameras, rejected, asked = cameras_of(body)
    if asked and not cameras:
        # 点了相机却一路都不可用：**不要**退到默认源（那会录成另一路画面，
        # 与配置不符且不报错）。直接拒掉，让客户端在日志里说清楚。
        return {
            "ok": False,
            "error": "配置的相机都不可用（%s），不按默认源顶替"
                     % (", ".join(rejected) or "未给出"),
            "cameras": [],
            "rejected": rejected,
        }

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
        if scope == "point":
            kind = "pt%d" % point_index
        elif scope == "leg":
            kind = "leg%d" % point_index
        else:
            kind = "task"
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

            base = "%s-%s-%s" % (kind, cam, stamp)
            suffix = 0
            # 撞名（同一秒里两次 start / 上一轮没删干净）：只往后加号，不要动前缀，
            # `_files_of` 是按 `<kind>-<cam>-` 前缀认文件的
            while any(name.startswith(base + "-") or name.startswith(base + ".")
                      for name in os.listdir(out_dir)) and suffix < 50:
                suffix += 1
                base = "%s-%s-%s-%d" % (kind, cam, stamp, suffix)
            # ⚠️ 片段序号 `%03d` 必须留着（只给 leg / task：航点录像是单文件）：
            # 没有它时 segment 每一片都写同一个文件，后一片把前一片覆盖掉
            # （2026-11-26 现场「航段录了 8 分钟只剩最后几秒」）
            template = os.path.join(
                out_dir, base + ("-%03d.mp4" if scope in ("leg", "task") else ".mp4"))
            segment_list = (
                os.path.join(out_dir, base + ".csv")
                if scope in ("leg", "task") else None
            )
            cmd = _ffmpeg_cmd(template, scope, seconds, segment_seconds, source,
                              segment_list)
            job = _Job(key, plan_id, scope, point_index, point_name, cam,
                       template, "%s-%s-" % (kind, cam), before, time.time(), cmd,
                       segment_list)
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
                # 这一路没录成：segment 可能已经开始写清单了，顺手清掉
                if segment_list:
                    try:
                        os.remove(segment_list)
                    except OSError:
                        pass
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
    # 先把原因记在 job 上：`_watch` 的 `proc.wait()` 会先返回、抢先写边车
    job.reason = job.reason or reason
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
        # 「起了 ffmpeg 但一帧都没录到」的那几路（空壳已删）：客户端把它写进日志，
        # 现场才知道是「录像配错了 / 相机没推流」，而不是「录了但平台上找不到」。
        "discarded": [item for job in targets for item in job.discarded],
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


def _scope_of_file(name):
    """边车丢了时按文件名推范围：`leg12-…` = 航段、`task-…` = 老全程、其余 = 航点。"""
    if name.startswith("task-"):
        return "task"
    if re.match(r"^leg\d+-", name):
        return "leg"
    return "point"


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
                "scope": meta.get("scope") or _scope_of_file(name),
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
            if name in busy:
                continue
            path = os.path.join(folder, name)
            if name.endswith(".csv"):
                # 切片清单的残留（Agent 在录像中途被杀时不会走到 `_finalize`）：
                # 只需按 mtime 清，不是回传产物
                try:
                    if os.path.getmtime(path) < cutoff:
                        os.remove(path)
                except OSError:
                    pass
                continue
            if not name.endswith(".mp4"):
                continue
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
