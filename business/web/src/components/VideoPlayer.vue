<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
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
 * - `rtsp`  → 浏览器无法直接播放，需服务端转 HLS/WebRTC，只做提示
 * - `none`  → 未配置，显示占位
 */
const props = defineProps<{
  url: string | null;
  type: string;
  /** 设备是否在线，离线时即使配了地址也不拉流 */
  online: boolean;
}>();

const videoRef = ref<HTMLVideoElement | null>(null);
const stageRef = ref<HTMLDivElement | null>(null);
const status = ref<"idle" | "loading" | "playing" | "error">("idle");
const errorText = ref("");
const muted = ref(true);

// hls.js / flv.js 体积大（各 200KB+），只有真正需要播放时才拉进来，
// 避免设备列表页为了一个播放器把整个 bundle 撑大
type HlsInstance = import("hls.js").default;
// flv.js 的类型是全局命名空间 `FlvJs`，没有具名导出 Player
type FlvPlayer = ReturnType<typeof import("flv.js").default.createPlayer>;
let hls: HlsInstance | null = null;
let flv: FlvPlayer | null = null;
/** 每次 attach 自增，异步加载回来时据此判断是否已被新请求取代 */
let attachSeq = 0;

const kind = computed(() => (props.type || "none").toLowerCase());

const playable = computed(
  () => props.online && !!props.url && ["hls", "flv", "mp4"].includes(kind.value)
);

const hint = computed(() => {
  if (!props.online) return "设备离线，无实时画面";
  if (!props.url) return "未配置视频地址，请在设备档案中填写";
  if (kind.value === "rtsp")
    return "RTSP 流浏览器无法直接播放，需服务端转成 HLS / WebRTC";
  if (kind.value === "webrtc") return "WebRTC 需要信令网关，当前未接入";
  if (kind.value === "none") return "视频协议未识别，请在设备档案中选择类型";
  if (status.value === "error") return errorText.value || "视频加载失败";
  return "";
});

const badge = computed(() => {
  if (!playable.value) return "OFFLINE";
  if (status.value === "playing") return "LIVE";
  if (status.value === "loading") return "连接中";
  return "待连接";
});

const badgeTone = computed(() => {
  if (!playable.value) return "off";
  if (status.value === "playing") return "live";
  return "wait";
});

function destroy() {
  if (hls) {
    hls.destroy();
    hls = null;
  }
  if (flv) {
    flv.destroy();
    flv = null;
  }
  const el = videoRef.value;
  if (el) {
    el.pause();
    el.removeAttribute("src");
    el.load();
  }
  status.value = "idle";
  errorText.value = "";
}

function fail(message: string) {
  status.value = "error";
  errorText.value = message;
}

async function attach() {
  destroy();
  const el = videoRef.value;
  if (!el || !playable.value || !props.url) return;
  const src = props.url;
  const seq = ++attachSeq;
  status.value = "loading";

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
      if (data.fatal) fail(`HLS 播放失败：${data.details}`);
    });
    return;
  }

  if (kind.value === "flv") {
    const { default: flvjs } = await import("flv.js");
    if (seq !== attachSeq) return;
    if (!flvjs.isSupported()) {
      fail("当前浏览器不支持 FLV 播放");
      return;
    }
    flv = flvjs.createPlayer({ type: "flv", url: src, isLive: true });
    flv.attachMediaElement(el);
    flv.load();
    flv.play()?.catch(() => fail("自动播放被浏览器拦截，请点击播放"));
    flv.on(flvjs.Events.ERROR, () => fail("FLV 拉流失败，检查地址与鉴权"));
    return;
  }

  // mp4 / webm 走原生
  el.src = src;
  el.play().catch(() => fail("自动播放被浏览器拦截，请点击播放"));
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
  () => [props.url, props.type, props.online],
  () => {
    // 等 DOM 更新完再挂载播放器
    requestAnimationFrame(() => {
      void attach();
    });
  },
  { immediate: true }
);

onBeforeUnmount(destroy);
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
      @playing="status = 'playing'"
      @waiting="status = 'loading'"
      @error="fail('视频源不可达')"
    />

    <div v-if="!playable || status !== 'playing'" class="video-fallback">
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
