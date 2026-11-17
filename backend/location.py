"""设备定位：采集小区信息 -> 解析 -> (联网时)解析经纬度 -> 本地文件记录(轨迹+最新)

设计目标（按"先跑通、再精细"落地）：

  1. 采集：AT+CEREG? / AT+QENG="servingcell" 读取小区号。
           这一步**离线可用**，不需要 PDP 数据连接（板子当前 NOCONN 也能跑）。
  2. 解析：从 CEREG 取权威的 TAC/CI（十六进制，已验证比 QENG 私有格式可信），
           从 QENG 取 MCC/MNC 与信号强度。
  3. 上报/解析：若板子能联网，先查国内基站库 cellocation(免 key，3 亿条)，
            命中即用其坐标；否则尝试 OpenCellID(需免费 key，国内覆盖弱)；
            都失败则 IP 地理定位(ipinfo.io，无需 key，城市级)兜底。
            三者都失败（未联网/小区未入库）→ 只记录小区号，lat/lng 留空待后续解析。
  4. 记录：追加写入本地 JSONL 轨迹文件（含时间戳，可用于"防丢找回"路径回放），
           并维护一个 latest.json 最新点文件。全部在板端本地，不依赖任何外部服务器。

对外暴露：
   collect_cell()            采集并解析小区信息（离线）
   resolve_location(cell)    联网解析经纬度（cellocation -> OpenCellID -> IP，无需付费 key）
   update_location()         采集 + 解析 + 本地落盘，返回合并后的记录
   load_latest() / load_trajectory(n)   供 /api/location 读取
"""
import os
import re
import json
import time
import threading

import telemetry
from at_lock import at_lock, clear_ghost_cat

# ---- 路径（默认放在本脚本所在目录，即 start.sh 当前目录；可经 LOCATION_DIR 覆盖） ----
_HERE = os.path.dirname(os.path.abspath(__file__))
LOCATION_DIR = os.getenv("LOCATION_DIR", _HERE)
TRAJECTORY_FILE = os.path.join(LOCATION_DIR, "location_trajectory.jsonl")
LATEST_FILE = os.path.join(LOCATION_DIR, "location_latest.json")

# ---- 解析配置 / key 来源 ----
# key 优先读环境变量，其次读本地文件（默认与本脚本同目录的 cell_keys.json）。
# cell_keys.json 示例： {"opencellid": "xxx"}
_KEYS = {}
_CELL_KEY_FILE = os.getenv("CELL_KEY_FILE") or os.path.join(_HERE, "cell_keys.json")
try:
    with open(_CELL_KEY_FILE, encoding="utf-8") as _kf:
        _d = json.load(_kf)
        if isinstance(_d, dict):
            _KEYS = _d
except Exception:
    pass

# OpenCellID 免费 key（可选）：https://opencellid.org 注册获取。全球众包库，但国内覆盖弱，
# 仅作为 cellocation 命中失败后的补充手段。
OPENCELLID_KEY = os.getenv("OPENCELLID_API_KEY") or _KEYS.get("opencellid", "")
# 国内基站库 cellocation（免 key）：http://api.cellocation.com:84/cell/
#   收录 3 亿条，覆盖移动/联通/电信 2/3/4G，返回 WGS84 经纬度 + 精度半径 + 地址。
#   IP 兜底：设备未联网或小区未入库时，用 ipinfo.io 城市级定位（无需 key）。

# 周期性上报间隔（秒）；NOCONN 时也能采集小区号，只是解析不到坐标。
REPORT_INTERVAL = int(os.getenv("LOCATION_INTERVAL", "60"))
# IP 兜底仅用于地图画一个大致城市级圆圈（视觉提示），不向用户展示具体精度数值。
# IP 定位本质指向运营商骨干网出口，非设备精确位置，故 accuracy 记为 None（不体现具体米数）。
IP_FALLBACK_ACCURACY = int(os.getenv("IP_FALLBACK_ACCURACY", "8000"))

