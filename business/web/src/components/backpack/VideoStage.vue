<script setup lang="ts">
/**
 * 单路视频舞台：WHEP 播放 + 叠加层 + 基础操作。
 *
 * 只负责「把一路流放出来」：拉流由 `lib/whep.ts` 完成，框由父组件通过 `boxes`
 * 传进来（坐标是抽帧原图像素）。坐标换算按 `object-fit: cover` 反算 ——
 * 与 Demo 的 `app.js` 同一套公式，否则窗口比例变化时框会整体偏移。
 *
 * 画框方式为「背包烧录（OSD）」时，父组件会把 `stream` 换成 preview 流并把
 * `showBoxes` 置 false：那时框已经在视频像素里了，再叠一层会双重画框。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { startWhep, type WhepPhase, type WhepSession } from "@/lib/whep";
import type { BackpackBox } from "@/api/backpack";
import AppIcon from "@/components/AppIcon.vue";

const props = defineProps<{
  /** 浏览器直连的 WHEP 基址（板端 mediamtx），null 表示后端未配置 */
  whepBase: string | null;
  /** mediamtx path */
  stream: string;
  title: string;
  /** 是否在 AI 识别中（画角标 + 「AI」徽标） */
  aiActive: boolean;
  /** AI 徽标文案：推理帧率 / 检出数量 */
  aiBadge: string;
  /** 是否叠加浏览器画框 */
  showBoxes: boolean;
  boxes: BackpackBox[];
  frameW: number;
  frameH: number;
  /** 覆盖在画面中央的提示（无源 / 未配置 / 拉流失败） */
  hint: string;
  /** 底部一行说明（画框方式 / 延时提示） */
  detail: string;
}>();

const emit = defineEmits<{
  (event: "phase", payload: { phase: WhepPhase; message: string }): void;
}>();

interface MappedBox {
  x: number;
  y: number;
  w: number;
  h: number;
  color: string;
  text: string;
}

const root = ref<HTMLDivElement | null>(null);
const videoEl = ref<HTMLVideoElement | null>(null);
const phase = ref<WhepPhase>("connecting");
const phaseMessage = ref("");
const fullscreen = ref(false);
const shown = ref<MappedBox[]>([]);
/** SVG 的 viewBox 必须跟着视频显示尺寸走：框坐标是在显示空间里算出来的 */
const view = ref({ w: 16, h: 9 });

let session: WhepSession | null = null;
let resizeObserver: ResizeObserver | null = null;
let raf = 0;

const reducedMotion = (): boolean =>
  typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

const live = computed(() => phase.value === "live");
const connectionText = computed(() => {
  switch (phase.value) {
    case "live":
      return "LIVE";
    case "retry":
      return phaseMessage.value || "重连中";
    case "closed":
      return "已停止";
    default:
      return "连接中";
  }
});

/* ---------------- WHEP 会话 ---------------- */

function whepUrl(): string | null {
  const base = props.whepBase;
  if (!base || !props.stream) return null;
  return `${base.replace(/\/+$/, "")}/${encodeURIComponent(props.stream)}/whep`;
}

function restart() {
  session?.close();
  session = null;
  phase.value = "connecting";
  phaseMessage.value = "";
  const url = whepUrl();
  if (!url || !videoEl.value) return;
  session = startWhep({
    url,
    video: videoEl.value,
    onPhase: detail => {
      phase.value = detail.phase;
      phaseMessage.value = detail.message;
      emit("phase", { phase: detail.phase, message: detail.message });
    }
  });
}

watch(() => [props.whepBase, props.stream], restart);

/* ---------------- 画框叠加（cover 反算 + 缓动） ---------------- */

let rawBoxes: BackpackBox[] = [];
let fromBoxes: MappedBox[] = [];
let toBoxes: MappedBox[] = [];
let animStart = 0;

function mapBoxes(): MappedBox[] {
  const element = videoEl.value;
  if (!element) return [];
  const width = element.clientWidth;
  const height = element.clientHeight;
  if (!width || !height) return [];
  const frameW = props.frameW || 1280;
  const frameH = props.frameH || 720;
  const scale = Math.max(width / frameW, height / frameH);
  const offsetX = (width - frameW * scale) / 2;
  const offsetY = (height - frameH * scale) / 2;
  view.value = { w: width, h: height };
  return rawBoxes.map(box => ({
    x: box.x1 * scale + offsetX,
    y: box.y1 * scale + offsetY,
    w: (box.x2 - box.x1) * scale,
    h: (box.y2 - box.y1) * scale,
    color: /^#[0-9a-fA-F]{6}$/.test(box.color || "") ? box.color : "#e24b4a",
    text: box.text
  }));
}

