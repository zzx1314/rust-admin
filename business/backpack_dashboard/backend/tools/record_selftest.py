"""航点 / 航段录像状态机自检（不需要板子、相机、ffmpeg）

改 `record.py` 的计划 / 落盘 / 清单 / 删除 / 超期清理逻辑后必跑：

    python3 business/backpack_dashboard/backend/tools/record_selftest.py

怎么在没有 ffmpeg / RTSP 的机器上验：
`record.start()` 里真正会碰硬件的只有 `subprocess.Popen(ffmpeg …)`，这里把它换成
「立刻在模板路径写一个假 mp4、`wait()` 挂到收到停止信号」的假进程，其余代码
（边车、清单、删除、越界校验、清盘）全部走真实实现。

四路推流的关键帧间隔（`stream._build_cmd` 的 `-g`，默认 1s）也在这里验：录像 `-c copy`
只能从关键帧开始写，GOP 一大，多路内容起点就差一个 GOP（2026-11-26「前置与左前 / 右前
差 10s」就是这个）。

「起了 ffmpeg 但一帧都没录到」的空壳 mp4（`ftyp` + 空 `moov`，现场 293 B）也必须验：
它以前会被当成正常录像回传（边车退回墙钟时长 → 平台上一段打不开的 5.3s 视频），
现在要删空壳 + 不进清单 + `stop` 回执带 `discarded`。
"""
import os
import shutil
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import config  # noqa: E402
import record  # noqa: E402
import relay  # noqa: E402
import stream  # noqa: E402

PROCS = []
ARGS = []
# 假 ffmpeg 一次写几片（多片用例临时改成 3）：真 ffmpeg 是按 `-segment_time` 自己切的
SEGMENTS = 1
# 假 ffmpeg 改成写「空壳 mp4」（一帧都没有）：验 `_finalize` 会不会把空壳当录像
EMPTY_CAPTURE = False


def fake_mp4(seconds=4, playable=True):
    """假 ffmpeg 写的 mp4 字节。

    `playable=True` 写带 `mvhd` 的（时长 `seconds`）：`record._finalize` 认为「写了画面的」
    文件 = 体积 ≥ `_MIN_PLAYABLE_BYTES` **或** 读得出 `mvhd`，所以这里体积故意很小
    （几百字节），正好把「小体积但有 `mvhd` 不许误删」这条也钉住。
    `playable=False` 写空壳：`ftyp` + 空 `moov`，`mvhd` 读不出来（现场 293 B 那种）。
    """
    ftyp = (24).to_bytes(4, "big") + b"ftypmp42" + b"\x00\x00\x00\x00" + b"mp42isom"
    if not playable:
        return ftyp + (8).to_bytes(4, "big") + b"moov" + b"\x00" * 40
    timescale = 1000
    mvhd = (
        b"\x00" * 4                                  # version 0 + flags
        + b"\x00" * 8                                # creation / modification
        + timescale.to_bytes(4, "big")
        + int(seconds * timescale).to_bytes(4, "big")
        + b"\x00" * 80
    )
    mvhd_box = (len(mvhd) + 8).to_bytes(4, "big") + b"mvhd" + mvhd
    moov = (len(mvhd_box) + 8).to_bytes(4, "big") + b"moov" + mvhd_box
    return ftyp + moov + (16).to_bytes(4, "big") + b"mdat" + b"\x00" * 8