# MNC -> 运营商（与 telemetry 一致）
_OPERATORS = {
    "46000": "中国移动", "46002": "中国移动", "46004": "中国移动",
    "46007": "中国移动", "46008": "中国移动",
    "46001": "中国联通", "46006": "中国联通", "46009": "中国联通",
    "46003": "中国电信", "46005": "中国电信", "46011": "中国电信",
}

# 串行保护：避免后台周期任务与手动刷新同时写文件/读串口
_local_lock = threading.Lock()


# =================== 1. 采集 + 解析 ===================
# 清残留 cat 已统一到 at_lock.clear_ghost_cat()，此处不再重复定义。

def collect_cell():
    """用 pyserial 直连读取 4G 模组小区信息，返回结构化字典。

    选择 pyserial 而非 `cat &` 方式：实测后者在并发/孤儿 cat 场景下会把
    模组响应截走，导致读回空串；pyserial 独占式读写在本板（EC200A）已验证可靠。
    失败返回带 error 字段的字典，不会让上层崩溃。
    """
    dev = telemetry.MODEM_DEV
    if not os.path.exists(dev):
        return {"error": f"AT 串口不存在：{dev}"}
    try:
        import serial
    except Exception as e:
        return {"error": f"pyserial 不可用：{e}"}

    clear_ghost_cat()
    try:
        ser = serial.Serial(dev, 115200, timeout=1.5, exclusive=False)
    except Exception as e:
        return {"error": f"打开串口失败 {dev}: {e}"}

    try:
        with at_lock():
            ser.flushInput()
            time.sleep(0.2)

            def _send(cmd, t=1.5):
                ser.write((cmd + "\r").encode())
                time.sleep(0.3)
                buf = b""
                start = time.time()
                while time.time() - start < t:
                    n = ser.in_waiting
                    if n:
                        buf += ser.read(n)
                    else:
                        time.sleep(0.05)
                return buf.decode(errors="replace").replace("\r", "").replace("\0", "")

            _send("AT", 0.5)              # 唤醒
            _send("AT+CEREG=2", 0.5)      # 设 CEREG=2 才能拿到 tac/ci
            cereg_raw = _send("AT+CEREG?")
            qeng_raw = _send('AT+QENG="servingcell"')
    finally:
        try:
            ser.close()
        except Exception:
            pass

    cell = parse_cereg(cereg_raw)
    qeng = parse_qeng(qeng_raw)

    # MCC/MNC 以 QENG 为准（CEREG 不报 MNC）；TAC/CI 以 CEREG 为准。
    mcc = qeng.get("mcc") or cell.get("mcc")
    mnc = qeng.get("mnc") if qeng.get("mnc") is not None else cell.get("mnc")
    tac = cell.get("tac")
    ci = cell.get("ci")

    rec = {
        "mcc": mcc,
        "mnc": mnc,
        "tac": tac,                       # 十进制（喂定位 API 用）
        "ci": ci,
        "tac_hex": cell.get("tac_hex"),
        "ci_hex": cell.get("ci_hex"),
        "rat": qeng.get("rat"),
        "rsrp_dbm": qeng.get("rsrp_dbm"),
        "registered": cell.get("registered", False),
        "operator": _OPERATORS.get(f"{mcc}{mnc:02d}") if (mcc and mnc is not None) else None,
        "raw_cereg": cereg_raw.strip(),
        "raw_qeng": qeng_raw.strip(),
    }
    return rec


def parse_cereg(text):
    """解析 +CEREG 响应，提取 stat / tac(hex) / ci(hex)。

    实测：`+CEREG: 2,1,"1805","102A15",7`
      n=2(我们设的 CEREG=2), stat=1(已注册), tac=1805(hex), ci=102A15(hex)
    """
    m = re.search(
        r'\+CEREG:\s*(\d+)(?:,(\d+))?'
        r'(?:,["\']?([0-9A-Fa-f]+)["\']?)?'
        r'(?:,["\']?([0-9A-Fa-f]+)["\']?)?',
        text or "")
    if not m:
        return {"registered": False}
    stat = m.group(2)
    tac_hex = m.group(3)
    ci_hex = m.group(4)
    out = {
        "registered": stat in ("1", "5"),
        "cereg_stat": stat,
        "tac_hex": tac_hex,
        "ci_hex": ci_hex,
    }
    if tac_hex:
        try:
            out["tac"] = int(tac_hex, 16)
        except ValueError:
            pass
    if ci_hex:
        try:
            out["ci"] = int(ci_hex, 16)
        except ValueError:
            pass
    return out


