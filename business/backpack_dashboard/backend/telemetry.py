"""遥测采集：封装 yd_test_v4.0.sh 中各检测项的取值逻辑，输出结构化字典。

为便于离线直连演示，所有函数都做了容错：命令缺失/失败返回 None 或默认值，
不会让接口整体 500。
"""
import subprocess
import json
import re
import os
import time
import shutil
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed
from config import (SHT4X_APP, MODEM_DEV, WIFI_IFACE, SSD_DEV,
                    LINGS_PLAY_WAV, LINGS_CMD_FIFO, LINGS_STATE_FILE,
                    ASR_SERVICE, LINGS_TEST_VOLUME, SN_PARTITION)
from at_lock import at_lock, clear_ghost_cat

# ---- 快照调度（性能）----
# 各采集项互不依赖，并发执行后总耗时≈最慢的一项，而不是逐项累加。
_SNAP_EXECUTOR = ThreadPoolExecutor(max_workers=8, thread_name_prefix="snap")
# 「单飞」锁：同一时刻只允许一次完整快照在跑。5s 周期任务若上一轮没结束，
# 直接复用缓存，避免多个快照堆在线程池里互相争抢 AT 串口锁（越刷越慢的根因）。
_SNAP_LOCK = threading.Lock()
_LAST_SNAP = None
# ping 结果短缓存：联网状态不是秒级变化，没必要每个快照都真 ping 一次（每次最坏 2s）。
_PING_CACHE = {}
_PING_CACHE_LOCK = threading.Lock()


def run(cmd, timeout=8):
    try:
        if isinstance(cmd, str):
            r = subprocess.run(cmd, shell=True, capture_output=True, text=True, timeout=timeout)
        else:
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return (r.stdout or "") + (r.stderr or "")
    except Exception:
        return ""


def ping_ok(host, count=1, wait=2, ttl=10):
    """带短 TTL 缓存的连通性探测。

    旧实现每个 5s 快照都真实 ping 一次，失败时要干等 wait 秒。缓存 ttl 秒内
    复用上次结果，联网状态本来就不是秒级变化，语义不变但省掉大量阻塞。
    """
    key = ("host", host, count, wait)
    now = time.time()
    with _PING_CACHE_LOCK:
        hit = _PING_CACHE.get(key)
        if hit and hit[0] > now:
            return hit[1]
    out = run(["ping", "-c", str(count), "-W", str(wait), host], timeout=wait + 3)
    ok = ("ttl=" in out) or ("icmp_seq" in out)
    with _PING_CACHE_LOCK:
        _PING_CACHE[key] = (now + ttl, ok)
    return ok


# ---------------- 温湿度 (SHT4x) ----------------
def get_temp_humidity():
    """直接运行 sht4x_app 并打印原始输出（最多 40 字符）。
    参考：/opt/xjpt/wenshidu/sht4x_app -> 'Temperature: 27.15 °C, Humidity: 56.64 %'
    """
    if not os.path.exists(SHT4X_APP):
        print(f"[DEBUG][temp] 未找到 {SHT4X_APP}", flush=True)
        return {"raw": None, "error": f"未找到 {SHT4X_APP}"}
    print(f"[DEBUG][temp] 运行 {SHT4X_APP}", flush=True)
    out = run([SHT4X_APP], timeout=8).strip()
    print(f"[DEBUG][temp] 原始输出={out!r}", flush=True)
    if not out:
        return {"raw": None, "error": "命令无输出（程序可能未就绪）"}
    return {"raw": out[:40]}


# ---------------- WiFi 扫描 ----------------
def detect_wifi_iface():
    out = run("iw dev | awk '/Interface/{print $2}'", timeout=5)
    for line in out.splitlines():
        line = line.strip()
        if line:
            return line
    return WIFI_IFACE or "wlan0"


def _signal_label(s):
    if s is None:
        return "未知"
    if s >= 80:
        return "优"
    if s >= 60:
        return "良"
    if s >= 40:
        return "中"
    return "差"


