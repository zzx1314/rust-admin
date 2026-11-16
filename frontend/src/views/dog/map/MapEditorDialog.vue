<script setup lang="ts">
/**
 * MapEditorDialog - 地图预览 / 橡皮擦编辑
 *
 * 预览：后端 `GET /api/dogMap/{id}/image` 优先把包内 map_edit/map.pgm（遥控器
 * 编辑后的地图）转成 PNG，没有编辑版时才用 model.pgm / map.pgm，
 * 这里按栅格原始像素尺寸画到 canvas 上，支持缩放、拖动浏览。
 *
 * 编辑：橡皮擦按栅格像素坐标记录笔画，提交 `POST /api/dogMap/{id}/edit`，
 * 后端把编辑版栅格的这些像素改写为 254（可通行，默认）或 205（未知）并重算
 * map_hash；根目录的建图原始 map.pgm 不会被改。
 * 已发布 / 已归档版本不可编辑，需要先派生新版本。
 */
import { computed, nextTick, ref } from "vue";
import { ElMessageBox } from "element-plus";
import {
  dogMapErase,
  dogMapGetImage,
  dogMapPushToDevice,
  type MapEditStroke
} from "@/api/dogMap";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import EraserIcon from "~icons/ri/eraser-line";
import HandIcon from "~icons/ri/hand";
import ZoomIn from "~icons/ri/zoom-in-line";
import ZoomOut from "~icons/ri/zoom-out-line";
import Fullscreen from "~icons/ri/fullscreen-line";
import UndoIcon from "~icons/ri/arrow-go-back-line";
import ResetIcon from "~icons/ri/restart-line";

defineOptions({ name: "MapEditorDialog" });

const emit = defineEmits<{ saved: [row: any]; pushed: [row: any] }>();

type Tool = "pan" | "eraser";

/** 可编辑的地图状态（与后端 DogMapService::erase 保持一致） */
const EDITABLE_STATUS = ["草稿", "部分标定"];

const visible = ref(false);
const loading = ref(false);
const saving = ref(false);
const pushing = ref(false);
const currentMap = ref<any>(null);
const meta = ref({
  resolution: 0,
  origin: [] as number[],
  width: 0,
  height: 0
});
const tool = ref<Tool>("pan");
const brushRadius = ref(8);
const eraseValue = ref(254);
const zoom = ref(1);
const dirty = ref(false);
const cursor = ref({ x: 0, y: 0, visible: false });

const canvasRef = ref<HTMLCanvasElement>();
const viewportRef = ref<HTMLDivElement>();

let ctx: CanvasRenderingContext2D | null = null;
let baseImage: HTMLImageElement | null = null;
/** 已提交但尚未保存的笔画（栅格像素坐标） */
let strokes: MapEditStroke[] = [];
let drawing = false;
let currentStroke: MapEditStroke | null = null;
let panState: { x: number; y: number; left: number; top: number } | null = null;

const canEdit = computed(() =>
  EDITABLE_STATUS.includes(currentMap.value?.mapStatus ?? "")
);
const canPreview = computed(() => !!currentMap.value?.filePath);
const hasPackageHint = computed(() =>
  currentMap.value?.mapWidth && currentMap.value?.mapHeight
    ? `${currentMap.value.mapWidth}×${currentMap.value.mapHeight}`
    : "-"
);
const canPush = computed(() => !!currentMap.value?.deviceCode);
const brushColor = computed(() =>
  eraseValue.value === 254
    ? "#ffffff"
    : `rgb(${eraseValue.value},${eraseValue.value},${eraseValue.value})`
);
const brushStyle = computed(() => {
  const size = brushRadius.value * 2 * zoom.value;
  return {
    left: `${cursor.value.x * zoom.value - size / 2}px`,
    top: `${cursor.value.y * zoom.value - size / 2}px`,
    width: `${size}px`,
    height: `${size}px`
  };
});

function resetState() {
  strokes = [];
  dirty.value = false;
  drawing = false;
  currentStroke = null;
  panState = null;
  tool.value = "pan";
  zoom.value = 1;
  brushRadius.value = 8;
  eraseValue.value = 254;
  cursor.value.visible = false;
}

function reset() {
  baseImage = null;
  ctx = null;
  meta.value = { resolution: 0, origin: [], width: 0, height: 0 };
  resetState();
}