class FakeProc:
    """假 ffmpeg：起手写 mp4（切片范围按 `%03d` 写多片 + 清单），`wait()` 挂到收到停止信号。"""

    def __init__(self, args, **_kwargs):
        self.pid = 424242
        # 每个 job 一个停止信号：停「航点录像」不能把同计划的「全程录像」一起停掉
        self.done = threading.Event()
        ARGS.append(list(args))
        self._code = None
        template = args[-1]
        count = SEGMENTS if "%03d" in template else 1
        names = [template.replace("%03d", "%03d" % index) for index in range(count)]
        lines = []
        os.makedirs(os.path.dirname(names[0]), exist_ok=True)
        for index, path in enumerate(names):
            with open(path, "wb") as handle:
                handle.write(fake_mp4(playable=not EMPTY_CAPTURE))
            PROCS.append(path)
            # 真 ffmpeg 的 `-segment_list csv` 就是「文件名,起点,终点」
            lines.append("%s,%d,%d" % (os.path.basename(path), index * 4, (index + 1) * 4))
        self.path = names[0]
        if "-segment_list" in args:
            with open(args[args.index("-segment_list") + 1], "w", encoding="utf-8") as handle:
                handle.write("\n".join(lines) + "\n")

    def poll(self):
        return self._code

    def wait(self, timeout=None):
        # 真实 ffmpeg 收到 SIGINT 才退出；`_watch` 里是不带超时的 wait
        self.done.wait(timeout)
        self._code = 0
        return 0

    def terminate(self):
        self.done.set()

    def kill(self):
        self.done.set()


def check(name, condition, detail=""):
    if condition:
        print("  ok   %s" % name)
        return True
    print("  FAIL %s %s" % (name, detail))
    return False


