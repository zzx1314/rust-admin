import axios from "axios";
import { ElMessage } from "element-plus";
import { apiBaseUrl } from "@/utils/runtime";

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
  // 默认同源 /api；「现场入口」模式下由 /runtime-config.js 覆盖成云端地址
  baseURL: apiBaseUrl(),
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
  async error => {
    if (error.response?.status === 401) {
      tokenStorage.clear();
      if (window.location.pathname !== "/login") {
        window.location.href = "/login";
      }
    }
    // 5xx 且响应体不是我们的 JSON（没有 msg）时，基本都是「后端没起来 / 端口不通」：
    // 用 vite 代理时上游连不上会退化成 500，axios 只能给出
    // "Request failed with status code 500"，看不出是哪个组件挂了。
    let body = error.response?.data;
    // `responseType: "blob"` 的请求（模板下载）出错时，后端 JSON 会被包成 Blob，
    // 这里读回文本，否则用户只看到「请求失败」而看不到原因
    if (body instanceof Blob) {
      try {
        body = JSON.parse(await body.text());
      } catch {
        body = undefined;
      }
    }
    const serverMsg =
      body && typeof body === "object" ? (body as { msg?: string }).msg : undefined;
    const message =
      serverMsg ||
      (error.response?.status >= 500
        ? "业务服务不可用：请确认后端已启动并能访问（默认 127.0.0.1:3100）"
        : "") ||
      error.message ||
      "网络异常，请稍后重试";
    ElMessage.error(message);
    return Promise.reject(error);
  }
);

export function get<T>(url: string, params?: object) {
  return service.get<unknown, T>(url, { params });
}

/** 下载文件（导入模板 / 导出包）：后端直接回二进制，这里触发浏览器保存 */
export function download(url: string, filename: string) {
  return service
    .get<unknown, Blob>(url, { responseType: "blob", timeout: 60000 })
    .then(blob => {
      const link = document.createElement("a");
      link.href = URL.createObjectURL(blob);
      link.download = filename;
      link.click();
      URL.revokeObjectURL(link.href);
    });
}

/** 上传文件（multipart）：boundary 由 axios 自己生成，调用方不要手写 Content-Type */
export function upload<T>(url: string, form: FormData, timeoutMs?: number) {
  return service.post<unknown, T>(url, form, timeoutMs ? { timeout: timeoutMs } : undefined);
}

export function post<T>(url: string, data?: object, timeoutMs?: number) {
  // `timeoutMs` 只给少数慢接口用（如背包 AI 启动：板端加载模型 + 画框自检，
  // 默认 20s 会被 axios 提前掐断，页面显示成「请求失败」）
  return service.post<unknown, T>(url, data, timeoutMs ? { timeout: timeoutMs } : undefined);
}

export function put<T>(url: string, data?: object) {
  return service.put<unknown, T>(url, data);
}

export function del<T>(url: string) {
  return service.delete<unknown, T>(url);
}
