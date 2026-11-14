/**
 * 运行时配置（现场入口模式）。
 *
 * 控制台有两种打开方式，前端代码不需要因此分叉：
 *
 * 1. **云端直连**（默认）：页面由业务服务自己托管（`http://39.107.55.13:3100`），
 *    REST/SSE 走同源 `/api`。
 * 2. **现场入口**（`deploy/site-console`）：同一份 `dist` 托管在内网机器上，
 *    页面 origin 是内网地址，后端仍在云端。这时由 `/runtime-config.js` 注入
 *    `apiBase`（云端 `/api`）与 `videoWsBase`（云端 ws）。
 *
 * 为什么需要第 2 种：Chrome 的 Local Network Access 禁止**公网页面**访问
 * **内网地址**（`corsError=InsecureLocalNetwork`），所以云端页面无法直连背包的
 * `:8000` / `:8889`。把页面本身放到内网，问题消失，且视频仍是端到端 WebRTC。
 */
interface RuntimeConfig {
  /** REST/SSE 基址，例如 `http://39.107.55.13:3100/api` */
  apiBase?: string;
  /** 设备视频 WebSocket 基址，例如 `ws://39.107.55.13:3100` */
  videoWsBase?: string;
}

declare global {
  interface Window {
    __ROBODOG_RUNTIME__?: RuntimeConfig;
  }
}

const runtime: RuntimeConfig =
  typeof window === "undefined" ? {} : window.__ROBODOG_RUNTIME__ ?? {};

export function apiBaseUrl(): string {
  return (runtime.apiBase || "/api").replace(/\/+$/, "");
}

/** 设备视频中继（`/api/devices/{id}/video/ws`）所在源 */
export function wsOrigin(): string {
  if (runtime.videoWsBase) return runtime.videoWsBase.replace(/\/+$/, "");
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${location.host}`;
}

/** 是否运行在「现场入口」模式下（用于界面提示与排障） */
export function isSiteEntry(): boolean {
  return !!runtime.apiBase;
}

/** 页面自身是否在内网/本机打开（决定能否直连背包这类内网地址） */
export function isPrivateOrigin(): boolean {
  const host = location.hostname;
  return (
    host === "localhost" ||
    host === "127.0.0.1" ||
    host === "::1" ||
    /^10\./.test(host) ||
    /^192\.168\./.test(host) ||
    /^172\.(1[6-9]|2\d|3[01])\./.test(host) ||
    /\.local$/i.test(host)
  );
}