def parse_qeng(text):
    """解析 +QENG:"servingcell" 响应，提取 MCC/MNC/RAT/RSRP。

    实测 LTE FDD：
      +QENG: "servingcell","NOCONN","LTE","FDD",460,01,102A15,425,1650,3,5,5,1805,-82,-4,-58,15,45
      字段(去引号后)： [NOCONN, LTE, FDD, 460, 01, 102A15, 425, 1650, 3, 5, 5, 1805, -82, -4, -58, 15, 45]
      => MCC=idx3, MNC=idx4, RSRP( LTE 取第12项, 0-indexed idx12 )=-82
    注意：QENG 的 TAC(102A15=1059349) 超过 LAC 2 字节上限，已被证明不可信，
          故这里只用它的 MCC/MNC/信号，TAC/CI 一律以 CEREG 为准。
    """
    m = re.search(r'\+QENG:\s*"servingcell",(.+)', text or "")
    if not m:
        return {}
    fields = [f.strip().strip('"') for f in m.group(1).split(",")]
    out = {}
    if len(fields) < 5:
        return out
    try:
        out["rat"] = fields[1]            # LTE / WCDMA / GSM
        out["mcc"] = int(fields[3]) if fields[3].isdigit() else None
        out["mnc"] = int(fields[4]) if fields[4].lstrip("-").isdigit() else None
    except (ValueError, IndexError):
        pass
    # RSRP：对 LTE，取 "RSRP" 字段。Quectel 在 servingcell 中 RSRP 通常紧跟在
    # PCID 之后约第 12 项（idx12）。用"负 dBm 且在合理范围"做一次启发式校验。
    if out.get("rat") == "LTE" and len(fields) > 12:
        try:
            v = int(fields[12])
            if -140 <= v <= -40:
                out["rsrp_dbm"] = v
        except ValueError:
            pass
    return out


# =================== 2. 联网解析经纬度 ===================
def _http_get_json(url, timeout=8):
    """极简 HTTPS GET，返回 (ok, dict)。证书校验失败时回退到不校验。"""
    import urllib.request
    import ssl
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "backpack-loc/1.0"})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return True, json.loads(r.read().decode("utf-8", "replace"))
    except Exception:
        # 板端证书包可能不全，回退到不校验
        try:
            ctx = ssl.create_default_context()
            ctx.check_hostname = False
            ctx.verify_mode = ssl.CERT_NONE
            req = urllib.request.Request(url, headers={"User-Agent": "backpack-loc/1.0"})
            with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
                return True, json.loads(r.read().decode("utf-8", "replace"))
        except Exception as e:
            return False, {"error": str(e)}


def _http_get_text(url, timeout=8):
    """极简 HTTP GET，返回 (ok, text)。证书校验失败时回退到不校验。"""
    import urllib.request
    import ssl
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "backpack-loc/1.0"})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return True, r.read().decode("utf-8", "replace").replace("\ufeff", "")
    except Exception:
        try:
            ctx = ssl.create_default_context()
            ctx.check_hostname = False
            ctx.verify_mode = ssl.CERT_NONE
            req = urllib.request.Request(url, headers={"User-Agent": "backpack-loc/1.0"})
            with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
                return True, r.read().decode("utf-8", "replace").replace("\ufeff", "")
        except Exception as e:
            return False, str(e)