def get_wifi():
    iface = WIFI_IFACE or detect_wifi_iface()
    out = run(["nmcli", "-t", "-f", "SSID,SIGNAL,SECURITY", "dev", "wifi",
               "list", "ifname", iface], timeout=25)
    nets = []
    seen = set()
    for line in out.splitlines():
        if not line.strip():
            continue
        parts = line.split(":")
        ssid = parts[0] if len(parts) > 0 else ""
        sig = parts[1] if len(parts) > 1 else ""
        sec = parts[2] if len(parts) > 2 else ""
        if ssid in seen:
            continue
        seen.add(ssid)
        try:
            s = int(sig)
        except Exception:
            s = None
        nets.append({"ssid": ssid, "signal": s,
                     "quality": _signal_label(s), "security": sec})
    nets.sort(key=lambda x: (x["signal"] is None, -(x["signal"] or 0)))
    return {"iface": iface, "total": len(nets), "networks": nets}


# ---------------- 4G 模块 (AT) ----------------
def at_cmd(cmd, dev=MODEM_DEV, read_timeout=1.2):
    """镜像 yd_test_v4.0.sh 的 send_at：cat 端口后台读 + printf 写 + 定时 kill。
    不依赖 pyserial（板端测试脚本即采用此方式），兼容性更好。

    读取期间持有串口锁，避免与定位采集(location.collect_cell)并发读同一串口。
    """
    script = (
        'cat "' + dev + '" & pid=$!; '
        'sleep 0.2; '
        "printf '%s\\r' '" + cmd + "' > \"" + dev + "\"; "
        'sleep 0.5; '
        'kill $pid 2>/dev/null; wait $pid 2>/dev/null'
    )
    with at_lock():
        out = run(["bash", "-c", script], timeout=6)
    return out.replace("\0", "").replace("\r", "")


def read_at_full():
    """一次性发送全部 AT 命令，用 pyserial 把整段响应缓冲读出来。

    改用 pyserial（与 location.collect_cell 一致，已在本板 EC200A 验证可靠）：
      - 独占式读写，不受遗留 `cat /dev/ttyUSB2` 孤儿进程干扰；
      - 开头先 clear_ghost_cat() 清掉历史上残留的 cat，避免它们抢走模组响应
        （这正是"命令发了但读回空、信号失败+注册异常"的根因）。
    之前逐条发送+逐条读取（at_cmd）时，由于命令发送很快、调制解调器响应有
    延迟，各命令的响应会**错位**到下一个读取窗口（调试可见 CPIN/CSQ 在不同
    轮次乱跳）。改为整段读取后，每个响应含唯一前缀（+CPIN/+CSQ/+CREG/
    +CEREG/+QCCID/+QNWINFO），用正则从整段缓冲里分别提取，与到达顺序无关。

    读取期间持有串口锁，避免与定位采集并发读同一串口。

    命令集：CPIN( SIM) / CSQ(信号) / CREG(2G注册,兜底) / CEREG=2+CEREG?(LTE
    注册,EC200A 实际用此) / QCCID(ICCID) / QNWINFO(网络类型/运营商/Band)。
    """
    dev = MODEM_DEV
    if not (os.path.exists(dev) and os.access(dev, os.R_OK) and os.access(dev, os.W_OK)):
        return None
    try:
        import serial
    except Exception as e:
        print(f"[DEBUG][4g] pyserial 不可用: {e}", flush=True)
        return ""
    clear_ghost_cat()
    try:
        ser = serial.Serial(dev, 115200, timeout=1.5, exclusive=False)
    except Exception as e:
        print(f"[DEBUG][4g] 打开串口失败 {dev}: {e}", flush=True)
        return ""

    try:
        with at_lock():
            ser.flushInput()
            time.sleep(0.05)

            def _send(cmd, t=1.0):
                """发送一条 AT 命令，收到终止符(OK/ERROR)即提前返回；t 只是超时上限。

                旧实现无条件等满 t 秒，8 条合计固定 6.4s，是首屏最慢的根源。
                模组实际几十~几百毫秒就回完，因此改为响应驱动：拿到 OK/ERROR
                后再补 60ms 把同一条命令的多行响应/URC 尾巴收齐即可返回。
                整段缓冲按 +CPIN/+CSQ/+CEREG 等唯一前缀解析，与到达顺序无关，
                所以即使某条命令的 URC 落到下一个窗口也不会错位。
                """
                ser.write((cmd + "\r").encode())
                buf = b""
                deadline = time.time() + t
                while True:
                    n = ser.in_waiting
                    if n:
                        buf += ser.read(n)
                        tail = buf[-64:]
                        if (b"OK\r" in tail or b"OK\n" in tail
                                or b"ERROR\r" in tail or b"ERROR\n" in tail):
                            time.sleep(0.06)   # 收齐尾巴（多行响应 / URC）
                            if ser.in_waiting:
                                buf += ser.read(ser.in_waiting)
                            return buf
                    if time.time() >= deadline:
                        return buf
                    time.sleep(0.02)

            chunks = []
            chunks.append(_send("AT", 0.6))          # 唤醒并清掉旧缓冲
            chunks.append(_send("AT+CPIN?", 1.0))    # SIM 状态
            chunks.append(_send("AT+CSQ", 1.0))      # 信号强度
            chunks.append(_send("AT+CREG?", 1.0))    # 2G 注册（兜底）
            chunks.append(_send("AT+CEREG=2", 0.8))  # LTE 注册：设 mode2 才能拿到 tac/ci
            chunks.append(_send("AT+CEREG?", 1.2))   # LTE 注册（EC200A 仅 LTE，实际用此）
            chunks.append(_send("AT+QCCID", 1.0))    # ICCID
            chunks.append(_send("AT+QNWINFO", 1.5))  # 网络类型/运营商/Band

        raw = b"".join(chunks)
        return raw.decode(errors="replace").replace("\0", "").replace("\r", "")
    except Exception as e:
        print(f"[DEBUG][4g] read_at_full 异常: {e}", flush=True)
        return ""
    finally:
        try:
            ser.close()
        except Exception:
            pass


