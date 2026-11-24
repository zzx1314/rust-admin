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

/** JWT 的 `exp`（秒）；不是合法 JWT 时返回 `null` */
function tokenExp(token: string): number | null {
  const payload = token.split(".")[1];
  if (!payload) return null;
  try {
    const base64 = payload.replace(/-/g, "+").replace(/_/g, "/");
    const padded = base64.padEnd(Math.ceil(base64.length / 4) * 4, "=");
    const claims = JSON.parse(atob(padded)) as { exp?: unknown };
    return typeof claims.exp === "number" ? claims.exp : null;
  } catch {
    return null;
  }
}

/**
 * 本地判断令牌是否已过期（不再向服务端问一次）。
 *
 * 路由守卫只看「有没有令牌」，过期令牌照样能进控制台：页面挂载后
 * `auth/me`、`dashboard/summary` 一起回 401，用户看到的是「刚进控制台就被踢回
 * 登录页，还弹一句『请先登录』」——他明明就站在登录页上。所以在 store 初始化时就把过期
 * 令牌丢掉，直接当作「未登录」。解析不出来也当过期：宁可让用户重登一次。
 */
export function tokenExpired(token: string | null | undefined): boolean {
  if (!token) return true;
  const exp = tokenExp(token);
  if (exp === null) return true;
  return exp * 1000 <= Date.now();
}

/** 是否已经站在登录页上（含「现场入口」模式下的子路径） */
function onLoginPage() {
  return window.location.pathname.replace(/\/+$/, "") === "/login";
}

/** 是否是登录接口本身（它的 401 是「密码错」，不走登出 / 换页那一套） */
function isLoginRequest(config: { url?: string } | undefined) {
  return (config?.url ?? "").replace(/\/+$/, "").endsWith("/auth/login");
}

/** 同一批并行请求一起 401 时只跳转一次 */
let lastUnauthorizedAt = 0;

/** 跳登录页前留个记号：登录页挂载时提示一次「登录已过期」（硬跳转会清掉内存里的 ElMessage） */
export const SESSION_EXPIRED_KEY = "robodog_session_expired";

function handleUnauthorized() {
  tokenStorage.clear();
  // 已经在登录页上：用户本来就要重新登录，不要再弹「请先登录」
  if (onLoginPage()) return;
  const now = Date.now();
  if (now - lastUnauthorizedAt < 3000) return;
  lastUnauthorizedAt = now;
  sessionStorage.setItem(SESSION_EXPIRED_KEY, "1");
  window.location.href = "/login";
}

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
    // 登录接口的 401 是「账号或密码错误」，要把后端说的原因照原样给用户看；
    // 其它接口的 401 是「令牌没了 / 过期了」，统一一句「登录已过期，请重新登录」——
    // 不能把服务端的「请先登录」原样弹出来：用户可能就站在登录页上。
    if (error.response?.status === 401 && !isLoginRequest(error.config)) {
      handleUnauthorized();
      return Promise.reject(error);
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

/**
 * 取二进制内容（录像播放）：返回 `Blob`，调用方自己 `createObjectURL`。
 *
 * 录像接口要 Bearer 鉴权，`<video src="/api/recordings/1/file">` 带不上头，
 * 所以前端先取成 Blob 再挂给播放器。全程录像按 5 分钟切片，单段几 MB～几十 MB，
 * 超时放宽到 2 分钟（弱网下多等一会儿，别报「请求失败」）。
 */
export function getBlob(url: string) {
  return service.get<unknown, Blob>(url, {
    responseType: "blob",
    timeout: 120000
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
