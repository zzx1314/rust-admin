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
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import relay_push as relay  # noqa: E402  （必须在 sys.path 之后导入）


class FakeSession:
    """只用来观察「档位变了有没有人去重启拉流」。"""

    def __init__(self, running: bool = True) -> None:
        self.running = running
        self.restarts: list[str] = []

    async def restart_for_profile(self, reason: str) -> None:
        self.restarts.append(reason)


def reset() -> None:
    relay._degraded = False
    relay._congested_until = 0.0
    relay._flaps = 0
    relay._recovered_at = 0.0
    relay._link_duty = 0.0
    relay._healthy_since = 0.0
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
    assert only.restarts == ["降档"]
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
        args = relay.ffmpeg_args("dogcam")
    finally:
        relay.pick_encoder = original

    assert "copy" not in args, f"降档后仍是 -c:v copy：{' '.join(args)}"
    assert args[args.index("-c:v") + 1] == "h264_rkmpp"
    assert args[args.index("-b:v") + 1] == relay.DEGRADE_BITRATE
    filters = args[args.index("-vf") + 1]
    assert f"scale={relay.DEGRADE_WIDTH}:{relay.DEGRADE_HEIGHT}" in filters
    assert f"fps={relay.DEGRADE_FPS}" in filters


def test_copy_profile_still_copies() -> None:
    """`RELAY_PROFILE=copy` 是显式要求只换容器，不能被「顺手转码」改掉。"""
    reset()
    original = relay.PROFILE
    relay.PROFILE = "copy"
    try:
        args = relay.ffmpeg_args("dogcam")
    finally:
        relay.PROFILE = original

    assert args[args.index("-c:v") + 1] == "copy"
    assert "-vf" not in args


def test_window_stats_decide_congestion() -> None:
    reset()
    session = fresh("dogcam")
    now = time.monotonic()

    # 链路恰好跑满但码率达标（1200k 都推出去了）：duty 再高也不算吃不下
    assert not window(session, relay.CONGEST_WINDOW, 1200, 1.0, now)
    # 被反压：send 全程阻塞，实测只有 400kbps
    assert window(session, relay.CONGEST_WINDOW, 400, 0.95, now)
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

    asyncio.run(relay.profile_check())
    asyncio.run(asyncio.sleep(0))

    assert not relay.degraded()
    assert relay.active_profile()[0] == relay.OUT_WIDTH
    assert only.restarts == ["回配置档"]


def test_idle_restores_without_probing() -> None:
    reset()
    asyncio.run(degrade_in_loop())
    relay._congested_until = time.monotonic() - 1
    relay._healthy_since = 0.0  # 没人看，没有窗口统计

    asyncio.run(relay.profile_check())

    assert not relay.degraded(), "没人在看时不做试探，到期直接清掉拥塞状态"


def test_flapping_doubles_the_hold_window() -> None:
    reset()
    asyncio.run(degrade_in_loop())
    assert relay.hold_seconds() == relay.DEGRADE_TTL

    relay._congested_until = time.monotonic() - 1
    relay._link_duty = 0.1
    relay._healthy_since = time.monotonic() - relay.RECOVER_STABLE - 1
    asyncio.run(relay.profile_check())  # 回到配置档
    assert not relay.degraded()

    # 刚回档又拥塞 = 链路其实没恢复：窗口翻倍，避免来回升降把画面折腾碎
    asyncio.run(degrade_in_loop(reason="又吃不下"))
    assert relay.degraded()
    assert relay.hold_seconds() == relay.DEGRADE_TTL * 2

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
