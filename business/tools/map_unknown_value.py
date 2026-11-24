#!/usr/bin/env python3
"""把地图包里的「旧未知 205」改写成 128（本体眼里的未知 / 不可通行）。

为什么不能再写 205
------------------
本体按地图包里的 `map.yaml` 做 trinary 判定：

    p = (255 - v) / 255
    p < free_thresh        -> 自由
    p > occupied_thresh    -> 占据
    其余                    -> 未知

现场这张图是 `free_thresh: 0.2 / occupied_thresh: 0.5`：

    128 -> p = 0.498  -> 未知（规划器不走）
    205 -> p = 0.1961 -> 自由（只有 v > 204 才算 free）

ROS `map_saver` 把未知写成 205，是因为它的默认 `free_thresh` 是 `0.196`；
差 0.004，205 就翻到了自由一侧。2026-11-24 广汇二期 `TK-1124-17` 就是现场用
「擦成未知 (205)」标了一大片不可通行区，本体把它当可通行直接开了进去，最后报
`13377 无有效绕障轨迹`。平台侧画笔已改成 128，这个脚本负责存量地图包。

用法
----
    # 1) 先干跑，看有多少像素会被改、在哪个范围
    python3 business/tools/map_unknown_value.py --zip 2026_11_23_10_49_02.zip

    # 2) 生成新包（默认只改 map_edit/ 下的 PGM，和平台「编辑版优先」一致）
    python3 business/tools/map_unknown_value.py --zip 2026_11_23_10_49_02.zip --apply

    # 只在某个文件里改 / 连根目录原始图一起改（一般不需要）
    python3 business/tools/map_unknown_value.py --zip x.zip --entry map_edit/map.pgm --apply
    python3 business/tools/map_unknown_value.py --zip x.zip --all-pgm --apply

`--apply` 永远写**新文件**（`<原名>.to128.zip`），不覆盖输入包。

改完之后走平台流程（不要手改数据库）：
    中台「地图」→ 修改这张图 / 派生新版本 → 上传新 zip → 发布 →
    设备页「下发地图」→ 客户端 4030 重新载图（本体地图目录里的 map_edit/ 被刷新）

然后在本体文件服务上核对（客户端 / 现场都能看）：

    curl -s 'http://192.168.234.234:12000/api/list?path=/map/history_map/<目录>/map_edit'

`map.quadtree.bin` 的 mtime 应该在你 4030 载图之后刷新——那说明导航真的用了新栅格。
"""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import sys
import zipfile

# 平台约定：0..=99 占据，100..=205 未知，206..=255 自由（见 clearance.rs）
OLD_UNKNOWN = 205
NEW_UNKNOWN = 128
FREE = 254


class Pgm:
    """最小 PGM 解析（P2/P5，8/16 位）。16 位按平台同款规则 >>8 归一成 8 位。"""

    def __init__(self, width: int, height: int, pixels: bytearray):
        self.width = width
        self.height = height
        self.pixels = pixels

    def to_p5(self) -> bytes:
        header = f"P5\n{self.width} {self.height}\n255\n".encode()
        return header + bytes(self.pixels)


def _tokens(data: bytes, count: int):
    """从 PGM 头里取 count 个 token，跳过 # 注释。返回 (tokens, data_start)。"""
    tokens: list[bytes] = []
    i = 0
    n = len(data)
    while len(tokens) < count:
        while i < n and data[i : i + 1].isspace():
            i += 1
        if i < n and data[i : i + 1] == b"#":
            while i < n and data[i : i + 1] not in (b"\n", b"\r"):
                i += 1
            continue
        start = i
        while i < n and not data[i : i + 1].isspace():
            i += 1
        if start == i:
            raise ValueError("PGM 头不完整")
        tokens.append(data[start:i])
    # 头之后恰好空一个空白字符
    if i < n and data[i : i + 1].isspace():
        i += 1
    return tokens, i