def main():
    tmp = tempfile.mkdtemp(prefix="record-selftest-")
    config.RECORD_DIR = tmp
    config.RECORD_MIN_FREE_MB = 0
    import subprocess

    real_popen = subprocess.Popen
    subprocess.Popen = FakeProc

    failures = 0

    # 1) 航点录像：起 → 清单为空（正在录） → 停 → 清单出现 + 边车 → 删除
    r = record.start({"planId": "TK-1", "scope": "point", "pointIndex": 2,
                      "pointName": "P3", "seconds": 5})
    failures += not check("start ok", r.get("ok") and r.get("created"), r)
    failures += not check("重复 start 幂等", record.start(
        {"planId": "TK-1", "scope": "point", "pointIndex": 2,
         "pointName": "P3", "seconds": 5}).get("created") is False)
    failures += not check("正在录时清单为空", record.list_files("TK-1") == [])
    s = record.status()
    failures += not check("status 有 1 个 job", len(s["jobs"]) == 1, s)
    failures += not check("status 不泄漏源 URL", "rtsp" not in str(s).lower(), s)

    # 真实 ffmpeg 至少跑几秒；这里睡 1.1s 让「时长」不是 0（也模拟一次真实窗口）
    time.sleep(1.1)
    stopped = record.stop({"planId": "TK-1", "scope": "point", "reason": "离开航点"})
    failures += not check("stop ok", stopped.get("ok") and stopped.get("stopped") == 1, stopped)
    files = record.list_files("TK-1")
    failures += not check("停止后清单有 1 段", len(files) == 1, files)
    if files:
        item = files[0]
        failures += not check("清单带航点下标", item["pointIndex"] == 2, item)
        failures += not check("清单带范围", item["scope"] == "point", item)
        failures += not check("清单名是 plan/文件", item["name"].startswith("TK-1/"), item)
        failures += not check("边车写了时长", item["durationSec"] >= 1.0, item)

    # 2) 旧版「全程录像」（scope=task）：老客户端还在用，继续兼容；
    #    切片长度按请求走（路线动作可配）
    ARGS.clear()
    r = record.start({"planId": "TK-2", "scope": "task", "segmentSeconds": 120,
                      "maxSeconds": 60})
    failures += not check("全程录像 start ok", r.get("ok"), r)
    record.stop({"planId": "TK-2", "reason": "任务终态"})
    failures += not check("全程录像落到清单", len(record.list_files("TK-2")) == 1)
    args = ARGS[-1] if ARGS else []
    failures += not check(
        "切片长度进 ffmpeg",
        "-segment_time" in args
        and args[args.index("-segment_time") + 1] == "120",
        args,
    )

    # 2.0) 航段录像（2026-11-26 起的段级录像）：文件名带 leg<起点下标>，
    #      切片长度按请求走，stop 按 scope+起点下标只收这一段。
    ARGS.clear()
    r = record.start({"planId": "TK-LEG", "scope": "leg", "pointIndex": 2,
                      "pointName": "航段 3→4", "segmentSeconds": 90})
    failures += not check("航段录像 start ok", r.get("ok") and r.get("created"), r)
    args = ARGS[-1] if ARGS else []
    failures += not check(
        "航段录像按切片落盘",
        "-segment_time" in args and args[args.index("-segment_time") + 1] == "90",
        args,
    )
    failures += not check(
        # 不关掉时 mov 封装器会为「第一帧 DTS ≠ 0」写一段空 edit，mvhd 比画面长，
        # 进度条尾巴播不动（2026-11-26 现场 leg3-cam1 / mvhd 65.35s vs 实际 52.4s）
        "航段切片关掉 edit list（use_editlist=0）",
        "-segment_format_options" in args
        and "use_editlist=0" in args[args.index("-segment_format_options") + 1]
        and "+faststart" in args[args.index("-segment_format_options") + 1],
        args,
    )
    failures += not check(
        "航段文件名带起点下标",
        any("/leg2-" in path for path in PROCS),
        PROCS,
    )
    # 同计划再挂一条别的航段：停 2 号不能把 3 号一起停掉
    record.start({"planId": "TK-LEG", "scope": "leg", "pointIndex": 3,
                  "pointName": "航段 4→5", "segmentSeconds": 90})
    stopped = record.stop({"planId": "TK-LEG", "scope": "leg", "pointIndex": 2,
                           "reason": "到达下一个航点"})
    failures += not check("航段 stop 只停这一段", stopped.get("stopped") == 1, stopped)
    items = record.list_files("TK-LEG")
    failures += not check("航段录像落到清单", len(items) == 1, items)
    if items:
        failures += not check("清单 scope=leg", items[0]["scope"] == "leg", items)
        failures += not check("清单带起点下标", items[0]["pointIndex"] == 2, items)
        failures += not check("清单带航段名", items[0]["pointName"] == "航段 3→4", items)
    record.stop({"planId": "TK-LEG", "reason": "任务终态"})

    # 2.0.1) 航段切片：每片一个文件 + 每片自己的时长（2026-11-26 现场 bug）
    #       以前模板只有开始时间戳、没有片段序号，segment 每片都写同一个文件、
    #       后一片覆盖前一片，一段 8 分钟的航段最后只剩最后几秒。
    ARGS.clear()
    global SEGMENTS
    SEGMENTS = 3
    r = record.start({"planId": "TK-SEG", "scope": "leg", "pointIndex": 0,
                      "pointName": "航段 1→2", "segmentSeconds": 60})
    failures += not check("切片用例 start ok", r.get("ok") and r.get("created"), r)
    args = ARGS[-1] if ARGS else []
    failures += not check("切片模板带片段序号 %03d", args[-1].endswith("-%03d.mp4"), args[-1])
    failures += not check("切片不再用 -strftime（会把 %03d 当日期吃掉）",
                          "-strftime" not in args, args)
    failures += not check("切片带 segment_list（每片真实时长）",
                          "-segment_list" in args, args)
    list_path = args[args.index("-segment_list") + 1] if "-segment_list" in args else ""
    record.stop({"planId": "TK-SEG", "scope": "leg", "pointIndex": 0,
                 "reason": "到达下一个航点"})
    items = record.list_files("TK-SEG")
    failures += not check("切 3 片就有 3 个片段（不互相覆盖）", len(items) == 3, items)
    failures += not check("每片都带自己的时长（不是整段航段）",
                          sorted(item["durationSec"] for item in items) == [4.0, 4.0, 4.0],
                          [item["durationSec"] for item in items])
    failures += not check("切片清单用完就删", not list_path or not os.path.exists(list_path),
                          list_path)
    SEGMENTS = 1

    # 2.1) 同一计划同时有全程（老 scope=task）+ 航点两路：各自只认自己的文件（前缀 + 相机）
    record.start({"planId": "TK-3", "scope": "task", "segmentSeconds": 60})
    record.start({"planId": "TK-3", "scope": "point", "pointIndex": 0, "seconds": 5})
    # 全程录像的第二段在航点录像开始之后才落盘（segment 切片就是这个时序）
    with open(os.path.join(tmp, "TK-3", "task-dogcam-20991231-000000.mp4"), "wb") as handle:
        handle.write(fake_mp4())
    record.stop({"planId": "TK-3", "scope": "point", "reason": "离开航点"})
    names = [item["name"] for item in record.list_files("TK-3")]
    failures += not check(
        "航点录像只收自己的片段",
        # 全程那一轮的片段此刻还在录，`list` 不会返回未收尾的文件（避免传半截）
        len(names) == 1 and "/pt0-" in names[0],
        names,
    )
    record.stop({"planId": "TK-3", "reason": "任务终态"})
    failures += not check("全程录像收尾后都在",
                          len(record.list_files("TK-3")) == 3,
                          record.list_files("TK-3"))

    # 2.2) 多选相机：同一航点同时录 3 路（左前 / 右前 / 正前）
    ARGS.clear()
    r = record.start({"planId": "TK-4", "scope": "point", "pointIndex": 1,
                      "seconds": 5, "cameras": ["cam0", "cam1", "dogcam"]})
    failures += not check("多路 start ok", r.get("ok") and r.get("created"), r)
    failures += not check("多路起了 3 个 job", len(record.status()["jobs"]) == 3,
                          record.status()["jobs"])
    sources = [args[args.index("-i") + 1] for args in ARGS]
    # 按 `stream_of()` 比，**别写死 `dogcam`**：720p30 档位下它解析成 `dogcam720`
    # （现场板子是 `dogcam_profile=720p30`，写死会在板端白白报 FAIL）
    failures += not check(
        "每路各拉各的流",
        sorted(url.rsplit("/", 1)[-1] for url in sources)
        == sorted(record.stream_of(cam) for cam in ("cam0", "cam1", "dogcam")),
        sources,
    )
    failures += not check(
        "三路输出文件名各不相同",
        len({args[-1] for args in ARGS}) == 3,
        [args[-1] for args in ARGS],
    )
    failures += not check(
        # 航点录像是单文件（不走 segment），同样不能留空 edit
        "航点录像也关掉 edit list（-use_editlist 0）",
        all("-use_editlist" in args
            and args[args.index("-use_editlist") + 1] == "0" for args in ARGS),
        ARGS,
    )
    again = record.start({"planId": "TK-4", "scope": "point", "pointIndex": 1,
                          "seconds": 5, "cameras": ["cam0", "cam1", "dogcam"]})
    failures += not check("多路重复 start 幂等", again.get("created") is False, again)
    failures += not check("幂等时不起新 ffmpeg", len(ARGS) == 3, len(ARGS))
    record.stop({"planId": "TK-4", "scope": "point", "pointIndex": 1,
                 "reason": "离开航点"})
    multi = record.list_files("TK-4")
    failures += not check("多路停止后 3 段都在", len(multi) == 3, multi)
    failures += not check(
        "每段的边车都记了相机",
        sorted(item["camera"] for item in multi) == ["cam0", "cam1", "dogcam"],
        [(item["name"], item["camera"]) for item in multi],
    )
    failures += not check(
        "文件名带相机名（回传后能分清哪一路）",
        all("-%s-" % item["camera"] in item["name"] for item in multi),
        [item["name"] for item in multi],
    )

    # 2.3) 相机名写错只丢那一路；超出上限截断（不能整条录像失败）
    ARGS.clear()
    r = record.start({"planId": "TK-5", "scope": "point", "pointIndex": 0,
                      "cameras": ["cam9", "cam0"]})
    failures += not check("坏相机名不影响好的一路", r.get("ok"), r)
    failures += not check("坏相机名进 rejected", "cam9" in (r.get("rejected") or []), r)
    failures += not check("只起了 1 路", len(ARGS) == 1, ARGS)
    record.stop({"planId": "TK-5", "reason": "清理"})
    ARGS.clear()

    # 2.35) 空壳 mp4（起了 ffmpeg 但窗口内等不到本路关键帧）**不能当录像**：
    #       现场 2026-11-26 17:44 `TK-1126-20` P1（配 dogcam + cam1 录 5s）：狗头 3.1s，
    #       右前 `pt0-cam1-20261126-174456.mp4` 只有 **293 B / mvhd=0**（一帧都没有），
    #       却因为「读不到 mvhd 就退回墙钟时长」被写成 5.3s 的正常录像，回传到平台就是
    #       一段打不开、时长还不对的视频。现在要：删空壳 + 不进清单 + stop 回执带原因。
    global EMPTY_CAPTURE
    EMPTY_CAPTURE = True
    PROCS.clear()
    ARGS.clear()
    r = record.start({"planId": "TK-EMPTY", "scope": "point", "pointIndex": 0,
                      "seconds": 5, "cameras": ["cam0", "dogcam"]})
    failures += not check("空壳用例：起录仍 ok（不报错，不影响巡检）", r.get("ok"), r)
    time.sleep(1.1)
    stopped = record.stop({"planId": "TK-EMPTY", "reason": "离开航点"})
    failures += not check(
        "空壳不进待回传清单（不再回传打不开的视频）",
        record.list_files("TK-EMPTY") == [],
        record.list_files("TK-EMPTY"),
    )
    failures += not check(
        "stop 回执里说明哪几路没录到画面",
        sorted(item.get("camera") for item in (stopped.get("discarded") or []))
        == ["cam0", "dogcam"],
        stopped,
    )
    failures += not check(
        "空壳文件已从板端删掉（不占 SSD / 不误传）",
        not any(os.path.exists(path) for path in PROCS),
        PROCS,
    )
    failures += not check(
        "空壳不写边车",
        not any(os.path.exists(path + ".json") for path in PROCS),
        PROCS,
    )
    EMPTY_CAPTURE = False
    PROCS.clear()
    ARGS.clear()

    # 2.4) 四路推流的关键帧间隔：录像（`-c copy`）只能从**本路的下一个关键帧**开始写，
    #      GOP 一大，同一时刻起的多路「内容起点」就各差一个 GOP（现场 2026-11-26：
    #      前置 ~2s 出画面、左前 / 右前 8~13s 才轮到 → 两路内容差 ~10s，而且 `-t`
    #      的窗口也被吃掉一截）。这里钉住「默认 = 输出帧率 = 1s」。
    gop_cmd = stream._build_cmd({"device": "/dev/usbcam0", "stream": "cam0"})
    gop = config.CAPTURE_GOP
    failures += not check(
        "四路推流带 -g（每秒一个关键帧）",
        "-g" in gop_cmd and gop_cmd[gop_cmd.index("-g") + 1] == str(gop),
        gop_cmd,
    )
    failures += not check(
        "默认 GOP = 1 秒（= 输出帧率）",
        gop == config.CAPTURE_NOUT_FPS and gop >= 1,
        (gop, config.CAPTURE_NOUT_FPS),
    )
    # dogcam（relay 重编码推 `dogcam720`）必须用同一份 `-g`：写死 30 配上
    # `CAPTURE_OUT_FPS=15` 就是 2s 一个关键帧，录像里它会比 cam0/cam1 晚一倍
    # （2026-11-26 部署实测：同为 15s 航点录像，dogcam 13.6s / cam1 15.0s）。
    relay_cmd = relay._build_cmd("rtsp://127.0.0.1:8554/test", "libx264")
    failures += not check(
        "dogcam relay 的关键帧间隔与四路一致",
        relay_cmd[relay_cmd.index("-g") + 1] == str(gop),
        relay_cmd,
    )
    too_many = ["cam0", "cam1", "cam2", "cam3", "dogcam", "dog-ipcam"]
    r = record.start({"planId": "TK-6", "scope": "point", "pointIndex": 0,
                      "cameras": too_many})
    cap = max(1, int(getattr(config, "RECORD_MAX_CAMERAS", 4)))
    failures += not check("超上限只起 %d 路" % cap, len(ARGS) == cap, len(ARGS))
    failures += not check("超出的进 rejected", len(r.get("rejected") or []) == len(too_many) - cap, r)
    record.stop({"planId": "TK-6", "reason": "清理"})
    # 不传相机时退到 RECORD_SOURCE_CAM（历史路线行为不变）
    ARGS.clear()
    record.start({"planId": "TK-7", "scope": "point", "pointIndex": 0})
    failures += not check("不配相机仍录默认源", len(ARGS) == 1, ARGS)
    record.stop({"planId": "TK-7", "reason": "清理"})
    # 点了相机却一路都不可用：**不拿默认源顶替**（否则录成另一路画面还不报错，
    # 2026-11-25 现场“航段配右前、录下来是正前”就是这个坑）
    ARGS.clear()
    r = record.start({"planId": "TK-8", "scope": "point", "pointIndex": 0,
                      "cameras": ["cam9"]})
    failures += not check("相机全不可用时 ok=false", r.get("ok") is False, r)
    failures += not check("相机全不可用时不启 ffmpeg", len(ARGS) == 0, ARGS)
    failures += not check("相机全不可用时回执带 rejected",
                          "cam9" in (r.get("rejected") or []), r)
    ARGS.clear()
    r = record.start({"planId": "TK-9", "scope": "leg", "pointIndex": 0,
                      "segmentSeconds": 60, "cameras": ["nope", ""]})
    failures += not check("航段相机全不可用时 ok=false", r.get("ok") is False, r)
    failures += not check("航段相机全不可用时不启 ffmpeg", len(ARGS) == 0, ARGS)

    # 3) 删除：只认 plan/文件名，越界一律拒绝
    name = files[0]["name"] if files else ""
    bad = record.delete({"names": ["../../etc/passwd.mp4"]})
    failures += not check("拒绝越界文件名", bad.get("ok") is False, bad)
    bad = record.delete({"names": ["TK-1/../x.mp4"]})
    failures += not check("拒绝带 .. 的文件名", bad.get("ok") is False, bad)
    if name:
        gone = record.delete({"names": [name]})
        failures += not check("删除成功", gone.get("ok") and gone.get("deleted") == 1, gone)
        failures += not check("删完计划目录收掉",
                              not os.path.isdir(os.path.join(tmp, "TK-1")))

    # 4) 非法计划号不落盘
    r = record.start({"planId": "../evil", "scope": "point", "pointIndex": 0})
    failures += not check("非法 planId 被拒", r.get("ok") is False, r)

    # 5) 超期清盘：老文件删，新文件留
    task_files = record.list_files("TK-2")
    failures += not check("全程录像文件在", len(task_files) == 1, task_files)
    if task_files:
        path = task_files[0]["path"]
        old = time.time() - (config.RECORD_RETENTION_HOURS + 1) * 3600
        os.utime(path, (old, old))
        removed = record.sweep()
        failures += not check("超期清盘删了 1 段", removed == 1, removed)
        failures += not check("清完清单为空", record.list_files("TK-2") == [])

    # 6) 真 ffmpeg 跑一遍切片（本机没装 ffmpeg / ffprobe 就跳过）：
    #    上面把 Popen 换成了假进程，这里先换回来
    subprocess.Popen = real_popen
    failures += real_segment_naming(tmp)
    failures += real_mp4_timeline(tmp)

    print("record selftest: %s" % ("FAILED" if failures else "ok"))
    return 1 if failures else 0


