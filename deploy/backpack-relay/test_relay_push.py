#!/usr/bin/env python3
"""`relay_push.py` 自适应档位的回归测试（纯状态机，不需要 websockets / ffmpeg / 网络）。

    python3 deploy/backpack-relay/test_relay_push.py

现场事故背景（`business/docs/backpack-video-integration.md` §6.10）：
- 只认「send 超时 10s」太晚：卡顿的大多数时间是「还能推，但被上行反压成 400kbps/7fps」，
  这时连接活着、不触发任何超时，用户看到的全是卡；
- 降档生效还要「超时 → 断连 → 重连」，正好错过最卡的那十几秒；
- 到点无条件回配置档，链路其实还不行 → 再降 → 画面反复升降（§6.10⑤ 第二行现象）。
"""

import asyncio
import contextlib
import os
import subprocess
import sys
import time
from types import SimpleNamespace

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import relay_push as relay  # noqa: E402  （必须在 sys.path 之后导入）


def pytest_approx(value: float, tol: float = 0.05):
    """极简近似比较（测试不引 pytest，但仍想要「容差」语义）。"""

    class _Approx:
        def __eq__(self, other):
            return abs(other - value) <= abs(value) * tol

        def __repr__(self):
            return f"≈{value}"

    return _Approx()


class FakeSession:
    """只用来观察「档位变了有没有人去重启拉流」。"""

    def __init__(self, running: bool = True) -> None:
        self.running = running
        self.restarts: list[str] = []

    async def restart_for_profile(self, reason: str) -> None:
        self.restarts.append(reason)


def reset() -> None:
    relay._degrade_level = 0
    relay._requested_tier = None
    relay._auto_degrade = True
    relay._locked_logged_at = 0.0
    relay._last_window_bps = 0.0
    relay._congested_until = 0.0
    relay._flaps = 0
    relay._recovered_at = 0.0
    relay._link_duty = 0.0
    relay._healthy_since = 0.0
    relay._probation_level = -1
    relay._probation_from = 0
    relay._probation_until = 0.0
    relay._ceiling_level = None
    relay._ceiling_until = 0.0
    relay._idle_logged_at = 0.0
    relay._last_egress_iface = None
    relay._sessions.clear()


def fresh(name: str) -> relay.StreamSession:
    """一个没起过 ffmpeg 的 session（只用来喂窗口统计）。"""
    session = relay.StreamSession(name)
    session._win_at = time.monotonic()
    session._win_blocked = 0.0
    session._win_bytes = 0
    return session


def window(session, seconds: float, kbps: float, duty: float, now: float) -> bool:
    """构造一个「已经跑满 seconds 秒」的窗口再判定一次。"""
    session._win_at = now - seconds
    session._win_bytes = int(kbps * 1000 * seconds / 8)
    session._win_blocked = seconds * duty
    return session._sample_congestion(now)


def test_idle_profile_is_the_configured_one() -> None:
    reset()
    assert not relay.degraded()
    assert relay.active_profile() == (
        relay.OUT_WIDTH,
        relay.OUT_HEIGHT,
        relay.OUT_FPS,
        relay.OUT_BITRATE,
    )
    assert relay.profile_text(relay.active_profile()) == "1280x720@20 1200k"
    assert relay.profile_bps(relay.active_profile()) == 1_200_000


async def degrade_in_loop(stream: str = "dogcam", reason: str = "上行吃不下当前档位") -> None:
    """`mark_congested` 会在事件循环里排重启任务，所以必须在循环内调用再让出一次。"""
    relay.mark_congested(stream, reason)
    await asyncio.sleep(0)
    await asyncio.sleep(0)


def test_congested_switches_to_low_profile_and_restreams() -> None:
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only

    asyncio.run(degrade_in_loop())

    assert relay.degraded()
    assert relay.active_profile() == (
        relay.DEGRADE_WIDTH,
        relay.DEGRADE_HEIGHT,
        relay.DEGRADE_FPS,
        relay.DEGRADE_BITRATE,
    )
    # 立刻换档，不用等断线重连
    assert only.restarts == ["降档到省流档"]
    # 首次降档按配置的窗口，不翻倍
    assert relay.hold_seconds() == relay.DEGRADE_TTL


def test_degraded_profile_actually_transcodes() -> None:
    """降档必须真的重编码（2026-11-18 线上事故）。

    当时的错：`encoder = None if (profile[0] == "copy" or low) else pick_encoder()`
    —— 降档时 `low` 为真，于是 encoder 被置成 None → `-c:v copy` → 1080p60 原样推
    4~8Mbps，浏览器全部 `lagged`，也就是「越降越卡」。`-c:v copy` 没有码率可降。
    """
    reset()
    original = relay.pick_encoder
    relay.pick_encoder = lambda: "h264_rkmpp"
    try:
        relay.mark_congested("dogcam", "单测触发")
        args = relay.ffmpeg_args("dogcam", "rtsp://127.0.0.1:9999/bp_dogcam")
    finally:
        relay.pick_encoder = original

    # 注意：`-c:v` 可能出现两次（输入硬件解码 + 输出编码），输出编码看**最后一个**
    codecs = [args[i + 1] for i, arg in enumerate(args) if arg == "-c:v"]
    assert "copy" not in args, f"降档后仍是 -c:v copy：{' '.join(args)}"
    assert codecs[-1] == "h264_rkmpp"
    assert args[args.index("-b:v") + 1] == relay.DEGRADE_BITRATE
    filters = args[args.index("-vf") + 1]
    assert f"scale={relay.DEGRADE_WIDTH}:{relay.DEGRADE_HEIGHT}" in filters
    assert f"fps={relay.DEGRADE_FPS}" in filters
    # mux 缓冲必须归零：默认 0.7s/0.5s 是白送的延迟
    assert args[args.index("-muxdelay") + 1] == "0"
    assert args[args.index("-muxpreload") + 1] == "0"


