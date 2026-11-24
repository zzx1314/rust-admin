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

    # 2) 全程录像：同一计划不同 scope 共存；切片长度按请求走（路线动作可配）
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

    # 2.1) 同一计划同时有全程 + 航点两路：各自只认自己的文件
    record.start({"planId": "TK-3", "scope": "task", "segmentSeconds": 60})
    record.start({"planId": "TK-3", "scope": "point", "pointIndex": 0, "seconds": 5})
    # 全程录像的第二段在航点录像开始之后才落盘（segment 切片就是这个时序）
    with open(os.path.join(tmp, "TK-3",
                           "task-%s.mp4" % time.strftime("%Y%m%d-%H%M%S")), "wb") as handle:
        handle.write(b"\x00\x00\x00\x18ftypmp42" + b"\x00" * 128)
    record.stop({"planId": "TK-3", "scope": "point", "reason": "离开航点"})
    names = [item["name"] for item in record.list_files("TK-3")]
    failures += not check(
        "航点录像只收自己的片段",
        len(names) == 1 and "/pt0-" in names[0],
        names,
    )
    record.stop({"planId": "TK-3", "reason": "任务终态"})
    failures += not check("全程录像收尾后两路都在",
                          len(record.list_files("TK-3")) == 2,
                          record.list_files("TK-3"))

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