def _parse_cellocation_csv(text):
    """解析 cellocation 的 CSV 返回：errcode,纬度,经度,精度半径,地址。
    errcode：0=成功，10000=参数错误，10001=无查询结果。返回 None 表示未成功解析。
    """
    lines = [ln.strip() for ln in (text or "").splitlines() if ln.strip()]
    if not lines:
        return None
    # 若有表头（errcode 开头）则取下一行
    first = lines[0]
    if first.lower().startswith("errcode"):
        if len(lines) < 2:
            return None
        first = lines[1]
    # maxsplit=4 让地址里可能包含的逗号不被切断
    parts = [p.strip().strip('"').strip("'") for p in first.split(",", 4)]
    if len(parts) < 4:
        return None
    try:
        errcode = int(parts[0])
    except ValueError:
        return None
    if errcode != 0:
        return None
    try:
        lat = float(parts[1])
        lng = float(parts[2])
        radius = float(parts[3]) if parts[3] else 300
    except ValueError:
        return None
    address = parts[4] if len(parts) > 4 and parts[4] else None
    return {"lat": lat, "lng": lng, "accuracy": radius, "address": address}


def _try_cellocation(cell):
    """国内基站库 cellocation.com（免 key，3 亿条，覆盖移动/联通/电信 2/3/4G）。
    返回 {lat,lng,accuracy,method,address,source} 或 None。
    """
    url = (f"http://api.cellocation.com:84/cell/?mcc={cell['mcc']}"
           f"&mnc={cell['mnc']}&lac={cell['tac']}&ci={cell['ci']}"
           f"&coord=wgs84&output=csv")
    ok, text = _http_get_text(url)
    if not ok or not text:
        return None
    parsed = _parse_cellocation_csv(text)
    if not parsed:
        return None
    return {
        "lat": parsed["lat"],
        "lng": parsed["lng"],
        "accuracy": parsed["accuracy"],
        "method": "cell",
        "address": parsed.get("address"),
        "source": "cellocation",
    }


def resolve_location(cell):
    """把小区号解析成经纬度。返回 {lat,lng,accuracy,method,address?,source?} 或 None(无法解析)。

    优先级：cellocation(国内库, 免 key) -> OpenCellID(全球库, 需 key) -> IP 地理定位(城市级兜底, 无需 key)。
    任一基站库命中即用其坐标（method='cell'，accuracy 为理论半径）；
    IP 兜底（method='ip'）accuracy 记 None：本质指向运营商骨干网出口，不向用户展示具体米数。
    """
    if not cell or cell.get("tac") is None or cell.get("ci") is None:
        return None

    # --- 2.1 国内基站库 cellocation（免 key，覆盖移动/联通/电信 2/3/4G，3 亿条）---
    r = _try_cellocation(cell)
    if r:
        return r

    # --- 2.2 OpenCellID（全球众包库，需免费 key，国内覆盖弱，常 Cell not found）---
    if OPENCELLID_KEY:
        url = (f"https://opencellid.org/cell/get?key={OPENCELLID_KEY}"
               f"&mcc={cell['mcc']}&mnc={cell['mnc']}"
               f"&lac={cell['tac']}&cell={cell['ci']}&format=json")
        ok, data = _http_get_json(url)
        if ok and isinstance(data, dict) and data.get("lat") is not None:
            return {
                "lat": float(data["lat"]),
                "lng": float(data["lon"]),
                "accuracy": data.get("range") or 300,
                "method": "cell",
                "source": "opencellid",
            }

    # --- 2.3 IP 地理定位（城市级兜底，无需任何 key）---
    for url in ("https://ipinfo.io/json", "http://ip-api.com/json"):
        ok, data = _http_get_json(url)
        if not ok or not isinstance(data, dict):
            continue
        if data.get("loc"):
            try:
                lat, lng = (float(x) for x in data["loc"].split(","))
                return {
                    "lat": lat, "lng": lng,
                    "accuracy": None,     # IP 城市级不展示具体米数
                    "method": "ip",
                    "source": "ip",
                }
            except ValueError:
                pass
        if data.get("lat") is not None and data.get("lon") is not None:
            try:
                return {
                    "lat": float(data["lat"]),
                    "lng": float(data["lon"]),
                    "accuracy": None,
                    "method": "ip",
                    "source": "ip",
                }
            except (ValueError, TypeError):
                pass
    return None