def test_second_degrade_tier_for_4g_links() -> None:
    """链路比省流档还小时（4G ~250kbps < 500k 目标）再降一档：320x180@8/200k。

    现场 2026-11-18 15:54：出口掉到 4G dongle，实测 254kbps 而省流档目标 500k —
    链路比目标还小一半，send 永远 100% 占用 → 延迟涨到 5.9s → 每 40 秒被掐一次。
    """
    reset()
    session = FakeSession()
    relay._sessions["dogcam"] = session
    asyncio.run(degrade_in_loop())                      # 配置档 → 省流档
    assert relay._degrade_level == 1

    # 省流档也吃不下（实测只有 250kbps）→ 自动再降一档
    relay._last_window_bps = 250_000
    asyncio.run(degrade_in_loop(reason="省流档也吃不下"))
    assert relay._degrade_level == 2, "链路比 500k 还小时应该降到极省流档"
    assert relay.active_profile() == (
        relay.DEGRADE2_WIDTH,
        relay.DEGRADE2_HEIGHT,
        relay.DEGRADE2_FPS,
        relay.DEGRADE2_BITRATE,
    )
    assert session.restarts == ["降档到省流档", "降档到极省流档"]

    original = relay.pick_encoder
    relay.pick_encoder = lambda: "h264_rkmpp"
    try:
        args = relay.ffmpeg_args("dogcam", "rtsp://127.0.0.1:9999/bp_dogcam")
    finally:
        relay.pick_encoder = original
    filters = args[args.index("-vf") + 1]
    assert "scale=320:180" in filters and "fps=8" in filters
    assert args[args.index("-b:v") + 1] == relay.DEGRADE2_BITRATE


def test_extreme_tier_is_not_escalated_further() -> None:
    """已经在极省流档就只续窗口，不再「降档」（也不会反复重启 ffmpeg）。"""
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    asyncio.run(degrade_in_loop())
    relay._last_window_bps = 250_000
    asyncio.run(degrade_in_loop())
    only.restarts.clear()

    relay._last_window_bps = 150_000   # 依然吃不下
    asyncio.run(degrade_in_loop(reason="还是不行"))
    assert relay._degrade_level == 2
    assert only.restarts == [], "已经是最低档，不该再重启 ffmpeg"


def test_lag_is_measured_as_growth_after_the_first_frame() -> None:
    """只量「首帧之后的积压增长」：起流等关键帧的耗时不能算进去（否则一起流就误判）。"""
    reset()
    session = relay.StreamSession("dogcam")
    session._started_wall = time.time() - 6.0   # 起流已经 6s（含等关键帧）
    session._rtp.last_video_ts = 2000 * 90
    original, relay.MAX_LAG = relay.MAX_LAG, 999  # 先别触发重同步
    try:
        session._update_lag(time.monotonic())         # 首帧 → 只建基准
        assert session.lag == 0.0
        session._rtp.last_video_ts = 3000 * 90          # 时间戳只走了 1s
        session._update_lag(time.monotonic() + 2.0)   # 但过了 2s → 积压 1s
    finally:
        relay.MAX_LAG = original
    assert 0.9 <= session.lag <= 1.1, session.lag


def test_lag_above_limit_requests_resync() -> None:
    reset()
    session = relay.StreamSession("dogcam")
    session._started_wall = time.time() - 40.0
    session._rtp.last_video_ts = 1000 * 90
    session._update_lag(time.monotonic())        # 首帧建基准
    session._rtp.last_video_ts += 1000 * 90       # 时间戳只走 1s

    async def scenario() -> None:
        session._update_lag(time.monotonic() + 6.0)   # 实际过了 6s → 积压 5s
        await asyncio.sleep(0)
        await asyncio.sleep(0)

    asyncio.run(scenario())
    assert session._lag_restart_at > 0, "超过 RELAY_MAX_LAG 就应该请求重同步"


def test_copy_profile_still_copies() -> None:
    """`RELAY_PROFILE=copy` 是显式要求只换容器，不能被「顺手转码」改掉。"""
    reset()
    original = relay.PROFILE
    relay.PROFILE = "copy"
    try:
        args = relay.ffmpeg_args("dogcam", "rtsp://127.0.0.1:9999/bp_dogcam")
    finally:
        relay.PROFILE = original

    codecs = [args[i + 1] for i, arg in enumerate(args) if arg == "-c:v"]
    assert codecs[-1] == "copy", f"-c:v 链：{codecs}"
    assert "-vf" not in args


def test_window_stats_decide_congestion() -> None:
    reset()
    session = fresh("dogcam")
    now = time.monotonic()

    # 链路恰好跑满但码率达标（1200k 都推出去了）：duty 再高也不算吃不下
    assert not window(session, relay.CONGEST_WINDOW, 1200, 1.0, now)
    # 被反压：send 全程阻塞，实测只有 400kbps
    assert window(session, relay.CONGEST_WINDOW, 400, 0.95, now)
    assert relay._last_window_bps == pytest_approx(400_000), relay._last_window_bps
    # 空闲：send 很快、码率也达标
    assert not window(session, relay.CONGEST_WINDOW, 1200, 0.05, now)
    # 窗口还没攒够不下结论
    assert not window(session, relay.CONGEST_WINDOW / 2, 100, 1.0, now)