/** 加载地图 PNG 并初始化画布。 */
async function load() {
  const row = currentMap.value;
  if (!row?.id) return;
  if (!canPreview.value) {
    message("该地图尚无地图包，无法预览", { type: "warning" });
    visible.value = false;
    return;
  }
  loading.value = true;
  try {
    const { code, msg, data } = await dogMapGetImage(row.id);
    if (code !== SUCCESS || !data) {
      message(msg || "地图加载失败", { type: "error" });
      visible.value = false;
      return;
    }
    meta.value = {
      resolution: data.resolution,
      origin: data.origin ?? [],
      width: data.width,
      height: data.height
    };
    await nextTick();
    // el-dialog 内容是懒挂载：等画布真正挂上再画，避免首帧空白
    for (let tries = 0; tries < 5 && !canvasRef.value; tries++) {
      await nextTick();
    }
    await drawBase(data.image);
  } finally {
    loading.value = false;
  }
}

function drawBase(
  source: string | HTMLImageElement,
  fit = true
): Promise<void> {
  return new Promise(resolve => {
    const canvas = canvasRef.value;
    if (!canvas) return resolve();
    const image =
      typeof source === "string" ? new Image() : (source as HTMLImageElement);
    const paint = () => {
      canvas.width = image.naturalWidth || image.width;
      canvas.height = image.naturalHeight || image.height;
      meta.value.width = canvas.width;
      meta.value.height = canvas.height;
      ctx = canvas.getContext("2d");
      ctx?.drawImage(image, 0, 0);
      baseImage = image;
      if (fit) fitToViewport();
      resolve();
    };
    if (typeof source === "string") {
      image.onload = paint;
      image.onerror = () => {
        message("地图图片解码失败", { type: "error" });
        resolve();
      };
      image.src = source;
    } else {
      paint();
    }
  });
}

/** 重绘：底图 + 回放未保存的笔画。 */
function render() {
  const canvas = canvasRef.value;
  if (!canvas || !ctx || !baseImage) return;
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(baseImage, 0, 0);
  for (const stroke of strokes) {
    paintStroke(ctx, stroke);
  }
}

function paintStroke(context: CanvasRenderingContext2D, stroke: MapEditStroke) {
  const radius = stroke.radius ?? brushRadius.value;
  const color =
    eraseValue.value === 254
      ? "#ffffff"
      : `rgb(${eraseValue.value},${eraseValue.value},${eraseValue.value})`;
  context.save();
  context.fillStyle = color;
  context.strokeStyle = color;
  context.lineWidth = radius * 2;
  context.lineCap = "round";
  context.lineJoin = "round";
  const points = stroke.points;
  if (points.length === 1) {
    context.beginPath();
    context.arc(points[0].x, points[0].y, radius, 0, Math.PI * 2);
    context.fill();
  } else {
    context.beginPath();
    context.moveTo(points[0].x, points[0].y);
    for (let index = 1; index < points.length; index++) {
      context.lineTo(points[index].x, points[index].y);
    }
    context.stroke();
  }
  context.restore();
}

function fitToViewport() {
  const viewport = viewportRef.value;
  if (!viewport || !meta.value.width || !meta.value.height) return;
  // 弹窗还在过渡时容器尺寸可能是 0：此时保持 100%，别把图缩成一个点
  if (viewport.clientWidth < 32 || viewport.clientHeight < 32) {
    zoom.value = 1;
    return;
  }
  const scale = Math.min(
    (viewport.clientWidth - 24) / meta.value.width,
    (viewport.clientHeight - 24) / meta.value.height,
    1
  );
  zoom.value = Math.max(scale, 0.02);
}

function setZoom(next: number) {
  zoom.value = Math.min(Math.max(next, 0.02), 16);
}

function zoomBy(factor: number) {
  setZoom(zoom.value * factor);
}

/** 以鼠标位置为中心缩放（保持光标下的像素不动）。 */
function onWheel(event: WheelEvent) {
  const viewport = viewportRef.value;
  if (!viewport) return;
  const rect = viewport.getBoundingClientRect();
  const offsetX = event.clientX - rect.left;
  const offsetY = event.clientY - rect.top;
  const contentX = (viewport.scrollLeft + offsetX) / zoom.value;
  const contentY = (viewport.scrollTop + offsetY) / zoom.value;
  const factor = event.deltaY < 0 ? 1.15 : 1 / 1.15;
  setZoom(zoom.value * factor);
  nextTick(() => {
    viewport.scrollLeft = contentX * zoom.value - offsetX;
    viewport.scrollTop = contentY * zoom.value - offsetY;
  });
}