def detect_4g_card():
    out = run("ip -o link show | grep -oP 'enx[0-9a-f]+(?=:)' | head -1", timeout=5)
    out = out.strip()
    if not out:
        return None
    return out.splitlines()[0].strip()


def ping_ok_card(card, host, count=1, wait=2):
    out = run(["ping", "-I", card, "-c", str(count), "-W", str(wait), host],
              timeout=wait + 2)
    return ("ttl=" in out) or ("icmp_seq" in out)


def _read_netstat(card, which):
    p = f"/sys/class/net/{card}/statistics/{which}"
    try:
        with open(p) as f:
            return int(f.read().strip())
    except Exception:
        return None


def _mask_iccid(iccid):
    if len(iccid) >= 12:
        return iccid[:6] + "******" + iccid[-6:]
    return iccid


_OPERATORS = {
    "46000": "China Mobile", "46002": "China Mobile", "46004": "China Mobile",
    "46007": "China Mobile", "46008": "China Mobile",
    "46001": "China Unicom", "46006": "China Unicom", "46009": "China Unicom",
    "46003": "China Telecom", "46005": "China Telecom", "46011": "China Telecom",
}


def build_4g_lines(res):
    """把结构化结果汇总成易读文本（对齐 yd_test_v4.0.sh 展示风格）。"""
    lines = []
    svc = res.get("service_state") or "未知"
    lines.append(f"4g-connect服务：{'已启动' if svc == 'active' else svc}")
    lines.append(f"网卡：{res.get('card') or '未检测到'} | USB={res.get('usb') or '未识别'} | AT={res.get('at_port') or '未找到'}")
    lines.append(f"SIM：{res.get('sim_state') or 'UNKNOWN'}")
    if res.get("rssi_dbm") is not None:
        sig = f"{res['rssi_dbm']}dBm({res['signal_label']})"
        net = " | ".join(x for x in [res.get("network_type"), res.get("operator"), res.get("band")] if x)
        lines.append(f"信号：{sig}" + (f" | {net}" if net else ""))
    else:
        lines.append("信号获取：失败")
    lines.append(f"注册：{res.get('register_state') or '异常'}")
    if res.get("iccid"):
        lines.append(f"ICCID：{res['iccid']}")
    if res.get("ip"):
        msg = f"IP：{res['ip']}"
        if res.get("gateway"):
            msg += f" | GW={res['gateway']}"
        if res.get("dns"):
            msg += f" | DNS={res['dns']}"
        lines.append(msg)
        lines.append(f"Internet：{res.get('internet_msg') or '连接失败'}")
        if res.get("rx_kb") is not None:
            lines.append(f"流量：RX={res['rx_kb']}KB | TX={res['tx_kb']}KB")
    else:
        if (res.get("sim_state") or "") in ("UNKNOWN", "unknown", ""):
            lines.append("IP：未获取，请检查SIM卡状态")
        else:
            lines.append("IP：未获取（可能正在拨号或信号较弱）")
    if res.get("note"):
        lines.append(res["note"])
    return lines