def test_still_congested_after_ttl_keeps_the_low_profile() -> None:
    reset()
    relay._sessions["dogcam"] = FakeSession()
    asyncio.run(degrade_in_loop())
    # 窗口到期，但 send 占用还是满的（4G 只够 500k）
    relay._congested_until = time.monotonic() - 1
    relay._link_duty = 0.9
    relay._healthy_since = 0.0

    asyncio.run(relay.profile_check())

    assert relay.degraded(), "链路没恢复就不该回配置档"
    assert relay._congested_until > time.monotonic()


def test_recovered_link_returns_to_configured_profile() -> None:
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    asyncio.run(degrade_in_loop())
    only.restarts.clear()

    # 窗口到期 + send 占用持续低于 RECOVER_DUTY 足够久 = 链路真的好了
    relay._congested_until = time.monotonic() - 1
    relay._link_duty = 0.1
    relay._healthy_since = time.monotonic() - relay.RECOVER_STABLE - 1
    relay._last_window_bps = relay.profile_bps(relay.active_profile())

    asyncio.run(relay.profile_check())
    asyncio.run(asyncio.sleep(0))

    assert not relay.degraded()
    assert relay.active_profile()[0] == relay.OUT_WIDTH
    assert only.restarts == ["升一档到配置档"]
    # 升上去之后要进试用期：这一档能不能稳住还没证明
    assert relay.probation_active()


def test_step_up_is_one_level_at_a_time() -> None:
    """极省流 → 省流 → 配置档，一次只升一档（2026-11-18 直接跳回配置档的根因）。"""
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    asyncio.run(degrade_in_loop())                     # 1
    relay._last_window_bps = 250_000                  # 省流档也吃不下
    asyncio.run(degrade_in_loop())
    assert relay._degrade_level == 2
    only.restarts.clear()

    relay._congested_until = time.monotonic() - 1
    relay._link_duty = 0.05
    relay._healthy_since = time.monotonic() - relay.RECOVER_STABLE - 1
    relay._last_window_bps = relay.profile_bps(relay.active_profile())
    asyncio.run(relay.profile_check())

    assert relay._degrade_level == 1, "不能从极省流直接跳回配置档"
    assert only.restarts == ["升一档到省流档"]


def test_probation_relapse_returns_one_level_and_pins_it() -> None:
    """升档后积压往上涨 = 这一档撑不住：退回上一档，并记住别马上再试。"""
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    asyncio.run(degrade_in_loop())                     # 配置档 → 省流档
    only.restarts.clear()

    relay._congested_until = time.monotonic() - 1
    relay._link_duty = 0.1
    relay._healthy_since = time.monotonic() - relay.RECOVER_STABLE - 1
    relay._last_window_bps = relay.profile_bps(relay.active_profile())
    asyncio.run(relay.profile_check())                 # 升到配置档，进试用期
    assert relay._degrade_level == 0 and relay.probation_active()
    only.restarts.clear()

    session = relay.StreamSession("dogcam")
    session._started_wall = time.time() - 60.0
    session._rtp.last_video_ts = 0
    session._update_lag(time.monotonic())              # 首帧建基准
    session._rtp.last_video_ts = 1000 * 90            # 时间戳只走 1s

    async def growth() -> None:
        session._update_lag(time.monotonic() + 2.0)    # 实际过了 2s → 积压 1s
        await asyncio.sleep(0)
        await asyncio.sleep(0)

    asyncio.run(growth())

    assert relay._degrade_level == 1, "试用期内积压超限就应该退回上一档"
    assert not relay.probation_active()
    assert relay._ceiling_level == 0, "配置档撑不住要记下来"
    assert relay._ceiling_until > time.monotonic()
    assert only.restarts == ["退回省流档"]

    # 窗口到期后不马上去碰已知撑不住的档位
    relay._congested_until = time.monotonic() - 1
    relay._healthy_since = time.monotonic() - relay.RECOVER_STABLE - 1
    relay._last_window_bps = relay.profile_bps(relay.active_profile())
    only.restarts.clear()
    asyncio.run(relay.profile_check())
    assert relay._degrade_level == 1, "ceiling 内的档位不该再试"
    assert only.restarts == []


def test_lag_growth_degrades_before_the_hard_restart() -> None:
    """积压持续上涨就先降档，不等 RELAY_MAX_LAG 的硬重启（现场那 5 分钟）。"""
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    session = relay.StreamSession("dogcam")
    session._started_wall = time.time()
    base = time.monotonic()
    session._rtp.last_video_ts = 0
    session._update_lag(base)                       # 首帧建基准

    def feed(elapsed: float, lag: float = 1.5) -> None:
        """让墙钟跑 elapsed 秒、时间戳只跑 (elapsed - lag) 秒 → 积压始终是 lag。"""
        session._rtp.last_video_ts = int((elapsed - lag) * 90_000)
        session._update_lag(base + elapsed)

    hold = relay.LAG_DEGRADE_HOLD

    async def growth() -> None:
        feed(2.5)                                   # 积压 1.5s：先记下起点（< HOLD）
        await asyncio.sleep(0)
        assert relay._degrade_level == 0
        feed(2.5 + hold + 2)                         # 持续超限 → 降档
        await asyncio.sleep(0)
        await asyncio.sleep(0)

    asyncio.run(growth())
    assert session.lag < relay.MAX_LAG, "还没到硬重同步的阈值就该先降档"
    assert relay._degrade_level == 1, "积压持续上涨就该降档，而不是等着被硬重启"
    assert only.restarts == ["降档到省流档"]
    assert session._lag_restart_at == 0.0, "不该走到 MAX_LAG 的 ffmpeg 重启"

    # 省流档也追不上：码率看着正常（`_last_window_bps` 达标）也要再降一档 ——
    # rkmpp 的 CBR 就是靠丢帧保码率的，光看码率会把「画面一路掉队」当成正常
    relay._last_window_bps = relay.profile_bps(relay.active_profile())
    later = 2.5 + hold + 2 + relay.LAG_DEGRADE_DEBOUNCE + 2

    async def growth_again() -> None:
        feed(later)
        await asyncio.sleep(0)
        await asyncio.sleep(0)

    asyncio.run(growth_again())
    assert relay._degrade_level == 2
    assert only.restarts == ["降档到省流档", "降档到极省流档"]


