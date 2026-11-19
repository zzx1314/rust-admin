#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""离线生成 backend/cjk_font.py —— 板端画框用的 16x16 中英文点阵字库。

何时需要重跑
============
字模是**预先烘焙**进 cjk_font.py 的，板端运行时不装 Pillow、不装字体也能画中文。
因此只要以下任一情况发生，就要在开发机重跑本脚本并把新 cjk_font.py 同步到板端：

  1. rknn/models/<task>/meta.json 里改了 / 新增了 zh 中文名；
  2. 新增了一个模型场景（labels.txt 对应的中文别名）；
  3. 改了 DETECT_LABEL_UNIT（默认「处」）或想让标签里出现别的汉字。

平时不用管：网页改类别勾选、调阈值、换目标路都不影响字库。

用法
====
    python backend/tools/gen_cjk_font.py
    # 指定字体（板端 Linux 或想换字体时）
    CJK_FONT_FILE=/usr/share/fonts/truetype/wqy/wqy-microhei.ttc \
        python backend/tools/gen_cjk_font.py

依赖：Pillow（**仅生成时**，板端不需要）。字体优先取 CJK_FONT_FILE，
否则依次找 Windows 的 SimHei / 微软雅黑，Linux 的 Noto CJK / 文泉驿。

生成的字模覆盖：
  - 所有 meta.json 的 labels.*.zh 里的汉字；
  - 一组常用补充字（处 / 共 / 无 / 检测 / 类别 / 数量…），便于以后改标签文案；
  - ASCII 可打印字符 32~126（数字、置信度小数、百分号、英文类名都够用）。

