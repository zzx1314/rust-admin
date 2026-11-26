import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { authApi } from "@/api/ops";
import { onSessionExpired, tokenExpired, tokenExpiresAt, tokenStorage } from "@/api/http";
import type { LoginResponse, UserInfo } from "@/api/types";

const USER_KEY = "robodog_user";

/** 令牌剩余不足这么久就换新（滑动续期）。24h 有效期的 2h 前换，页面开着就不会掉 */
const RENEW_AHEAD_MS = 2 * 60 * 60 * 1000;

function readUser(): UserInfo | null {
  try {
    const raw = localStorage.getItem(USER_KEY);
    return raw ? (JSON.parse(raw) as UserInfo) : null;
  } catch {
    return null;
  }
}

export const useAuthStore = defineStore("auth", () => {
  // 过期令牌不能当「已登录」：路由守卫只看令牌是否存在，留着它会让用户
  // 先闪进控制台、再被几个 401 一起踢回登录页（还带一句『请先登录』的提示）。
  const stored = tokenStorage.get();
  if (stored && tokenExpired(stored)) tokenStorage.clear();
  const token = ref(stored && !tokenExpired(stored) ? stored : "");
  const user = ref<UserInfo | null>(token.value ? readUser() : null);
  const isLoggedIn = computed(() => Boolean(token.value));

  /** 落地一枚令牌 + 用户信息（登录与续期共用同一条路径） */
  function applySession(response: LoginResponse) {
    token.value = response.token;
    user.value = response.user;
    tokenStorage.set(response.token);
    localStorage.setItem(USER_KEY, JSON.stringify(response.user));
  }

  function clearSession() {
    token.value = "";
    user.value = null;
    tokenStorage.clear();
  }

  // 401 被判会话过期时（`api/http.ts`）把内存里的令牌一起清掉，
  // 否则页面会带着一枚已经作废的令牌继续跑（幽灵会话）。
  onSessionExpired(clearSession);

  async function login(username: string, password: string) {
    applySession(await authApi.login(username, password));
  }

  async function loadMe() {
    if (!token.value) return;
    try {
      user.value = await authApi.me();
      localStorage.setItem(USER_KEY, JSON.stringify(user.value));
    } catch (error) {
      // **只有 401 才算登录态没了**。以前这里 `catch` 一律 `logout()`：发布重启
      // （现场一天重启 8 次）、4G 抖动、中台 503 造成的网络错误都会把令牌清掉，
      // 操作员看到的就是「莫名其妙自动退出」。
      const status = (error as { response?: { status?: number } })?.response?.status;
      if (status === 401) logout();
    }
  }

  /**
   * 滑动续期：令牌快到期时换一枚新的。
   *
   * 业务平台的 JWT 是绝对过期的（`ROBODOG_JWT_TTL_HOURS`，默认 24h），没有续期
   * 手段时每天必掉一次登录态。页面开着时由 `OpsLayout` 定时调这里；
   * 失败（网络抖动 / 后端重启）不登出，等下一轮或 401 拦截器兜底。
   */
  async function renew(force = false) {
    if (!token.value) return;
    if (!force) {
      const expiresAt = tokenExpiresAt(token.value);
      if (expiresAt === null || expiresAt - Date.now() > RENEW_AHEAD_MS) return;
    }
    try {
      applySession(await authApi.refresh());
    } catch {
      /* 留给 401 拦截器判断：网络错误不该把用户踢出去 */
    }
  }

  function logout() {
    clearSession();
  }

  return { token, user, isLoggedIn, login, loadMe, renew, logout };
});