def test_idle_keeps_the_level_instead_of_restoring() -> None:
    """没人在看：不升档（send 占用 0% 什么都没证明），保持当前档位。"""
    reset()
    asyncio.run(degrade_in_loop())
    relay._congested_until = time.monotonic() - 1
    relay._healthy_since = 0.0  # 没人看，没有窗口统计

    asyncio.run(relay.profile_check())

    assert relay.degraded(), "没人在看时不该盲回配置档（那正是越看越卡的起点）"
    assert relay._degrade_level == 1
    assert not relay.probation_active(), "没人看就别开试用期"


def test_probation_success_waits_for_the_next_window() -> None:
    """试用期通过后不立刻再升一档：升档节奏还是按窗口走（2→1…→0 不能一次跳完）。"""
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    relay._degrade_level = 2
    relay._congested_until = time.monotonic() - 1
    relay._link_duty = 0.05
    relay._healthy_since = time.monotonic() - relay.RECOVER_STABLE - 1
    asyncio.run(relay.profile_check())
    assert relay._degrade_level == 1 and relay.probation_active()

    # 试用期平静结束
    relay._probation_until = time.monotonic() - 1
    relay._healthy_since = time.monotonic() - relay.RECOVER_STABLE - 1
    asyncio.run(relay.profile_check())

    assert relay._degrade_level == 1, "刚试完一档不该立刻再升一档"
    assert relay._congested_until > time.monotonic(), "下一次升档要等一个窗口"


def test_ceiling_blocks_only_the_level_that_could_not_hold() -> None:
    """封顶只挡「试过撑不住的那一档及以上」，更低的一档照样可以试。"""
    reset()
    relay._ceiling_level = 0
    relay._ceiling_until = time.monotonic() + 300
    assert not relay.step_up_allowed(0), "配置档已知撑不住，别再去撞"
    assert relay.step_up_allowed(1), "省流档没被否定，还能试"

    relay._ceiling_until = time.monotonic() - 1
    assert relay.step_up_allowed(0), "封顶到期了应该允许再试一次"


def test_flapping_doubles_the_hold_window() -> None:
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    asyncio.run(degrade_in_loop())
    assert relay.hold_seconds() == relay.DEGRADE_TTL

    relay._congested_until = time.monotonic() - 1
    relay._link_duty = 0.1
    relay._healthy_since = time.monotonic() - relay.RECOVER_STABLE - 1
    asyncio.run(relay.profile_check())  # 升一档 → 配置档
    assert not relay.degraded()

    # 刚回档又拥塞 = 链路其实没恢复：窗口翻倍，避免来回升降把画面折腾碎
    asyncio.run(degrade_in_loop(reason="又吃不下"))
    assert relay.degraded()
    assert relay.hold_seconds() == relay.DEGRADE_TTL * 2
    # 同时记住「配置档撑不住」，到期前不再自己升回去
    assert relay._ceiling_level == 0
    assert relay._ceiling_until > time.monotonic()

    # 一直翻倍到上限
    relay._flaps = 9
    assert relay.hold_seconds() == relay.DEGRADE_TTL_MAX


def test_hard_timeout_extends_without_restreaming() -> None:
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    first = relay.StreamSession("dogcam")

    async def hard_timeout() -> None:
        relay.mark_congested("dogcam", "上行阻塞超过 10s", restream=False)
        await asyncio.sleep(0)

    asyncio.run(hard_timeout())
    assert only.restarts == [], "链路已经死了，别再重启别的流去撞南墙"
    assert relay.degraded()

    # 省流档里继续超时：只续窗口，不重复降档
    hold = relay.hold_seconds()
    relay._congested_until = time.monotonic() + hold
    asyncio.run(hard_timeout())
    assert relay._congested_until >= time.monotonic() + hold - 1
    assert first.profile is None  # 附带确认：没起过的 session 不会被误判为「已在目标档位」


def test_restream_skips_idle_sessions() -> None:
    reset()
    running = FakeSession(running=True)
    idle = FakeSession(running=False)
    relay._sessions["dogcam"] = running
    relay._sessions["cam0"] = idle

    async def restream() -> None:
        relay.restream_all("降档")
        await asyncio.sleep(0)
        await asyncio.sleep(0)

    asyncio.run(restream())

    assert running.restarts == ["降档"]
    assert idle.restarts == [], "没在推的流不该被拉起来"


def test_platform_default_tier_sets_the_starting_point() -> None:
    """页面选「省流档」：起流就是 640x360@12/500k，而不是先推 720p 再降。"""
    reset()
    assert relay.apply_remote_profile({"tier": "degrade", "auto": True})
    assert relay.base_level() == 1
    assert relay.active_profile() == (
        relay.DEGRADE_WIDTH,
        relay.DEGRADE_HEIGHT,
        relay.DEGRADE_FPS,
        relay.DEGRADE_BITRATE,
    )
    # 起点本身就是省流档，不算「自动降过档」
    assert not relay.degraded()

    # 链路比省流档还小：还能再降一档到极省流
    relay._last_window_bps = 250_000
    asyncio.run(degrade_in_loop(reason="省流档也吃不下"))
    assert relay._degrade_level == 2
    assert relay.active_profile()[0] == relay.DEGRADE2_WIDTH

    # 已经到了最低档：再拥塞也不会继续降（只会续窗口）
    assert relay.active_profile()[0] == relay.DEGRADE2_WIDTH
    asyncio.run(degrade_in_loop(reason="还是不行"))
    assert relay._degrade_level == 2


