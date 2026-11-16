<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { tokenStorage } from "@/api/http";
import { wsOrigin } from "@/utils/runtime";
import AppIcon from "./AppIcon.vue";

/**
 * 机器狗实时视频
 *
 * 智航 Pro 协议没有视频接口，视频由本体自带服务提供，后台只存地址与协议类型：
 *
 * - `hls`   → `*.m3u8`，走 hls.js（Safari 原生支持则直连）
 * - `flv`   → `*.flv`，走 flv.js（MSE）
 * - `mp4`   → 原生 `<video>`，也兼容 webm
 * - `webrtc`→ `whep://` 或 `webrtc://`，需要额外信令网关，这里只做提示
 * - `rtsp`  → 浏览器不能直连：由设备客户端拉流转 FLV、经业务平台中继（见 server/src/video.rs）
 * - `relay` → 背包中继：板端推流服务把 RTSP 转成 FLV 推到平台，这里同源播放
 * - `none`  → 未配置，显示占位
 *
 * ## 直播「不许闪」的四条硬规则（改这块之前先读）
 *
 * 1. **缓冲 ≠ 断开**：`waiting` / `stalled` 只置一个「缓冲中」角标，绝不盖全屏错误层。
 *    早期版本一 `waiting` 就把 `连接中` 的半透明层铺满画面，看起来就是一闪一闪。
 * 2. **重连不清画面**：断流时先把当前帧抓成 poster 继续显示，再重建 flv 播放器，
 *    所以重连过程中观众看到的是「上一帧 + 重连中」，不是黑屏闪一下。
 * 3. **快速重连 + 退避**：服务端主动断开（推流端换流 / 观看端追不上）时 250ms 就重连，
 *    连续失败才指数退避（上限 3s），不会 3s 一次空转刷屏。
 * 4. **推流端要保持在跑**：服务端最后一个观看者离开时不立刻杀 ffmpeg，
 *    所以这里的 `video_start` 重复下发也不会重启推流端（客户端按地址去重）。
 */
const props = defineProps<{
  url: string | null;
  type: string;
  /** 设备是否在线，离线时即使配了地址也不拉流 */
  online: boolean;
  /** 设备主键：RTSP 走 `/api/devices/{id}/video/ws` 中继，必须传 */
  deviceId?: number | null;
  /**
   * 背包通道（`cam0`…`dogcam`）：`type="relay"` 时走 `/api/backpack/video/ws`。
   *
   * 这条通路不经过浏览器直连板端，因此不受「换 WiFi / 换浏览器 / LNA 许可」影响。
   */
  stream?: string | null;
}>();

type Status = "idle" | "loading" | "playing" | "retrying" | "error";

const videoRef = ref<HTMLVideoElement | null>(null);
const stageRef = ref<HTMLDivElement | null>(null);
const status = ref<Status>("idle");
const errorText = ref("");
const muted = ref(true);
/** 卡顿时角标显示「缓冲中」，不铺错误层 */
const buffering = ref(false);
/** 断流瞬间抓下来的最后一帧：重连期间继续显示，避免黑屏闪一下 */
const poster = ref("");
const retryAttempt = ref(0);

// hls.js / flv.js 体积大（各 200KB+），只有真正需要播放时才拉进来，
// 避免设备列表页为了一个播放器把整个 bundle 撑大
type HlsInstance = import("hls.js").default;
// flv.js 的类型是全局命名空间 `FlvJs`，没有具名导出 Player
type FlvPlayer = ReturnType<typeof import("flv.js").default.createPlayer>;
let hls: HlsInstance | null = null;
let flv: FlvPlayer | null = null;
/** 每次 attach 自增，异步加载回来时据此判断是否已被新请求取代 */
let attachSeq = 0;
/**
 * 中继断流的重连计时器与次数。
 *
 * 推流端重连、观看端追不上、弱网抖动都会让这条 WS 被关掉；这里自动重连拿干净的
 * 新流（服务端会补流头 + 关键帧），不让用户手动点「重连」。
 */
