/**
 * 数据采集背包（板端 Agent）接口封装。
 *
 * 页面只跟**同源** `/api/backpack/*` 打交道：REST/SSE 由业务平台代理转发，
 * 免掉 Agent（FastAPI，无 CORS 中间件）的跨域问题；视频（WHEP）不走这里，
 * 由浏览器直连板端 mediamtx，地址取 `status.webrtc.base`。
 *
 * 字段与 Agent 的返回保持一致，只做类型标注与轻度归一化，
 * 详细契约见 `business/docs/backpack-video-integration.md`。
 */
import { get, post } from "./http";
import { apiBaseUrl } from "@/utils/runtime";

/** 6 路视频源之一（顺序固定：狗前置 → 四路相机 → 红外） */
export interface BackpackSource {
  key: string;
  /** 中文名（左前/右前/…），由板端 config.py 提供 */
  label: string;
  /** mediamtx path，WHEP 地址用它拼 */
  stream: string;
  kind: "usb" | "relay" | "infrared";
  /** 是否支持 AI：红外为 false */
  ai: boolean;
  /** 当前是否可用（狗前置未配置源时为 false，页面置灰） */
  available: boolean;
  /** 不可用 / 半可用时的原因，直接展示给现场 */
  hint: string;
}

export interface BackpackRelay {
  enabled: boolean;
  running: boolean;
  restarts: number;
  last_error: string;
  stream: string;
}

export interface BackpackLabel {
  id: number;
  /** 模型类别名（英文，传给后端） */
  name: string;
  /** 中文展示名 */
  zh: string;
  color: string | null;
  ignored: boolean;
}

/** 一个模型包 = 一个场景 */
export interface BackpackTask {
  task: string;
  name: string;
  desc: string;
  nc: number;
  labels: BackpackLabel[];
  conf: number;
  alarm: string[];
  target: string;
  model_size: number;
  ok: boolean;
  issues: string[];
}

export interface BackpackTaskList {
  models_dir: string;
  count: number;
  ready: number;
  note: string;
  tasks: BackpackTask[];
  error?: string;
}

export interface BackpackOsd {
  enabled: boolean;
  running: boolean;
  /** 真正被 ffmpeg 读走画布才算就绪；只有进程不算 */
  ready: boolean;
  session: number;
  cam: string | null;
  stream: string;
  error: string;
  frames: number;
  write_fps: number;
  render_ms: number;
  video_fps?: number;
  fifo_fps?: number;
  missing_glyphs: string[];
  log_tail: string[];
}

export interface BackpackDetectStat {
  frames: number;
  hits: number;
  fps: number;
  last_ms: number;
  ts: number;
}

export interface BackpackDetectStatus {
  running: boolean;
  /** stopped / starting / running */
  state: string;
  task: string;
  labels: string[] | null;
  conf: number;
  cams: string[];
  fps: number;
  warnings: string[];
  stats: Record<string, BackpackDetectStat>;
  grabbers: Record<string, { has_frame: boolean; error: string }>;
  error: string;
  single_cam: boolean;
  allowed_cams: string[];
  default_cam: string;
  preview_stream: string;
  osd?: BackpackOsd;
}

/** 接法：`direct` = 浏览器直连板端（正式环境）；`proxy` = 业务服务代理；`none` = 未配置 */
export type BackpackMode = "direct" | "proxy" | "none";

export interface BackpackStatus {
  configured: boolean;
  mode?: BackpackMode;
  /** 直连模式下浏览器要用的两个基址（由后端下发，避免页面自己猜板子 IP） */
  direct?: { apiBase: string; webrtcBase: string | null };
  agent: { base: string; reachable: boolean; error: string };
  /** 浏览器直连的 WHEP 基址；为 null 表示后端也算不出来 */
  webrtc: { base: string | null };
  sources: BackpackSource[];
  relay: BackpackRelay | null;
  relay_error: string;
  detect: {
    tasks: BackpackTaskList | null;
    tasks_error: string;
    status: BackpackDetectStatus | null;
    status_error: string;
  };
  osd: {
    enabled: boolean;
    ready: boolean;
    preview_stream: string;
    error: string;
    log_tail: string[] | null;
  };
  ai: { single_cam: boolean; allowed_cams: string[]; default_cam: string };
  label_format: { show_count: boolean; show_conf: boolean; unit: string; example: string } | null;
}

