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

/**
 * 接法（页面只关心「AI 的 REST/SSE 走谁」）：
 * - `direct` = 浏览器直连板端（现场入口，要求同网段）；
 * - `proxy` = 业务服务直连板端（平台与背包同网段）；
 * - `tunnel` = 业务服务经**设备客户端**代调（平台/浏览器都不在背包网段时）；
 * - `none` = 未配置且没有设备客户端。
 */
export type BackpackMode = "direct" | "proxy" | "tunnel" | "none";

export interface BackpackStatus {
  configured: boolean;
  /** 设备编号（多设备部署）：空 = 默认设备（旧单机配置） */
  device?: string;
  /** 设备显示名（配置里的 label，没有时后端给编号 / 「默认设备」） */
  deviceLabel?: string | null;
  mode?: BackpackMode;
  /**
   * 设备客户端隧道是否在位。
   *
   * `direct` 模式下浏览器会自己探测板端；探不通（换了 WiFi / 公网入口 / 出差）
   * 且这里 `available` 为真时，页面改拉 `?transport=tunnel` 走平台代调，
   * 不用再回现场入口。
   */
  tunnel?: { available: boolean; device: string };
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

/**
 * 拉一次完整状态，把「三条通道的退让」收在一处。
 *
 * 1. 后端给 `mode`（direct / proxy / tunnel / none）；
 * 2. `direct` 模式下浏览器自己探测板端（换了 WiFi、公网入口、Chrome 拦内网都会失败）；
 * 3. 探不通但后端报 `tunnel.available`（板端客户端在线）→ 带 `?transport=tunnel` 重拉，
 *    改由平台经设备客户端代调。这样「不在同一个网络」不再等于「AI 不能用」。
 */
export async function loadStatus(device?: string | null): Promise<BackpackStatus> {
  const shell = await backpackApi.status(undefined, device);
  const data = shell.mode === "direct" ? await loadDirectStatus(shell) : shell;
  if (data.mode === "direct" && !data.agent.reachable && shell.tunnel?.available) {
    return await backpackApi.status("tunnel", device);
  }
  return data;
}

/** 统一入口：代理模式走业务服务，直连模式走板端；页面只认这一层 */
export interface AgentClient {
  mode: BackpackMode;
  /** 这台客户端对应的设备编号（空 = 默认设备）；AI 动作必须打到同一台 */
  device: string;
  start: (body: BackpackStartBody) => Promise<BackpackActionPayload>;
  stop: () => Promise<BackpackActionPayload>;
  config: (body: Partial<BackpackStartBody>) => Promise<BackpackActionPayload>;
  eventsUrl: string;
}

export function clientFor(status: BackpackStatus | null): AgentClient {
  // 设备编号必须一路带到端点：多设备下「看 RD-002、AI 起在 RD-001」是最难发现的事故
  const device = (status?.device ?? "").trim();
  if (status?.mode !== "direct" || !status.direct) {
    return {
      // tunnel 与 proxy 的页面行为一致（都走业务服务），只是排障时看得见区别
      mode: status?.mode === "tunnel" ? "tunnel" : "proxy",
      device,
      start: async body => (await backpackApi.start(body, device)) ?? {},
      stop: async () => (await backpackApi.stop(device)) ?? {},
      config: async body => (await backpackApi.config(body, device)) ?? {},
      eventsUrl: backpackEventUrl(device)
    };
  }
  const api = status.direct.apiBase.replace(/\/+$/, "");
  return {
    mode: "direct",
    device,
    start: body => postJson(`${api}/api/detect/start`, body, 60000),
    stop: () => postJson(`${api}/api/detect/stop`, {}, 30000),
    config: body => postJson(`${api}/api/detect/config`, body, 20000),
    eventsUrl: `${api}/api/events`
  };
}

/** 平台中继房间状态（`GET /api/backpack/video/state`，服务端内存快照） */
export interface BackpackVideoRoom {
  /** 房间名：`bp:<stream>`（默认设备）/ `bp:<device>:<stream>`（多设备） */
  room: string;
  /** 设备编号；`null` = 不是背包房间。空串 = 默认设备 */
  device?: string | null;
  /** mediamtx 通道名（`cam0` / `dogcam` …） */
  stream?: string | null;
  /** 板端 relay 的推流连接是否在线（否则中继一定没有画面） */
  publisher: boolean;
  streaming: boolean;
  viewers: number;
  /** (秒) 最后一个观看者离开后过了多久；null = 还有人在看 / 从没人看过 */
  idleSecs: number | null;
  /** 板端上报的媒体通路：`rtsp` = WebRTC 出口；null/`ws` = 板端 relay 没升级 */
  transport: "rtsp" | "ws" | null;
}

/** 视频页「设备编号」下拉的一项（`GET /api/backpack/devices`） */
export interface BackpackDevice {
  /** 设备编号；空串 = 默认设备（旧单机配置） */
  id: string;
  /** 下拉里显示的名字 */
  label: string;
  /** 板端 relay 至少一路在线 */
  online: boolean;
  /** 配了 Agent 地址（没配只能看视频，AI 不可用） */
  configured: boolean;
  /** 当前在线的通道 */
  streams: string[];
  /** 板端上报的媒体通路：`rtsp` = 正常；null/`ws` = relay 没升级 */
  transport?: "rtsp" | "ws" | null;
}

/** 给 `/api/backpack/*` 的 POST 带上设备编号（AI 动作必须打到选中的那台） */
function withDevice(path: string, device?: string | null): string {
  const id = (device ?? "").trim();
  return id ? `${path}?device=${encodeURIComponent(id)}` : path;
}

export const backpackApi = {
  /**
   * @param transport 传 `tunnel` 表示「跳过浏览器直连/服务端直连，走设备客户端代调」
   *   （直连模式探测失败后的回退，见 `BackpackStatus.tunnel`）
   * @param device 设备编号；空 = 默认设备
   */
  status: (transport?: "tunnel", device?: string | null) =>
    get<BackpackStatus>("/backpack/status", {
      transport: transport || undefined,
      device: (device ?? "").trim() || undefined
    }),
  /** 设备下拉：配置清单 + 实际在线的推流房间（服务端合并） */
  devices: () => get<{ devices: BackpackDevice[] }>("/backpack/devices"),
  start: (body: BackpackStartBody, device?: string | null) =>
    // 板端要加载模型（最多 30s）+ 启动画框（8s 确认窗口），20s 默认超时不够
    post<BackpackActionPayload>(withDevice("/backpack/detect/start", device), body, 60000),
  stop: (device?: string | null) =>
    post<BackpackActionPayload>(withDevice("/backpack/detect/stop", device)),
  config: (body: Partial<BackpackStartBody>, device?: string | null) =>
    post<BackpackActionPayload>(withDevice("/backpack/detect/config", device), body as object),
  /** 中继房间快照：设备/任务页据此判断「默认走中继」时中继到底在不在 */
  videoState: () => get<{ rooms: BackpackVideoRoom[] }>("/backpack/video/state")
};

/**
 * SSE 事件流地址。
 *
 * `EventSource` 不能自定义请求头，令牌只能放查询串（后端 `require_auth`
 * 对无 Authorization 头的请求会回落到 `?token=`，与视频 WS 端点同一套约定）。
 */
export function backpackEventUrl(device?: string | null): string {
  const token = localStorage.getItem("robodog_token") || "";
  const id = (device ?? "").trim();
  // 用运行时基址拼绝对地址：「现场入口」模式下页面在内网静态服务上，
  // 相对路径 /api 会打到静态服务（404），必须显式指向云端 API。
  const query = [`token=${encodeURIComponent(token)}`];
  // 多设备：只订阅选中那台的 Agent 事件（后端按来源设备过滤）
  if (id) query.push(`device=${encodeURIComponent(id)}`);
  return `${apiBaseUrl()}/backpack/events?${query.join("&")}`;
}