def get_4g():
    res = {
        "service_state": "unknown", "card": None, "usb": None,
        "at_port": None, "sim_state": "UNKNOWN", "sim_ok": False,
        "register_state": "异常", "iccid": None, "rssi_dbm": None,
        "signal_label": "未知", "network_type": None, "operator": None,
        "band": None, "ip": None, "gateway": None, "dns": None,
        "internet": False, "internet_msg": "", "rx_kb": None, "tx_kb": None,
    }

    # 1) 4G 网卡（enx*）
    card = detect_4g_card()
    res["card"] = card
    if not card:
        res["note"] = "未检测到 4G 网卡(enx*)"
        res["lines"] = build_4g_lines(res)
        return res

    # 2) 服务状态（单次快速检查，避免阻塞周期推送）
    res["service_state"] = run(["systemctl", "is-active", "4g-connect.service"]).strip() or "unknown"
    print(f"[DEBUG][4g] service_state={res['service_state']!r} card={card!r}", flush=True)

    # 3) USB(Quectel)
    usb = run("lsusb | grep -i Quectel | head -1", timeout=5)
    if usb.strip():
        bus = re.search(r'Bus (\d+)', usb)
        devn = re.search(r'Device (\d+)', usb)
        res["usb"] = f"Bus{bus.group(1) if bus else '?'} Dev{devn.group(1) if devn else '?'}"
    else:
        res["usb"] = None

    # 4) AT 口（沿用脚本：存在且可读写才用 /dev/ttyUSB2）
    at_port = None
    if os.path.exists(MODEM_DEV) and os.access(MODEM_DEV, os.R_OK) and os.access(MODEM_DEV, os.W_OK):
        at_port = MODEM_DEV
    res["at_port"] = at_port
    print(f"[DEBUG][4g] at_port={at_port!r}", flush=True)
    if not at_port:
        res["note"] = "AT 串口不可用（/dev/ttyUSB2 不存在或无读写权限）"
        res["lines"] = build_4g_lines(res)
        return res

    # 5) AT 信息：一次性发送全部命令并读取整段缓冲，再各自解析（避免错位）
    buf = read_at_full()
    print(f"[DEBUG][4g] AT整段缓冲={buf!r}", flush=True)
    cpin = csq = creg = qccid = qnw = buf or ""

    # SIM
    if "READY" in cpin:
        res["sim_state"] = "READY"
        res["sim_ok"] = True
    else:
        m = re.search(r"\+CPIN:\s*([^\r\n]+)", cpin)
        res["sim_state"] = (m.group(1).strip() if m else "") or "UNKNOWN"
        res["sim_ok"] = False

    # 信号：RSSI(0-31) -> dBm = RSSI*2-113
    m = re.search(r"\+CSQ:\s*(\d+),(\d+)", csq)
    if m:
        rssi = int(m.group(1))
        if rssi != 99:
            dbm = rssi * 2 - 113
            res["rssi_dbm"] = dbm
            if dbm >= -70:
                res["signal_label"] = "优"
            elif dbm >= -85:
                res["signal_label"] = "良"
            elif dbm >= -100:
                res["signal_label"] = "中"
            else:
                res["signal_label"] = "差"

    # 注册状态：LTE(Cat.1) 用 CEREG，2G 用 CREG 兜底。
    # EC200A 仅支持 LTE，AT+CREG? 恒返回 0,0（未注册），必须以 CEREG 为准。
    reg_stat = None
    m_cereg = re.search(r"\+CEREG:\s*\d+,(\d+)", buf or "")
    if m_cereg:
        reg_stat = m_cereg.group(1)
    else:
        m_creg = re.search(r"\+CREG:\s*\d+,(\d+)", buf or "")
        if m_creg:
            reg_stat = m_creg.group(1)
    if reg_stat:
        res["register_state"] = {
            "1": "已注册", "5": "漫游", "2": "搜索中",
            "3": "拒绝", "4": "未知",
        }.get(reg_stat, "异常")

    # ICCID（脱敏）
    m = re.search(r"\+QCCID:\s*(\d+)", qccid)
    if m:
        res["iccid"] = _mask_iccid(m.group(1))

    # 网络类型 / 运营商 / Band
    # QNWINFO 形如："LTE FDD","46000","LTE",BAND 3（第 4 字段常未加引号）。
    # 按逗号切分再逐字段找含 BAND 的，兼容引号/无引号两种写法，避免误取第三字段。
    m = re.search(r'\+QNWINFO:\s*(.+)', buf or "")
    if m:
        parts = [p.strip().strip('"') for p in m.group(1).split(",")]
        if parts:
            res["network_type"] = parts[0] or None
        if len(parts) >= 2:
            mccmnc = parts[1].strip()
            res["operator"] = _OPERATORS.get(mccmnc, mccmnc or None)
        band = None
        for p in parts[2:]:
            if re.search(r'(?i)BAND', p):
                band = re.sub(r'(?i)^(LTE\s+)?BAND\s*', '', p.strip())
                break
        if band:
            res["band"] = "Band" + band

    # 6) IP / 网关 / DNS
    ip_out = run(["ip", "-4", "addr", "show", card], timeout=5)
    m = re.search(r'inet (\d+\.\d+\.\d+\.\d+/\d+)', ip_out)
    if m:
        res["ip"] = m.group(1)
    gw = run(f"ip route | awk -v dev={card} '$1==\"default\" && $5==dev {{print $3; exit}}'", timeout=5)
    if gw.strip():
        res["gateway"] = gw.strip()
    if shutil.which("nmcli"):
        # 只取 nmcli 输出中"看起来像 DNS 服务器"的行。
        # 注意：4G 网卡常由厂商脚本直连内核，NetworkManager 并不托管它，
        # nmcli 会回 "Device 'xxx' not found"（走 stderr）。这里 2>/dev/null
        # 丢 stderr，并用正则只留 IPv4 行，避免把报错串当成 DNS 显示。
        raw = run(f"nmcli -g IP4.DNS dev show {card} 2>/dev/null", timeout=5)
        servers = [l.strip() for l in raw.splitlines()
                   if l.strip() and re.search(r"\d+\.\d+\.\d+\.\d+", l)]
        if servers:
            res["dns"] = ",".join(servers)
    # 兜底：nmcli 取不到（网卡非 NM 托管）时，从 /etc/resolv.conf 读全局 DNS。
    # 4G 拨号拿到的 DNS 通常就写在这里，作为该网卡实际使用的解析服务器是合理的。
    if not res.get("dns"):
        rrc = run("grep -h '^nameserver' /etc/resolv.conf 2>/dev/null", timeout=5)
        flat = []
        for l in rrc.splitlines():
            m = re.search(r"nameserver\s+(\S+)", l)
            if m:
                flat.append(m.group(1))
        if flat:
            res["dns"] = ",".join(flat)

    # 7) 外网连通性：3 个目标并发探测。
    #    旧版串行最坏 3×2s=6s；并发后只等最慢的一个（约 1~3s），
    #    任一成功即按原优先级取对应提示，判定语义完全不变。
    if res["ip"]:
        targets = ("223.5.5.5", "8.8.8.8", "baidu.com")
        with ThreadPoolExecutor(max_workers=3, thread_name_prefix="ping4g") as ex:
            futs = [ex.submit(ping_ok_card, card, h, 1, 1) for h in targets]
            results = []
            for f in futs:
                try:
                    results.append(f.result(timeout=3))
                except Exception:
                    results.append(False)
        for h, ok in zip(targets, results):
            if ok:
                res["internet"] = True
                res["internet_msg"] = ("OK (DNS正常)" if h == "baidu.com"
                                       else f"OK ({h})")
                break
        if not res["internet"]:
            res["internet_msg"] = "连接失败"

    # 8) 流量统计（不依赖是否拿到 IP）
    rx = _read_netstat(card, "rx_bytes")
    tx = _read_netstat(card, "tx_bytes")
    if rx is not None:
        res["rx_kb"] = rx // 1024
    if tx is not None:
        res["tx_kb"] = tx // 1024

    res["lines"] = build_4g_lines(res)
    return res


