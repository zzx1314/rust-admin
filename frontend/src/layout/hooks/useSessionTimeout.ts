import { onMounted, onUnmounted, ref } from "vue";
import { getSafePolicy } from "@/api/system";
import { getToken } from "@/utils/auth";
import { useUserStoreHook } from "@/store/modules/user";
import { message } from "@/utils/message";
import { SUCCESS } from "@/api/base";

const ACTIVITY_EVENTS = [
  "mousedown",
  "mousemove",
  "keydown",
  "scroll",
  "touchstart",
  "click"
];

export function useSessionTimeout() {
  const timeoutMs = ref(0);
  let activityTimer: ReturnType<typeof setTimeout> | null = null;
  let checkTimer: ReturnType<typeof setInterval> | null = null;

  const clearActivityTimer = () => {
    if (activityTimer) {
      clearTimeout(activityTimer);
      activityTimer = null;
    }
  };

  const clearCheckTimer = () => {
    if (checkTimer) {
      clearInterval(checkTimer);
      checkTimer = null;
    }
  };

  const logout = () => {
    clearActivityTimer();
    clearCheckTimer();
    message("会话已过期，请重新登录", { type: "warning" });
    useUserStoreHook().logOut();
  };

  const resetActivityTimer = () => {
    if (timeoutMs.value <= 0) return;
    clearActivityTimer();
    activityTimer = setTimeout(() => {
      logout();
    }, timeoutMs.value);
  };

  const checkTokenExpiry = () => {
    const tokenData = getToken();
    if (!tokenData) {
      logout();
      return;
    }
    const expires = tokenData.expires as number;
    if (expires && expires < Date.now()) {
      logout();
    }
  };

  const startTimer = () => {
    if (timeoutMs.value <= 0) return;

    ACTIVITY_EVENTS.forEach(event => {
      document.addEventListener(event, resetActivityTimer, { passive: true });
    });

    resetActivityTimer();

    checkTimer = setInterval(checkTokenExpiry, 30_000);
  };

  const stopTimer = () => {
    clearActivityTimer();
    clearCheckTimer();
    ACTIVITY_EVENTS.forEach(event => {
      document.removeEventListener(event, resetActivityTimer);
    });
  };

  onMounted(async () => {
    const tokenData = getToken();
    if (!tokenData) return;

    try {
      const res = await getSafePolicy();
      if (res?.code === SUCCESS && res.data?.sysOvertime) {
        const seconds = parseInt(res.data.sysOvertime, 10);
        if (!isNaN(seconds) && seconds > 0) {
          timeoutMs.value = seconds * 1000;
          startTimer();
        }
      }
    } catch {
      // Ignore — timer simply won't start
    }
  });

  onUnmounted(() => {
    stopTimer();
  });
}
