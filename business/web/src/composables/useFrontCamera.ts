/**
 * 机器狗前置摄像头（背包 `dogcam`）——「实时视频」页默认那一路。
 *
 * 设备管理的控制抽屉、计划管理的实时监控都用它，保证三处的画面**同源同路**：
 * 板端 relay 把本体前置 RTSP 归一化成 `dogcam`，**默认走平台中继**（板端推到平台
 * mediamtx，浏览器 WHEP 拉）。
 * 只有中继推流端不在（板端 `backpack-relay` 没连上 / 没部署）时，才退回浏览器直连
 * mediamtx（WHEP）—— 现场入口、同网段时仍能看，不至于直接黑屏。
 *
 * 为什么不复用设备档案里的视频地址：那条路是「设备客户端 ffmpeg 拉 RTSP → 平台中继」，
 * 与背包 relay 是两套推流端，现场经常出现「实时视频很稳、设备抽屉一直重连」
 * （见 `docs/backpack-video-integration.md` §6.6）。2026-11-18 起旧链路已整体删除，
 * 三处画面只有这一路；没接背包时只给配置说明，不做兜底播放。
 *
 * 状态共享：`/api/backpack/status` 要聚合板端四个接口，多开几个抽屉/弹窗时
 * 不能各拉一份。这里做进程内缓存 + 单飞（in-flight 合并），TTL 15s。
 *
 * **多设备**（2026-11-26）：调用方可以带设备编号（设备抽屉 / 任务监控天然知道是哪台狗）。
 * 但单机部署里板端 relay 没有 `RELAY_DEVICE_ID`，房间还是 `bp:dogcam`、平台也没配
 * `[[backpack.devices]]` —— 这时代码里传进来的设备号在平台上是「不存在」的，
 * 必须落回默认设备，否则抽屉会对着一条不存在的流永远「起流中」。
 * 判据就是 `GET /api/backpack/devices`（配置清单 + 实际在线的推流房间）。
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import {
  backpackApi,
  loadStatus,
  type BackpackDevice,
  type BackpackStatus
} from "@/api/backpack";
import { isPrivateOrigin } from "@/utils/runtime";

/** 前置通道在 mediamtx / 平台中继里的固定名字（与板端 `mediamtx.yml` 一致） */
export const FRONT_STREAM = "dogcam";

/** 多个组件共享一份 status：15s 内的重复调用直接用缓存 */
const SHARE_TTL_MS = 15_000;
/** 打开页面期间的低频刷新：换网 / 板端重启后不用手动刷新 */
const POLL_MS = 20_000;

/**
 * status 缓存**按设备编号分开**：多设备部署时设备抽屉 / 任务监控 / 实时视频页会同时看
 * 不同的狗，共用一份会把别人的相机清单与 AI 状态显示出来。
 * 键是规范化后的设备编号，空串 = 默认设备。
 */
const statusCache = new Map<string, { at: number; data: BackpackStatus }>();
const statusPending = new Map<string, Promise<BackpackStatus>>();
/** 设备清单（配置里的 + 实际在线的推流房间）：全局一份 */
let devicesCache: { at: number; list: BackpackDevice[] } | null = null;
let devicesPending: Promise<BackpackDevice[]> | null = null;

function cacheKey(device?: string | null): string {
  return (device ?? "").trim();
}

async function sharedStatus(device: string, force = false): Promise<BackpackStatus> {
  const cached = statusCache.get(device);
  if (!force && cached && Date.now() - cached.at < SHARE_TTL_MS) return cached.data;
  let pending = statusPending.get(device);
  if (!pending) {
    pending = loadStatus(device)
      .then(data => {
        statusCache.set(device, { at: Date.now(), data });
        return data;
      })
      .finally(() => {
        statusPending.delete(device);
      });
    statusPending.set(device, pending);
  }
  return pending;
}

async function sharedDevices(force = false): Promise<BackpackDevice[]> {
  if (!force && devicesCache && Date.now() - devicesCache.at < SHARE_TTL_MS) {
    return devicesCache.list;
  }
  if (!devicesPending) {
    devicesPending = backpackApi
      .devices()
      .then(data => {
        const list = data.devices ?? [];
        devicesCache = { at: Date.now(), list };
        return list;
      })
      .finally(() => {
        devicesPending = null;
      });
  }
  return devicesPending;
}

/**
 * 实际能用的设备编号：清单里没有这台就落回默认设备。
 *
 * 单机部署（没有 `[[backpack.devices]]`、relay 也没配 `RELAY_DEVICE_ID`）时，
 * 抽屉传进来的 `deviceId` 在平台设备清单里不存在 → 走默认设备的 `bp:dogcam`。
 */
function resolveDevice(wanted: string, list: BackpackDevice[]): string {
  if (!wanted) return "";
  return list.some(item => item.id === wanted) ? wanted : "";
}

