"""模拟电话（phone 模式）：板端 ↔ 同网段 PC 的双向对讲

```text
任务详情 / 板端 Dashboard 的「电话」  →  设备客户端  →  POST /api/phone {peer, peerPort}
                                                      POST /api/phone/stop {reason}
    → 聆思命令 FIFO 写 `phone <对端IP> <对端端口>` / `phone stop`
      → voice_call 模块（asr.service 里的同一个进程）把麦克风 PCM 打到对端 UDP
        16000Hz / 单声道 / S16_LE / 20ms 一帧（12 字节极简 RTP 头 + 640 字节 PCM）
      → PC 端跑 `pc_tools/voice_call_pc.py --peer <板端IP> --peer-port 5000 --local-port 5000`
```

三条约定（改这块先看）：

1. **必须校验 ip / 端口**：这条命令最终进的是 `telemetry._write_lingsi_cmd` 里的
   `echo "{cmd}" > fifo`（shell），而对端 IP 来自界面输入 —— 不做严格白名单就是
   命令注入。只认 IPv4 点分十进制 + 1~65535 的端口，白名单之外一律 400。
2. **走局域网 UDP，不需要外网**：对端 PC 必须和板子同网段（现场是 `192.168.234.x`
   eth0 / `192.168.41.x` Wi-Fi），所以「跨公网打电话」不成立 —— 平台只负责下发
   起停与展示对端，音频是 PC ⇄ 板子直连。PC 端脚本可以从板子下载：
   `http://<板端IP>:8000/api/lingsi/voice_call_pc.py`。
3. **失败不影响别的**：asr 起不来 / FIFO 写不进去只回 `ok=false + error`，
   呼叫方（客户端 / 页面）只提示一句，绝不抛错。

与 [`speak`] 的关系：FIFO 单通道，进电话模式会打断正在循环的播报，
所以 `start` 先 `speak.stop()` 把播报状态收干净（否则客户端会以为还在喊）。
"""
import re

import speak
import telemetry

# 板端 voice_call 固定监听端口：`phone <IP> <端口>` 里的端口是**对端（PC）**的，
# 板子自己始终在 5000 收 —— 与 pc_tools/voice_call_pc.py 的 --peer-port 对应。
LOCAL_PORT = 5000

_IPV4 = re.compile(r"^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$")


def _log(msg):
    print("[phone] %s" % msg, flush=True)


def normalize_peer(peer, port):
    """校验对端地址，返回 `(ip, port, error)`。

    纯点分十进制 IPv4 + 端口 1~65535；其它一律拒绝（这是 FIFO 的注入护栏）。
    """
    # 不做 strip：这条命令最终是 shell 里的 `echo "phone <ip> <port>"`，
    # 带空白 / 换行的输入一律当非法（用 fullmatch，别让 `$` 放过尾部换行）。
    text = str(peer or "")
    match = _IPV4.fullmatch(text)
    if not match:
        return None, None, "对端 IP 格式不正确（只支持 IPv4 点分十进制、不能有空格，例如 192.168.234.101）"
    octets = [int(part) for part in match.groups()]
    if any(value > 255 for value in octets):
        return None, None, "对端 IP 不合法（每段取值 0~255）"
    if not any(octets):
        return None, None, "对端 IP 不能是 0.0.0.0"
    raw_port = str(port if port is not None else "")
    if not raw_port.isdigit():
        return None, None, "对端端口需要是 1~65535 的整数"
    port_no = int(raw_port)
    if not 1 <= port_no <= 65535:
        return None, None, "对端端口需要是 1~65535 的整数"
    return text, port_no, None


def _state():
    """从状态文件推导当前会话：`(active, ip, port, mode)`。"""
    raw = telemetry.last_lingsi_cmd() or ""
    mode = telemetry.last_lingsi_mode()
    parts = raw.split()
    if raw.startswith("phone ") and len(parts) >= 3:
        ip, port_no, error = normalize_peer(parts[1], parts[2])
        if not error:
            return True, ip, port_no, mode
    return False, None, None, mode


def start(body):
    """发起通话：`phone <对端IP> <对端端口>`。

    `peer` / `peerPort`（也接受 `port`）由调用方给；重复请求同一对端是幂等的
    （板端 voice_call 会自己重建会话，不必先去重）。
    """
    peer = body.get("peer") or body.get("ip")
    port = body.get("peerPort")
    if port is None:
        port = body.get("port")
    ip, port_no, error = normalize_peer(peer, port)
    if error:
        _log("拒绝发起通话：%s（peer=%r port=%r）" % (error, peer, port))
        return {"ok": False, "error": error, "phone": status()}

    # FIFO 单通道：进电话模式前把播报收掉，否则客户端会以为循环播报还在响
    speak.stop("切换到电话模式")

    # 电话模式同样是 asr 进程在读写 FIFO：没起来就拉一把（已在跑只查一次 systemctl）
    if not telemetry.ensure_asr():
        return {
            "ok": False,
            "error": "asr 服务未启动（电话模式要靠它读写 FIFO），本条跳过",
            "phone": status(),
        }

    cmd = "phone %s %d" % (ip, port_no)
    reply = telemetry.send_lingsi_cmd(cmd)
    if not reply.get("ok"):
        _log("发起通话失败（%s）：%s" % (cmd, reply.get("error") or "写入 FIFO 失败"))
        return {"ok": False, "error": "写入聆思命令失败（FIFO 不可用？）", "phone": status()}
    active, saved_ip, saved_port, mode = _state()
    if not active:
        _log("发起通话后状态未切换到 phone（%s mode=%s）" % (cmd, mode))
        return {
            "ok": False,
            "error": "板端没有切换到 phone 模式（asr 是否真的在跑？）",
            "phone": status(),
        }
    _log("通话中：对端 %s:%d（本机监听 %d）" % (saved_ip, saved_port, LOCAL_PORT))
    return {"ok": True, "cmd": cmd, "phone": status()}


def stop(reason="挂断"):
    """挂断：`phone stop` 回到 voice 模式（幂等：不在通话中什么都不写）。"""
    active, ip, port_no, _mode = _state()
    reason = str(reason or "挂断")[:40]
    if not active:
        return {"ok": True, "changed": False, "reason": reason, "phone": status()}
    reply = telemetry.send_lingsi_cmd("phone stop")
    if not reply.get("ok"):
        _log("挂断失败：%s" % (reply.get("error") or "写入 FIFO 失败"))
        return {"ok": False, "error": "写入聆思命令失败（FIFO 不可用？）", "phone": status()}
    _log("已挂断（%s）：对端 %s:%d" % (reason, ip, port_no))
    return {"ok": True, "changed": True, "reason": reason, "peer": ip,
            "peerPort": port_no, "phone": status()}


def status():
    """通话运行态 + 板端 IPv4 网卡（页面据此拼 PC 端命令 / 判断在不在同一网段）。"""
    active, ip, port_no, mode = _state()
    return {
        "active": active,
        "peer": ip,
        "peerPort": port_no,
        "localPort": LOCAL_PORT,
        "mode": mode,
        "interfaces": telemetry.get_interfaces(),
        "script": "/api/lingsi/voice_call_pc.py",
    }