let retryTimer: ReturnType<typeof setTimeout> | null = null;
/** 首连重连的等待：服务端主动断开时几乎立刻就能拿回干净的新流 */
const RETRY_BASE = 250;
const RETRY_MAX = 3000;
/** 连续失败这么多次才把错误铺在画面上（前几次只是角标提示，不打扰画面） */
const HARD_ERROR_AFTER = 5;
/** 画面停住多久判定「假死」并重连（弱网下 MSE 会卡在 readyState=2 不再前进） */
const STALL_MS = 3000;
/** 一次连接尝试多久没画面算超时（现场首次冷启动 ffmpeg 要 3~7s，留足余量） */
const CONNECT_TIMEOUT_MS = 10_000;
let stallTimer: ReturnType<typeof setInterval> | null = null;
let lastTime = 0;
let lastProgressAt = 0;
let attachStartedAt = 0;

const kind = computed(() => (props.type || "none").toLowerCase());

/**
 * 中继地址：业务平台把推流端回传的 FLV 通过 WS 转给浏览器。
 *
 * - `rtsp`：设备客户端拉 `devices.video_url`，房间名是设备编号；
 * - `relay`：背包板端 relay 拉本地流，房间名是 `bp:<stream>`（服务端前缀固定）。
 *
 * 浏览器 WS 握手不能带 Authorization 头，令牌只能放查询串（服务端 require_auth 支持）。
 */
const relayUrl = computed(() => {
  const token = tokenStorage.get() || "";
  if (kind.value === "relay") {
    if (!props.stream) return "";
    // 页面可能由现场入口托管（内网静态服务），中继仍在云端：用运行时基址
    return `${wsOrigin()}/api/backpack/video/ws?stream=${encodeURIComponent(props.stream)}&token=${encodeURIComponent(token)}`;
  }
  if (!props.deviceId) return "";
  return `${wsOrigin()}/api/devices/${props.deviceId}/video/ws?token=${encodeURIComponent(token)}`;
});

const isRelay = computed(
  () => (kind.value === "rtsp" && !!props.deviceId) || (kind.value === "relay" && !!props.stream)
);

/** 走 MSE / HTTP 拉流的直播通路：断流要自愈（mp4 / webm 是点播，不重连） */
const retryable = computed(() => ["hls", "flv", "rtsp", "relay"].includes(kind.value));

const playable = computed(() => {
  if (!props.online) return false;
  // 背包中继没有「视频地址」这个概念：板端推什么就看什么
  if (kind.value === "relay") return !!relayUrl.value;
  if (!props.url) return false;
  // RTSP 浏览器不能直连，只有拿到中继地址才谈得上播放
  if (kind.value === "rtsp") return isRelay.value;
  return ["hls", "flv", "mp4"].includes(kind.value);
});

/** 全屏错误层只在这些状态出现：不能播 / 连续重连失败 */
const showFallback = computed(() => !playable.value || status.value === "error");

const hint = computed(() => {
  if (!props.online) return "设备离线，无实时画面";
  if (kind.value === "relay") {
    if (!props.stream) return "未选择背包通道";
    if (status.value === "error")
      return errorText.value || "中继拉流失败：确认背包上 backpack-relay 服务在线";
    if (status.value === "loading") return "正在连接平台中继…";
    return "";
  }
  if (!props.url) return "未配置视频地址，请在设备档案中填写";
  if (kind.value === "rtsp" && !props.deviceId)
    return "RTSP 需要经设备客户端回传播放，缺少设备标识";
  if (kind.value === "webrtc") return "WebRTC 需要信令网关，当前未接入";
  if (kind.value === "none") return "视频协议未识别，请在设备档案中选择类型";
  if (status.value === "error") return errorText.value || "视频加载失败";
  return "";
});

const badge = computed(() => {
  if (!playable.value) return "OFFLINE";
  if (status.value === "playing") return buffering.value ? "缓冲中" : "LIVE";
  if (status.value === "retrying") return `重连中 ${retryAttempt.value}`;
  if (status.value === "error") return "连接失败";
  return "连接中";
});

const badgeTone = computed(() => {
  if (!playable.value) return "off";
  if (status.value === "playing") return buffering.value ? "wait" : "live";
  if (status.value === "error") return "err";
  return "wait";
});