每个字模 = 16 行 × 16 bit（每行 2 字节 big-endian），另存一个「步进宽度」：
中文固定 16px，ASCII 按实际墨迹宽度（约 6~9px），混排才不会散开。
"""
import glob
import json
import os
import string
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
BACKEND = os.path.dirname(HERE)
DASHBOARD = os.path.dirname(BACKEND)
OUT = os.path.join(BACKEND, "cjk_font.py")

GLYPH_W = 16          # 字模网格宽
GLYPH_H = 16          # 字模网格高（画布 640x360 上 16px，放大到 1280x720 后 32px）
FONT_SIZE = 14         # SimHei 14px 在 16x16 网格里笔画最干净，不糊也不出格
TEXT_Y = 1             # 让汉字在网格里垂直居中
THRESHOLD = 100        # 抗锯齿灰度二值化阈值

# 常见补充字：以后把标签文案改成「共 3 处」「未检测到」之类也能直接显示
EXTRA_CJK = "处共无未检测类别数量总个只块片中文字识别结果目标区域方框置信度百分之未知"

FONT_CANDIDATES = [
    os.getenv("CJK_FONT_FILE", ""),
    "C:/Windows/Fonts/simhei.ttf",
    "C:/Windows/Fonts/msyh.ttc",
    "C:/Windows/Fonts/simsun.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "/usr/share/fonts/truetype/arphic/uming.ttc",
]


def find_font():
    for p in FONT_CANDIDATES:
        if p and os.path.isfile(p):
            return p
    raise SystemExit(
        "找不到中文字体。请设置环境变量 CJK_FONT_FILE 指向一个 TTF/TTC 文件，"
        "例如 Windows: C:/Windows/Fonts/simhei.ttf")


def collect_chars():
    """扫描所有场景的 meta.json，收集中文别名里的汉字 + ASCII 全集。"""
    chars = set(string.printable[:95])       # ASCII 32~126
    chars.update(EXTRA_CJK)
    metas = sorted(glob.glob(os.path.join(DASHBOARD, "rknn", "models", "*", "meta.json")))
    if not metas:
        print("[warn] 没找到任何 meta.json，只生成 ASCII 与补充字", file=sys.stderr)
    for p in metas:
        try:
            with open(p, "r", encoding="utf-8") as f:
                meta = json.load(f)
        except Exception as e:
            print("[warn] 跳过 %s: %s" % (p, e), file=sys.stderr)
            continue
        for cfg in (meta.get("labels") or {}).values():
            zh = cfg.get("zh") if isinstance(cfg, dict) else None
            if zh:
                chars.update(str(zh))
    # 换行/制表符等控制字符没有可见字形，交给运行时按空白跳过
    return sorted(c for c in chars if c.isprintable())


def render_all(chars, font_path):
    from PIL import Image, ImageDraw, ImageFont
    font = ImageFont.truetype(font_path, FONT_SIZE)
    out = {}
    for ch in chars:
        img = Image.new("L", (GLYPH_W, GLYPH_H), 0)
        ImageDraw.Draw(img).text((0, TEXT_Y), ch, font=font, fill=255)
        px = img.load()
        rows = []
        for y in range(GLYPH_H):
            bits = 0
            for x in range(GLYPH_W):
                if px[x, y] > THRESHOLD:
                    bits |= 1 << (GLYPH_W - 1 - x)
            rows.append(bits)
        if ord(ch) < 128:
            # ASCII：按墨迹右边界定步进，避免 "1" 和 "W" 一样宽
            ink = img.point(lambda v: 255 if v > THRESHOLD else 0).getbbox()
            adv = max(6, min(GLYPH_W, (ink[2] if ink else 6)))
        else:
            adv = GLYPH_W
        raw = bytearray()
        for bits in rows:
            raw += bytes((bits >> 8, bits & 0xFF))
        # 全空的字模（空格等）用空串表示，运行时直接补零行
        out[ch] = (adv, raw.hex() if any(rows) else "")
    return out


def main():
    font_path = find_font()
    chars = collect_chars()
    glyphs = render_all(chars, font_path)
    cjk = [c for c in chars if ord(c) >= 128]

    lines = [
        '# -*- coding: utf-8 -*-',
        '"""板端画框用的 16x16 中英文点阵字库 —— 由 tools/gen_cjk_font.py 自动生成，请勿手改。',
        '',
        '板端运行时只需要本文件（纯标准库），不需要 Pillow、不需要字体文件、不需要 FreeType。',
        '改了模型 meta.json 里的中文别名后，在开发机重跑生成脚本并同步本文件即可。',
        '数据来源字体：%s' % os.path.basename(font_path),
        '"""',
        'import functools',
        '',
        'GLYPH_W = %d' % GLYPH_W,
        'GLYPH_H = %d' % GLYPH_H,
        'GLYPH_COUNT = %d' % len(glyphs),
        '',
        '# ch -> (步进宽度, 16 行 x 16bit 的 hex 串；空串表示该字全透明)',
        '_DATA = {',
    ]
    for ch in chars:
        adv, hx = glyphs[ch]
        # 用 unicode 转义写 key，文件本身保持纯 ASCII，避免板端编码环境差异
        lines.append('    %s: (%d, "%s"),' % (_key(ch), adv, hx))
    lines += [
        '}',
        '',
        '_ZERO_ROWS = (0,) * GLYPH_H',
        '',
        '',
        '@functools.lru_cache(maxsize=512)',
        'def glyph(ch):',
        '    """返回 (步进宽度, rows)；rows 为 16 个 int，bit15..bit0 对应左->右 16 像素。',
        '    字库里没有的字符返回 None（调用方画方框并上报缺字）。',
        '    """',
        '    item = _DATA.get(ch)',
        '    if item is None:',
        '        return None',
        '    adv, hx = item',
        '    if not hx:',
        '        return adv, _ZERO_ROWS',
        '    raw = bytes.fromhex(hx)',
        '    return adv, tuple((raw[i * 2] << 8) | raw[i * 2 + 1]',
        '                     for i in range(GLYPH_H))',
        '',
        '',
        'def measure(text):',
        '    """标签文字的像素宽度（含每字 1px 间距）。"""',
        '    w = 0',
        '    for c in (text or ""):',
        '        item = _DATA.get(c)',
        '        w += (item[0] if item else GLYPH_W) + 1',
        '    return w',
        '',
        '',
        'def missing(text):',
        '    """返回 text 中字库没有的字符（空白不算），供 /api/detect/status 提示。"""',
        '    return sorted({c for c in (text or "")',
        '                  if not c.isspace() and c not in _DATA})',
        '',
    ]
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines))

    print("字体      :", font_path)
    print("输出      :", OUT)
    print("字符数    : %d（汉字 %d，ASCII %d）" % (len(glyphs), len(cjk), len(chars) - len(cjk)))
    print("文件大小  : %d bytes" % os.path.getsize(OUT))


def _key(ch):
    """把 key 写成 ASCII 安全的 Python 字符串字面量。"""
    if ch == "\\":
        return "'\\\\'"
    if ch == "'":
        return '"\'"'
    if 32 <= ord(ch) < 127:
        return "'" + ch + "'"
    if ord(ch) > 0xFFFF:          # 非 BMP 字符要用 \UXXXXXXXX
        return "'\\U%08x'" % ord(ch)
    return "'\\u%04x'" % ord(ch)


if __name__ == "__main__":
    main()