# ---------------- 环境信息 ----------------
def core_temp():
    try:
        temps = []
        base = "/sys/class/thermal"
        for z in os.listdir(base):
            if z.startswith("thermal_zone"):
                with open(os.path.join(base, z, "temp")) as f:
                    temps.append(int(f.read().strip()) / 1000.0)
        if temps:
            return round(sum(temps) / len(temps), 1)
    except Exception:
        pass
    return None


def ssd_info():
    cap = None
    temp = None
    try:
        out = run(["lsblk", "-b", "-n", "-o", "SIZE", SSD_DEV], timeout=5)
        for line in out.splitlines():
            line = line.strip()
            if line.isdigit():
                cap = int(line)
                break
    except Exception:
        pass
    out = run(["smartctl", "-A", SSD_DEV], timeout=8)
    m = re.search(r"Temperature:\s*([\-0-9]+)", out)
    if m:
        temp = int(m.group(1))
    else:
        m = re.search(r"Temperature_Celsius\S*\s+(\d+)", out)
        if m:
            temp = int(m.group(1))
    return {"capacity_bytes": cap, "temperature_c": temp}


def system_info():
    uname = run(["uname", "-snr"]).strip()
    uptime = run(["uptime", "-p"]).strip().replace("up ", "")
    mem = None
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if line.startswith("MemTotal:"):
                    mem = int(line.split()[1]) * 1024  # bytes
                    break
    except Exception:
        pass
    return {"uname": uname, "uptime": uptime, "mem_total_bytes": mem}


