"""航点拍照自检（不需要板子、相机、ffmpeg）

改 `photo.py` 的抓拍 / 落盘 / 清单 / 删除 / 超期清理逻辑后必跑：

    python3 business/backpack_dashboard/backend/tools/photo_selftest.py

怎么在没有 ffmpeg / RTSP 的机器上验：
`photo.capture()` 里真正会碰硬件的只有 `subprocess.run(ffmpeg …)`，这里把它换成
「按输出模板写几个假 jpg 文件」的假实现，其余代码（目录分层、文件名解析、清单、
删除、空目录收口、超期清盘、磁盘下限、相机白名单）全部走真实实现。
"""
import os
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import config  # noqa: E402
import photo  # noqa: E402

ARGS = []


class FakeResult:
    def __init__(self, returncode=0, stderr=b""):
        self.returncode = returncode
        self.stderr = stderr


def fake_run(cmd, **_kwargs):
    """假 ffmpeg：按命令里的输出模板写 jpg（`-frames:v N` 决定写几张）。"""
    ARGS.append(list(cmd))
    frames = 1
    if "-frames:v" in cmd:
        frames = int(cmd[cmd.index("-frames:v") + 1])
    template = cmd[-1]
    written = []
    if "%03d" in template:
        for index in range(1, frames + 1):
            path = template % index
            with open(path, "wb") as handle:
                handle.write(b"\xff\xd8\xff\xe0" + b"\x00" * 64)
            written.append(path)
    else:
        with open(template, "wb") as handle:
            handle.write(b"\xff\xd8\xff\xe0" + b"\x00" * 64)
        written.append(template)
    return FakeResult(0, b"")


def check(name, condition, detail=""):
    if condition:
        print("  ok   %s" % name)
        return True
    print("  FAIL %s %s" % (name, detail))
    return False


