import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { authApi } from "@/api/ops";
import { tokenExpired, tokenStorage } from "@/api/http";
import type { UserInfo } from "@/api/types";

const USER_KEY = "robodog_user";

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

  async function login(username: string, password: string) {
    const response = await authApi.login(username, password);
    token.value = response.token;
    user.value = response.user;
    tokenStorage.set(response.token);
    localStorage.setItem(USER_KEY, JSON.stringify(response.user));
  }

  async function loadMe() {
    if (!token.value) return;
    try {
      user.value = await authApi.me();
      localStorage.setItem(USER_KEY, JSON.stringify(user.value));
    } catch {
      logout();
    }
  }

  function logout() {
    token.value = "";
    user.value = null;
    tokenStorage.clear();
  }

  return { token, user, isLoggedIn, login, loadMe, logout };
});