def get_env():
    interfaces = []
    try:
        data = json.loads(run(["ip", "-json", "addr"]))
        for ifc in data:
            name = ifc.get("ifname", "")
            if name in ("lo",):
                continue
            ips = [a["local"] for a in ifc.get("addr_info", [])
                   if a.get("family") == "inet"]
            if ips:
                interfaces.append({"name": name, "ip": ips[0]})
    except Exception:
        pass
    return {"interfaces": interfaces, "core_temp_c": core_temp(),
            "ssd": ssd_info(), "system": system_info(), "sn": get_sn()}


# ---------------- 风扇 ----------------
def get_fan():
    # 脚本 test_fan 仅有手动提示、无传感器；演示用绿色图标即可
    return {"status": "normal", "color": "green",
            "note": "无温度传感器，演示固定绿色图标"}


# ---------------- 聆思小板 ----------------
def _read_last_cmd():
    """读取状态文件里最近一次写入的『原始命令文本』（含 phone/call 参数）。

    之前从 FIFO /tmp/voice_assistant_cmd 读，但 FIFO 的数据被 asr 消费后就没了，
    永远读不回自己刚写的内容。现在改读普通状态文件 LINGS_STATE_FILE，
    不阻塞、永远可读。
    """
    try:
        with open(LINGS_STATE_FILE) as f:
            lines = [l.strip() for l in f.read().splitlines() if l.strip()]
        return lines[-1] if lines else None
    except Exception:
        return None


def _parse_mode(raw):
    """从原始命令文本推导当前模式（call/voice/phone/None）。"""
    if not raw:
        return None
    if raw == "voice" or raw.startswith("voice "):
        return "voice"
    if raw.startswith("phone"):
        return "phone"
    if raw == "call" or raw.startswith("call"):
        return "call"
    return None


def _read_lingsi_cmd():
    """兼容旧逻辑：返回 call/voice/phone/None（供 run_lingsi_test 等使用）。"""
    return _parse_mode(_read_last_cmd())


def _write_lingsi_cmd(text):
    """同时写 FIFO（给 asr 消费）+ 状态文件（供前端查询最近模式/音量）。

        echo "call"          > /tmp/voice_assistant_cmd   ← FIFO，给 asr
        echo "call"          > /tmp/lingsi_last_cmd        ← 普通文件，给我们读

    FIFO 那侧加 2 秒超时保护：asr 默认未启动时无 reader，echo 的 > 重定向会阻塞。

    注意：phone stop / call stop 之后板子会回到 voice 模式，因此状态文件里
    记录为 "voice"，以如实反映板端当前所处模式（FIFO 仍写原始 stop 指令）。
    """
    text = (text or "").strip()
    # 1) 写 FIFO（带超时；失败不影响状态记录）
    fifo_cmd = f'echo "{text}" > {LINGS_CMD_FIFO}'
    try:
        subprocess.run(fifo_cmd, shell=True, capture_output=True, timeout=2)
    except subprocess.TimeoutExpired:
        pass
    except Exception:
        pass

    # 2) 写状态文件（普通文件，永远不会阻塞）
    #    stop 类指令后板子回到 voice，状态文件记 voice 反映真实模式
    state_text = "voice" if text in ("phone stop", "call stop") else text
    try:
        with open(LINGS_STATE_FILE, "w") as f:
            f.write(state_text + "\n")
        return True
    except Exception:
        return False


def send_lingsi_cmd(cmd):
    """发送任意聆思指令（phone / call 带内容 / call stop / voice 等）到 FIFO + 状态文件。

    比 set_lingsi_mode 更通用，覆盖新增的 phone 模式与 call 带内容播报。
    返回 {ok, cmd, mode}；mode 由状态文件回读推导，便于前端确认生效。
    """
    cmd = (cmd or "").strip()
    if not cmd:
        return {"ok": False, "error": "cmd 不能为空"}
    ok = _write_lingsi_cmd(cmd)
    raw = _read_last_cmd()
    return {"ok": ok, "cmd": cmd, "mode": _parse_mode(raw)}