def test_platform_low_tier_is_already_the_floor() -> None:
    """页面选「极省流档」：没有更低的档可以降，也不反复重启 ffmpeg。"""
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    assert relay.apply_remote_profile({"tier": "degrade2", "auto": True})
    assert relay.active_profile()[0] == relay.DEGRADE2_WIDTH

    asyncio.run(degrade_in_loop(reason="还是不行"))
    assert relay._degrade_level == 2
    assert only.restarts == [], "已经在最低档，不该再降档/重启"


def test_platform_can_lock_the_tier() -> None:
    """页面关掉「自动降档」：链路再差也保持平台选的档位（只留 send 超时保护）。"""
    reset()
    only = FakeSession()
    relay._sessions["dogcam"] = only
    assert relay.apply_remote_profile({"tier": "degrade", "auto": False})

    relay._last_window_bps = 200_000
    asyncio.run(degrade_in_loop(reason="上行吃不下当前档位"))

    assert relay._degrade_level == 1, "锁定后不该再降档"
    assert only.restarts == []
    assert not relay.degraded()


def test_platform_tier_change_resets_old_degrade_state() -> None:
    """页面把档位从省流档改成高清：旧档位的拥塞窗口/封顶作废，按新起点重来。"""
    reset()
    assert relay.apply_remote_profile({"tier": "degrade", "auto": True})
    relay._congested_until = time.monotonic() + 300
    relay._ceiling_level = 0
    relay._ceiling_until = time.monotonic() + 600

    assert relay.apply_remote_profile({"tier": "light", "auto": True})
    assert relay.base_level() == 0
    assert relay.active_profile()[0] == relay.OUT_WIDTH
    assert relay._congested_until == 0
    assert relay._ceiling_level is None
    # 同一份配置重复下发不算变化（避免每次 video_start 都重启 ffmpeg）
    assert not relay.apply_remote_profile({"tier": "light", "auto": True})
    # 老平台不带 profile：不改任何东西，继续按已下发的档位
    assert not relay.apply_remote_profile(None)
    assert relay.base_level() == 0


def test_legacy_platform_without_profile_keeps_env_default() -> None:
    reset()
    assert relay.base_level() == 0
    assert relay.active_profile() == (
        relay.OUT_WIDTH,
        relay.OUT_HEIGHT,
        relay.OUT_FPS,
        relay.OUT_BITRATE,
    )


@contextlib.contextmanager
def patched(**attrs):
    """临时把 relay 模块上的属性（subprocess / _decoder_works 等）换掉。"""
    old = {name: getattr(relay, name) for name in attrs}
    for name, value in attrs.items():
        setattr(relay, name, value)
    try:
        yield
    finally:
        for name, value in old.items():
            setattr(relay, name, value)


def test_decoder_pick_is_per_stream() -> None:
    """一条流探测通过，不代表另一条能用（cam0~3 硬解一帧都解不出来）。

    2026-11-18 事故：`_decoder_pick` 是全局缓存，dogcam 探测通过后 cam* 也拿到
    `h264_rkmpp` → 推 0 字节、每 15s 被卡住看门狗重启，USB 相机通道永远没画面。
    """

    class Listed:
        stdout = " V....D h264_rkmpp   Rockchip MPP (Media Process Platform)"

    with patched(
        _decoder_works=lambda name, stream: stream == "dogcam",
        subprocess=SimpleNamespace(run=lambda *a, **k: Listed()),
    ):
        relay._decoder_picks.clear()
        try:
            assert relay.probe_decoder("dogcam") == "h264_rkmpp"
            assert relay.probe_decoder("cam0") is None, "cam0 不能继承 dogcam 的硬解结论"
            assert relay.pick_decoder("dogcam") == "h264_rkmpp"
            assert relay.pick_decoder("cam0") is None
            # 没探测过的流先用软解（探测不在起流路径上同步跑）
            assert relay.pick_decoder("cam1") is None
        finally:
            relay._decoder_picks.clear()


def test_decoder_probe_requires_real_frames() -> None:
    """只看 `returncode == 0` 不够：一帧没解出来时 ffmpeg 也可能返回 0。"""

    class ZeroFrames:
        returncode = 0
        stdout = b""

    class TwoFrames:
        returncode = 0
        stdout = b"\x00" * 4096

    try:
        with patched(subprocess=SimpleNamespace(run=lambda *a, **k: ZeroFrames())):
            assert not relay._decoder_works("h264_rkmpp", "cam0")
        with patched(subprocess=SimpleNamespace(run=lambda *a, **k: TwoFrames())):
            assert relay._decoder_works("h264_rkmpp", "dogcam")

        def hang(*args, **kwargs):
            raise subprocess.TimeoutExpired("ffmpeg", relay.DECODER_PROBE_TIMEOUT)

        with patched(subprocess=SimpleNamespace(run=hang)):
            assert not relay._decoder_works("h264_rkmpp", "cam0"), "解不出来会挂住，超时算不可用"
    finally:
        relay._decoder_picks.clear()