function clearRetry() {
  if (retryTimer) {
    clearTimeout(retryTimer);
    retryTimer = null;
  }
}

/** 只在销毁播放器内核，保留 `<video>` 与画面（重连时不留黑屏） */
function teardownPlayer() {
  if (hls) {
    hls.destroy();
    hls = null;
  }
  if (flv) {
    flv.destroy();
    flv = null;
  }
}

/** 抓当前帧当 poster：重连期间继续显示「最后一帧」而不是黑屏 */
function capturePoster() {
  const el = videoRef.value;
  if (!el || !el.videoWidth) return;
  try {
    const canvas = document.createElement("canvas");
    const width = 320;
    canvas.width = width;
    canvas.height = Math.max(1, Math.round((el.videoHeight / el.videoWidth) * width));
    canvas.getContext("2d")?.drawImage(el, 0, 0, canvas.width, canvas.height);
    // 小图 + jpeg：一次 ~2ms，只在断流瞬间做一次
    poster.value = canvas.toDataURL("image/jpeg", 0.5);
  } catch {
    /* 取不到帧就算了，重连本身不依赖它 */
  }
}

/** 断流：先留帧，再按退避重连；前几次不弹错误，只改角标 */
function scheduleRetry(reason: string) {
  if (!retryable.value || !playable.value) return;
  if (retryTimer) return;
  // 作废可能还在异步加载 / 建连中的那一次尝试，避免两套播放器抢同一个 <video>
  attachSeq += 1;
  capturePoster();
  teardownPlayer();
  retryAttempt.value += 1;
  buffering.value = false;
  errorText.value = reason;
  const delay = Math.min(RETRY_BASE * 2 ** (retryAttempt.value - 1), RETRY_MAX);
  if (retryAttempt.value >= HARD_ERROR_AFTER) {
    status.value = "error";
  } else {
    status.value = "retrying";
  }
  retryTimer = setTimeout(() => {
    retryTimer = null;
    void attach();
  }, delay);
}

function destroy() {
  clearRetry();
  teardownPlayer();
  const el = videoRef.value;
  if (el) {
    el.pause();
    el.removeAttribute("src");
    el.load();
  }
  status.value = "idle";
  errorText.value = "";
  poster.value = "";
  buffering.value = false;
  retryAttempt.value = 0;
  lastTime = 0;
  lastProgressAt = 0;
  attachStartedAt = 0;
}

function fail(message: string) {
  status.value = "error";
  errorText.value = message;
}

/** flv.js 的直播参数：关掉 stash 缓冲，吃多少播多少（默认攒 3s 才起播，抖动就来回跳） */
function liveFlvConfig(url: string) {
  return {
    type: "flv" as const,
    url,
    isLive: true,
    hasAudio: false,
    lazyLoad: false,
    enableStashBuffer: false,
    stashInitialSize: 128,
    autoCleanupSourceBuffer: true,
    fixAudioTimestampGap: false
  };
}

