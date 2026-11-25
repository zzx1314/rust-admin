"""航点 / 航段语音播报（聆思 TTS，板端执行）

巡检路线里的两个「语音播报」动作最终都在这里落地：

```text
路线「航点动作 → 语音播报」→ 客户端到点      POST /api/speak {text, loop, seconds}
                          离开该航点      POST /api/speak/stop
路线「行进任务 → 语音播报」→ 客户端起步      POST /api/speak {text, loop:true}
                            到达下一个航点 / 终态  POST /api/speak/stop
计划详情页「远程呼叫」→ 客户端代调        POST /api/speak/call {text, loop, seconds}
                            人工停止       POST /api/speak/stop
```

- **谁触发**：设备客户端（与 Agent 同机，`127.0.0.1:8000`）。平台/浏览器不直接调：
  播报属于任务执行的一部分，起停时机只有 5009 的状态机知道。
- **手动「呼叫」**（`manual=true`，`/api/speak/call`）：操作员在执行任务详情页
  喊一句话，**不做幂等**（同一句话可以再喊一遍），内容仍走同一份白名单净化；
  循环时长缺省值压到 30s（计划播报仍按 `SPEAK_MAX_SECS` 兜底）。
- **通道**：聆思 asr 服务的命令 FIFO（`config.LINGS_CMD_FIFO`，默认
  `/tmp/voice_assistant_cmd`），与 Dashboard 的「开始播报」按钮同一条：
  `call 0 <内容>` 单次、`call 1 <内容>` 循环、`call stop` 停止并回到 voice 模式。
  TTS 在**云端**合成 → 必须联网，`ensure_asr()` 会在 asr 没起来时拉起它。
- **内容限制**（板端固件）：不能有空格，只认中文汉字 + 中文标点 + 数字，
  中文 ≤ `SPEAK_MAX_CHARS`（默认 30）字、总长 ≤ `SPEAK_MAX_LEN`（默认 40）。
  这份白名单同时是**命令注入的护栏**（FIFO 那条路走 shell `echo "..."`）。
  平台编辑器按同一份规则校验，这里再兜一次（老数据 / 手工调接口不炸）。

三条约定（改这块先看）：

1. **一次只播一条**：板端 FIFO 是单通道，新 `call` 会打断旧播报。所以每次 start
   先显式 `call stop` 再起新的 —— 否则旧播报的循环状态会挂在那里。
2. **一定要收尾**：循环播报靠客户端在「离开航点 / 任务终态」发 stop；客户端掉线
   或被杀时由看门狗（`SPEAK_MAX_SECS`）兜。单次播报说完后主动 `call stop`
   回到 voice 模式 —— 一直留在 call 模式既占网络，也听不到唤醒词。
3. **播报失败绝不影响巡检**：asr 起不来 / 没联网 / FIFO 写不进去都只回
   `ok=false + error`，客户端只打一行日志，任务照跑。
"""
import re
import threading
import time

import config
import telemetry

# 中文汉字 / 中文标点 / 数字：与前端路线编辑器的校验、板端固件的限制一致
_CN = re.compile(r"[\u4e00-\u9fff]")
_PUNCT = "，。！？、；：（）《》〈〉「」『』【】…—·"
_DIGITS = "0123456789"

_LOCK = threading.RLock()

# 当前播报状态（进程内唯一）。`token` 每换一条 / 收一次尾就 +1，
# 定时器醒来先比 token，过期就什么都不做（避免把新播报停掉）。
_STATE = {
    "active": False,
    "key": "",
    "text": "",
    "loop": False,
    "seconds": 0,
    "plan_id": "",
    "point_index": -1,
    "scope": "",
    "started_at": 0.0,
    "reason": "",
    "token": 0,
}


def _log(msg):
    print("[speak] %s" % msg, flush=True)


def _safe_log_text(text):
    """日志里只放可打印的内容（白名单之外的字符不该出现在日志里）。"""
    return str(text or "")[:80]