def ensure_asr():
    """asr 默认不启动，需要时拉起并等待 2s（与 yd_test_v4.0.sh 一致）。"""
    svc = ASR_SERVICE
    if "active" not in run(["systemctl", "is-active", svc]).strip():
        run(["systemctl", "start", svc])
        time.sleep(2)
    return "active" in run(["systemctl", "is-active", svc]).strip()


def set_lingsi_mode(mode):
    """兼容旧按钮：仅 call/voice 两种基础模式（无参数）。"""
    if mode not in ("call", "voice"):
        return {"ok": False, "error": "mode 仅支持 call/voice"}
    return send_lingsi_cmd(mode)


def set_lingsi_volume(level):
    try:
        lv = int(level)
    except Exception:
        return {"ok": False, "error": "level 需为 0-100 整数"}
    lv = max(0, min(100, lv))
    return {"ok": _write_lingsi_cmd(f"set volume={lv}"), "volume": lv}


def get_lingsi_status():
    """轻量状态（供周期快照，不切换模式/不拉起服务）。"""
    usb = run(["lsusb"])
    adb = "ADB" in usb
    mic = "MIC" in usb.upper()
    asr_state = run(["systemctl", "is-active", ASR_SERVICE]).strip() or "unknown"
    asr = asr_state == "active"
    raw = _read_last_cmd()
    mode = _parse_mode(raw)
    phone = None
    if mode == "phone" and raw and raw.startswith("phone ") and len(raw.split()) >= 3:
        try:
            _, ip, port = raw.split()[:3]
            phone = {"ip": ip, "port": int(port)}
        except Exception:
            pass
    # 播报中：call 后跟 0/1 再加内容（单次/循环播报），call stop 不在此列
    broadcasting = bool(raw and (raw.startswith("call 0 ") or raw.startswith("call 1 ")))
    net = ping_ok("www.baidu.com")
    # phone 走局域网 UDP，不要求外网；call/voice/播报 需背包联网
    net_required = mode != "phone"
    return {"adb_detected": adb, "mic_detected": mic,
            "asr_running": asr, "asr_state": asr_state,
            "mode": mode, "cmd": raw, "phone": phone,
            "broadcasting": broadcasting, "network_ok": net,
            "net_required": net_required,
            "wake_tip": "可联网，可用『小谷小谷』唤醒对话（对话请保持周围环境安静）" if net
            else "未联网，无法语音唤醒"}


def get_asr_status():
    """查询 asr 服务状态（镜像 yd_test_v4.0.sh 的 case 逻辑）：
        active | activating | inactive | failed | unknown。
    """
    state = run(["systemctl", "is-active", ASR_SERVICE]).strip()
    return {"state": state or "unknown", "ok": state == "active"}


def start_asr():
    """systemctl start asr —— 后台触发，立即返回。
    不做 sleep / 状态回读，按钮按下后无 UI 变化；用户随后点刷新按钮查 asr 状态。
    """
    try:
        subprocess.Popen(["systemctl", "start", ASR_SERVICE])
        return {"ok": True, "action": "start"}
    except Exception as e:
        return {"ok": False, "error": str(e), "action": "start"}


def stop_asr():
    """systemctl stop asr —— 后台触发，立即返回。"""
    try:
        subprocess.Popen(["systemctl", "stop", ASR_SERVICE])
        return {"ok": True, "action": "stop"}
    except Exception as e:
        return {"ok": False, "error": str(e), "action": "stop"}


def run_lingsi_test():
    """完整测试序列（对应"执行测试"按钮）：
    ADB/MIC 检测 -> 启动 asr -> 设音量 -> 切 call 模式做播放测试 -> 切回 voice -> 联网判断。
    """
    usb = run(["lsusb"])
    adb = "ADB" in usb
    mic = "MIC" in usb.upper()
    asr = ensure_asr()
    _write_lingsi_cmd(f"set volume={LINGS_TEST_VOLUME}")
    time.sleep(0.3)
    _write_lingsi_cmd("call")
    time.sleep(3)
    mode_after = _read_lingsi_cmd()
    playback_ok = bool(asr and mode_after == "call")
    net = ping_ok("www.baidu.com")
    _write_lingsi_cmd("voice")  # 恢复默认 voice 模式
    return {"adb_detected": adb, "mic_detected": mic, "asr_running": asr,
            "mode": "voice", "playback_ok": playback_ok, "network_ok": net,
            "wake_tip": "可联网，可用『小谷小谷』唤醒对话（对话请保持周围环境安静）" if net
            else "未联网，无法语音唤醒"}


