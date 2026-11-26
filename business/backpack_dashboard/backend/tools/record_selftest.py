"""航点 / 航段录像状态机自检（不需要板子、相机、ffmpeg）

改 `record.py` 的计划 / 落盘 / 清单 / 删除 / 超期清理逻辑后必跑：

    python3 business/backpack_dashboard/backend/tools/record_selftest.py

怎么在没有 ffmpeg / RTSP 的机器上验：
`record.start()` 里真正会碰硬件的只有 `subprocess.Popen(ffmpeg …)`，这里把它换成
「立刻在模板路径写一个假 mp4、`wait()` 挂到收到停止信号」的假进程，其余代码
（边车、清单、删除、越界校验、清盘）全部走真实实现。
"""
import os
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import config  # noqa: E402
import record  # noqa: E402

PROCS = []
ARGS = []


class FakeProc:
    """假 ffmpeg：起手写一个 mp4，`wait()` 一直挂到收到停止信号。"""

    def __init__(self, args, **_kwargs):
        self.pid = 424242
        # 每个 job 一个停止信号：停「航点录像」不能把同计划的「全程录像」一起停掉
        self.done = threading.Event()
        ARGS.append(list(args))
        self._code = None
        template = args[-1]
        path = time.strftime(template, time.localtime())
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as handle:
            handle.write(b"\x00\x00\x00\x18ftypmp42" + b"\x00" * 256)
        self.path = path
        PROCS.append(path)

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

    # 2.1) 同一计划同时有全程（老 scope=task）+ 航点两路：各自只认自己的文件（前缀 + 相机）
    record.start({"planId": "TK-3", "scope": "task", "segmentSeconds": 60})
    record.start({"planId": "TK-3", "scope": "point", "pointIndex": 0, "seconds": 5})
    # 全程录像的第二段在航点录像开始之后才落盘（segment 切片就是这个时序）
    with open(os.path.join(tmp, "TK-3", "task-dogcam-20991231-000000.mp4"), "wb") as handle:
        handle.write(b"\x00\x00\x00\x18ftypmp42" + b"\x00" * 128)
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
    failures += not check(
        "每路各拉各的流",
        sorted(url.rsplit("/", 1)[-1] for url in sources) == ["cam0", "cam1", "dogcam"],
        sources,
    )
    failures += not check(
        "三路输出文件名各不相同",
        len({args[-1] for args in ARGS}) == 3,
        [args[-1] for args in ARGS],
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

    print("record selftest: %s" % ("FAILED" if failures else "ok"))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