def normalize(text):
    """净化 + 校验播报内容，返回 `(内容, 错误原因)`。

    - 去掉所有空白（固件不接受空格；`\u3000` 全角空格也算）；
    - 白名单之外的字符直接丢弃（并计数，方便现场对账）；
    - 超长截断到 `SPEAK_MAX_CHARS` / `SPEAK_MAX_LEN`；
    - 净化后为空 → 报错（没有可播的内容，别白写一条 FIFO）。
    """
    raw = str(text or "")
    compact = re.sub(r"\s+", "", raw)
    if not compact:
        return None, "播报内容不能为空"
    kept = []
    dropped = 0
    cn = 0
    for ch in compact:
        is_cn = bool(_CN.match(ch))
        if is_cn or ch in _DIGITS or ch in _PUNCT:
            if is_cn:
                if cn >= config.SPEAK_MAX_CHARS:
                    dropped += 1
                    continue
                cn += 1
            if len(kept) >= config.SPEAK_MAX_LEN:
                dropped += 1
                continue
            kept.append(ch)
        else:
            dropped += 1
    result = "".join(kept)
    if not result:
        return None, "播报内容没有可用字符（只支持中文汉字、中文标点与数字）"
    if dropped:
        _log("内容已净化：丢弃 %d 个不支持的字符 → %s" % (dropped, result))
    return result, None


def _estimate_seconds(text):
    """单次播报多久能说完（云端合成 + 播报）：用于说完后主动回到 voice 模式。

    TTS 大约 4 字/秒，再加 3s 的合成 + 连接开销；宁可多等，不能提前掐断。
    """
    return max(4.0, 0.4 * len(text or "") + 3.0)


def _int(value, default=0, low=0, high=None):
    try:
        number = int(value)
    except (TypeError, ValueError):
        number = default
    if number < low:
        number = low
    if high is not None and number > high:
        number = high
    return number


def _send(cmd):
    """写一条聆思命令（FIFO + 状态文件，Dashboard 的「播报中」读自状态文件）。"""
    try:
        return telemetry.send_lingsi_cmd(cmd)
    except Exception as error:  # pragma: no cover - 极端情况下 FIFO 进程异常
        _log("写聆思命令失败: %s" % error)
        return {"ok": False, "error": str(error)}


def _arm(token, delay, reason):
    """定时器：醒来后如果 token 还是自己那条，就收尾。"""
    timer = threading.Timer(delay, _expire, args=(token, reason))
    timer.daemon = True
    timer.start()
    return timer


def _expire(token, reason):
    with _LOCK:
        if not _STATE["active"] or _STATE["token"] != token:
            return
    _log("定时收尾（%s）" % reason)
    stop(reason)