# ---------------- SN 码（镜像 yd_test_v4.0.sh: test_sn_code） ----------------
def get_sn():
    """SN 取自 /dev/mmcblk0p5，镜像 test_sn_code 的取数命令：
        hexdump -C /dev/mmcblk0p5 2>/dev/null | sed -n '3s/.*|\\(.*\\)|.*/\\1/p' | tr -d ' '
    校验规则：必须为 16 位且每个字符是数字或大写字母，且 '0' 的数量不超过 10；
    合规才输出 SN，否则前端显示 '-'。
    """
    cmd = f"hexdump -C {SN_PARTITION} 2>/dev/null | sed -n '3s/.*|\\(.*\\)|.*/\\1/p' | tr -d ' '"
    print(f"[DEBUG][sn] cmd={cmd!r}", flush=True)
    raw = run(cmd, timeout=10).strip()
    print(f"[DEBUG][sn] 原始输出={raw!r}", flush=True)
    if not raw:
        return {"sn": None, "status": "error", "note": "SN 读取为空"}
    if re.fullmatch(r"[0-9A-Z]{16}", raw) and raw.count("0") <= 10:
        return {"sn": raw, "status": "ok", "note": "16位SN(合规)"}
    return {"sn": None, "status": "error", "note": "SN 不合规"}


# ---------------- 聚合快照（SSE 推送用） ----------------
def _safe(name, fn, fallback):
    try:
        return fn()
    except Exception as e:
        import traceback as _tb
        print(f"[ERROR][{name}] {e}", flush=True)
        print(_tb.format_exc(), flush=True)
        return fallback


def _empty_snapshot():
    """结构完整的空快照，保证任何时刻推给前端都不会渲染报错。"""
    return {
        "temp": {"raw": None, "error": "采集中"},
        "wifi": {"total": 0, "networks": []},
        "g4": {"lines": ["采集中…"]},
        "env": {"interfaces": [], "sn": {"sn": None, "status": "error"}},
        "fan": {"status": "normal"},
        "lingsi": {"asr_running": False},
        "ts": int(time.time()),
    }


def cached_snapshot():
    """立即返回最近一次完整快照（纯内存，毫秒级，不触发任何采集）。

    供 SSE 建连首帧使用：页面一打开就能把各框填好，随后由 5s 周期任务
    自动刷新成最新值，恢复「打开即见 + 自动刷新」的体验。
    """
    return _LAST_SNAP


def snapshot():
    """并发采集各项遥测；总耗时≈最慢的一项，而不是逐项累加。

    单飞：若已有快照正在采集（上一轮未结束），直接返回缓存，避免 5s 周期
    任务在线程池里堆积、并发争抢 AT 串口锁造成「越刷越慢」。
    """
    global _LAST_SNAP
    if not _SNAP_LOCK.acquire(blocking=False):
        return _LAST_SNAP or _empty_snapshot()
    try:
        items = (
            ("temp",   get_temp_humidity, {"raw": None, "error": "采集异常"}),
            ("wifi",   get_wifi,          {"total": 0, "networks": []}),
            ("g4",     get_4g,            {"lines": ["4G 采集异常"]}),
            ("env",    get_env,           {"interfaces": [], "sn": {"sn": None, "status": "error"}}),
            ("fan",    get_fan,           {"status": "normal"}),
            ("lingsi", get_lingsi_status, {"asr_running": False}),
        )

        def _job(item):
            name, fn, fb = item
            return name, _safe(name, fn, fb)

        futs = [_SNAP_EXECUTOR.submit(_job, it) for it in items]
        data = _empty_snapshot()
        try:
            # 统一 12s 上限：个别项卡住也不会拖死整个快照，已完成的项照常展示
            for f in as_completed(futs, timeout=12):
                try:
                    k, v = f.result()
                    data[k] = v
                except Exception as e:
                    print(f"[WARN][snapshot] 采集项异常: {e}", flush=True)
        except Exception as e:
            print(f"[WARN][snapshot] 部分采集超时: {e}", flush=True)
        data["ts"] = int(time.time())
        _LAST_SNAP = data
        return data
    finally:
        _SNAP_LOCK.release()