/** 屏幕坐标 → 栅格像素坐标 */
function toPixel(event: PointerEvent) {
  const canvas = canvasRef.value;
  if (!canvas) return { x: 0, y: 0 };
  const rect = canvas.getBoundingClientRect();
  return {
    x: ((event.clientX - rect.left) / rect.width) * canvas.width,
    y: ((event.clientY - rect.top) / rect.height) * canvas.height
  };
}

function onPointerDown(event: PointerEvent) {
  if (tool.value !== "eraser" || !canEdit.value) return;
  if (event.button !== 0) return;
  const canvas = canvasRef.value;
  if (!canvas) return;
  canvas.setPointerCapture(event.pointerId);
  drawing = true;
  const point = toPixel(event);
  currentStroke = {
    points: [point],
    radius: brushRadius.value
  };
  strokes.push(currentStroke);
  dirty.value = true;
  if (ctx) paintStroke(ctx, currentStroke);
}

function onPointerMove(event: PointerEvent) {
  const point = toPixel(event);
  cursor.value = { x: point.x, y: point.y, visible: tool.value === "eraser" };
  if (!drawing || !currentStroke) return;
  const last = currentStroke.points[currentStroke.points.length - 1];
  const minGap = Math.max((currentStroke.radius ?? 4) / 3, 0.8);
  if (Math.hypot(point.x - last.x, point.y - last.y) < minGap) return;
  const next = {
    x: Math.round(point.x * 100) / 100,
    y: Math.round(point.y * 100) / 100
  };
  currentStroke.points.push(next);
  if (ctx) {
    // 只补画新增的一段，避免每次重绘整张图
    const radius = currentStroke.radius ?? brushRadius.value;
    ctx.save();
    ctx.fillStyle = brushColor.value;
    ctx.strokeStyle = brushColor.value;
    ctx.lineWidth = radius * 2;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(last.x, last.y);
    ctx.lineTo(next.x, next.y);
    ctx.stroke();
    ctx.restore();
  }
}

function onPointerUp(event: PointerEvent) {
  if (drawing) {
    drawing = false;
    currentStroke = null;
    canvasRef.value?.releasePointerCapture?.(event.pointerId);
  }
  panState = null;
}

function onPointerLeave() {
  cursor.value.visible = false;
}

/** 拖动平移（浏览模式） */
function onViewportPointerDown(event: PointerEvent) {
  if (tool.value !== "pan" || event.button !== 0) return;
  const viewport = viewportRef.value;
  if (!viewport) return;
  panState = {
    x: event.clientX,
    y: event.clientY,
    left: viewport.scrollLeft,
    top: viewport.scrollTop
  };
  viewport.setPointerCapture(event.pointerId);
}

function onViewportPointerMove(event: PointerEvent) {
  if (!panState) return;
  const viewport = viewportRef.value;
  if (!viewport) return;
  viewport.scrollLeft = panState.left - (event.clientX - panState.x);
  viewport.scrollTop = panState.top - (event.clientY - panState.y);
}

function undo() {
  if (!strokes.length) return;
  strokes.pop();
  dirty.value = strokes.length > 0;
  render();
}

function discard() {
  if (!strokes.length) return;
  ElMessageBox.confirm("放弃本次尚未保存的擦除？", "放弃编辑", {
    type: "warning",
    confirmButtonText: "放弃",
    cancelButtonText: "取消"
  })
    .then(() => {
      strokes = [];
      dirty.value = false;
      render();
    })
    .catch(() => {});
}

async function save() {
  const row = currentMap.value;
  if (!row?.id || !strokes.length || saving.value) return;
  saving.value = true;
  try {
    const { code, msg, data } = await dogMapErase(
      row.id,
      strokes,
      eraseValue.value
    );
    if (code !== SUCCESS || !data) {
      message(msg || "保存失败", { type: "error" });
      return;
    }
    strokes = [];
    dirty.value = false;
    meta.value = {
      resolution: data.resolution,
      origin: data.origin ?? [],
      width: data.width,
      height: data.height
    };
    await drawBase(data.image, false);
    message(
      `已保存，共擦除 ${data.erasedPixels ?? 0} 个像素（map_hash 已更新）`,
      { type: "success" }
    );
    emit("saved", row);
    confirmPushAfterSave();
  } finally {
    saving.value = false;
  }
}