async function attach() {
  const el = videoRef.value;
  if (!el || !playable.value) return;
  clearRetry();
  teardownPlayer();
  const src = props.url ?? "";
  const seq = ++attachSeq;
  attachStartedAt = performance.now();
  // 重连途中不要退回「连接中」：角标要一直显示重连次数
  if (status.value !== "retrying") {
    status.value = "loading";
    errorText.value = "";
  }
  buffering.value = false;

  if (kind.value === "hls") {
    // Safari / iOS 原生支持 m3u8，省掉 hls.js
    if (el.canPlayType("application/vnd.apple.mpegurl")) {
      el.src = src;
      el.play().catch(() => fail("自动播放被浏览器拦截，请点击播放"));
      return;
    }
    const { default: Hls } = await import("hls.js");
    if (seq !== attachSeq) return;
    if (!Hls.isSupported()) {
      fail("当前浏览器不支持 HLS 播放");
      return;
    }
    hls = new Hls({ lowLatencyMode: true, liveSyncDurationCount: 2 });
    hls.loadSource(src);
    hls.attachMedia(el);
    hls.on(Hls.Events.MANIFEST_PARSED, () => {
      el.play().catch(() => fail("自动播放被浏览器拦截，请点击播放"));
    });
    hls.on(Hls.Events.ERROR, (_event, data) => {
      if (!data.fatal) return;
      scheduleRetry(`HLS 播放失败：${data.details}`);
    });
    return;
  }

  if (kind.value === "rtsp" || kind.value === "relay" || kind.value === "flv") {
    // 推流端已把源转成 FLV，flv.js 的 WebSocketLoader 直接消费二进制帧
    const { default: flvjs } = await import("flv.js");
    if (seq !== attachSeq) return;
    const target = kind.value === "flv" ? src : relayUrl.value;
    if (!flvjs.isSupported() || !target) {
      fail(
        kind.value === "flv"
          ? "当前浏览器不支持 FLV 播放"
          : "当前浏览器不支持实时视频播放"
      );
      return;
    }
    const player = flvjs.createPlayer(liveFlvConfig(target));
    flv = player;
    player.attachMediaElement(el);
    player.load();
    player.play()?.catch(() => undefined);
    player.on(flvjs.Events.ERROR, (_type: string, detail: string) => {
      // 已销毁 / 已被新播放器取代的旧实例的迟到事件不能搅乱新流
      if (flv !== player) return;
      scheduleRetry(
        kind.value === "relay"
          ? `中继断流（${detail || "unknown"}）：确认背包上 backpack-relay 服务在线`
          : `实时视频断流（${detail || "unknown"}）：确认设备客户端在线且能访问 RTSP 地址`
      );
    });
    return;
  }

  // mp4 / webm 走原生
  el.src = src;
  el.play().catch(() => fail("自动播放被浏览器拦截，请点击播放"));
}

/* ---------------- 播放状态 ---------------- */

function onPlaying() {
  status.value = "playing";
  buffering.value = false;
  errorText.value = "";
  retryAttempt.value = 0;
  poster.value = "";
  lastTime = videoRef.value?.currentTime ?? 0;
  lastProgressAt = performance.now();
}

function onWaiting() {
  // 直播偶发缓冲是常态：给个角标，别盖错误层
  if (status.value === "playing") buffering.value = true;
}

/** `<video>` 自身的 error：装/卸播放器时也会冒（src 被清掉），只在真正播放中才算断流 */
function onMediaError() {
  if (status.value === "idle" || status.value === "retrying") return;
  if (retryable.value) scheduleRetry("视频源不可达");
  else fail("视频源不可达");
}

/** MSE 「假死」看门狗：playing 却连续 3s 时间轴不前进就自己重连；一直连不上也算超时 */
function tickStall() {
  const el = videoRef.value;
  if (!el || !playable.value) {
    lastProgressAt = performance.now();
    return;
  }
  if (status.value === "loading" || status.value === "retrying") {
    if (attachStartedAt && performance.now() - attachStartedAt > CONNECT_TIMEOUT_MS) {
      scheduleRetry("连接超时，正在重新拉流");
    }
    return;
  }
  if (status.value !== "playing") {
    lastProgressAt = performance.now();
    return;
  }
  if (el.currentTime !== lastTime) {
    lastTime = el.currentTime;
    lastProgressAt = performance.now();
    if (el.readyState >= 3) buffering.value = false;
    return;
  }
  if (performance.now() - lastProgressAt > STALL_MS) {
    scheduleRetry("画面停住，正在重新拉流");
  }
}

/** 切回标签页时视频可能已被浏览器冻结：立刻检查一次，别等 3s 看门狗 */
function onVisibility() {
  if (document.visibilityState === "visible") tickStall();
}

function toggleMute() {
  muted.value = !muted.value;
  if (videoRef.value) videoRef.value.muted = muted.value;
}

async function enterFullscreen() {
  const el = stageRef.value;
  if (!el) return;
  if (document.fullscreenElement) {
    await document.exitFullscreen();
  } else {
    await el.requestFullscreen?.();
  }
}

watch(
  () => [props.url, props.type, props.online, props.deviceId, props.stream],
  () => {
    // 设备 / 地址变化视为全新播放：清掉重连状态
    retryAttempt.value = 0;
    destroy();
    // 等 DOM 更新完再挂载播放器
    requestAnimationFrame(() => {
      void attach();
    });
  },
  { immediate: true }
);

