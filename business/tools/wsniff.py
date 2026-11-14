#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""wsniff.py —— 抓取智航网关的 WebSocket 报文，并做「官方 App vs 我们客户端」任务参数比对。

为什么要这个工具
----------------
2026-11-14 现场：官方 APK 下发巡检任务能跑完，我们客户端下发（`5000 kind=2`）一导航就让
`.1` 的 `dog_task` 崩（`robots_dog_msgs` 版本不一致）。要确认「官方到底发了什么参数」，
就需要抓一次官方 App 与网关（`.234:10010`）之间的 WS 报文，再和我们客户端实际下发的
`5000` 载荷逐字段比对。现场机器**没有 tcpdump / tshark**，所以这里全部用 Python 标准库：

- `live` 模式：`AF_PACKET` 原始套接字抓包（root），自己解析 Ethernet/IPv4/TCP、做 TCP 重组、
  解 RFC6455 WebSocket 帧（含客户端 mask），把每条 JSON 报文打印 + 落 JSONL；
- `pcap` 模式：同一条解析链路，输入换成 `.pcap`（Wireshark/tcpdump 抓的都行）；
- `compare` 模式：把官方 App 的 `5000` 载荷与我们客户端日志/文件里的 `5000` 载荷做字段级对比。

用法
----
```bash
# ① 在机器人上抓（推荐：算法机 .234，sudo 免密；root 才能开原始套接字）
sudo python3 wsniff.py live --seconds 600 --out /tmp/wsniff-official.jsonl
#   期间用官方 APK 走一遍任务；Ctrl-C 或到时间自动结束，结束时会把所有连接收尾解析

# 只抓某个对端（手机），并换过滤端口
sudo python3 wsniff.py live --host 192.168.234.50 --ports 10010,12000

# ② 有 pcap 时离线解析
python3 wsniff.py pcap --in cap.pcap --port 10010 --out ws.jsonl

# ③ 参数比对（官方抓包 vs 我们客户端日志；日志里找 `5000 载荷（...）: {json}`）
python3 wsniff.py compare --official /tmp/wsniff-official.jsonl \
    --client client-5000.log
```

输出
----
- 控制台：`12:34:56.789 C→S type=5000 kind=4 name="…"`（只打报文头，避免刷屏）
- `--out`：JSONL，每行 `{ts, dir, type, kind, payload}`，`payload` 是完整 JSON 对象
- `--full`：控制台也打印完整 payload（排障时用）

注意
----
- 只解析 IPv4 + 明文 WebSocket（`ws://`，网关没有 TLS）；IPv6 会跳过并提示；
- 抓包必须在**网关所在机器**（`.234`）上做，或让流量经过抓包机；手机直连 `.234` 时，
  `.246` 上看不到这些包；