async function pushToDog() {
  const row = currentMap.value;
  if (!row?.id || pushing.value) return;
  if (!canPush.value) {
    message("该地图没有关联上报设备，无法下发到狗体", { type: "warning" });
    return;
  }
  if (dirty.value) {
    message("还有未保存的擦除，请先「保存擦除」再下发到狗体", {
      type: "warning"
    });
    return;
  }
  pushing.value = true;
  try {
    const { code, msg, data } = await dogMapPushToDevice(
      row.id,
      row.deviceCode
    );
    if (code !== SUCCESS) {
      message(msg || "下发失败", { type: "error" });
      return;
    }
    // 业务平台会等设备真正写回本体并重新载图后才返回，message 是设备侧结果
    const detail =
      data?.result?.message ?? data?.result?.deviceResult?.message ?? "";
    message(
      detail
        ? `已下发到 ${row.deviceCode}：${detail}`
        : `已下发到 ${row.deviceCode}，设备已确认执行`,
      { type: "success", duration: 10000 }
    );
    emit("pushed", row);
  } catch {
    // 失败原因（急停 / 任务占用 / 设备未回执）已由请求拦截器提示
  } finally {
    pushing.value = false;
  }
}

/** 保存成功后询问是否立刻下发到狗体（保证执行任务用的是最新地图）。 */
function confirmPushAfterSave() {
  if (!canPush.value) return;
  ElMessageBox.confirm(
    "地图已保存。是否立即下发到狗体并重新载图？下发会覆盖本体上的地图栅格（有 map_edit/ 时只改编辑版，原始地图保持不动），设备需处于空闲状态。",
    "下发到狗体",
    { type: "warning", confirmButtonText: "立即下发", cancelButtonText: "稍后" }
  )
    .then(() => pushToDog())
    .catch(() => {});
}

async function open(row: any) {
  currentMap.value = row;
  resetState();
  visible.value = true;
  await nextTick();
  await load();
}

defineExpose({ open });
</script>

