import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { authApi } from "@/api/ops";
import { tokenStorage } from "@/api/http";
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
  const token = ref(tokenStorage.get() || "");
  const user = ref<UserInfo | null>(readUser());
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