- 官方 App 会占住控制源（`ctrl_source=3`），跑的期间别在平台上下发任务，避免两边抢权。
"""

from __future__ import annotations

import argparse
import ctypes
import json
import re
import socket
import struct
import sys
import time
from collections import defaultdict
from typing import Dict, Iterable, List, Optional, Tuple

# --------------------------------------------------------------------------------------
# 常量
# --------------------------------------------------------------------------------------

ETH_P_ALL = 0x0003
ETH_P_IP = 0x0800
ETH_P_8021Q = 0x8100
ETH_P_8021AD = 0x88A8

DEFAULT_PORTS = (10010, 12000)  # 网关 control/bulkdata、文件服务
WS_OP_TEXT, WS_OP_BIN, WS_OP_CLOSE, WS_OP_PING, WS_OP_PONG, WS_OP_CONT = 1, 2, 8, 9, 10, 0

# 抓包里我们关心的报文（其余只记 type，不落全量，避免 JSONL 巨大）
# 高频心跳类报文：默认不打印、不落盘（否则 10 分钟抓包能到几十 MB）
# 1304 是摇杆（50~100Hz），排障遥控时再 --show-heartbeat
HEARTBEAT = {1207, 1300, 1301, 1304, 1315, 1317, 1325, 1327, 1401, 5009}

INTERESTING = {
    4012, 4013,          # 地图 / 路径列表
    4018, 4022, 4044,    # 回充点 / 定位 / 避障等级
    5000, 5001, 5002,    # 建任务 / 应答 / 取消
    5009,                # 任务状态快照（心跳）
    5010, 5011, 5014,    # 巡检记录 / 回充
    1204, 1205, 1302, 1304, 1305, 1315, 1207,
}


# --------------------------------------------------------------------------------------
# TCP 重组
# --------------------------------------------------------------------------------------


class TcpStream:
    """单向 TCP 流：按 seq 收集 payload，支持重传/乱序；同时记录每段到达时刻。

    关键点：只返回**从最早期望位置开始的连续前缀**。丢了一个段时，绝不把后面的字节
    直接拼上来——那会让 WS 帧边界整体错位，后续所有报文都解析不出来（2026-11-14 抓官方
    App 时就吃了这个亏：C→S 从 11:39:33 起丢了一段，导致 11:39:50 那条关键的 5000
    彻底看不到）。等空洞补上后前缀自然变长，增量解析继续。
    """

    __slots__ = ("segments", "next_seq", "fin", "gap_at", "missing")

    def __init__(self) -> None:
        self.segments: List[Tuple[int, float, bytes]] = []
        self.next_seq: Optional[int] = None
        self.fin = False
        self.gap_at: Optional[int] = None   # 缺口在流里的偏移（None = 没有缺口）
        self.missing = 0                     # 缺口缺了多少字节（按 seq 估算）

    def add(self, seq: int, payload: bytes, ts: float) -> None:
        if not payload:
            return
        if self.next_seq is None:
            self.next_seq = seq
        self.segments.append((seq, ts, payload))

    def data(self) -> Tuple[bytes, List[Tuple[int, float]]]:
        """返回 (连续前缀字节, [(该段在流中的起始偏移, 到达时刻)])。"""
        if not self.segments:
            return b"", []
        segs = sorted(self.segments, key=lambda item: item[0])
        out = bytearray()
        marks: List[Tuple[int, float]] = []
        cursor: Optional[int] = None
        self.gap_at = None
        self.missing = 0
        for seq, ts, payload in segs:
            if cursor is None:
                marks.append((0, ts))
                out += payload
                cursor = seq + len(payload)
                continue
            if seq > cursor:
                # 空洞：只保留已经连续的部分，等缺的那段补上再继续
                self.gap_at = len(out)
                self.missing = seq - cursor
                break
            if seq + len(payload) <= cursor:
                continue  # 完全重复
            overlap = cursor - seq
            marks.append((len(out), ts))
            out += payload[overlap:]
            cursor = seq + len(payload)
        return bytes(out), marks


def ts_at(marks: List[Tuple[int, float]], offset: int, fallback: float) -> float:
    """取「覆盖该偏移」的那个 TCP 段的到达时刻。"""
    stamp = fallback
    for start, ts in marks:
        if start <= offset:
            stamp = ts
        else:
            break
    return stamp


# --------------------------------------------------------------------------------------
# WebSocket（RFC6455）帧解析
# --------------------------------------------------------------------------------------


class WsFrame:
    __slots__ = ("opcode", "payload", "fin")

    def __init__(self, opcode: int, payload: bytes, fin: bool) -> None:
        self.opcode = opcode
        self.payload = payload
        self.fin = fin


def _skip_http_handshake(data: bytes) -> int:
    """跳过开头的 HTTP 升级请求/应答，返回 WS 帧起始偏移。

    只在偏移 0 处找一次 `\\r\\n\\r\\n`；没找到说明这条流不是 WS（或抓包不全），返回 0。
    """
    if not data.startswith((b"GET ", b"POST ", b"HTTP/1.1", b"HTTP/1.0")):
        return 0
    head_end = data.find(b"\r\n\r\n")
    return 0 if head_end < 0 else head_end + 4


def parse_ws_frames(data: bytes) -> List[WsFrame]:
    """把重组后的字节流切成 WS 帧；遇到坏帧就停下（不抛异常）。"""
    return [WsFrame(op, payload, fin) for op, payload, fin, _s in parse_ws_frames_with_offset(data)]


def parse_ws_frames_with_offset(data: bytes) -> List[Tuple[int, bytes, bool, int]]:
    """同 ::func:`parse_ws_frames`，返回 (opcode, payload, fin, 帧起始偏移)。"""
    frames: List[Tuple[int, bytes, bool, int]] = []
    pos = _skip_http_handshake(data)
    total = len(data)
    while pos + 2 <= total:
        frame_start = pos
        b0, b1 = data[pos], data[pos + 1]
        fin = bool(b0 & 0x80)
        opcode = b0 & 0x0F
        masked = bool(b1 & 0x80)
        length = b1 & 0x7F
        pos += 2
        if length == 126:
            if pos + 2 > total:
                break
            length = struct.unpack(">H", data[pos:pos + 2])[0]
            pos += 2
        elif length == 127:
            if pos + 8 > total:
                break
            length = struct.unpack(">Q", data[pos:pos + 8])[0]
            pos += 8
        mask_key = b""
        if masked:
            if pos + 4 > total:
                break
            mask_key = data[pos:pos + 4]
            pos += 4
        if pos + length > total:
            break  # 帧还没收全（或丢包）：停止，已解析的照常返回
        payload = data[pos:pos + length]
        pos += length
        if masked and mask_key:
            payload = bytes(byte ^ mask_key[i % 4] for i, byte in enumerate(payload))
        frames.append((opcode, payload, fin, frame_start))
    return frames


def ws_messages(data: bytes) -> Iterable[bytes]:
    """产出完整消息（把分片的 continuation 帧拼起来，跳过控制帧）。"""
    for _offset, payload in ws_messages_with_offset(data):
        yield payload


def ws_messages_with_offset(data: bytes) -> Iterable[Tuple[int, bytes]]:
    """同 ::func:`ws_messages`，但多带消息起始字节在流里的偏移（用来取时间戳）。"""
    buf = bytearray()
    pending: Optional[int] = None
    buf_start = 0
    for opcode, payload, fin, start in parse_ws_frames_with_offset(data):
        if opcode == WS_OP_CONT:
            buf += payload
            if fin and pending is not None:
                yield buf_start, bytes(buf)
                buf.clear()
                pending = None
        elif opcode in (WS_OP_TEXT, WS_OP_BIN):
            if fin:
                yield start, payload
            else:
                buf = bytearray(payload)
                buf_start = start
                pending = opcode
    if pending is not None and buf:
        yield buf_start, bytes(buf)


# --------------------------------------------------------------------------------------
# 报文归一化：把 {head:{type}, data:{...}} / {msgType,...} 统一成 (type, kind, payload)
# --------------------------------------------------------------------------------------


def normalize(obj: object) -> Optional[Tuple[Optional[int], Optional[int], dict]]:
    if not isinstance(obj, dict):
        return None
    payload = obj
    msg_type: Optional[int] = None
    kind: Optional[int] = None

    head = obj.get("head")
    if isinstance(head, dict):
        raw = head.get("type")
        if isinstance(raw, int):
            msg_type = raw
        elif isinstance(raw, str) and raw.isdigit():
            msg_type = int(raw)
        data = obj.get("data")
        if isinstance(data, dict):
            payload = data
    if msg_type is None:
        raw = obj.get("msgType")
        if isinstance(raw, int):
            msg_type = raw
        elif isinstance(raw, str) and raw.isdigit():
            msg_type = int(raw)
    raw_kind = payload.get("kind")
    if isinstance(raw_kind, int):
        kind = raw_kind
    elif isinstance(raw_kind, str) and raw_kind.isdigit():
        kind = int(raw_kind)
    return msg_type, kind, payload


# --------------------------------------------------------------------------------------
# 抓到的报文一条一行
# --------------------------------------------------------------------------------------


ENTITY_HINT = {
    4012: "地图/路径列表请求",
    4013: "地图/路径列表应答",
    4044: "设置避障等级",
    5000: "创建任务",
    5001: "创建任务应答",
    5002: "取消任务",
    5009: "任务状态快照",
    5010: "巡检记录请求",
    5011: "巡检记录应答",
    5014: "返回充电点",
    1204: "取控制权",
    1205: "取控制权应答",
    1302: "动作/模式命令",
    1304: "摇杆",
    1315: "本体状态",
    1207: "模块可用性",
}


def emit(ts: float, direction: str, obj: object, out, full: bool,
         show_heartbeat: bool = False, conn: str = "", max_payload: int = 4000) -> Optional[dict]:
    norm = normalize(obj)
    if norm is None:
        return None
    msg_type, kind, payload = norm
    if msg_type in HEARTBEAT and not show_heartbeat and not full:
        return None
    hhmmss = time.strftime("%H:%M:%S", time.localtime(ts)) + f".{int(ts * 1000) % 1000:03d}"
    name = ""
    if isinstance(payload.get("name"), str):
        name = f' name="{payload["name"][:40]}"'
    kind_txt = f" kind={kind}" if kind is not None else ""
    hint = f"  # {ENTITY_HINT[msg_type]}" if msg_type in ENTITY_HINT else ""
    print(f"{hhmmss} {direction} type={msg_type}{kind_txt}{name}{hint}")
    if full:
        print(json.dumps(obj, ensure_ascii=False, indent=2)[:4000])

    store = payload
    if max_payload > 0:
        text = json.dumps(payload, ensure_ascii=False)
        if len(text) > max_payload:
            store = {"_truncated": True, "_head": text[:max_payload]}
    record = {
        "ts": round(ts, 3),
        "dir": direction,
        "conn": conn,
        "type": msg_type,
        "kind": kind,
        "payload": store,
    }
    if out is not None:
        out.write(json.dumps(record, ensure_ascii=False) + "\n")
        out.flush()
    return record



# --------------------------------------------------------------------------------------
# 内核级过滤 + 大接收缓冲（不装 tcpdump 也能稳定抓，别让用户态丢包）
# --------------------------------------------------------------------------------------

SO_ATTACH_FILTER = 26
BPF_LD_B, BPF_LD_H = 0x30, 0x28
BPF_LD_H_IND = 0x48     # ldh [x + k]：间接寻址（TCP 头偏移随 IP 头长度变）
BPF_LDX_MSH = 0xB1      # ldx 4*([k]&0xf)：X = IP 头长度
BPF_JMP_JEQ, BPF_JMP_JA = 0x15, 0x05
BPF_RET_K = 0x06


def _bpf_program(ports: Iterable[int]):
    """生成 `ip and tcp and (port P1 or port P2 ...)` 的经典 BPF 程序。

    指令布局（N = 7 + 4*端口数）：
        0: ldh [12]              A = ethertype
        1: jeq #0x0800 ? 2 : DROP
        2: ldb [23]              A = ip.proto
        3: jeq #6 ? 4 : DROP
        4: ldx 4*([14]&0xf)      X = IP 头长度（20/60 字节都要对）
        5+4i+0: ldh [x+14]       A = sport
        5+4i+1: jeq #Pi ? ACCEPT : 下一条
        5+4i+2: ldh [x+16]       A = dport
        5+4i+3: jeq #Pi ? ACCEPT : 下一条
        N-2: ret #0              DROP
        N-1: ret #0x40000        ACCEPT

    ⚠️ 两个坑，2026-11-14 都踩过：
    1. BPF 跳转是「从下一条指令算起」的 8 位偏移，第一版少算 2 跳 → 把自己要的包全丢了；
    2. TCP 头不在固定偏移 14，要先按 IHL 算（`ldx 4*([14]&0xf)` + 间接 `ldh [x+…]`）。
    挂载前会跑 ::func:`bpf_selftest` 自检，不通过就退回用户态过滤，不会再“静默全丢”。
    """
    ports = list(ports)
    n = 7 + 4 * len(ports)
    drop, accept = n - 2, n - 1
    insns: List[Tuple[int, int, int, int]] = [
        (BPF_LD_H, 0, 0, 12),
        (BPF_JMP_JEQ, 0, drop - 2, 0x0800),
        (BPF_LD_B, 0, 0, 23),
        (BPF_JMP_JEQ, 0, drop - 4, 6),
        (BPF_LDX_MSH, 0, 0, 14),
    ]
    for index, port in enumerate(ports):
        base = 5 + 4 * index
        insns.append((BPF_LD_H_IND, 0, 0, 14))
        insns.append((BPF_JMP_JEQ, accept - base - 2, 0, port))
        insns.append((BPF_LD_H_IND, 0, 0, 16))
        insns.append((BPF_JMP_JEQ, accept - base - 4, 0, port))
    insns.append((BPF_RET_K, 0, 0, 0))
    insns.append((BPF_RET_K, 0, 0, 0x40000))
    raw = b"".join(struct.pack("HBBI", *item) for item in insns)
    return raw, len(insns)


def _bpf_run(program: List[Tuple[int, int, int, int]], frame: bytes) -> int:
    """极简 BPF 解释器（只覆盖本模块用到的指令），给自检用。"""
    acc = 0
    xreg = 0
    pc = 0
    for _ in range(10000):
        if pc >= len(program):
            return 0
        code, jt, jf, k = program[pc]
        if code == BPF_LD_H:
            if k + 2 > len(frame):
                return 0
            acc = (frame[k] << 8) | frame[k + 1]
            pc += 1
        elif code == BPF_LD_B:
            if k + 1 > len(frame):
                return 0
            acc = frame[k]
            pc += 1
        elif code == BPF_LDX_MSH:
            if k + 1 > len(frame):
                return 0
            xreg = 4 * (frame[k] & 0x0F)
            pc += 1
        elif code == BPF_LD_H_IND:
            at = xreg + k
            if at + 2 > len(frame):
                return 0
            acc = (frame[at] << 8) | frame[at + 1]
            pc += 1
        elif code == BPF_JMP_JEQ:
            pc += 1 + (jt if acc == k else jf)
        elif code == BPF_RET_K:
            return k
        else:
            return 0
    return 0


def bpf_selftest(ports: Iterable[int]) -> Tuple[bool, str]:
    """用合成报文验证 BPF 程序真的会放行/丢弃，避免“过滤器把包全丢了”。"""
    ports = list(ports)
    program = [
        struct.unpack("HBBI", raw)
        for raw in (struct.pack("HBBI", *item) for item in _bpf_instructions(ports))
    ]

    def frame(ethertype: int, proto: int, sport: int, dport: int) -> bytes:
        eth = b"\x02" * 6 + b"\x04" * 6 + struct.pack("!H", ethertype)
        ip = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 40, 0, 0, 64, proto, 0,
                         b"\x0a\x00\x00\x01", b"\x0a\x00\x00\x02")
        tcp = struct.pack("!HHIIBBHHH", sport, dport, 0, 0, 0x50, 0x18, 65535, 0, 0)
        return eth + ip + tcp

    port = ports[0]
    decoy = next(p for p in (port + 1, port + 2, port + 3) if p not in ports)
    cases = [
        ("目标端口命中", frame(0x0800, 6, 40000, port), True),
        ("源端口命中", frame(0x0800, 6, port, 40000), True),
        ("其它端口", frame(0x0800, 6, 40000, decoy), False),
        ("UDP", frame(0x0800, 17, 40000, port), False),
        ("ARP", frame(0x0806, 0, 0, 0), False),
    ]
    for name, data, expect in cases:
        got = _bpf_run(program, data) != 0
        if got != expect:
            return False, f"{name}: 期望 {'放行' if expect else '丢弃'}，实际 {'放行' if got else '丢弃'}"
    return True, "5 条合成报文全部符合预期"


def _bpf_instructions(ports: Iterable[int]) -> List[Tuple[int, int, int, int]]:
    """返回指令列表（与 ::func:`_bpf_program` 同源，供自检使用）。"""
    raw, count = _bpf_program(ports)
    return [struct.unpack("HBBI", raw[i * 8:(i + 1) * 8]) for i in range(count)]


def attach_bpf(sock, ports: Iterable[int]) -> None:
    ok, detail = bpf_selftest(ports)
    if not ok:
        raise OSError(f"BPF 自检未通过（{detail}），已放弃挂载以免丢包")
    raw, count = _bpf_program(ports)
    buf = ctypes.create_string_buffer(raw, len(raw))
    fprog = struct.pack("HL", count, ctypes.addressof(buf))
    sock.setsockopt(socket.SOL_SOCKET, SO_ATTACH_FILTER, fprog)


def tune_rcvbuf(sock, megabytes: int) -> int:
    want = max(1, megabytes) * 1024 * 1024
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, want)
    return sock.getsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF)


# --------------------------------------------------------------------------------------
# live：AF_PACKET 原始抓包
# --------------------------------------------------------------------------------------


class LiveSniffer:
    def __init__(self, ports: Iterable[int], host: Optional[str], out, full: bool,
                 show_heartbeat: bool = False) -> None:
        self.ports = set(ports)
        self.host = host
        self.out = out
        self.full = full
        self.show_heartbeat = show_heartbeat
        self.streams: Dict[Tuple[str, int, str, int], TcpStream] = defaultdict(TcpStream)
        self.seen_conns: set = set()
        # 每条流已经吐到哪个偏移（流内字节偏移，避免重复打印）
        self.consumed: Dict[Tuple[str, int, str, int], int] = defaultdict(lambda: -1)
        self.gaps_reported: Dict[Tuple[str, int, str, int], Optional[int]] = {}

    def run(self, seconds: float, iface: Optional[str], buf_mb: int = 32,
            use_bpf: bool = True) -> None:
        sock = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(ETH_P_ALL))
        try:
            if iface:
                sock.bind((iface, 0))
            if use_bpf:
                try:
                    attach_bpf(sock, self.ports)
                    print(f"已挂内核 BPF 过滤器（tcp 端口 {sorted(self.ports)}）")
                except OSError as error:
                    print(f"挂 BPF 失败（{error}），退回用户态过滤", file=sys.stderr)
            effective = tune_rcvbuf(sock, buf_mb)
            print(f"接收缓冲 SO_RCVBUF = {effective / 1048576:.1f} MB"
                  f"（想再大需 sysctl -w net.core.rmem_max=…）")
            sock.settimeout(1.0)
            deadline = time.time() + seconds if seconds > 0 else None
            print(f"抓包中（端口 {sorted(self.ports)}"
                  f"{', 只抓对端 ' + self.host if self.host else ''}），Ctrl-C 结束…")
            while True:
                if deadline is not None and time.time() >= deadline:
                    break
                try:
                    frame, _meta = sock.recvfrom(262144)
                except socket.timeout:
                    continue
                self._handle_frame(frame, time.time())
        except KeyboardInterrupt:
            print("\n收到 Ctrl-C，收尾解析…")
        finally:
            sock.close()
            self.flush()

    # --- 报文解析 ---------------------------------------------------------------
    def _handle_frame(self, frame: bytes, ts: float) -> None:
        if len(frame) < 14:
            return
        ethertype = struct.unpack("!H", frame[12:14])[0]
        offset = 14
        # 剥 VLAN 标签
        while ethertype in (ETH_P_8021Q, ETH_P_8021AD):
            if len(frame) < offset + 4:
                return
            ethertype = struct.unpack("!H", frame[offset + 2:offset + 4])[0]
            offset += 4
        if ethertype != ETH_P_IP:
            return
        packet = frame[offset:]
        if len(packet) < 20:
            return
        version_ihl = packet[0]
        if version_ihl >> 4 != 4:
            return  # IPv6 跳过
        ihl = (version_ihl & 0x0F) * 4
        if len(packet) < ihl + 20:
            return
        if packet[9] != 6:  # 只要 TCP
            return
        total_len = struct.unpack("!H", packet[2:4])[0]
        tcp = packet[ihl:total_len] if total_len else packet[ihl:]
        src_ip = socket.inet_ntoa(packet[12:16])
        dst_ip = socket.inet_ntoa(packet[16:20])
        if self.host and self.host not in (src_ip, dst_ip):
            return
        if len(tcp) < 20:
            return
        sport, dport = struct.unpack("!HH", tcp[0:4])
        if sport not in self.ports and dport not in self.ports:
            return
        seq = struct.unpack("!I", tcp[4:8])[0]
        data_offset = (tcp[12] >> 4) * 4
        if len(tcp) < data_offset:
            return
        payload = tcp[data_offset:]
        key = (src_ip, sport, dst_ip, dport)
        stream = self.streams[key]
        stream.add(seq, payload, ts)
        self._emit_stream(key, ts, incremental=True)
        if tcp[13] & 0x01:  # FIN
            stream.fin = True
            self._flush_stream(key, ts)
        if key not in self.seen_conns:
            self.seen_conns.add(key)
            print(f"新连接 {src_ip}:{sport} → {dst_ip}:{dport}")

    def _flush_stream(self, key: Tuple[str, int, str, int], ts: float) -> None:
        self._emit_stream(key, ts, incremental=True)
        self.streams.pop(key, None)
        self.consumed.pop(key, None)

    def _emit_stream(self, key: Tuple[str, int, str, int], ts: float,
                     incremental: bool = True) -> None:
        stream = self.streams.get(key)
        if stream is None:
            return
        data, marks = stream.data()
        if not data:
            return
        src_ip, sport, dst_ip, dport = key
        direction = "C→S" if dport in self.ports else "S→C"
        conn = f"{src_ip}:{sport}"
        if stream.gap_at is not None and self.gaps_reported.get(key) != stream.gap_at:
            self.gaps_reported[key] = stream.gap_at
            print(f"⚠ 丢包：{conn} {direction} 在偏移 {stream.gap_at} 处缺 "
                  f"{stream.missing} 字节，该方向后续报文可能缺失（建议加大 --buf-mb 或用 --iface 限定网卡）",
                  file=sys.stderr)
            if self.out is not None:
                self.out.write(json.dumps({
                    "ts": round(ts, 3), "dir": direction, "conn": conn,
                    "type": "gap", "kind": None,
                    "payload": {"offset": stream.gap_at, "missing": stream.missing},
                }, ensure_ascii=False) + "\n")
                self.out.flush()
        for offset, raw in ws_messages_with_offset(data):
            if incremental and offset <= self.consumed[key]:
                continue  # 这条之前已经吐过了
            try:
                obj = json.loads(raw.decode("utf-8", "replace"))
            except json.JSONDecodeError:
                continue
            self.consumed[key] = offset
            emit(ts_at(marks, offset, ts), direction, obj, self.out, self.full,
                 self.show_heartbeat, conn)

    def flush(self) -> None:
        """收尾：把还没 FIN 的连接也解析一遍（避免丢最后几条）。"""
        now = time.time()
        for key in list(self.streams.keys()):
            self._flush_stream(key, now)


# --------------------------------------------------------------------------------------
# pcap 文件解析（Wireshark / tcpdump 抓的都能读）
# --------------------------------------------------------------------------------------

PCAP_MAGICS = {
    # 只读 4 字节大端判断：本身就是大端写入 → 头字段用 ">"；反了 → 小端
    0xA1B2C3D4: (">", 1e-6),
    0xD4C3B2A1: ("<", 1e-6),
    0xA1B23C4D: (">", 1e-9),
    0x4D3CB2A1: ("<", 1e-9),
}


def sniff_pcap(path: str, ports: Iterable[int], host: Optional[str], out, full: bool,
               show_heartbeat: bool = False) -> None:
    ports = set(ports)
    with open(path, "rb") as fh:
        header = fh.read(24)
        if len(header) < 24:
            print("不是有效的 pcap：文件太短", file=sys.stderr)
            return
        magic = struct.unpack(">I", header[0:4])[0]
        if magic not in PCAP_MAGICS:
            print("不支持的 pcap 魔数（可能是 pcapng，请用 `tshark -F pcap` 转换）", file=sys.stderr)
            return
        endian, scale = PCAP_MAGICS[magic]
        linktype = struct.unpack(endian + "I", header[20:24])[0]
        streams: Dict[Tuple[str, int, str, int], TcpStream] = defaultdict(TcpStream)
        while True:
            rec = fh.read(16)
            if len(rec) < 16:
                break
            ts_sec, ts_frac, incl_len, _orig = struct.unpack(endian + "IIII", rec)
            frame = fh.read(incl_len)
            if len(frame) < incl_len:
                break
            ts = ts_sec + ts_frac * scale
            parsed = _pcap_frame_to_tcp(frame, linktype)
            if parsed is None:
                continue
            src_ip, dst_ip, sport, dport, seq, payload, fin = parsed
            if self_ports := ports:
                if sport not in self_ports and dport not in self_ports:
                    continue
            if host and host not in (src_ip, dst_ip):
                continue
            streams[(src_ip, sport, dst_ip, dport)].add(seq, payload, ts)
            if fin:
                _flush(streams, (src_ip, sport, dst_ip, dport), ports, out, full, ts, show_heartbeat)
        for key in list(streams.keys()):
            _flush(streams, key, ports, out, full, time.time(), show_heartbeat)


def _pcap_frame_to_tcp(frame: bytes, linktype: int):
    if linktype == 1:      # Ethernet
        if len(frame) < 14:
            return None
        ethertype = struct.unpack("!H", frame[12:14])[0]
        offset = 14
        while ethertype in (ETH_P_8021Q, ETH_P_8021AD):
            ethertype = struct.unpack("!H", frame[offset + 2:offset + 4])[0]
            offset += 4
        packet = frame[offset:]
    elif linktype in (113, 276):  # Linux cooked (SLL / SLL2)
        offset = 16 if linktype == 113 else 20
        ethertype = struct.unpack("!H", frame[offset - 2:offset])[0]
        if ethertype != ETH_P_IP:
            return None
        packet = frame[offset:]
    elif linktype == 101:  # raw IP
        packet = frame
    else:
        return None
    if len(packet) < 20 or packet[0] >> 4 != 4 or packet[9] != 6:
        return None
    ihl = (packet[0] & 0x0F) * 4
    total_len = struct.unpack("!H", packet[2:4])[0]
    tcp = packet[ihl:total_len] if total_len else packet[ihl:]
    if len(tcp) < 20:
        return None
    src_ip = socket.inet_ntoa(packet[12:16])
    dst_ip = socket.inet_ntoa(packet[16:20])
    sport, dport = struct.unpack("!HH", tcp[0:4])
    seq = struct.unpack("!I", tcp[4:8])[0]
    data_offset = (tcp[12] >> 4) * 4
    payload = tcp[data_offset:] if len(tcp) >= data_offset else b""
    fin = bool(tcp[13] & 0x01)
    return src_ip, dst_ip, sport, dport, seq, payload, fin


def _flush(streams, key, ports, out, full, ts, show_heartbeat: bool = False) -> None:
    stream = streams.pop(key, None)
    if stream is None:
        return
    src_ip, sport, dst_ip, dport = key
    direction = "C→S" if dport in ports else "S→C"
    data, marks = stream.data()
    for offset, raw in ws_messages_with_offset(data):
        try:
            obj = json.loads(raw.decode("utf-8", "replace"))
        except json.JSONDecodeError:
            continue
        emit(ts_at(marks, offset, ts), direction, obj, out, full, show_heartbeat,
             f"{src_ip}:{sport}")


# --------------------------------------------------------------------------------------
# compare：官方 App 的 5000 vs 我们客户端的 5000
# --------------------------------------------------------------------------------------

LOG_PAYLOAD_RE = re.compile(r"5000 载荷[^:]*:\s*(\{.*\})\s*$")


def load_official(path: str) -> List[dict]:
    """从 wsniff 的 JSONL 里挑出「建任务」的 payload（5000 / kind 2|4）。"""
    found: List[dict] = []
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("type") == 5000 and isinstance(rec.get("payload"), dict):
                found.append(rec["payload"])
    return found


def load_client(path: str) -> List[dict]:
    """从客户端日志（journalctl 文本）或 JSON 文件里挑出 5000 载荷。

    兼容三种来源：
    - `5000 载荷（第 1-1 点，1 个航点，need_locate=true）: {json}`
    - 直接是 JSON 文件（单个对象或对象数组）
    - wsniff 的 JSONL（type=5000 的行）
    """
    text = open(path, "r", encoding="utf-8", errors="replace").read()
    found: List[dict] = []
    for match in LOG_PAYLOAD_RE.finditer(text):
        try:
            found.append(json.loads(match.group(1)))
        except json.JSONDecodeError:
            continue
    if found:
        return found
    stripped = text.strip()
    if stripped.startswith("{"):
        try:
            obj = json.loads(stripped)
            return obj if isinstance(obj, list) else [obj]
        except json.JSONDecodeError:
            pass
    if stripped.startswith("["):
        try:
            return json.loads(stripped)
        except json.JSONDecodeError:
            pass
    return load_official(path)


def _fmt(value: object) -> str:
    text = json.dumps(value, ensure_ascii=False) if not isinstance(value, str) else value
    return text if len(text) <= 120 else text[:117] + "…"


def compare(official: List[dict], client: List[dict]) -> None:
    if not official:
        print("官方抓包里没有 type=5000 的建任务报文；确认抓的是网关 10010 且 App 已下发任务。")
        return
    print(f"官方 App：抓到 {len(official)} 条 5000 报文")
    for idx, payload in enumerate(official, 1):
        print(f"  #{idx} kind={payload.get('kind')} name={_fmt(payload.get('name'))}")
    ours = client[0] if client else None
    if ours is None:
        print("我方没有解析到 5000 载荷（检查日志里是否有「5000 载荷…」或传入 JSON 文件）。")
    else:
        print(f"我方客户端：kind={ours.get('kind')} name={_fmt(ours.get('name'))}")

    off = official[0]
    off_params = off.get("params") if isinstance(off.get("params"), dict) else {}
    our_params = (ours or {}).get("params") if isinstance((ours or {}).get("params"), dict) else {}

    print("\n=== 顶层字段 ===")
    top_keys = sorted(set(off) | set(ours or {}))
    for key in top_keys:
        a = off.get(key, "<缺>")
        b = (ours or {}).get(key, "<缺>")
        mark = "" if a == b else "  ← 不同"
        print(f"{key:12s} 官方={_fmt(a):28s} 我方={_fmt(b)}{mark}")

    print("\n=== params 字段 ===")
    for key in sorted(set(off_params) | set(our_params)):
        a = off_params.get(key, "<缺>")
        b = our_params.get(key, "<缺>")
        mark = "" if a == b else "  ← 不同"
        print(f"{key:20s} 官方={_fmt(a):40s} 我方={_fmt(b)}{mark}")

    only_off = sorted(set(off_params) - set(our_params))
    only_our = sorted(set(our_params) - set(off_params))
    if only_off:
        print(f"\n官方独有（我方没实现）：{', '.join(only_off)}")
    if only_our:
        print(f"我方独有（官方没用）：{', '.join(only_our)}")

    off_goals = off_params.get("goals")
    our_goals = our_params.get("goals")
    if off_goals or our_goals:
        print("\n=== goals 数量 / 首点 ===")
        for label, goals in (("官方", off_goals), ("我方", our_goals)):
            if isinstance(goals, list):
                first = goals[0] if goals else None
                print(f"{label}: {len(goals)} 个，首点={_fmt(first)}")
            else:
                print(f"{label}: 无 goals（官方巡检任务通常用 path_id/points，不用 goals）")


# --------------------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------------------


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="抓智航网关 WebSocket 报文 / 比对官方 App 与客户端下发的 5000 参数",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    live = sub.add_parser("live", help="在机器人上实时抓包（需要 root）")
    live.add_argument("--seconds", type=float, default=0, help="抓多久，0=一直抓到 Ctrl-C")
    live.add_argument("--iface", default=None, help="网卡名（默认所有网卡）")
    live.add_argument("--ports", default=",".join(map(str, DEFAULT_PORTS)),
                      help=f"过滤端口，默认 {','.join(map(str, DEFAULT_PORTS))}")
    live.add_argument("--host", default=None, help="只抓某个对端 IP（例如手机 IP）")
    live.add_argument("--out", default="wsniff-out.jsonl", help="JSONL 输出文件")
    live.add_argument("--append", action="store_true",
                      help="追加写（做 systemd 服务时必须加，否则重启会截断上一轮）")
    live.add_argument("--full", action="store_true", help="控制台打印完整 payload")
    live.add_argument("--buf-mb", type=int, default=32,
                      help="接收缓冲（MB，默认 32）；仍丢包就加到 128 并把 net.core.rmem_max 调大")
    live.add_argument("--no-bpf", action="store_true",
                      help="不挂内核 BPF（链路带 VLAN 标签时必须加，退回用户态过滤）")
    live.add_argument("--show-heartbeat", action="store_true",
                      help="也打印/落盘 5009/1315 等高频心跳报文（默认过滤）")

    pcap = sub.add_parser("pcap", help="离线解析 .pcap")
    pcap.add_argument("--in", dest="input", required=True)
    pcap.add_argument("--ports", default=",".join(map(str, DEFAULT_PORTS)))
    pcap.add_argument("--host", default=None)
    pcap.add_argument("--out", default="wsniff-out.jsonl")
    pcap.add_argument("--full", action="store_true")
    pcap.add_argument("--show-heartbeat", action="store_true")

    cmp_ = sub.add_parser("compare", help="官方抓包 vs 我方客户端日志/JSON 的 5000 参数比对")
    cmp_.add_argument("--official", required=True, help="wsniff 输出的 JSONL")
    cmp_.add_argument("--client", required=True, help="客户端日志（journalctl 文本）或 JSON")

    args = parser.parse_args(argv)

    def parse_ports(text: str) -> List[int]:
        return [int(part) for part in text.replace(" ", "").split(",") if part]

    if args.cmd == "live":
        out = open(args.out, "a" if args.append else "w", encoding="utf-8")
        try:
            LiveSniffer(parse_ports(args.ports), args.host, out, args.full,
                        args.show_heartbeat).run(args.seconds, args.iface,
                                                 args.buf_mb, not args.no_bpf)
        finally:
            out.close()
            print(f"已写入 {args.out}")
        return 0

    if args.cmd == "pcap":
        out = open(args.out, "w", encoding="utf-8")
        try:
            sniff_pcap(args.input, parse_ports(args.ports), args.host, out, args.full,
                       args.show_heartbeat)
        finally:
            out.close()
            print(f"已写入 {args.out}")
        return 0

    compare(load_official(args.official), load_client(args.client))
    return 0


if __name__ == "__main__":
    sys.exit(main())