/**
 * 走哪条路（设备管理 / 计划管理固定「中继优先」）：
 * - `relay`：板端推流 → 平台 mediamtx → 浏览器 WHEP（同源反代信令）—— **默认**，
 *   与「实时视频」的「平台中继」同一条通路，任何网络都能看；
 * - `direct`：中继推流端不在时的兜底，浏览器 ⇄ 板端 mediamtx（WHEP，延时最低）；
 * - `none`：本部署没接背包，前置这一路不存在。
 *
 * 以前这里和 `/monitor` 一样「直连优先」，公网页面/探不通才切中继，结果设备抽屉、
 * 任务监控默认都在直连；现场要的是**统一走中继**（板端 WHEP 一抖就黑），所以改成中继优先。
 */
export type FrontCameraPath = "direct" | "relay" | "none";

export function frontCameraPath(
  status: BackpackStatus | null,
  relayOnline: boolean,
  /** 有没有拿到「设备清单」快照；`false` = 还不知道（接口刚失败过），此时不能判成离线 */
  relayKnown = true
): FrontCameraPath {
  if (!status?.configured) return "none";
  // 中继在线 → 中继；**状态未知也先中继**：它任何网络都能用，而直连一旦判错就是
  // 「连不上板端视频服务（mediamtx :8889）」——2026-11-18 现场就是这样卡住的。
  if (relayOnline || !relayKnown) return "relay";
  // 只有「确定中继没在推 + 页面本身在内网」才轮到直连（现场入口 / 同网段的控制台）。
  // `status.agent.reachable` 是**服务端**能不能连到板端，不代表操作员的浏览器能直连
  // 192.168.x.x:8889 —— 拿它决定直连是错的（公网页面必然弹 :8889 连不上）。
  return isPrivateOrigin() && status.webrtc.base ? "direct" : "relay";
}

/**
 * `device` = 设备编号（多设备部署）：决定 Agent 地址、WHEP 基址与中继房间名。
 * 空 = 默认设备（旧单机部署，行为与以前完全一致）。
 *
 * 返回的 `device` 是**解析后**真正在用的编号（可能因为清单里没有而落回 `""`），
 * 播放器要用它去拼房间名 —— 直接拿调用方传进来的值会对着不存在的流等起流。
 */
export function useFrontCamera(device?: () => string | null | undefined) {
  const status = ref<BackpackStatus | null>(null);
  const devices = ref<BackpackDevice[]>([]);
  /** 设备清单是否拿到过：接口偶发失败时保留上一次结果，别把「中继在线」误判成离线 */
  const devicesKnown = ref(false);
  /** 解析后的设备编号（空 = 默认设备） */
  const activeDevice = ref("");
  const error = ref("");
  let timer: ReturnType<typeof setInterval> | null = null;

  async function refresh(force = false) {
    // 先拿设备清单：它决定「调用方要的那台在平台里有没有」
    try {
      devices.value = await sharedDevices(force);
      devicesKnown.value = true;
    } catch {
      // 清单失败**不能清空**：清空会让 relayOnline 变假 → 页面切到直连 →
      // 公网页面必然「连不上板端视频服务（mediamtx :8889）」。保留上一次结果。
    }
    activeDevice.value = resolveDevice(cacheKey(device?.()), devices.value);
    try {
      status.value = await sharedStatus(activeDevice.value, force);
      error.value = "";
    } catch (cause) {
      error.value = cause instanceof Error ? cause.message : "读取背包状态失败";
    }
  }

  onMounted(() => {
    void refresh();
    timer = setInterval(() => {
      if (document.visibilityState === "hidden") return;
      void refresh();
    }, POLL_MS);
  });

  onBeforeUnmount(() => {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  });

  /** 这台设备的前置通道是否在线（板端 relay 连着平台，决定中继能不能看） */
  const relayOnline = computed(() =>
    devices.value
      .find(item => item.id === activeDevice.value)
      ?.streams.includes(FRONT_STREAM) ?? false
  );

  const path = computed(() => {
    // 现场直连只对「部署里唯一的那台」有意义：部署级 `webrtc` 基址只指向一台板子，
    // 多设备时用它「直连」另一台就是看到别的狗的画面（比黑屏更糟）。
    // 单台部署（绝大多数）即使板端自动带上了 SN，也仍然允许直连。
    if (activeDevice.value && devices.value.length > 1) return "relay";
    return frontCameraPath(status.value, relayOnline.value, devicesKnown.value);
  });
  const whepBase = computed(() => status.value?.webrtc.base ?? null);

  /** 直连模式下压在画面中央的可读原因（中继模式的报错由 VideoPlayer 自己显示） */
  const hint = computed(() => {
    const current = status.value;
    if (error.value) return error.value;
    if (!current) return "正在读取背包状态…";
    if (!current.configured) return "";
    if (!current.webrtc.base) return "后端未给出浏览器可访问的背包地址（ROBODOG_BACKPACK_WEBRTC_URL）";
    if (current.mode === "direct" && !current.agent.reachable) {
      return current.agent.error || "连不上背包 Agent：确认板端 Agent 已启动、浏览器与背包同网段";
    }
    return "";
  });

  /** 画面底部的一行说明：现场一眼看出看的是哪条路 */
  const detail = computed(() => {
    if (path.value === "direct") return "现场直连（中继不可用）· 机器狗前置 WHEP · 延时最低";
    if (path.value === "relay") return "平台中继 · 机器狗前置 · 延时 ~300ms";
    return "";
  });

  return { path, whepBase, hint, detail, device: activeDevice };
}