def test_slow_egress_jumps_straight_to_the_lowest_tier() -> None:
    """出口切到 4G dongle：直接压到极省流档，不等窗口、不一次只降一档。

    现场（2026-11-18）：Wi-Fi 一掉，内核把流量回落到 USB 4G（上行 0.25~0.6Mbps），
    而原来要 5s 超时 + 5s 窗口 + 两轮降档才能到底 —— 这十几秒里现场只能看着卡。
    """
    reset()
    relay.apply_remote_profile({"tier": "light", "auto": True})  # 起点配置档 720p15/1000k
    assert relay._degrade_level == 0

    assert relay.is_slow_egress("enx020c29a39b6d"), "USB 4G dongle 应算慢速出口"
    assert not relay.is_slow_egress("wlp4s0")
    assert not relay.is_slow_egress("eth0")
    assert not relay.is_slow_egress(None)

    assert relay.force_low_profile("enx020c29a39b6d"), "应该真的换了档"
    assert relay._degrade_level == 2
    assert relay.active_profile() == (
        relay.DEGRADE2_WIDTH,
        relay.DEGRADE2_HEIGHT,
        relay.DEGRADE2_FPS,
        relay.DEGRADE2_BITRATE,
    )
    # 慢链路不是「这一档撑不住」：不能把上面几档封顶，Wi-Fi 回来还要升
    assert relay._ceiling_level is None
    # 重复调用不重复降档（只续窗口）
    assert not relay.force_low_profile("enx020c29a39b6d")


def test_locked_profile_is_not_forced_to_the_lowest_tier() -> None:
    """平台把自动降档关掉时，出口是 4G 也不擅自换档（只靠 send 超时断线重连）。"""
    reset()
    relay.apply_remote_profile({"tier": "light", "auto": False})
    assert not relay.force_low_profile("enx020c29a39b6d")
    assert relay._degrade_level == 0


def test_fast_egress_shrinks_the_degrade_window() -> None:
    """出口回到 Wi-Fi：把 300s 的窗口压到 60s，好快点按试用期升回去。"""
    reset()
    relay.apply_remote_profile({"tier": "light", "auto": True})
    relay.force_low_profile("enx020c29a39b6d")
    assert relay._congested_until > time.monotonic() + 100

    relay.allow_fast_egress_recovery()
    remaining = relay._congested_until - time.monotonic()
    assert 0 < remaining <= relay.SLOW_IFACE_RECOVER + 1, f"窗口应缩到 {relay.SLOW_IFACE_RECOVER}s"


def test_send_timeout_jumps_to_the_lowest_tier() -> None:
    """`send` 硬阻塞（上行已死）：一次到位降到底，重连后直接用极省流档起流。"""
    reset()
    relay.apply_remote_profile({"tier": "light", "auto": True})
    relay.mark_congested("dogcam", "上行阻塞超过 5s", restream=False, lowest=True)
    assert relay._degrade_level == 2, "硬阻塞应从配置档直接降到极省流档"


def test_congestion_window_still_steps_one_level() -> None:
    """软拥塞（还能推、只是码率不够）仍是一档一档降：不要一惊一咋就掉到 320x180。"""
    reset()
    relay.apply_remote_profile({"tier": "light", "auto": True})
    relay._last_window_bps = 300_000  # 远低于 1000k 目标 → 这一档吃不下
    relay.mark_congested("dogcam", "5s 内实测 400kbps", restream=False)
    assert relay._degrade_level == 1


def test_platform_tier_does_not_override_a_slow_egress() -> None:
    """钉死 4G（`PIN_IFACE=none`）时，平台下发的起点档位也要让位给慢链路。

    现场（2026-11-18）：页面默认档位是省流档 500k，`video_start` 一到就把 `route_watcher`
    压好的极省流档覆盖回 500k，而 4G 上行只有 0.25~0.6Mbps → 每次开页面先卡 5~10 秒，
    等拥塞窗口（`CONGEST_WINDOW`）再降一次档。
    """
    reset()
    try:
        # 快链路（Wi-Fi）：平台选什么就是什么
        relay._last_egress_iface = "wlp4s0"
        assert relay.apply_remote_profile({"tier": "degrade", "auto": True})
        assert relay._degrade_level == 1, "Wi-Fi 上应该按平台选的省流档起流"

        # 慢链路（4G dongle）：起点直接拉到极省流档，不等窗口
        reset()
        relay._last_egress_iface = "enx020c29a39b6d"
        assert relay.apply_remote_profile({"tier": "degrade", "auto": True})
        assert relay._degrade_level == 2, "4G 上不能按 500k 起流，直接极省流档"
        assert relay.active_profile() == (
            relay.DEGRADE2_WIDTH,
            relay.DEGRADE2_HEIGHT,
            relay.DEGRADE2_FPS,
            relay.DEGRADE2_BITRATE,
        )
        assert relay._ceiling_level is None, "这不是「这一档撑不住」，Wi-Fi 回来还要升"

        # 平台锁死档位（自动降档关）：用户明确要求锁档，不擅自改
        reset()
        relay._last_egress_iface = "enx020c29a39b6d"
        assert relay.apply_remote_profile({"tier": "degrade", "auto": False})
        assert relay._degrade_level == 1, "锁档时不该被强压到极省流档"
    finally:
        relay._last_egress_iface = None


def test_fixed_decoder_env_skips_probing() -> None:
    """`RELAY_DECODER=sw` / 写死 `h264_rkmpp` 时不探测，照办。"""
    real = relay.DECODER
    try:
        relay.DECODER = "sw"
        relay._decoder_picks.clear()
        assert relay.pick_decoder("cam0") is None
        relay.DECODER = "h264_rkmpp"
        assert relay.pick_decoder("cam0") == "h264_rkmpp"
        assert "cam0" not in relay._decoder_picks, "写死时不该写缓存"
    finally:
        relay.DECODER = real
        relay._decoder_picks.clear()


