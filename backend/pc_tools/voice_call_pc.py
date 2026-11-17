#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
PC 端对讲 demo（与板子 voice_call 模块配套）

音频：16000Hz / 单声道 / S16_LE / 20ms 一帧（320 采样 = 640 字节）
协议：12 字节极简 RTP 头 + 640 字节 PCM
      0x80 | PT=96 | seq(2,BE) | timestamp(4,BE) | ssrc(4,BE)

依赖：pip install sounddevice

用法：
  1) 板子上先用 FIFO 建立会话（板子监听 5000，往 PC 的 5000 发）：
     echo "phone 192.168.1.50 5000" > /tmp/voice_assistant_cmd
  2) PC 上运行：
     python voice_call_pc.py --peer 192.168.234.200 --peer-port 5000 --local-port 5000
  3) 本机自环测试（不接板子，验证声卡链路）：
     python voice_call_pc.py --loopback
  4) 不指定设备时用系统默认设备，想指定就 --in-dev / --out-dev，编号用 --list-devices 查

注意：Windows 防火墙需要放行 UDP 5000 入站，否则听不到板子声音。
"""

import argparse
import socket
import struct
import sys
import threading
import time
from collections import deque

try:
    import sounddevice as sd
except ImportError:
    print("缺少依赖，请先执行： pip install sounddevice")
    sys.exit(1)

# ====================== 协议常量（必须与 voice_call.cpp 一致）======================
RATE = 16000
FRAME_SAMPLES = 320            # 20ms
FRAME_BYTES = FRAME_SAMPLES * 2
HDR = struct.Struct("!BBHII")  # V/P/PT, PT, seq, ts, ssrc
HDR_BYTES = HDR.size           # 12
PT = 96
SSRC = 0x50434731              # 'PCG1'

JB_CAP = 10                    # 抖动缓冲上限 200ms
JB_PRIME = 3                   # 攒 3 帧(60ms) 再出声
JB_DROP_HIGH = 9               # 积压超过 9 帧丢最旧

SEQ_RESYNC = 200               # seq 跳变超过 200 包(4s) 视为对端重启，重新对齐

SILENCE = b"\x00" * FRAME_BYTES


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.jb = deque(maxlen=JB_CAP)
        self.primed = False
        self.tx = 0
        self.rx = 0
        self.lost = 0
        self.tx_err = 0
        self.underrun = 0
        self.seq = 0
        self.ts = 0
        self.last_seq = None
        self.running = True

    # ---------- 抖动缓冲 ----------
    def push(self, payload: bytes):
        with self.lock:
            self.jb.append(payload)

    def pop(self):
        with self.lock:
            if not self.primed:
                if len(self.jb) >= JB_PRIME:
                    self.primed = True
                else:
                    return None
            if not self.jb:
                return None
            if len(self.jb) >= JB_DROP_HIGH:
                self.jb.popleft()       # 延迟太大，丢一帧往回压
            return self.jb.popleft()

    def buffered(self):
        with self.lock:
            return len(self.jb)


def build_packet(st: State, payload: bytes) -> bytes:
    with st.lock:
        seq = st.seq & 0xFFFF
        ts = st.ts & 0xFFFFFFFF
        st.seq = (st.seq + 1) & 0xFFFF
        st.ts = (st.ts + FRAME_SAMPLES) & 0xFFFFFFFF
    return HDR.pack(0x80, PT, seq, ts, SSRC) + payload


def rx_thread(sock: socket.socket, st: State):
    while st.running:
        try:
            data, _addr = sock.recvfrom(2048)
        except socket.timeout:
            continue
        except OSError:
            break
        if len(data) <= HDR_BYTES:
            continue
        try:
            _v, _pt, seq, _ts, _ssrc = HDR.unpack_from(data, 0)
        except struct.error:
            continue
        v = data[0] >> 6
        if v != 2:
            continue

        with st.lock:
            if st.last_seq is None:
                st.last_seq = seq
            else:
                d = (seq - st.last_seq) & 0xFFFF
                if d == 0:                        # 重复包
                    continue
                accept = False
                if d < 0x8000:                    # 比上一个新
                    if d > SEQ_RESYNC:            # 跳太多 => 对端重启，重新对齐
                        accept = True
                    else:
                        if d > 1:
                            st.lost += d - 1
                        accept = True
                    st.last_seq = seq
                else:                             # 比上一个旧
                    if (0x10000 - d) > SEQ_RESYNC:   # 回退太多 => 对端重启
                        st.last_seq = seq
                        accept = True
                if not accept:
                    continue

        payload = data[HDR_BYTES:]
        if len(payload) > FRAME_BYTES:
            payload = payload[:FRAME_BYTES]
        elif len(payload) < FRAME_BYTES:
            payload = payload + b"\x00" * (FRAME_BYTES - len(payload))

        st.push(payload)
        with st.lock:
            st.rx += 1


def _fill_out(outdata, data: bytes):
    """把一帧 PCM 写进 outdata（兼容 ndarray / CFFI buffer 两种形态）"""
    mv = memoryview(outdata)
    if mv.format != "B":
        mv = mv.cast("B")
    n = mv.nbytes
    if len(data) < n:
        data = data + b"\x00" * (n - len(data))
    mv[:] = data[:n]


def make_callback(sock, st: State, peer):
    def callback(indata, outdata, frames, time_info, status):
        # 上行：麦克风数据直接打包发出
        try:
            pcm = bytes(memoryview(indata))
            nblk = max(1, len(pcm) // FRAME_BYTES)
            for i in range(nblk):
                chunk = pcm[i * FRAME_BYTES:(i + 1) * FRAME_BYTES]
                if len(chunk) < FRAME_BYTES:
                    chunk = chunk + b"\x00" * (FRAME_BYTES - len(chunk))
                try:
                    sock.sendto(build_packet(st, chunk), peer)
                    with st.lock:
                        st.tx += 1
                except OSError:
                    with st.lock:
                        st.tx_err += 1
        except Exception:
            pass

        # 下行：从抖动缓冲取一帧，没有就出静音（保持 AEC 参考连续）
        frame = st.pop()
        if frame is None:
            frame = SILENCE
            with st.lock:
                st.underrun += 1
        _fill_out(outdata, frame)

    return callback


def main():
    ap = argparse.ArgumentParser(description="PC 端 UDP 对讲 demo")
    ap.add_argument("--peer", help="板子 IP（--loopback 时不用填）")
    ap.add_argument("--peer-port", type=int, default=5000, help="板子端口，默认 5000")
    ap.add_argument("--local-port", type=int, default=5000, help="本机端口，默认 5000")
    ap.add_argument("--loopback", action="store_true", help="本机自环测试，不需要板子")
    ap.add_argument("--in-dev", type=int, default=None, help="输入设备编号")
    ap.add_argument("--out-dev", type=int, default=None, help="输出设备编号")
    ap.add_argument("--list-devices", action="store_true", help="列出音频设备后退出")
    args = ap.parse_args()

    if args.list_devices:
        print(sd.query_devices())
        return 0

    if args.loopback:
        peer = ("127.0.0.1", args.local_port)
    else:
        if not args.peer:
            print("请用 --peer 指定板子 IP，或加 --loopback 做本机自环")
            return 1
        peer = (args.peer, args.peer_port)

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("0.0.0.0", args.local_port))
    sock.settimeout(0.5)

    st = State()
    th = threading.Thread(target=rx_thread, args=(sock, st), daemon=True)
    th.start()

    print("=" * 60)
    print("  UDP 对讲 demo   本机端口=%d  对端=%s:%d" % (args.local_port, peer[0], peer[1]))
    print("  16kHz/mono/s16le  20ms/包  640B PCM + 12B 头")
    print("  按 Ctrl+C 结束")
    print("=" * 60)

    cb = make_callback(sock, st, peer)
    dev = None
    if args.in_dev is not None or args.out_dev is not None:
        dev = (args.in_dev, args.out_dev)

    try:
        with sd.RawStream(samplerate=RATE, channels=1, dtype="int16",
                          blocksize=FRAME_SAMPLES, latency="low",
                          callback=cb, device=dev):
            t0 = time.time()
            while True:
                time.sleep(5)
                with st.lock:
                    tx, rx, lost, terr, ur = st.tx, st.rx, st.lost, st.tx_err, st.underrun
                    st.underrun = 0
                print("[%6.1fs] tx=%d rx=%d lost=%d txerr=%d underrun/5s=%d jbuf=%d"
                      % (time.time() - t0, tx, rx, lost, terr, ur, st.buffered()))
    except KeyboardInterrupt:
        print("\n结束")
    except Exception as e:
        print("音频流异常: %r" % (e,))
        print("提示：先 --list-devices 看设备名，再 --in-dev/--out-dev 指定")
        return 1
    finally:
        st.running = False
        sock.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