export interface BackpackBox {
  label: string;
  zh: string;
  color: string;
  conf: number;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  count: number;
  /** 后端统一生成的画框文字：中文名 + 同类数量（如「火焰 3处」） */
  text: string;
}

export interface BackpackDetectEvent {
  type: "detect";
  cam: string;
  ts: number;
  frame_w: number;
  frame_h: number;
  task: string;
  infer_ms: number;
  boxes: BackpackBox[];
  counts: Record<string, number>;
}

export interface BackpackActionPayload {
  ok?: boolean;
  error?: string;
  status?: BackpackDetectStatus;
}

export interface BackpackStartBody {
  task: string;
  labels: string[] | null;
  conf: number;
  cams: string[];
  fps: number;
}

/* ------------------------- 直连模式（正式环境） ------------------------- */
//
// 平台部署在公网、背包在现场内网：业务服务连不上板端，但操作员浏览器与背包同网段。
// 这时后端只下发 `direct.{apiBase,webrtcBase}`，REST/SSE 由页面直连 Agent（板端已开
// CORS），视频仍是浏览器 ⇄ mediamtx。归一化规则与后端 `backpack.rs::sources_of`
// 保持一致（六路固定顺序、红外不支持 AI、狗前置未配置源时置灰）；
// 两处实现都对应 docs/backpack-video-integration.md 的同一份契约。

const AI_SOURCES = ["dogcam", "cam0", "cam1", "cam2", "cam3"] as const;

interface RawCameras {
  cameras?: Record<string, { title?: string; stream?: string }>;
  infrared?: { stream?: string };
}

async function getJson<T>(url: string, timeoutMs = 8000): Promise<T> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, { signal: controller.signal, cache: "no-store" });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return (await response.json()) as T;
  } finally {
    clearTimeout(timer);
  }
}

async function postJson<T>(url: string, body: object, timeoutMs = 20000): Promise<T> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
      signal: controller.signal
    });
    if (!response.ok) {
      const text = await response.text().catch(() => "");
      throw new Error(`HTTP ${response.status}${text ? `：${text.slice(0, 120)}` : ""}`);
    }
    return (await response.json()) as T;
  } finally {
    clearTimeout(timer);
  }
}

function deriveWebrtcBase(apiBase: string): string | null {
  try {
    return `http://${new URL(apiBase).hostname}:8889`;
  } catch {
    return null;
  }
}

function normalizeSources(cameras: RawCameras, relay: BackpackRelay | null): BackpackSource[] {
  const relayEnabled = !!relay?.enabled;
  const relayRunning = !!relay?.running;
  const list: BackpackSource[] = AI_SOURCES.map(key => {
    const entry = cameras.cameras?.[key];
    const isRelay = key === "dogcam";
    const hint = !isRelay
      ? ""
      : !relayEnabled
        ? "板端未配置机器狗前置 RTSP 源（DOG_RTSP_URL 或 backend/dog_rtsp_url），配置并重启 Agent 后可用"
        : !relayRunning
          ? "已配置源但当前没有收到机器狗推流：检查本体是否在推、网线是否通"
          : "";
    return {
      key,
      label: entry?.title || key,
      stream: entry?.stream || key,
      kind: isRelay ? "relay" : "usb",
      ai: true,
      available: !isRelay || relayEnabled,
      hint
    };
  });
  list.push({
    key: "infrared",
    label: "红外",
    stream: cameras.infrared?.stream || "dog-ipcam",
    kind: "infrared",
    ai: false,
    available: true,
    hint: "红外通道不参与 AI 识别（热成像不具备可见光模型特征）"
  });
  return list;
}

/**
 * 直连模式下由浏览器自己拼出与后端同构的 status。
 *
 * 任何一项失败都不影响其余项（与后端「各自带 *_error」的策略一致）；
 * `cameras` 失败才判定为「Agent 不可达」——那是最基础的连通性探针。
 */