def main():
    import subprocess

    tmp = tempfile.mkdtemp(prefix="photo-selftest-")
    config.PHOTO_DIR = tmp
    config.PHOTO_MIN_FREE_MB = 0
    config.PHOTO_MAX_FRAMES = 5
    config.PHOTO_SOURCE_CAM = "dogcam"
    subprocess.run = fake_run

    failures = 0

    # 1) 抓一张：按 <计划>/pt<n>/ 落盘，返回清单字段齐全
    print("[1] 单张抓拍")
    ARGS.clear()
    r = photo.capture({"planId": "TK-1", "pointIndex": 2, "pointName": "P3",
                       "camera": "dogcam", "frames": 1})
    failures += not check("返回 ok", r.get("ok") is True, r)
    files = r.get("files") or []
    failures += not check("落盘 1 张", len(files) == 1, files)
    failures += not check(
        "name 是 <计划>/pt<下标>/<文件>.jpg",
        files and files[0]["name"].startswith("TK-1/pt2/") and files[0]["name"].endswith(".jpg"),
        files,
    )
    failures += not check("path 指向真实文件", files and os.path.exists(files[0]["path"]))
    failures += not check("带了相机与抓拍时间",
                          files and files[0]["camera"] == "dogcam" and bool(files[0]["capturedAt"]))
    failures += not check("ffmpeg 用 TCP 拉 RTSP 且限帧",
                          ARGS and "-rtsp_transport" in ARGS[0]
                          and "tcp" in ARGS[0] and "-frames:v" in ARGS[0], ARGS)
    failures += not check("单张不抽样（没有 -vf）", ARGS and "-vf" not in ARGS[0], ARGS)

    # 2) 多帧：按 PHOTO_FPS 抽样
    print("[2] 多帧抓拍")
    ARGS.clear()
    r = photo.capture({"planId": "TK-1", "pointIndex": 3, "camera": "cam2", "frames": 3})
    failures += not check("落盘 3 张", len(r.get("files") or []) == 3, r)
    failures += not check("多帧带 fps 抽样",
                          ARGS and "-vf" in ARGS[0] and "fps=" in ARGS[0][ARGS[0].index("-vf") + 1],
                          ARGS)
    failures += not check("相机名进了文件名", all("cam2" in f["name"] for f in r["files"]))

    # 3) 清单：能反解出计划 / 航点 / 相机，按 planId 过滤
    print("[3] 清单")
    all_files = photo.list_files()
    failures += not check("清单 4 张", len(all_files) == 4, len(all_files))
    failures += not check(
        "按计划过滤", len(photo.list_files("TK-1")) == 4 and photo.list_files("TK-X") == [],
        [item["name"] for item in all_files],
    )
    point3 = [item for item in all_files if item["pointIndex"] == 3]
    failures += not check("相机从文件名反解", point3 and point3[0]["camera"] == "cam2", point3)
    point2 = [item for item in all_files if item["pointIndex"] == 2]
    failures += not check(
        "航点名从 points.json 带出来（客户端重启后回传也有名字）",
        point2 and point2[0]["pointName"] == "P3",
        point2,
    )

    # 4) 相机白名单：不认识的相机直接拒绝（别让现场配错了还以为在拍）
    print("[4] 相机白名单")
    r = photo.capture({"planId": "TK-1", "pointIndex": 0, "camera": "cam9", "frames": 1})
    failures += not check("未知相机 ok=false", r.get("ok") is False, r)
    failures += not check("错误信息里有可选项", "cam0" in (r.get("error") or ""), r.get("error"))
    r = photo.capture({"planId": "../etc", "pointIndex": 0})
    failures += not check("非法计划号拒绝", r.get("ok") is False, r)

    # 5) 抓拍失败（流没在推）：只回错，不留空目录里的残留
    print("[5] 抓拍失败")
    photo.subprocess.run = lambda *_a, **_k: FakeResult(1, b"Connection refused")
    r = photo.capture({"planId": "TK-2", "pointIndex": 0, "camera": "dogcam"})
    failures += not check("ok=false", r.get("ok") is False, r)
    failures += not check("错误里有 ffmpeg 输出", "Connection refused" in (r.get("error") or ""),
                          r.get("error"))
    failures += not check("失败的这一轮没有文件", not photo.list_files("TK-2"))
    photo.subprocess.run = fake_run

    # 6) 删除：路径白名单 + 空目录收口
    print("[6] 删除")
    r = photo.delete({"names": [files[0]["name"]]})
    failures += not check("删除成功", r.get("ok") is True and r.get("deleted"), r)
    failures += not check("清单少一张", len(photo.list_files()) == 3, photo.list_files())
    failures += not check(
        "空目录收掉", not os.path.isdir(os.path.join(tmp, "TK-1", "pt2")),
        os.listdir(os.path.join(tmp, "TK-1")),
    )
    r = photo.delete({"names": ["TK-1/../etc/passwd.jpg"]})
    failures += not check("路径穿越拒绝", r.get("ok") is False, r)
    r = photo.delete({"names": ["TK-1/pt3/x.gif"]})
    failures += not check("非 jpg 拒绝", r.get("ok") is False, r)
    r = photo.delete({"names": ["TK-1/pt3/不存在的照片.jpg"]})
    failures += not check("已经不在了也算成功（客户端重试是常态）", r.get("ok") is True, r)

    # 7) 全部传完：计划目录连 points.json 一起收掉（别留空壳）
    print("[7] 传完收目录")
    rest = [item["name"] for item in photo.list_files()]
    r = photo.delete({"names": rest})
    failures += not check("全部删除", r.get("ok") is True and not photo.list_files(), r)
    failures += not check("计划目录（含 points.json）已收掉",
                          not os.path.isdir(os.path.join(tmp, "TK-1")),
                          os.listdir(tmp))

    # 8) 超期清盘
    print("[8] 超期清盘")
    photo.capture({"planId": "TK-9", "pointIndex": 0, "camera": "dogcam", "frames": 3})
    old = time.time() - 100 * 3600
    removed = 0
    for item in photo.list_files():
        os.utime(item["path"], (old, old))
        removed += 1
    failures += not check("清掉了超期照片", removed == 3 and photo.sweep() == removed, removed)
    failures += not check("清完清单为空", photo.list_files() == [], photo.list_files())

    # 9) status 字段
    print("[9] 运行态")
    st = photo.status()
    failures += not check("status 带目录 / 上限 / 相机清单",
                          st.get("dir") == tmp and st.get("max_frames") == 5
                          and "dogcam" in st.get("allowed_cameras", []), st)

    print()
    if failures:
        print("失败 %d 项" % failures)
        return 1
    print("全部通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())