<template>
  <el-dialog
    v-model="visible"
    :title="`地图预览 / 编辑 - ${currentMap?.mapName ?? ''}`"
    width="88%"
    top="4vh"
    append-to-body
    destroy-on-close
    @closed="reset"
  >
    <div v-loading="loading" class="map-editor">
      <div class="map-toolbar">
        <el-radio-group v-model="tool" size="small">
          <el-radio-button value="pan">
            <el-icon><HandIcon /></el-icon>
            浏览
          </el-radio-button>
          <el-radio-button value="eraser" :disabled="!canEdit">
            <el-icon><EraserIcon /></el-icon>
            橡皮擦
          </el-radio-button>
        </el-radio-group>

        <el-tooltip content="擦除后写入的灰度值" placement="top">
          <el-select
            v-model="eraseValue"
            size="small"
            style="width: 140px"
            :disabled="!canEdit"
          >
            <el-option label="擦成可通行 (254)" :value="254" />
            <el-option label="擦成未知 (205)" :value="205" />
          </el-select>
        </el-tooltip>

        <div class="brush-size">
          <span class="label">笔刷</span>
          <el-slider
            v-model="brushRadius"
            :min="1"
            :max="120"
            :step="1"
            size="small"
            style="width: 130px"
            :disabled="!canEdit"
          />
          <span class="value">{{ brushRadius }}px</span>
        </div>

        <div class="spacer" />

        <el-button-group>
          <el-button
            size="small"
            :icon="useRenderIcon(ZoomOut)"
            @click="zoomBy(1 / 1.25)"
          />
          <el-button size="small" @click="setZoom(1)"
            >{{ Math.round(zoom * 100) }}%</el-button
          >
          <el-button
            size="small"
            :icon="useRenderIcon(ZoomIn)"
            @click="zoomBy(1.25)"
          />
          <el-button
            size="small"
            :icon="useRenderIcon(Fullscreen)"
            title="适应窗口"
            @click="fitToViewport"
          />
        </el-button-group>
        <el-button
          size="small"
          :icon="useRenderIcon(UndoIcon)"
          :disabled="!dirty"
          @click="undo"
          >撤销</el-button
        >
        <el-button
          size="small"
          :icon="useRenderIcon(ResetIcon)"
          :disabled="!dirty"
          @click="discard"
          >放弃</el-button
        >
        <el-button
          size="small"
          type="primary"
          :loading="saving"
          :disabled="!dirty || !canEdit"
          @click="save"
          >保存擦除</el-button
        >
      </div>

      <el-alert
        v-if="!canEdit"
        type="warning"
        show-icon
        :closable="false"
        title="当前版本已发布 / 已归档，只能预览，不能编辑。"
        description="需要擦除噪点时，请先在列表中派生出新版本（新版本为草稿），再对新版本编辑。"
      />
      <el-alert
        v-else
        type="info"
        show-icon
        :closable="false"
        title="橡皮擦只改写编辑版栅格（优先 map_edit/map.pgm），不会改根目录的建图原始地图。"
        description="保存后已下载过该图的设备需要重新同步地图；分辨率、原点与版本号保持不变。"
      />

      <div class="map-meta">
        <span>栅格：{{ hasPackageHint }} px</span>
        <span>分辨率：{{ meta.resolution || "-" }} m/px</span>
        <span>原点：{{ meta.origin.join(", ") || "-" }}</span>
        <span>设备编号：{{ currentMap?.deviceCode || "手工上传" }}</span>
        <span>版本：{{ currentMap?.mapVersion || "-" }}</span>
        <span v-if="dirty" class="dirty">有未保存的擦除</span>
      </div>

      <div
        ref="viewportRef"
        class="map-viewport"
        :class="{
          'is-pan': tool === 'pan',
          'is-erase': tool === 'eraser' && canEdit
        }"
        @wheel.prevent="onWheel"
        @pointerdown="onViewportPointerDown"
        @pointermove="onViewportPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerUp"
      >
        <div class="map-canvas-wrap">
          <canvas
            ref="canvasRef"
            :style="{
              width: `${meta.width * zoom}px`,
              height: `${meta.height * zoom}px`
            }"
            @pointerdown="onPointerDown"
            @pointermove="onPointerMove"
            @pointerup="onPointerUp"
            @pointercancel="onPointerUp"
            @pointerleave="onPointerLeave"
          />
          <div
            v-show="tool === 'eraser' && canEdit && cursor.visible"
            class="brush-cursor"
            :style="brushStyle"
          />
        </div>
      </div>
    </div>
    <template #footer>
      <el-tooltip
        :disabled="canPush"
        content="该地图没有关联上报设备，无法下发到狗体"
        placement="top"
      >
        <span>
          <el-button
            type="warning"
            :loading="pushing"
            :disabled="!canPush"
            @click="pushToDog"
          >
            下发到狗体（{{ currentMap?.deviceCode || "未关联设备" }}）
          </el-button>
        </span>
      </el-tooltip>
      <el-button @click="visible = false">关闭</el-button>
    </template>
  </el-dialog>
</template>

<style lang="scss" scoped>
.map-editor {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.map-toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  align-items: center;

  .brush-size {
    display: flex;
    gap: 8px;
    align-items: center;

    .label {
      font-size: 13px;
      color: var(--el-text-color-secondary);
    }

    .value {
      width: 46px;
      font-family: var(--el-font-family-mono, monospace);
      font-size: 12px;
      color: var(--el-text-color-secondary);
    }
  }

  .spacer {
    flex: 1;
  }
}

.map-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 16px;
  font-size: 12px;
  color: var(--el-text-color-secondary);

  .dirty {
    font-weight: 600;
    color: var(--el-color-warning);
  }
}

.map-viewport {
  position: relative;
  height: 62vh;
  overflow: auto;
  touch-action: none;
  background-color: #0f172a;
  background-image:
    linear-gradient(45deg, #1e293b 25%, transparent 25%),
    linear-gradient(-45deg, #1e293b 25%, transparent 25%),
    linear-gradient(45deg, transparent 75%, #1e293b 75%),
    linear-gradient(-45deg, transparent 75%, #1e293b 75%);
  background-position:
    0 0,
    0 10px,
    10px -10px,
    -10px 0;
  background-size: 20px 20px;
  border: 1px solid var(--el-border-color);
  border-radius: 6px;

  &.is-pan {
    cursor: grab;
  }

  &.is-erase {
    cursor: none;
  }
}

.map-canvas-wrap {
  position: relative;
  display: inline-block;
  margin: 12px;
}

canvas {
  display: block;
  user-select: none;
  image-rendering: pixelated;
}

.brush-cursor {
  position: absolute;
  pointer-events: none;
  border: 1px solid #f56c6c;
  border-radius: 50%;
  box-shadow: 0 0 0 1px rgb(255 255 255 / 85%);
}
</style>