export async function loadDirectStatus(shell: BackpackStatus): Promise<BackpackStatus> {
  const apiBase = (shell.direct?.apiBase || "").replace(/\/+$/, "");
  const webrtcBase = shell.direct?.webrtcBase ?? deriveWebrtcBase(apiBase);
  const base: BackpackStatus = {
    ...shell,
    configured: true,
    mode: "direct",
    direct: { apiBase, webrtcBase },
    webrtc: { base: webrtcBase }
  };
  if (!apiBase) {
    return {
      ...base,
      agent: { base: "", reachable: false, error: "后端未下发浏览器可访问的背包地址" },
      sources: []
    };
  }

  const [cameras, relay, tasks, detect] = await Promise.allSettled([
    getJson<RawCameras>(`${apiBase}/api/cameras`),
    getJson<BackpackRelay>(`${apiBase}/api/relay/status`),
    getJson<BackpackTaskList>(`${apiBase}/api/detect/tasks`, 12000),
    getJson<BackpackDetectStatus>(`${apiBase}/api/detect/status`)
  ]);

  const reachable = cameras.status === "fulfilled";
  const relayValue = relay.status === "fulfilled" ? relay.value : null;
  const detectStatus = detect.status === "fulfilled" ? detect.value : null;
  const osd = detectStatus?.osd;

  return {
    ...base,
    agent: {
      base: apiBase,
      reachable,
      error: reachable
        ? ""
        : `连不上背包 Agent（${apiBase}）：确认板端 Agent 已启动、浏览器与背包同网段`
    },
    sources: reachable ? normalizeSources(cameras.value, relayValue) : [],
    relay: relayValue,
    relay_error: relay.status === "rejected" ? String(relay.reason) : "",
    detect: {
      tasks: tasks.status === "fulfilled" ? tasks.value : null,
      tasks_error: tasks.status === "rejected" ? String(tasks.reason) : "",
      status: detectStatus,
      status_error: detect.status === "rejected" ? String(detect.reason) : ""
    },
    osd: {
      enabled: !!osd?.enabled,
      ready: !!osd?.ready,
      preview_stream: osd?.stream || detectStatus?.preview_stream || "preview",
      error: osd?.error || "",
      log_tail: osd?.log_tail ?? null
    },
    ai: {
      single_cam: detectStatus?.single_cam ?? true,
      allowed_cams: detectStatus?.allowed_cams ?? [...AI_SOURCES],
      default_cam: detectStatus?.default_cam ?? "dogcam"
    }
  };
}

/** 统一入口：代理模式走业务服务，直连模式走板端；页面只认这一层 */
export interface AgentClient {
  mode: BackpackMode;
  start: (body: BackpackStartBody) => Promise<BackpackActionPayload>;
  stop: () => Promise<BackpackActionPayload>;
  config: (body: Partial<BackpackStartBody>) => Promise<BackpackActionPayload>;
  eventsUrl: string;
}

export function clientFor(status: BackpackStatus | null): AgentClient {
  if (status?.mode !== "direct" || !status.direct) {
    return {
      mode: "proxy",
      start: async body => (await backpackApi.start(body)) ?? {},
      stop: async () => (await backpackApi.stop()) ?? {},
      config: async body => (await backpackApi.config(body)) ?? {},
      eventsUrl: backpackEventUrl()
    };
  }
  const api = status.direct.apiBase.replace(/\/+$/, "");
  return {
    mode: "direct",
    start: body => postJson(`${api}/api/detect/start`, body, 60000),
    stop: () => postJson(`${api}/api/detect/stop`, {}, 30000),
    config: body => postJson(`${api}/api/detect/config`, body, 20000),
    eventsUrl: `${api}/api/events`
  };
}

export const backpackApi = {
  status: () => get<BackpackStatus>("/backpack/status"),
  start: (body: BackpackStartBody) =>
    // 板端要加载模型（最多 30s）+ 启动画框（8s 确认窗口），20s 默认超时不够
    post<BackpackActionPayload>("/backpack/detect/start", body, 60000),
  stop: () => post<BackpackActionPayload>("/backpack/detect/stop"),
  config: (body: Partial<BackpackStartBody>) =>
    post<BackpackActionPayload>("/backpack/detect/config", body as object)
};

/**
 * SSE 事件流地址。
 *
 * `EventSource` 不能自定义请求头，令牌只能放查询串（后端 `require_auth`
 * 对无 Authorization 头的请求会回落到 `?token=`，与视频 WS 端点同一套约定）。
 */
export function backpackEventUrl(): string {
  const token = localStorage.getItem("robodog_token") || "";
  // 用运行时基址拼绝对地址：「现场入口」模式下页面在内网静态服务上，
  // 相对路径 /api 会打到静态服务（404），必须显式指向云端 API。
  return `${apiBaseUrl()}/backpack/events?token=${encodeURIComponent(token)}`;
}