def parse_pgm(data: bytes) -> Pgm:
    tokens, offset = _tokens(data, 4)
    magic = tokens[0]
    width, height, maxval = int(tokens[1]), int(tokens[2]), int(tokens[3])
    if magic == b"P5":
        if maxval <= 255:
            need = width * height
            if len(data) - offset < need:
                raise ValueError("P5 像素数据不完整")
            pixels = bytearray(data[offset : offset + need])
        else:
            need = width * height * 2
            if len(data) - offset < need:
                raise ValueError("16 位 P5 像素数据不完整")
            raw = data[offset : offset + need]
            pixels = bytearray(
                ((raw[i] << 8) | raw[i + 1]) >> 8 for i in range(0, need, 2)
            )
    elif magic == b"P2":
        values = data[offset:].split()
        if len(values) < width * height:
            raise ValueError("P2 像素数据不完整")
        scale = 255.0 / maxval if maxval else 1.0
        pixels = bytearray(
            min(255, int(round(int(values[i]) * scale))) for i in range(width * height)
        )
    else:
        raise ValueError(f"不支持的 PGM 魔数: {magic!r}")
    return Pgm(width, height, pixels)


def parse_yaml(text: str) -> dict[str, object]:
    """只取我们关心的四个字段（map.yaml 是扁平的，不用 yaml 依赖）。"""
    out: dict[str, object] = {}
    for line in text.splitlines():
        line = line.split("#", 1)[0].strip()
        if not line or ":" not in line:
            continue
        key, value = line.split(":", 1)
        key, value = key.strip(), value.strip()
        if key in ("free_thresh", "occupied_thresh"):
            try:
                out[key] = float(value)
            except ValueError:
                pass
        elif key == "resolution":
            try:
                out[key] = float(value)
            except ValueError:
                pass
        elif key == "origin":
            try:
                out[key] = [
                    float(part.strip()) for part in value.strip("[]").split(",")
                ]
            except ValueError:
                pass
    return out


def classify(value: int, free_thresh: float, occupied_thresh: float) -> str:
    p = (255 - value) / 255.0
    if p < free_thresh:
        return "可通行"
    if p > occupied_thresh:
        return "占据"
    return "未知"


def pick_entries(names: list[str], entry: str | None, all_pgm: bool) -> list[str]:
    pgms = [n for n in names if n.lower().endswith(".pgm") and not n.lower().endswith(".pgm.zst")]
    if entry:
        if entry not in pgms:
            raise SystemExit(f"包内没有 {entry}；可选的 .pgm：{pgms}")
        return [entry]
    if all_pgm:
        return [n for n in pgms if "map_edit.bak" not in n.lower()]
    edited = [n for n in pgms if "map_edit/" in n.lower()]
    if edited:
        return edited
    # 没有编辑版：和平台一致，model.pgm 优先，否则第一个 .pgm
    prefer = [n for n in pgms if n.lower().endswith("model.pgm")]
    return prefer[:1] or pgms[:1]


