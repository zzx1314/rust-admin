import axios from "axios";
import { ElMessage } from "element-plus";

const TOKEN_KEY = "robodog_token";

export const tokenStorage = {
  get: () => localStorage.getItem(TOKEN_KEY),
  set: (token: string) => localStorage.setItem(TOKEN_KEY, token),
  clear: () => {
    localStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem("robodog_user");
  }
};

const service = axios.create({
  baseURL: "/api",
  timeout: 20000
});

service.interceptors.request.use(config => {
  const token = tokenStorage.get();
  if (token) {
    config.headers.Authorization = `Bearer ${token}`;
  }
  return config;
});

service.interceptors.response.use(
  response => {
    const body = response.data as { code?: number; msg?: string; data?: unknown };
    if (body && typeof body.code === "number") {
      if (body.code === 10200) {
        return body.data;
      }
      ElMessage.error(body.msg || "请求失败");
      return Promise.reject(new Error(body.msg || "请求失败"));
    }
    return response.data;
  },
  error => {
    if (error.response?.status === 401) {
      tokenStorage.clear();
      if (window.location.pathname !== "/login") {
        window.location.href = "/login";
      }
    }
    const message =
      error.response?.data?.msg || error.message || "网络异常，请稍后重试";
    ElMessage.error(message);
    return Promise.reject(error);
  }
);

export function get<T>(url: string, params?: object) {
  return service.get<unknown, T>(url, { params });
}

export function post<T>(url: string, data?: object) {
  return service.post<unknown, T>(url, data);
}

export function put<T>(url: string, data?: object) {
  return service.put<unknown, T>(url, data);
}

export function del<T>(url: string) {
  return service.delete<unknown, T>(url);
}
