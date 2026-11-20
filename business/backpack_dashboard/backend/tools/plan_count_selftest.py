#!/usr/bin/env python3
"""航点计划模式自检：计划 ID 存证目录 + 两类任务（存在性 / 数量）判定。

两类任务（`check_type`）：

- `exist`（检测是否存在）：命中即存证；数量不参与判定，未命中只算「没找到」；
- `count`（检测数量是否正确）：命中后把检出数量与 `expect_count` 比对，不一致告警；
  窗口结束仍未命中 = 0 ≠ N，同样算数量不一致。

为什么单独一个工具：这条逻辑（`detect.DetectManager._plan_frame` / `_plan_flush` /
`_plan_miss`）是纯状态机，跟推理、ffmpeg 无关，但现场排「为什么没告警 / 为什么数量
判错」时必须能脱离整条链路单独验证。这里把 `evidence.save_frame` 换成「直接写文件」
的桩，其余代码路径与板端完全一致。

用法（板端或开发机都行，Python 3.8+）：

    python3 backend/tools/plan_count_selftest.py
"""
import os
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
BACKEND = os.path.dirname(HERE)
sys.path.insert(0, BACKEND)

import config  # noqa: E402
import detect  # noqa: E402
import evidence  # noqa: E402


def _install_stubs(root):
    """把落盘改成「桩」：不跑 ffmpeg，也不碰 /tmp 下的真实帧文件。"""
    config.DETECT_JOBS_DIR = os.path.join(root, "detect_jobs")
    config.DETECT_FRAME_DIR = os.path.join(root, "frames")
    # 媒体归档也指到临时目录：开发机上没有 /media/media-data（板端才有），
    # 真往那儿写真会 EACCES，自检就变成「一步三行报错」。
    config.MEDIA_DATA_DIR = os.path.join(root, "media-data", "picture")
    config.DETECT_PLAN_COUNT_SAMPLE = 3
    config.DETECT_PLAN_SAVE_FRAMES = 3
    detect.FRAME_DIR = config.DETECT_FRAME_DIR
    os.makedirs(config.DETECT_FRAME_DIR, exist_ok=True)

    def fake_save_frame(jpg_path, boxes, frame_w, frame_h, out_path):
        os.makedirs(os.path.dirname(out_path), exist_ok=True)
        with open(out_path, "wb") as f:
            f.write(b"\xff\xd8stub-jpeg")     # 内容不重要，只看文件是否生成
        return True

    evidence.save_frame = fake_save_frame


def _box(i=0):
    return {"label": "extinguisher", "zh": "灭火器", "color": "#E24B4A", "conf": 0.9,
            "x1": 10 + i * 20, "y1": 10, "x2": 60 + i * 20, "y2": 80,
            "count": 1, "text": "灭火器 1处 0.90"}


def _run(manager, cam, total, events):
    """推一帧命中（`total` 个框），返回本次新增的 plan_hit（可能没有）。"""
    before = len(events)
    boxes = [_box(i) for i in range(total)]
    manager._plan_frame(cam, b"\xff\xd8stub-jpeg", boxes,
                        {"extinguisher": total}, {"w": 640, "h": 360})
    hits = [e for e in events[before:] if e.get("type") == "plan_hit"]
    return hits[0] if hits else None


def _sample(manager, cam, total, events):
    """按 DETECT_PLAN_COUNT_SAMPLE 帧推完一轮采样，返回最终 plan_hit。"""
    hit = None
    for _ in range(max(1, config.DETECT_PLAN_COUNT_SAMPLE)):
        hit = _run(manager, cam, total, events) or hit
    return hit


