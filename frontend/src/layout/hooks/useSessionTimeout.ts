import { onMounted, onUnmounted, ref } from "vue";
import { getSafePolicy } from "@/api/system";
import { getToken } from "@/utils/auth";
import { refreshAccessToken } from "@/utils/http";
import {
  markSessionActivity,
  onSessionActivity
} from "@/utils/sessionActivity";
import { useUserStoreHook } from "@/store/modules/user";
import { message } from "@/utils/message";
import { SUCCESS } from "@/api/base";

/**
 * 会被算作「用户还在用」的浏览器事件。
 * 注意接口流量也算（见 `utils/sessionActivity`），否则挂在实时视频/轮询页面上不动
 * 鼠标会被当成空闲。
 */
const ACTIVITY_EVENTS = [
  "mousedown",
  "mousemove",
  "keydown",
  "scroll",
  "touchstart",
  "click"
];

/** 退出前多久先提醒（毫秒） */
const WARN_AHEAD_MS = 60 * 1000;
/** access token 距离过期多久开始主动续期（上限，毫秒） */
const KEEP_ALIVE_AHEAD_MS = 2 * 60 * 1000;
/** 主动续期检查间隔（毫秒） */
const KEEP_ALIVE_INTERVAL_MS = 30 * 1000;

/**
 * 会话管理：
 *
 * 1. **空闲登出**：`sysOvertime` 内既没有 DOM 交互也没有前台接口请求 → 退回登录页；
 * 2. **主动续期**：access token 快到期时先换一对新令牌，不再等第一个 401
 *    （后端 access token 是**绝对过期**的，`validate_token` 不做滑动续期）。
 *
 * 两者都需要「活动」信号，所以 DOM 事件统一走 `markSessionActivity()` 打点。
 */
export function useSessionTimeout() {
  const timeoutMs = ref(0);
  let logoutTimer: ReturnType<typeof setTimeout> | null = null;
  let warnTimer: ReturnType<typeof setTimeout> | null = null;
  let keepAliveTimer: ReturnType<typeof setInterval> | null = null;
  let unsubscribeActivity: (() => void) | null = null;
  let keepAliveRunning = false;

  const clearIdleTimers = () => {
    if (warnTimer) {
      clearTimeout(warnTimer);
      warnTimer = null;
    }
    if (logoutTimer) {
      clearTimeout(logoutTimer);
      logoutTimer = null;
    }
  };

  const stopTimers = () => {
    clearIdleTimers();
    if (keepAliveTimer) {
      clearInterval(keepAliveTimer);
      keepAliveTimer = null;
    }
    ACTIVITY_EVENTS.forEach(event => {
      document.removeEventListener(event, markSessionActivity);
    });
    document.removeEventListener("visibilitychange", handleVisibilityChange);
    unsubscribeActivity?.();
    unsubscribeActivity = null;
  };

  const logout = () => {
    stopTimers();
    message("会话已过期，请重新登录", { type: "warning" });
    useUserStoreHook().logOut();
  };

  /** 重置空闲倒计时（有任何活动就调用） */
  const resetActivityTimer = () => {
    if (timeoutMs.value <= 0) return;
    clearIdleTimers();
    // 超时时间本身很短时，提醒点最多取一半，避免「刚提醒就退出」
    const warnAhead = Math.min(WARN_AHEAD_MS, Math.floor(timeoutMs.value / 2));
    if (timeoutMs.value > warnAhead) {
      warnTimer = setTimeout(() => {
        message(
          `长时间未操作，${Math.round(warnAhead / 1000)} 秒后将自动退出`,
          { type: "warning" }
        );
      }, timeoutMs.value - warnAhead);
    }
    logoutTimer = setTimeout(logout, timeoutMs.value);
  };

  /**
   * 主动续期：access token 剩余时间不足就提前换新。
   * 刷新失败**不直接踢人**（网络抖动/服务重启瞬间很常见），交给 401 拦截器兜底判断。
   */
  const keepAlive = async () => {
    if (keepAliveRunning) return;
    const tokenData = getToken();
    const expires = Number(tokenData?.expires ?? 0);
    if (!expires || !tokenData?.refreshToken) return;
    const ttl = timeoutMs.value > 0 ? timeoutMs.value : KEEP_ALIVE_AHEAD_MS * 8;
    const ahead = Math.min(
      KEEP_ALIVE_AHEAD_MS,
      Math.max(KEEP_ALIVE_INTERVAL_MS, Math.floor(ttl / 5))
    );
    if (expires - Date.now() > ahead) return;

    keepAliveRunning = true;
    try {
      await refreshAccessToken();
    } catch {
      // 交给 http 拦截器：下一次请求 401 时还会再试（那里有退避重试），仍失败才登出
    } finally {
      keepAliveRunning = false;
    }
  };

  /** 切回前台立刻检查一次（后台标签页的定时器会被浏览器节流） */
  function handleVisibilityChange() {
    if (document.visibilityState !== "visible") return;
    void keepAlive();
  }

  const startTimers = () => {
    if (timeoutMs.value <= 0) return;

    ACTIVITY_EVENTS.forEach(event => {
      document.addEventListener(event, markSessionActivity, { passive: true });
    });
    unsubscribeActivity = onSessionActivity(resetActivityTimer);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    keepAliveTimer = setInterval(
      () => void keepAlive(),
      KEEP_ALIVE_INTERVAL_MS
    );

    resetActivityTimer();
    void keepAlive();
  };

  onMounted(async () => {
    const tokenData = getToken();
    if (!tokenData) return;

    try {
      const res = await getSafePolicy();
      if (res?.code !== SUCCESS) return;
      const raw = String(res.data?.sysOvertime ?? "").trim();
      let seconds = NaN;
      if (/^\d+$/.test(raw)) {
        seconds = parseInt(raw, 10);
      } else {
        // 兼容库里存成 "30分钟" 这类带单位的历史值
        const mins = raw.match(/^(\d+)\s*分钟/);
        if (mins) seconds = parseInt(mins[1], 10) * 60;
      }
      if (!isNaN(seconds) && seconds > 0) {
        timeoutMs.value = seconds * 1000;
        startTimers();
      }
    } catch {
      // Ignore — timer simply won't start
    }
  });

  onUnmounted(() => {
    stopTimers();
  });
}