def start(body):
    """开始一条播报（客户端到点 / 起步时调用）。

    `body`：
    - `text`  播报内容（必填，会被 `normalize` 净化）；
    - `loop`  `true` = 循环播报（`call 1`），`false` = 单次（`call 0`）；
    - `seconds` 循环播报最多播多久（0 = `SPEAK_MAX_SECS`，手动呼叫 0 = 30s）；单次忽略；
    - `planId` / `pointIndex` / `scope`（`point`|`leg`）只用于幂等键与日志；
    - `manual` **手动「呼叫」**（任务详情页喊话）：不做幂等、`scope` 记为 `call`。

    幂等：同一 `(planId, scope, pointIndex)` 的同一条文本重复请求直接返回
    （客户端跟着 5009 每 500ms 一帧，不去重会一直重播）。
    """
    text, error = normalize(body.get("text"))
    if error:
        return {"ok": False, "error": error, "speak": status()}
    loop = bool(body.get("loop"))
    # 手动「呼叫」（任务详情页喊话）与计划播报走同一套实现，区别只在幂等键与默认时长：
    # 操作员连点两次就是要再喊一遍，不能像 5009 那样被去重吞掉。
    manual = bool(body.get("manual"))
    seconds = _int(body.get("seconds"), 0, 0, config.SPEAK_MAX_SECS)
    if loop and seconds <= 0:
        # 手动呼叫默认只喊 30s（点一下别在园区里喊十分钟；要更久就显式传 seconds）
        seconds = min(config.SPEAK_MAX_SECS, 30) if manual else config.SPEAK_MAX_SECS
    plan_id = _safe_log_text(body.get("planId") or body.get("taskCode") or "")
    point_index = -1 if manual else _int(body.get("pointIndex"), -1, -1)
    if manual:
        scope = "call"
        key = "call|%.6f" % time.time()
    else:
        scope = "leg" if str(body.get("scope") or "").strip() == "leg" else "point"
        key = "%s|%s|%d" % (plan_id, scope, point_index)

    with _LOCK:
        if (
            _STATE["active"]
            and _STATE["key"] == key
            and _STATE["text"] == text
            and _STATE["loop"] == loop
        ):
            return {"ok": True, "changed": False, "speak": status()}
        token = _STATE["token"] + 1
        was_active = _STATE["active"]
        # 旧播报作废（它的定时器看到 token 变了会自己退出）
        _STATE["token"] = token
        _STATE["active"] = False

    # 先显式收掉上一条：FIFO 单通道，新 call 会打断旧播报，不 stop 就回不到 voice
    if was_active:
        _send("call stop")

    # TTS 在云端：asr 没起来时先拉起（已在跑则只查一次 systemctl，不 sleep）
    if not telemetry.ensure_asr():
        with _LOCK:
            _STATE.update({"reason": "asr 未启动"})
        return {
            "ok": False,
            "error": "asr 服务未启动（语音播报需要联网 + asr），本条跳过",
            "speak": status(),
        }

    cmd = ("call 1 " if loop else "call 0 ") + text
    reply = _send(cmd)
    ok = bool(reply.get("ok"))
    now = time.time()
    with _LOCK:
        _STATE.update(
            {
                "active": ok,
                "key": key,
                "text": text,
                "loop": loop,
                "seconds": seconds if loop else 0,
                "plan_id": plan_id,
                "point_index": point_index,
                "scope": scope,
                "started_at": now if ok else 0.0,
                "reason": "",
            }
        )
    if not ok:
        _log("播报失败（%s）：%s" % (text, reply.get("error") or "写入 FIFO 失败"))
        return {"ok": False, "error": "写入聆思命令失败（FIFO 不可用？）", "speak": status()}

    if loop:
        _arm(token, seconds, "循环播报到时长 %ds" % seconds)
        _log("循环播报 %s（最多 %ds，%s/%s）" % (text, seconds, plan_id, scope))
    else:
        _arm(token, _estimate_seconds(text), "单次播报结束，回到 voice")
        _log("单次播报 %s（%s/%s）" % (text, plan_id, scope))
    return {"ok": True, "changed": True, "cmd": cmd, "speak": status()}


def stop(reason="计划结束"):
    """停止当前播报并回到 voice 模式（离开航点 / 任务终态 / 取消）。

    幂等：当前没有播报就什么都不写（5009 的收尾帧很多，别每条都写一次 FIFO）。
    """
    reason = _safe_log_text(reason) or "计划结束"
    with _LOCK:
        active = _STATE["active"]
        text = _STATE["text"]
        loop = _STATE["loop"]
        _STATE["token"] += 1
        _STATE.update(
            {
                "active": False,
                "key": "",
                "text": "",
                "loop": False,
                "seconds": 0,
                "plan_id": "",
                "point_index": -1,
                "scope": "",
                "started_at": 0.0,
                "reason": reason,
            }
        )
    if not active:
        return {"ok": True, "changed": False, "reason": reason}
    _send("call stop")
    _log("停止播报（%s）：%s" % (reason, text))
    return {"ok": True, "changed": True, "reason": reason, "text": text, "loop": loop}


def status():
    """播报运行态（Dashboard / 排障用）。"""
    with _LOCK:
        snapshot = dict(_STATE)
    snapshot.pop("token", None)
    # `key`（`planId|scope|pointIndex`）是内部去重用的，接口只给内容与状态
    snapshot.pop("key", None)
    elapsed = 0.0
    if snapshot["active"] and snapshot["started_at"]:
        elapsed = round(time.time() - snapshot["started_at"], 1)
    snapshot["elapsed_secs"] = elapsed
    snapshot["max_secs"] = config.SPEAK_MAX_SECS
    snapshot["max_chars"] = config.SPEAK_MAX_CHARS
    return snapshot