function step() {
  raf = 0;
  if (!toBoxes.length) {
    shown.value = [];
    return;
  }
  const duration = reducedMotion() ? 0 : 200;
  const t = duration ? Math.min(1, (performance.now() - animStart) / duration) : 1;
  // ease-out：6Hz 的推理结果之间要「滑」过去，看着才不像卡住
  const eased = 1 - (1 - t) ** 3;
  shown.value = toBoxes.map((box, index) => {
    const start = fromBoxes[index] && fromBoxes[index].text === box.text ? fromBoxes[index] : box;
    return {
      ...box,
      x: start.x + (box.x - start.x) * eased,
      y: start.y + (box.y - start.y) * eased,
      w: start.w + (box.w - start.w) * eased,
      h: start.h + (box.h - start.h) * eased
    };
  });
  if (t < 1) raf = requestAnimationFrame(step);
}

watch(
  () => props.boxes,
  next => {
    rawBoxes = next;
    if (!props.showBoxes || !next.length) {
      rawBoxes = [];
      toBoxes = [];
      fromBoxes = [];
      shown.value = [];
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      return;
    }
    fromBoxes = shown.value.length === next.length ? shown.value.map(box => ({ ...box })) : mapBoxes();
    toBoxes = mapBoxes();
    animStart = performance.now();
    if (!raf) raf = requestAnimationFrame(step);
  }
);

watch(
  () => props.showBoxes,
  visible => {
    if (!visible) {
      shown.value = [];
      rawBoxes = [];
      toBoxes = [];
    } else {
      rawBoxes = props.boxes;
      toBoxes = mapBoxes();
      fromBoxes = toBoxes.map(box => ({ ...box }));
      animStart = 0;
      if (!raf) raf = requestAnimationFrame(step);
    }
  }
);

/* ---------------- 交互 ---------------- */

async function toggleFullscreen() {
  if (!root.value) return;
  try {
    if (document.fullscreenElement) await document.exitFullscreen();
    else await root.value.requestFullscreen();
  } catch {
    /* 浏览器拒绝（iframe / 权限策略）时保持原样，不弹错 */
  }
}

function onFullscreenChange() {
  fullscreen.value = !!document.fullscreenElement;
}

/** 抓拍当前画面（WebRTC 流不受跨域限制，canvas 可直接导出） */
function snapshot() {
  const element = videoEl.value;
  if (!element || !element.videoWidth) return;
  const canvas = document.createElement("canvas");
  canvas.width = element.videoWidth;
  canvas.height = element.videoHeight;
  const context = canvas.getContext("2d");
  if (!context) return;
  context.drawImage(element, 0, 0, canvas.width, canvas.height);
  const link = document.createElement("a");
  const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  link.href = canvas.toDataURL("image/png");
  link.download = `${props.stream}-${stamp}.png`;
  link.click();
}

onMounted(() => {
  document.addEventListener("fullscreenchange", onFullscreenChange);
  if (videoEl.value) {
    resizeObserver = new ResizeObserver(() => {
      const element = videoEl.value;
      if (element) view.value = { w: element.clientWidth, h: element.clientHeight };
      if (!props.showBoxes || !rawBoxes.length) return;
      toBoxes = mapBoxes();
      fromBoxes = toBoxes.map(box => ({ ...box }));
      animStart = 0;
      if (!raf) raf = requestAnimationFrame(step);
    });
    resizeObserver.observe(videoEl.value);
  }
  if (videoEl.value) {
    view.value = { w: videoEl.value.clientWidth, h: videoEl.value.clientHeight };
  }
  restart();
});

onBeforeUnmount(() => {
  document.removeEventListener("fullscreenchange", onFullscreenChange);
  session?.close();
  session = null;
  resizeObserver?.disconnect();
  if (raf) cancelAnimationFrame(raf);
});

defineExpose({ toggleFullscreen, snapshot, fullscreen });
</script>

