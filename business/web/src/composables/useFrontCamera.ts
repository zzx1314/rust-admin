/**
 * 机器狗前置摄像头（背包 `dogcam`）——「实时视频」页默认那一路。
 *
 * 设备管理的控制抽屉、计划管理的实时监控都用它，保证三处的画面**同源同路**：
 * 板端 relay 把本体前置 RTSP 归一化成 `dogcam`，**默认走平台中继**（FLV/WS）。
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
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import {
  backpackApi,
  loadStatus,
  type BackpackStatus,
  type BackpackVideoRoom
} from "@/api/backpack";
import { isPrivateOrigin } from "@/utils/runtime";

/** 前置通道在 mediamtx / 平台中继里的固定名字（与板端 `mediamtx.yml` 一致） */
export const FRONT_STREAM = "dogcam";
/** 平台中继的房间名前缀（与服务端 `video.rs::BACKPACK_ROOM_PREFIX` 一致） */
const RELAY_ROOM_PREFIX = "bp:";

/** 多个组件共享一份 status：15s 内的重复调用直接用缓存 */
const SHARE_TTL_MS = 15_000;
/** 打开页面期间的低频刷新：换网 / 板端重启后不用手动刷新 */
const POLL_MS = 20_000;

let shared: { at: number; data: BackpackStatus } | null = null;
let pending: Promise<BackpackStatus> | null = null;
/** 中继房间快照（同样共享缓存）：判断板端 relay 是否在线 */
let sharedRooms: { at: number; rooms: BackpackVideoRoom[] } | null = null;
let pendingRooms: Promise<BackpackVideoRoom[]> | null = null;

async function sharedStatus(force = false): Promise<BackpackStatus> {
  if (!force && shared && Date.now() - shared.at < SHARE_TTL_MS) return shared.data;
  if (!pending) {
    pending = loadStatus()
      .then(data => {
        shared = { at: Date.now(), data };
        return data;
      })
      .finally(() => {
        pending = null;
      });
  }
  return pending;
}

async function sharedRelayRooms(force = false): Promise<BackpackVideoRoom[]> {
  if (!force && sharedRooms && Date.now() - sharedRooms.at < SHARE_TTL_MS) {
    return sharedRooms.rooms;
  }
  if (!pendingRooms) {
    pendingRooms = backpackApi
      .videoState()
      .then(data => {
        const rooms = data.rooms ?? [];
        sharedRooms = { at: Date.now(), rooms };
        return rooms;
      })
      .finally(() => {
        pendingRooms = null;
      });
  }
  return pendingRooms;
}

/**
 * 走哪条路（设备管理 / 计划管理固定「中继优先」）：
 * - `relay`：板端推流 → 平台房间 → 同源 WS（FLV）—— **默认**，与「实时视频」的
 *   「平台中继」同一条通路，任何网络都能看；
 * - `direct`：中继推流端不在时的兜底，浏览器 ⇄ 板端 mediamtx（WHEP，延时最低）；
 * - `none`：本部署没接背包，前置这一路不存在。
 *
 * 以前这里和 `/monitor` 一样「直连优先」，公网页面/探不通才切中继，结果设备抽屉、
 * 任务监控默认都在直连；现场要的是**统一走中继**（板端 WHEP 一抖就黑），所以改成中继优先。
 */
export type FrontCameraPath = "direct" | "relay" | "none";

export function frontCameraPath(
  status: BackpackStatus | null,
  relayOnline: boolean
): FrontCameraPath {
  if (!status?.configured) return "none";
  if (relayOnline) return "relay";
  // 中继推流端不在：现场入口 / 内网页面还有直连可用（公网页面只能等中继恢复）
  if (status.mode === "direct") {
    return status.agent.reachable && status.webrtc.base ? "direct" : "relay";
  }
  return isPrivateOrigin() && status.webrtc.base ? "direct" : "relay";
}

export function useFrontCamera() {
  const status = ref<BackpackStatus | null>(null);
  const rooms = ref<BackpackVideoRoom[]>([]);
  const error = ref("");
  let timer: ReturnType<typeof setInterval> | null = null;

  async function refresh(force = false) {
    const [statusResult, roomsResult] = await Promise.allSettled([
      sharedStatus(force),
      sharedRelayRooms(force)
    ]);
    if (statusResult.status === "fulfilled") {
      status.value = statusResult.value;
      error.value = "";
    } else {
      error.value =
        statusResult.reason instanceof Error
          ? statusResult.reason.message
          : "读取背包状态失败";
    }
    // 房间快照失败不影响主流程：默认仍按中继走，真正的报错由播放器显示
    rooms.value = roomsResult.status === "fulfilled" ? roomsResult.value : [];
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

  /** 板端 relay 是否连着平台（决定中继能不能看） */
  const relayOnline = computed(() =>
    rooms.value.some(
      room => room.room === `${RELAY_ROOM_PREFIX}${FRONT_STREAM}` && room.publisher
    )
  );

  const path = computed(() => frontCameraPath(status.value, relayOnline.value));
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
    if (path.value === "relay") return "平台中继 · 机器狗前置 · 延时 1~2s";
    return "";
  });

  return { path, whepBase, hint, detail };
}