def lavfi_encoder(ffmpeg):
    """lavfi 假相机没法 `-c copy`，挑本机真有的 H264 编码器。

    板端（现场 `.246`）**没有 libx264**，只有 `h264_rkmpp` / `h264_v4l2m2m`；
    写死 libx264 会让两条真 ffmpeg 用例在板子上直接 FAIL（exit 8），
    而文档恰恰要求「改录像先跑这个 selftest」。
    """
    import subprocess

    try:
        listed = subprocess.run([ffmpeg, "-hide_banner", "-encoders"],
                                capture_output=True, text=True, timeout=30).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    for name in ("libx264", "h264_rkmpp", "h264_v4l2m2m"):
        if name in listed:
            return name
    return None


def real_segment_naming(tmp):
    """用真 ffmpeg 验「切 3 片必须落 3 个文件」。

    假进程只能证明「参数里写了 `%03d`」，证明不了 ffmpeg 真的按它分文件；
    而这个 bug 就是「模板没序号 → 后一片覆盖前一片」，只有真 ffmpeg 能坐实。
    输入换成 lavfi 假相机（本机不需要 mediamtx / 板子），输出侧参数全部取
    `record._ffmpeg_cmd()` 生成的那一份。
    """
    import subprocess

    ffmpeg = shutil.which("ffmpeg")
    ffprobe = shutil.which("ffprobe")
    if not (ffmpeg and ffprobe):
        print("  skip 真 ffmpeg 切片（本机没有 ffmpeg/ffprobe）")
        return 0
    folder = os.path.join(tmp, "real")
    os.makedirs(folder, exist_ok=True)
    template = os.path.join(folder, "leg0-cam1-20260101-000000-%03d.mp4")
    list_path = os.path.join(folder, "leg0-cam1-20260101-000000.csv")
    args = record._ffmpeg_cmd(template, "leg", 9, 3,
                              "rtsp://127.0.0.1:8554/cam1", list_path)
    # 输出侧参数（`-t` 之后：编码 + 切片命名 + 清单 + faststart）原样复用
    tail = args[args.index("-t"):]
    copy_at = tail.index("-c") if "-c" in tail else -1
    enc = lavfi_encoder(ffmpeg)
    if enc is None:
        print("  skip 真 ffmpeg 切片（本机没有可用的 H264 编码器）")
        return 0
    if copy_at >= 0:
        # `-c copy` 对 lavfi 不适用：换成本机真有的编码器（不影响切片命名相关参数）
        enc_args = ["-c:v", enc, "-b:v", "2000k", "-g", "15"]
        if enc == "libx264":
            enc_args += ["-preset", "ultrafast", "-pix_fmt", "yuv420p"]
        tail[copy_at:copy_at + 2] = enc_args
    cmd = [ffmpeg, "-hide_banner", "-loglevel", "error", "-f", "lavfi",
           "-i", "testsrc2=size=320x180:rate=15"] + tail
    failures = 0
    try:
        subprocess.run(cmd, check=True, timeout=180,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    except (subprocess.SubprocessError, OSError) as error:
        print("  FAIL 真 ffmpeg 切片跑不起来 %s" % error)
        return 1
    names = sorted(name for name in os.listdir(folder) if name.endswith(".mp4"))
    failures += not check("真 ffmpeg：切 3 片落 3 个文件（不互相覆盖）",
                          len(names) == 3, names)
    durations = []
    for name in names:
        probe = subprocess.run(
            [ffprobe, "-v", "error", "-show_entries", "format=duration",
             "-of", "default=nw=1:nk=1", os.path.join(folder, name)],
            capture_output=True, text=True)
        try:
            durations.append(float(probe.stdout.strip()))
        except ValueError:
            durations.append(0.0)
    failures += not check("真 ffmpeg：3 片加起来是整段（没有被覆盖掉的部分）",
                          sum(durations) > 8.0, durations)
    listed = record._segment_durations(list_path)
    failures += not check("切片清单能读出每片时长",
                          len(listed) == len(names), (listed, names))
    return failures


def real_mp4_timeline(tmp):
    """真 ffmpeg 验「进度条尾巴能播」：mvhd 必须等于内容时长（没有空 edit）。

    现场 bug（2026-11-26 `leg3-cam1-20261126-144815-000.mp4`）：ffmpeg 从 RTSP 拉流时
    第一帧 DTS 不是 0（要等源的下一个关键帧），mov 封装器把这段偏移写成**空 edit**，
    于是 `mvhd.duration = 内容 + 偏移`（65.35s vs 52.4s）。播放器按 mvhd 画进度条、
    按第一帧开播——最后那段偏移只有进度条没有画面。

    这里用 `-itsoffset 5` 复刻「第一帧 DTS ≠ 0」：先确认**不带** `use_editlist=0` 时
    真能造出 `mvhd > 内容`（否则这条用例没验到东西，直接 skip），再用
    `record._ffmpeg_cmd()` 的真实输出参数验 `mvhd == 内容` 且 `start_time == 0`。
    """
    import subprocess

    ffmpeg = shutil.which("ffmpeg")
    ffprobe = shutil.which("ffprobe")
    if not (ffmpeg and ffprobe):
        print("  skip 真 ffmpeg 验 mvhd（本机没有 ffmpeg/ffprobe）")
        return 0
    folder = os.path.join(tmp, "timeline")
    os.makedirs(folder, exist_ok=True)
    # `record._ffmpeg_cmd()` 的输出侧参数原样复用（`-t` 之后那段）
    args = record._ffmpeg_cmd(os.path.join(folder, "pt0-cam1-20260101-000000.mp4"),
                              "point", 12, 12, "rtsp://127.0.0.1:8554/cam1")
    tail = args[args.index("-t"):]
    copy_at = tail.index("-c") if "-c" in tail else -1
    enc = lavfi_encoder(ffmpeg)
    if enc is None:
        print("  skip 真 ffmpeg 验 mvhd（本机没有可用的 H264 编码器）")
        return 0
    if copy_at >= 0:
        enc_args = ["-c:v", enc, "-b:v", "2000k", "-g", "15"]
        if enc == "libx264":
            enc_args += ["-preset", "ultrafast", "-pix_fmt", "yuv420p"]
        tail[copy_at:copy_at + 2] = enc_args

    def run(path, extra):
        # `extra` 放在 tail 之后：同一个选项后写的那份生效（对照组要盖掉 `use_editlist 0`）
        cmd = ([ffmpeg, "-hide_banner", "-loglevel", "error",
                "-itsoffset", "5", "-f", "lavfi", "-i",
                "testsrc2=size=320x180:rate=15"] + tail[:-2] + extra + ["-y", path])
        subprocess.run(cmd, check=True, timeout=180,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        probe = subprocess.run(
            [ffprobe, "-v", "error", "-show_entries", "format=start_time,duration",
             "-of", "default=nw=1", path], capture_output=True, text=True)
        info = dict(
            line.split("=", 1) for line in probe.stdout.strip().splitlines() if "=" in line
        )
        content = float(info.get("duration") or 0)
        start = float(info.get("start_time") or 0)
        return content, start, record._mp4_duration(path)

    failures = 0
    control = os.path.join(folder, "control.mp4")
    content, _start, mvhd = run(control, ["-use_editlist", "1"])
    if mvhd is None or mvhd < content + 4:
        print("  skip 真 ffmpeg 验 mvhd（造不出空 edit：content=%s mvhd=%s）"
              % (content, mvhd))
        return failures
    failures += not check("造出空 edit：mvhd 比画面长（复现现场 bug）",
                          mvhd > content + 4, (content, mvhd))

    fixed = os.path.join(folder, "fixed.mp4")
    content, start, mvhd = run(fixed, [])
    failures += not check("航点录像的 mvhd == 画面时长（进度条尾巴能播）",
                          mvhd is not None and abs(mvhd - content) <= 0.2,
                          (content, mvhd))
    failures += not check("mvhd 时间轴从 0 开始（没有空 edit）",
                          abs(start) < 0.05, start)
    return failures


if __name__ == "__main__":
    raise SystemExit(main())