def convert(archive: zipfile.ZipFile, names: list[str], src: int, dst: int):
    """返回 {entry: (new_pgm_bytes, changed, bbox, width, height)}。"""
    result = {}
    for name in names:
        pgm = parse_pgm(archive.read(name))
        changed = 0
        min_col = min_row = 1 << 30
        max_col = max_row = -1
        for index, value in enumerate(pgm.pixels):
            if value != src:
                continue
            changed += 1
            row, col = divmod(index, pgm.width)
            min_col, max_col = min(min_col, col), max(max_col, col)
            min_row, max_row = min(min_row, row), max(max_row, row)
            pgm.pixels[index] = dst
        bbox = None
        if changed:
            bbox = (min_col, min_row, max_col, max_row)
        result[name] = (pgm.to_p5(), changed, bbox, pgm.width, pgm.height)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--zip", required=True, help="输入地图包（.zip）")
    parser.add_argument("--from", dest="src", type=int, default=OLD_UNKNOWN, help="要改写的灰度值（默认 205）")
    parser.add_argument("--to", dest="dst", type=int, default=NEW_UNKNOWN, help="改写后的灰度值（默认 128）")
    parser.add_argument("--entry", help="只处理这个 zip 条目，例如 map_edit/map.pgm")
    parser.add_argument("--all-pgm", action="store_true", help="处理所有 .pgm（默认只处理 map_edit/ 下的）")
    parser.add_argument("--apply", action="store_true", help="写出新 zip（默认只干跑）")
    args = parser.parse_args()

    if not 0 <= args.src <= 255 or not 0 <= args.dst <= 255:
        raise SystemExit("灰度值必须在 0..255")
    if args.src == args.dst:
        raise SystemExit("--from 和 --to 相同，没有可改的")

    with zipfile.ZipFile(args.zip) as archive:
        names = archive.namelist()
        yaml_name = next(
            (
                n
                for n in ("map_edit/map.yaml", "map_edit/map.yml", "model.yaml", "map.yaml")
                if n in names
            ),
            None,
        )
        meta = parse_yaml(archive.read(yaml_name).decode("utf-8", "replace")) if yaml_name else {}
        free_thresh = float(meta.get("free_thresh", 0.196))  # type: ignore[arg-type]
        occupied_thresh = float(meta.get("occupied_thresh", 0.65))  # type: ignore[arg-type]
        resolution = float(meta.get("resolution", 0.05))  # type: ignore[arg-type]
        origin = meta.get("origin") or [-0.0, -0.0, 0.0]  # type: ignore[assignment]

        entries = pick_entries(names, args.entry, args.all_pgm)
        converted = convert(archive, entries, args.src, args.dst)

        print(f"包：{args.zip}")
        if yaml_name:
            print(
                f"yaml：{yaml_name}  free_thresh={free_thresh}  occupied_thresh={occupied_thresh}"
                f"  resolution={resolution}  origin={origin}"
            )
        print(
            f"取值判定：{args.src} -> {classify(args.src, free_thresh, occupied_thresh)}；"
            f"{args.dst} -> {classify(args.dst, free_thresh, occupied_thresh)}"
        )
        if classify(args.dst, free_thresh, occupied_thresh) != "未知":
            print(
                "⚠️  改写后的值不是「未知」：目标值或 map.yaml 阈值不对，先别发布",
                file=sys.stderr,
            )
        if classify(args.src, free_thresh, occupied_thresh) == "未知":
            print(
                f"⚠️  {args.src} 在当前阈值下已经是未知，可能不需要改（确认一下再动）",
                file=sys.stderr,
            )
        print()

        total = 0
        for name, (_, changed, bbox, width, height) in converted.items():
            total += changed
            line = f"  {name}: {width}x{height}  改写 {changed} 像素"
            if bbox:
                col0, row0, col1, row1 = bbox
                x0 = float(origin[0]) + col0 * resolution  # type: ignore[index]
                x1 = float(origin[0]) + (col1 + 1) * resolution  # type: ignore[index]
                # PGM row 0 在图像顶部，地图 y 要翻过来
                y1 = float(origin[1]) + (height - row0) * resolution  # type: ignore[index]
                y0 = float(origin[1]) + (height - row1 - 1) * resolution  # type: ignore[index]
                line += f"  范围 x[{x0:.1f}, {x1:.1f}] y[{y0:.1f}, {y1:.1f}]（米）"
                line += f"  面积约 {changed * resolution * resolution:.1f} m²"
            print(line)
        if total == 0:
            print("\n没有找到要改写的像素，未生成新包。")
            return 0
        if not args.apply:
            print("\n干跑结束；确认无误后加 --apply 生成新包。")
            return 0

        stem, _ = os.path.splitext(args.zip)
        out_path = f"{stem}.to{args.dst}.zip"
        if os.path.exists(out_path):
            raise SystemExit(f"{out_path} 已存在，先移走再跑（脚本不覆盖输出）")
        with zipfile.ZipFile(out_path, "w", zipfile.ZIP_DEFLATED) as out:
            for info in archive.infolist():
                if info.is_dir():
                    out.writestr(info, b"")
                    continue
                if info.filename in converted:
                    out.writestr(info, converted[info.filename][0])
                    continue
                with archive.open(info) as src_file, out.open(info, "w") as dst_file:
                    shutil.copyfileobj(src_file, dst_file, 1024 * 1024)

    digest = hashlib.sha256()
    with open(out_path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    print(f"\n已写出：{out_path}（{os.path.getsize(out_path)} 字节）")
    print(f"sha256：{digest.hexdigest()}")
    print(
        "\n下一步：中台「地图」→ 修改 / 派生新版本 → 上传这个 zip → 发布 →\n"
        "        设备页「下发地图」→ 客户端 4030 重新载图。\n"
        "        发布后核对本体 map_edit/map.quadtree.bin 的 mtime 是否刷新。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