# =================== 3. 本地落盘 ===================
def _ensure_dir():
    try:
        os.makedirs(LOCATION_DIR, exist_ok=True)
    except Exception as e:
        # 创建失败也要让用户看到，否则会"静默无文件"
        print(f"[location] 警告：无法创建目录 {LOCATION_DIR}: {e}", flush=True)


def record_local(record):
    """追加轨迹（JSONL）+ 写最新点（latest.json）。"""
    _ensure_dir()
    # 轨迹：每行一条，含时间戳，便于路径回放
    try:
        with open(TRAJECTORY_FILE, "a", encoding="utf-8") as f:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")
    except Exception as e:
        print(f"[location] 写轨迹失败: {e}", flush=True)
    # 最新点
    try:
        with open(LATEST_FILE, "w", encoding="utf-8") as f:
            json.dump(record, f, ensure_ascii=False, indent=2)
    except Exception as e:
        print(f"[location] 写最新点失败: {e}", flush=True)


def update_location():
    """一次完整更新：采集 -> 解析 -> 落盘。返回合并后的记录。"""
    with _local_lock:
        cell = collect_cell()
        if cell.get("error"):
            rec = {"ts_epoch": int(time.time()),
                   "ts": _now_iso(), "error": cell["error"]}
            record_local(rec)
            return rec

        resolved = resolve_location(cell)   # 可能 None（未联网/未入库）
        rec = {
            "ts_epoch": int(time.time()),
            "ts": _now_iso(),
            "mcc": cell.get("mcc"),
            "mnc": cell.get("mnc"),
            "tac": cell.get("tac"),
            "ci": cell.get("ci"),
            "tac_hex": cell.get("tac_hex"),
            "ci_hex": cell.get("ci_hex"),
            "rat": cell.get("rat"),
            "rsrp_dbm": cell.get("rsrp_dbm"),
            "registered": cell.get("registered"),
            "operator": cell.get("operator"),
            "lat": (resolved or {}).get("lat"),
            "lng": (resolved or {}).get("lng"),
            "accuracy": (resolved or {}).get("accuracy"),
            "method": (resolved or {}).get("method"),   # cell / ip / None
            "address": (resolved or {}).get("address"),  # 聚合数据返回的中文地址（可选）
            "raw_cereg": cell.get("raw_cereg"),
            "raw_qeng": cell.get("raw_qeng"),
        }
        record_local(rec)
        return rec


def load_latest():
    try:
        with open(LATEST_FILE, encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return None


def load_trajectory(limit=200):
    """读取最近 limit 条轨迹（按时间升序返回）。"""
    out = []
    try:
        with open(TRAJECTORY_FILE, encoding="utf-8") as f:
            lines = f.readlines()
        for line in lines[-limit:]:
            line = line.strip()
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except Exception:
                continue
    except Exception:
        pass
    return out


def _now_iso():
    try:
        return time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime())
    except Exception:
        return ""


if __name__ == "__main__":
    import pprint
    import traceback

    print(f"[location] LOCATION_DIR = {LOCATION_DIR}", flush=True)
    print(f"[location] MODEM_DEV    = {telemetry.MODEM_DEV}", flush=True)
    print(f"[location] 串口是否存在= {os.path.exists(telemetry.MODEM_DEV)}", flush=True)
    try:
        rec = update_location()
        print("[location] update_location() 返回：", flush=True)
        pprint.pprint(rec)
        print(f"[location] 已写入：", flush=True)
        print(f"  最新点: {LATEST_FILE} -> exists={os.path.exists(LATEST_FILE)}", flush=True)
        print(f"  轨迹  : {TRAJECTORY_FILE} -> exists={os.path.exists(TRAJECTORY_FILE)}", flush=True)
    except Exception:
        print("[location] 运行异常（完整回溯）：", flush=True)
        traceback.print_exc()
        raise