onMounted(() => {
  document.addEventListener("visibilitychange", onVisibility);
  stallTimer = setInterval(tickStall, 1000);
});

onBeforeUnmount(() => {
  document.removeEventListener("visibilitychange", onVisibility);
  if (stallTimer) {
    clearInterval(stallTimer);
    stallTimer = null;
  }
  destroy();
});
</script>

<template>
  <div ref="stageRef" class="video-stage">
    <video
      ref="videoRef"
      class="video-el"
      :class="{ on: status === 'playing' }"
      :muted="muted"
      playsinline
      autoplay
      @playing="onPlaying"
      @waiting="onWaiting"
      @stalled="onWaiting"
      @error="onMediaError"
    />

    <!-- 断流重连期间继续显示最后一帧，避免「黑一下再闪回来」 -->
    <img v-if="poster && status !== 'playing'" class="video-poster" :src="poster" alt="" />

    <div v-if="showFallback" class="video-fallback">
      <AppIcon name="video" />
      <p>{{ hint }}</p>
      <button v-if="playable && status === 'error'" class="btn sm" @click="attach">
        <AppIcon name="refresh" />重连
      </button>
    </div>

    <div class="video-top">
      <span class="video-badge" :class="badgeTone"><i />{{ badge }}</span>
      <span v-if="playable" class="video-proto">{{ kind.toUpperCase() }}</span>
    </div>

    <div v-if="playable" class="video-bottom">
      <button class="video-btn" :title="muted ? '取消静音' : '静音'" @click="toggleMute">
        <AppIcon name="sound" :style="{ opacity: muted ? 0.45 : 1 }" />
      </button>
      <button class="video-btn" title="全屏" @click="enterFullscreen">
        <AppIcon name="maximize" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.video-stage {
  position: relative;
  width: 100%;
  aspect-ratio: 16 / 9;
  border-radius: 10px;
  overflow: hidden;
  background: #0b1220;
  border: 1px solid var(--line);
}

.video-el {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
  opacity: 0;
  transition: opacity 0.2s;
}

.video-el.on {
  opacity: 1;
}

/* 重连期间垫在底下的最后一帧：和 video 同样 object-fit，切换不跳位 */
.video-poster {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.video-fallback {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: #94a3b8;
  padding: 12px;
  text-align: center;
  background: rgb(11 18 32 / 72%);
}

.video-fallback p {
  font-size: 12px;
  line-height: 1.6;
  margin: 0;
  max-width: 80%;
}

.video-top {
  position: absolute;
  top: 8px;
  left: 8px;
  right: 8px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  pointer-events: none;
}

.video-badge {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.06em;
  padding: 3px 7px;
  border-radius: 4px;
  background: rgb(15 23 42 / 72%);
}

.video-badge i {
  width: 6px;
  height: 6px;
  border-radius: 50%;
}

.video-badge.live {
  color: #f87171;
}

.video-badge.live i {
  background: #ef4444;
  animation: pulse 1.4s infinite;
}

.video-badge.wait {
  color: #fbbf24;
}

.video-badge.wait i {
  background: #f59e0b;
}

.video-badge.err {
  color: #fca5a5;
}

.video-badge.err i {
  background: #ef4444;
}

.video-badge.off {
  color: #94a3b8;
}

.video-badge.off i {
  background: #64748b;
}

.video-proto {
  font-size: 10px;
  color: #cbd5f5;
  background: rgb(15 23 42 / 72%);
  padding: 3px 6px;
  border-radius: 4px;
  font-family: var(--mono, monospace);
}

.video-bottom {
  position: absolute;
  right: 8px;
  bottom: 8px;
  display: flex;
  gap: 6px;
}

.video-btn {
  width: 26px;
  height: 26px;
  display: grid;
  place-items: center;
  border-radius: 6px;
  border: none;
  cursor: pointer;
  color: #e2e8f0;
  background: rgb(15 23 42 / 72%);
}

.video-btn:hover {
  background: rgb(15 23 42 / 92%);
}

@keyframes pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
}
</style>