<template>
  <div ref="root" class="stage" :class="{ 'stage-ai': aiActive }">
    <video ref="videoEl" class="stage-video" autoplay muted playsinline />

    <svg
      v-if="showBoxes"
      class="stage-boxes"
      :viewBox="`0 0 ${view.w} ${view.h}`"
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <g v-for="(box, index) in shown" :key="`${box.text}-${index}`">
        <rect
          class="box-rect"
          :x="box.x"
          :y="box.y"
          :width="box.w"
          :height="box.h"
          :stroke="box.color"
        />
        <g :transform="`translate(${box.x}, ${Math.max(box.y, 20)})`">
          <rect
            class="box-label-bg"
            x="0"
            :y="-19"
            :width="Math.max(38, box.text.length * 13 + 12)"
            height="19"
            rx="4"
            :fill="box.color"
          />
          <text class="box-label" x="6" y="-5" fill="#fff">{{ box.text }}</text>
        </g>
      </g>
    </svg>

    <!-- 无画面时的说明：不用黑框，直接写清原因与恢复方式 -->
    <div v-if="hint" class="stage-hint">
      <AppIcon name="video" :size="26" />
      <p>{{ hint }}</p>
    </div>

    <div class="stage-top">
      <span class="stage-title">
        <AppIcon :name="aiActive ? 'scan' : 'camera'" :size="13" />
        {{ title }}
      </span>
      <span v-if="aiActive" class="stage-ai-badge">{{ aiBadge }}</span>
      <span class="stage-live" :class="{ on: live }">
        <i />{{ connectionText }}
      </span>
    </div>

    <div class="stage-bottom">
      <span class="stage-detail">{{ detail }}</span>
      <div class="stage-actions">
        <button
          class="stage-btn"
          type="button"
          title="抓拍当前画面"
          aria-label="抓拍当前画面"
          :disabled="!live"
          @click="snapshot"
        >
          <AppIcon name="camera" :size="14" />
        </button>
        <button
          class="stage-btn"
          type="button"
          :title="fullscreen ? '退出全屏' : '全屏观看'"
          :aria-label="fullscreen ? '退出全屏' : '全屏观看'"
          @click="toggleFullscreen"
        >
          <AppIcon name="maximize" :size="14" />
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.stage {
  position: relative;
  aspect-ratio: 16 / 9;
  border-radius: var(--r);
  overflow: hidden;
  background:
    radial-gradient(120% 120% at 20% 0%, #16233a 0%, #0b1220 55%, #080e18 100%);
  border: 1px solid #16233a;
  isolation: isolate;
}

.stage-ai {
  /* AI 识别中：顶部一条冷光，和右侧面板的「运行中」状态呼应 */
  box-shadow:
    0 0 0 1px rgba(45, 212, 191, 0.28),
    0 10px 32px rgba(2, 132, 199, 0.12);
}

.stage-video {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
  background: transparent;
}

.stage-boxes {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  pointer-events: none;
}

.stage-boxes .box-rect {
  fill: none;
  stroke-width: 2;
  vector-effect: non-scaling-stroke;
}

.stage-boxes text.box-label {
  font-size: 12px;
  font-weight: 600;
  font-family: inherit;
  dominant-baseline: alphabetic;
}

.stage-hint {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 24px;
  text-align: center;
  color: #b6c6dc;
  background: rgba(8, 14, 24, 0.55);
  backdrop-filter: blur(2px);
}

.stage-hint p {
  margin: 0;
  max-width: 480px;
  font-size: 12.5px;
  line-height: 1.7;
}

.stage-top {
  position: absolute;
  top: 10px;
  left: 12px;
  right: 12px;
  display: flex;
  align-items: center;
  gap: 8px;
  z-index: 2;
}

.stage-title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border-radius: 8px;
  background: rgba(8, 14, 24, 0.6);
  border: 1px solid rgba(148, 197, 255, 0.18);
  color: #e8f1ff;
  font-size: 12px;
  font-weight: 600;
  backdrop-filter: blur(6px);
}

.stage-ai-badge {
  padding: 4px 9px;
  border-radius: 8px;
  font-size: 11px;
  font-weight: 600;
  color: #7ff0e0;
  background: rgba(13, 148, 136, 0.22);
  border: 1px solid rgba(45, 212, 191, 0.35);
}

.stage-live {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border-radius: 8px;
  font-size: 11px;
  font-weight: 600;
  color: #cbd5e1;
  background: rgba(8, 14, 24, 0.6);
  border: 1px solid rgba(148, 163, 184, 0.2);
  backdrop-filter: blur(6px);
  max-width: 52%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.stage-live i {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #94a3b8;
  flex: none;
}

.stage-live.on {
  color: #bbf7d0;
  border-color: rgba(74, 222, 128, 0.35);
  background: rgba(6, 78, 59, 0.55);
}

.stage-live.on i {
  background: #4ade80;
  box-shadow: 0 0 0 0 rgba(74, 222, 128, 0.65);
  animation: live-pulse 2s infinite;
}

@keyframes live-pulse {
  70% {
    box-shadow: 0 0 0 7px rgba(74, 222, 128, 0);
  }
  100% {
    box-shadow: 0 0 0 0 rgba(74, 222, 128, 0);
  }
}

.stage-bottom {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 22px 12px 10px;
  background: linear-gradient(180deg, transparent, rgba(4, 9, 17, 0.78));
  z-index: 2;
}

.stage-detail {
  font-size: 11.5px;
  color: #9fb3cc;
  line-height: 1.5;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.stage-actions {
  margin-left: auto;
  display: flex;
  gap: 6px;
  flex: none;
}

.stage-btn {
  width: 30px;
  height: 30px;
  display: grid;
  place-items: center;
  border-radius: 8px;
  border: 1px solid rgba(148, 163, 184, 0.24);
  background: rgba(8, 14, 24, 0.55);
  color: #cbd5e1;
  cursor: pointer;
  transition: background 0.15s, color 0.15s, border-color 0.15s;
}

.stage-btn:hover:not(:disabled) {
  background: rgba(2, 132, 199, 0.28);
  border-color: rgba(56, 189, 248, 0.5);
  color: #e0f2fe;
}

.stage-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.stage-btn:focus-visible {
  outline: 2px solid #38bdf8;
  outline-offset: 2px;
}

/* 全屏时铺满，不再受 16:9 约束 */
.stage:fullscreen {
  aspect-ratio: auto;
  height: 100vh;
  border-radius: 0;
}

@media (prefers-reduced-motion: reduce) {
  .stage-live.on i {
    animation: none;
  }
}
</style>