def test_rtsp_scanner_reads_interleaved_timestamps() -> None:
    """RTSP/TCP 通路靠 RTP 时间戳量延迟：交错帧头 + RTP 头，90kHz。

    现场等价物：`-use_wallclock_as_timestamps 1` 把采集时刻写成 PTS，RTP 时间戳
    跟着 PTS 走 —— 所以「时间戳增量 vs 墙钟增量」就是帧积压。
    分片边界会切断 `$` 头 / RTP 头，夹在中间的 RTSP 文本（keepalive 请求）也得跳过。
    """

    def interleaved(ts: int, channel: int = 0, payload: bytes = b"\x00" * 8) -> bytes:
        rtp = bytes([0x80, 0x60, 0x00, 0x01]) + ts.to_bytes(4, "big") + payload
        return b"$" + bytes([channel]) + len(rtp).to_bytes(2, "big") + rtp

    scanner = relay.RtpScanner()
    assert scanner.clock_rate == relay.RtpScanner.clock_rate == 90_000
    # 切成两半喂进去（模拟 8KB 分片切断头）
    blob = interleaved(90_000) + b"OPTIONS rtsp://x RTSP/1.0\r\n\r\n" + interleaved(180_000)
    scanner.feed(blob[:7])
    assert scanner.last_video_ts is None
    scanner.feed(blob[7:])
    assert scanner.last_video_ts == 180_000, scanner.last_video_ts
    # 奇通道是 RTCP（没有时间戳字段），不能污染指标
    scanner.feed(interleaved(42, channel=1))
    assert scanner.last_video_ts == 180_000


def test_media_path_is_rtsp_only() -> None:
    """媒体只有一条路：ffmpeg 输出 RTSP/TCP 到本机中转端口。

    2026-11-19 之后没有 `RELAY_OUTPUT`、没有 `-f flv`、也没有「rtsp 失败退回 ws」：
    起不来的处理是断开重连（`stream_worker`），平台还有人在看就再补一次 video_start。
    """
    reset()
    assert not hasattr(relay, "OUTPUT"), "RELAY_OUTPUT 开关应已删除"
    assert not hasattr(relay, "output_mode"), "退让状态机应已删除"
    assert not hasattr(relay, "FlvScanner"), "FLV 扫描器应已删除"

    args = relay.ffmpeg_args("dogcam", "rtsp://127.0.0.1:9999/bp_dogcam")
    assert "flv" not in args, f"不该再出现 FLV 输出参数：{' '.join(args)}"
    assert args[args.index("-f") + 1] == "rtsp"
    assert args[args.index("-rtsp_transport") + 1] == "tcp"
    assert args[-1] == "rtsp://127.0.0.1:9999/bp_dogcam", "推的是本机中转端口（RtspUplink）"


def test_rtsp_publish_plan_appends_stream_and_keeps_credentials() -> None:
    """`RELAY_RTSP_URL` 是前缀（…/bp_），每路拼自己的名字；口令要能转义。"""
    real = relay.RTSP_PUSH
    try:
        relay.RTSP_PUSH = "rtsp://relay:pa%2Fss@39.107.55.13:8554/bp_"
        host, port, path, userinfo = relay.rtsp_publish_plan("dogcam")
        assert (host, port, path) == ("39.107.55.13", 8554, "bp_dogcam")
        assert userinfo == "relay:pa%2Fss@"
        relay.RTSP_PUSH = "rtsp://relay:pass@10.0.0.8/bp_"
        assert relay.rtsp_publish_plan("cam0")[1] == 8554, "没写端口时用 RTSP 默认值"
        relay.RTSP_PUSH = "http://wrong"
        try:
            relay.rtsp_publish_plan("dogcam")
        except ValueError:
            pass
        else:
            raise AssertionError("非 rtsp 地址应该直接报错")
    finally:
        relay.RTSP_PUSH = real



def test_wss_without_ca_returns_no_context_and_warns(capsys=None) -> None:
    """平台只开 https 入口（2026-11-26 起 3008 关闭）时控制信道是 wss://：
    没配 `RELAY_PLATFORM_CA` 就是 `CERTIFICATE_VERIFY_FAILED`，现场表现为「实时视频
    一直起不来」，所以这里要保证它至少**说清楚**怎么配。"""
    real_platform = relay.PLATFORM
    real_ca = os.environ.pop("RELAY_PLATFORM_CA", None)
    try:
        relay.PLATFORM = "https://39.107.55.13:3443"
        assert relay.endpoint() == "wss://39.107.55.13:3443/ws/device/video", relay.endpoint()
        assert relay.platform_ssl() is None, "没配 CA 时不能返回上下文（会静默校验失败）"
        os.environ["RELAY_PLATFORM_CA"] = "/nonexistent/platform.crt"
        assert relay.platform_ssl() is None, "CA 文件不存在时同样要退回 None"
    finally:
        relay.PLATFORM = real_platform
        os.environ.pop("RELAY_PLATFORM_CA", None)
        if real_ca is not None:
            os.environ["RELAY_PLATFORM_CA"] = real_ca


def test_plain_ws_never_builds_ssl_context() -> None:
    """明文入口（老部署 / 内网）行为不变：`ws://` 不碰 ssl。"""
    real_platform = relay.PLATFORM
    real_ca = os.environ.pop("RELAY_PLATFORM_CA", None)
    try:
        relay.PLATFORM = "http://127.0.0.1:3008"
        assert relay.endpoint() == "ws://127.0.0.1:3008/ws/device/video"
        os.environ["RELAY_PLATFORM_CA"] = "/etc/robodog/platform.crt"
        assert relay.platform_ssl() is None, "ws:// 不需要也不该建 SSL 上下文"
    finally:
        relay.PLATFORM = real_platform
        os.environ.pop("RELAY_PLATFORM_CA", None)
        if real_ca is not None:
            os.environ["RELAY_PLATFORM_CA"] = real_ca