def main():
    root = tempfile.mkdtemp(prefix="plan_selftest_")
    _install_stubs(root)

    manager = detect.manager
    events = []
    manager.set_publisher(events.append)
    manager.cfg.update({"task": "firextinguisher", "labels": None, "conf": 0.25,
                        "cams": ["dogcam"], "fps": 3})

    failures = []

    def check(name, ok, detail=""):
        print(("  OK   " if ok else "  FAIL ") + name + (("  <- " + detail) if detail and not ok else ""))
        if not ok:
            failures.append(name)

    def miss_of(point_index):
        """最近一条该航点的 plan_miss（同一个下标可能先后用于不同用例）。"""
        found = [e for e in events if e.get("type") == "plan_miss"
                 and e.get("point_index") == point_index]
        return found[-1] if found else None

    # ---- 任务类型 1：只判是否存在（数量即使误传也不参与判定）----
    manager.set_plan("SELFTEST-PLAN", 0, 3, "exist")
    hit = _sample(manager, "dogcam", 2, events)      # 检出 2 个（与 3 不同，但存在性任务不判）
    check("存在性：生成 plan_hit", hit is not None)
    if hit:
        check("存在性：check_type=exist", hit.get("check_type") == "exist", str(hit.get("check_type")))
        check("存在性：count_ok=None（不判数量）", hit.get("count_ok") is None, str(hit.get("count_ok")))
        check("存在性：count_alert=false", hit.get("count_alert") is False)
        check("存在性：存证目录 = 计划/航点",
              os.path.basename(os.path.dirname(hit["dir"])) == "SELFTEST-PLAN"
              and os.path.basename(hit["dir"]) == "pt0", hit["dir"])
        check("存在性：图片 + result.json 都在",
              all(os.path.isfile(os.path.join(hit["dir"], n))
                  for n in (os.path.basename(hit["evidence"]), "result.json")),
              hit["dir"])

    # ---- 任务类型 2：数量一致 ----
    manager.set_plan("SELFTEST-PLAN", 1, 3, "count")
    hit = _sample(manager, "cam0", 3, events)
    check("数量一致：生成 plan_hit", hit is not None)
    if hit:
        check("数量一致：found_count=3", hit.get("found_count") == 3, str(hit.get("found_count")))
        check("数量一致：count_ok=true", hit.get("count_ok") is True, str(hit.get("count_ok")))
        check("数量一致：count_alert=false", hit.get("count_alert") is False)

    # ---- 媒体归档（中台「同步媒体」从这里取图）----
    manager.set_plan("SELFTEST-MEDIA", 0, 2, "count")
    hit = _sample(manager, "dogcam", 2, events)
    check("媒体归档：plan_hit 带 media_dir", hit is not None and bool(hit.get("media_dir")))
    if hit:
        frames = hit.get("media_frames") or []
        check("媒体归档：固定保存 3 帧", len(frames) == 3, str(len(frames)))
        check("媒体归档：帧都落盘",
              all(os.path.isfile(os.path.join(hit["media_dir"], n)) for n in frames),
              hit["media_dir"])
        check("媒体归档：目录布局 = 计划/航点",
              os.path.basename(os.path.dirname(hit["media_dir"])) == "SELFTEST-MEDIA"
              and os.path.basename(hit["media_dir"]) == "pt0", hit["media_dir"])
        check("媒体归档：result.json 与图同目录",
              os.path.isfile(os.path.join(hit["media_dir"], "result.json")))
        # 临时存证（detect_jobs）与媒体归档是两份目录：前者会被 ai_cleanup 删，
        # 后者要留给中台媒体资源，两者不能是同一个路径。
        check("媒体归档与临时存证目录分离",
              os.path.abspath(hit["media_dir"]) != os.path.abspath(hit["dir"]),
              "%s vs %s" % (hit["media_dir"], hit["dir"]))

    # ---- 任务类型 2：数量不一致 ----
    manager.set_plan("SELFTEST-PLAN", 2, 5, "count")
    hit = _sample(manager, "cam0", 3, events)
    check("数量不一致：生成 plan_hit", hit is not None)
    if hit:
        check("数量不一致：count_ok=false", hit.get("count_ok") is False)
        check("数量不一致：count_alert=true", hit.get("count_alert") is True)

    # ---- 旧调用口径：只传 expect_count 就当数量任务 ----
    manager.set_plan("SELFTEST-LEGACY", 0, 2)
    hit = _sample(manager, "cam0", 1, events)
    check("旧口径：给了数量即数量任务",
          hit is not None and hit.get("check_type") == "count"
          and hit.get("count_alert") is True, str(hit and hit.get("check_type")))

    # ---- 未命中：存在性 = 没找到；数量 = 数量不一致（0 ≠ N）----
    manager.set_plan("SELFTEST-MISS", 0, None, "exist")
    manager._plan_flush("窗口结束")
    miss = miss_of(0)
    check("未命中(存在性)：发 plan_miss", miss is not None)
    if miss:
        check("未命中(存在性)：count_alert=false", miss.get("count_alert") is False)
        check("未命中(存在性)：found=false", miss.get("found") is False)

    manager.set_plan("SELFTEST-MISS2", 0, 3, "count")
    manager._plan_flush("窗口结束")
    miss = miss_of(0)
    check("未命中(数量)：发 plan_miss", miss is not None and miss.get("expect_count") == 3)
    if miss:
        check("未命中(数量)：count_alert=true", miss.get("count_alert") is True)
        check("未命中(数量)：result.json 已落盘",
              bool(miss.get("result_json")) and os.path.isfile(miss["result_json"]))

    # ---- 采样中途收尾（窗口结束 / 换计划）不丢证据 ----
    manager.set_plan("SELFTEST-PLAN", 3, 1, "count")
    first = _run(manager, "dogcam", 1, events)   # 只有第 1 帧，采样还没完
    check("采样中：暂不发 plan_hit", first is None)
    manager._plan_flush("窗口结束")
    hit = next((e for e in events if e.get("type") == "plan_hit"
                and e.get("point_index") == 3), None)
    check("采样中收尾：仍然落盘", hit is not None and bool(hit.get("result_json")))
    if hit:
        check("采样中收尾：reason 为窗口结束", hit.get("reason") == "窗口结束", str(hit.get("reason")))

    # ---- 换计划时上一个采样自动收尾 ----
    manager.set_plan("SELFTEST-PLAN", 4, 1, "count")
    _run(manager, "dogcam", 1, events)
    manager.set_plan("SELFTEST-OTHER", 0, 1, "exist")
    check("换计划：旧计划被收尾",
          any(e.get("type") == "plan_hit" and e.get("point_index") == 4 for e in events))

    listed = manager.list_jobs()["count"]
    check("list_jobs 递归列出计划结果", listed >= 4, str(listed))

    print()
    print("存证目录：", config.DETECT_JOBS_DIR)
    if failures:
        print("FAILED: %d 项（%s）" % (len(failures), "; ".join(failures)))
        return 1
    print("ALL OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