def test_device_id_scopes_the_rtsp_path_and_room() -> None:
    """多设备：配了 `RELAY_DEVICE_ID` 才加设备段，不配时命名一个字节都不变。

    两台板子都推 `bp_cam0` 时，后推上来的那台会被 mediamtx 拒绝（path already in use），
    平台上的房间也会被后注册的顶掉 —— `video_start` 打给另一台，现场就是
    「一台有画面、另一台怎么点都黑」。
    """
    real_rtsp = relay.RTSP_PUSH
    real_device = relay.DEVICE_ID
    try:
        relay.RTSP_PUSH = "rtsp://relay:pass@39.107.55.13:8554/bp_"

        # 不配设备号 = 旧命名（现场已部署的单机不用改任何东西）
        relay.DEVICE_ID = ""
        host, port, path, userinfo = relay.rtsp_publish_plan("cam0")
        assert (host, port) == ("39.107.55.13", 8554), (host, port)
        assert path == "bp_cam0", path
        assert userinfo == "relay:pass@", userinfo
        assert relay.room_name("cam0") == "bp:cam0"

        # 配了设备号：path 与房间都带设备维度，两台板子互不影响
        relay.DEVICE_ID = "RD-002"
        _, _, path, _ = relay.rtsp_publish_plan("cam0")
        assert path == "bp_RD-002_cam0", path
        assert path.startswith("bp_"), "平台 mediamtx 的推流鉴权是 ~^bp_，前缀不能变"
        assert relay.room_name("cam0") == "bp:RD-002:cam0"
        assert relay.room_name("dogcam") == "bp:RD-002:dogcam"
    finally:
        relay.RTSP_PUSH = real_rtsp
        relay.DEVICE_ID = real_device


def test_device_id_charset_matches_the_platform() -> None:
    """字符集必须与平台 `video.rs::normalize_device` 一致（它会进 RTSP path / 房间 / URL）。"""
    for good in ("RD-001", "gh-test_2.1", "a"):
        assert relay.DEVICE_ID_RE.fullmatch(good), good
    for bad in ("", "a:b", "../x", "1号狗", "a" * 33, "-lead", "with space"):
        assert not relay.DEVICE_ID_RE.fullmatch(bad), bad


def _partition(sn: str, offset: int) -> bytes:
    """造一个「SN 分区镜像」：SN 写在指定的 16 字节行（现场在 0x200）。"""
    data = bytearray(4096)
    data[offset : offset + 16] = f"{sn:<16}".encode()
    return bytes(data)


def test_device_id_is_read_from_the_sn_partition() -> None:
    """真机零配置：设备编号 = 本体 SN，与设备客户端同一套读法。

    以前要手工在 unit 里配 `RELAY_DEVICE_ID`：换一台狗就得改一次，抄错就是
    「两台板子抢同一条 path」。客户端早就不要求配置（读分区），relay 也照做。
    """
    assert relay.sn_from_bytes(_partition("0202222672400230", 0x200)) == "0202222672400230"
    # 不依赖行号：0x20 / 0x200 / 0x800 都能取到
    for offset in (0x20, 0x200, 0x800):
        assert relay.sn_from_bytes(_partition("0202222672400230", offset)) == "0202222672400230", hex(offset)
    # 前面的普通文字不能抢走 16 位十六进制
    data = bytearray(_partition("0202222672400230", 0x200))
    data[0:16] = f"{'MAPDATA1':<16}".encode()
    assert relay.sn_from_bytes(bytes(data)) == "0202222672400230"
    # 空白 / 全 0 分区不能编出一个编号（宁可回旧命名，也不能推错房间）
    assert relay.sn_from_bytes(bytes(4096)) == ""


def test_device_id_resolution_order() -> None:
    """`RELAY_DEVICE_ID` > `ROBODOG_DEVICE_ID` > 本体 SN 分区。"""
    import tempfile

    real_path = relay.SN_PATH
    saved = {key: os.environ.pop(key, None) for key in ("RELAY_DEVICE_ID", "ROBODOG_DEVICE_ID")}
    path = ""
    try:
        handle = tempfile.NamedTemporaryFile(delete=False)
        path = handle.name
        handle.write(_partition("0202222672400230", 0x200))
        handle.close()
        relay.SN_PATH = path

        # 真机（零配置）：直接用分区里的 SN
        assert relay.resolve_device_id()[:2] == ("0202222672400230", path)

        # 调试 / 非真机：环境变量优先
        os.environ["ROBODOG_DEVICE_ID"] = "RD-002"
        assert relay.resolve_device_id()[:2] == ("RD-002", "ROBODOG_DEVICE_ID")
        os.environ["RELAY_DEVICE_ID"] = "RD-001"
        assert relay.resolve_device_id()[:2] == ("RD-001", "RELAY_DEVICE_ID")
    finally:
        relay.SN_PATH = real_path
        for key, value in saved.items():
            os.environ.pop(key, None)
            if value is not None:
                os.environ[key] = value
        if path:
            os.unlink(path)


def main() -> int:
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    failed = 0
    for test in tests:
        try:
            test()
        except AssertionError as exc:
            failed += 1
            print(f"FAIL {test.__name__}: {exc}")
        else:
            print(f"ok   {test.__name__}")
    print(f"\n{len(tests) - failed}/{len(tests)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
