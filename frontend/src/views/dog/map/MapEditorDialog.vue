<script setup lang="ts">
/**
 * MapEditorDialog - 地图预览 / 橡皮擦编辑
 *
 * 预览：后端 `GET /api/dogMap/{id}/image` 优先把包内 map_edit/map.pgm（遥控器
 * 编辑后的地图）转成 PNG，没有编辑版时才用 model.pgm / map.pgm，
 * 这里按栅格原始像素尺寸画到 canvas 上，支持缩放、拖动浏览。
 *
 * 编辑：橡皮擦按栅格像素坐标记录笔画，每一笔都记下自己的擦除值（254 = 可通行，
 * 默认；128 = 未知），提交 `POST /api/dogMap/{id}/edit`。后端按笔画把这些像素
 * 改写为对应灰度值并重算 map_hash；根目录的建图原始 map.pgm 不会被改。
 * 已发布 / 已归档版本不可编辑，需要先派生新版本。
 *
 * ⚠️ 「未知」必须写 128，不能写 205：本体 `map.yaml` 是 `free_thresh: 0.2`，
 * `205 → p=0.1961 < 0.2` 会被判成**可通行**（205 是 ROS map_saver 配
 * `free_thresh: 0.196` 的旧约定）。2026-11-24 广汇二期就是这个坑：现场把
 * 一片不可通行区擦成 205，本体直接穿了过去。
 */
import { computed, nextTick, ref } from "vue";
import { ElMessageBox } from "element-plus";
import {
  dogMapErase,
  dogMapGetImage,
  dogMapPushToDevice,
  dogMapSetForbiddenAreas,
  type MapEditStroke,
  type MapForbiddenArea
} from "@/api/dogMap";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import EraserIcon from "~icons/ri/eraser-line";
import ForbiddenIcon from "~icons/ri/forbidden-line";
import HandIcon from "~icons/ri/hand";
import ZoomIn from "~icons/ri/zoom-in-line";
import ZoomOut from "~icons/ri/zoom-out-line";
import Fullscreen from "~icons/ri/fullscreen-line";
import UndoIcon from "~icons/ri/arrow-go-back-line";
import ResetIcon from "~icons/ri/restart-line";

defineOptions({ name: "MapEditorDialog" });

const emit = defineEmits<{ saved: [row: any]; pushed: [row: any] }>();

type Tool = "pan" | "eraser" | "forbidden";

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

/** 已保存的禁止区域（来自后端；遥控器画的那块也在里面） */
const savedAreas = ref<MapForbiddenArea[]>([]);
/** 本次新画的禁止区域（id = 0，保存时后端按毫秒时间戳补 id） */
const pendingAreas = ref<MapForbiddenArea[]>([]);
/** 本次要撤销的已有区域 id */
const removedAreaIds = ref<number[]>([]);
/** 正在拖拽的矩形（栅格像素） */
const draftingRect = ref<{
  x0: number;
  y0: number;
  x1: number;
  y1: number;
} | null>(null);
const areaSaving = ref(false);

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
/**
 * 可下发的设备：上报设备在前，分享设备在后。
 * 分享是同一张图给多台设备用，不是地图副本。
 */
const pushTargets = computed<string[]>(() => {
  const row = currentMap.value;
  if (!row) return [];
  const codes: string[] = [];
  if (row.deviceCode) codes.push(row.deviceCode);
  for (const code of row.sharedDeviceCodes ?? []) {
    if (!codes.includes(code)) codes.push(code);
  }
  return codes;
});
const canPush = computed(() => pushTargets.value.length > 0);
/** 当前生效的禁止区域：已保存的（去掉撤销的）+ 新画的 */
const activeAreas = computed<MapForbiddenArea[]>(() => [
  ...savedAreas.value.filter(area => !removedAreaIds.value.includes(area.id!)),
  ...pendingAreas.value
]);
const areaDirty = computed(
  () => pendingAreas.value.length > 0 || removedAreaIds.value.length > 0
);
const dirtyAny = computed(() => dirty.value || areaDirty.value);

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
  savedAreas.value = [];
  pendingAreas.value = [];
  removedAreaIds.value = [];
  draftingRect.value = null;
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
    savedAreas.value = data.forbiddenAreas ?? [];
    pendingAreas.value = [];
    removedAreaIds.value = [];
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

/** 灰度值 → 画布颜色（PNG 是灰度图，直接按同值上色即可）。 */
function valueColor(value: number) {
  return `rgb(${value},${value},${value})`;
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
  paintForbiddenAreas(ctx);
}

/** 画禁止区域（半透明红块 + 边框），以及正在拖拽的矩形。 */
function paintForbiddenAreas(context: CanvasRenderingContext2D) {
  const draw = (
    x0: number,
    y0: number,
    x1: number,
    y1: number,
    dashed: boolean
  ) => {
    const left = Math.min(x0, x1);
    const top = Math.min(y0, y1);
    const width = Math.abs(x1 - x0);
    const height = Math.abs(y1 - y0);
    context.save();
    context.fillStyle = "rgba(220, 38, 38, 0.35)";
    context.strokeStyle = "rgba(220, 38, 38, 0.95)";
    context.lineWidth = Math.max(1 / zoom.value, 0.5);
    if (dashed) context.setLineDash([6 / zoom.value, 4 / zoom.value]);
    context.fillRect(left, top, width, height);
    context.strokeRect(left, top, width, height);
    context.restore();
  };
  for (const area of activeAreas.value) {
    draw(
      area.center.x - area.width / 2,
      area.center.y - area.height / 2,
      area.center.x + area.width / 2,
      area.center.y + area.height / 2,
      false
    );
  }
  if (draftingRect.value) {
    const { x0, y0, x1, y1 } = draftingRect.value;
    draw(x0, y0, x1, y1, true);
  }
}

function paintStroke(context: CanvasRenderingContext2D, stroke: MapEditStroke) {
  const radius = stroke.radius ?? brushRadius.value;
  // 颜色必须跟着笔画自己记录的值走：先擦成可通行、再切到未知并撤销时，
  // 早先那几笔不能被当前选中的值重新上色（否则可通行区域会变成未知）。
  const color = valueColor(stroke.value ?? eraseValue.value);
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
  if (event.button !== 0 || !canEdit.value) return;
  if (tool.value === "forbidden") {
    const canvas = canvasRef.value;
    if (!canvas) return;
    canvas.setPointerCapture(event.pointerId);
    const point = toPixel(event);
    draftingRect.value = {
      x0: Math.round(point.x),
      y0: Math.round(point.y),
      x1: Math.round(point.x),
      y1: Math.round(point.y)
    };
    render();
    return;
  }
  if (tool.value !== "eraser") return;
  const canvas = canvasRef.value;
  if (!canvas) return;
  canvas.setPointerCapture(event.pointerId);
  drawing = true;
  const point = toPixel(event);
  currentStroke = {
    points: [point],
    radius: brushRadius.value,
    // 值在落笔时就固定，之后切换「可通行 / 未知」不影响这一笔
    value: eraseValue.value
  };
  strokes.push(currentStroke);
  dirty.value = true;
  if (ctx) paintStroke(ctx, currentStroke);
}

function onPointerMove(event: PointerEvent) {
  const point = toPixel(event);
  cursor.value = {
    x: point.x,
    y: point.y,
    visible: tool.value === "eraser"
  };
  if (tool.value === "forbidden" && draftingRect.value) {
    draftingRect.value = {
      ...draftingRect.value,
      x1: Math.round(point.x),
      y1: Math.round(point.y)
    };
    render();
    return;
  }
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
    // 只补画新增的一段，避免每次重绘整张图；颜色用笔画自己的值
    const radius = currentStroke.radius ?? brushRadius.value;
    const color = valueColor(currentStroke.value ?? eraseValue.value);
    ctx.save();
    ctx.fillStyle = color;
    ctx.strokeStyle = color;
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
  if (tool.value === "forbidden" && draftingRect.value) {
    const { x0, y0, x1, y1 } = draftingRect.value;
    draftingRect.value = null;
    canvasRef.value?.releasePointerCapture?.(event.pointerId);
    const width = Math.abs(x1 - x0);
    const height = Math.abs(y1 - y0);
    if (width >= 2 && height >= 2) {
      pendingAreas.value.push({
        center: { x: (x0 + x1) / 2, y: (y0 + y1) / 2 },
        width,
        height,
        rotation: 0
      });
    }
    render();
    return;
  }
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
  // 优先撤销还没保存的禁止区域，再撤销笔画
  if (pendingAreas.value.length) {
    pendingAreas.value.pop();
    render();
    return;
  }
  if (!strokes.length) return;
  strokes.pop();
  dirty.value = strokes.length > 0;
  render();
}

/** 撤销一个已保存的禁止区域（保存后才会真正删掉并还原栅格）。 */
function removeArea(area: MapForbiddenArea) {
  if (!canEdit.value) return;
  if (!area.id) {
    pendingAreas.value = pendingAreas.value.filter(item => item !== area);
  } else if (!removedAreaIds.value.includes(area.id)) {
    removedAreaIds.value.push(area.id);
  }
  render();
}

function discard() {
  if (!dirtyAny.value) return;
  ElMessageBox.confirm("放弃本次尚未保存的擦除 / 禁止区域改动？", "放弃编辑", {
    type: "warning",
    confirmButtonText: "放弃",
    cancelButtonText: "取消"
  })
    .then(() => {
      strokes = [];
      dirty.value = false;
      pendingAreas.value = [];
      removedAreaIds.value = [];
      render();
    })
    .catch(() => {});
}

async function save() {
  const row = currentMap.value;
  if (!row?.id || saving.value || areaSaving.value) return;
  if (!dirty.value && !areaDirty.value) return;
  saving.value = true;
  try {
    let erasedPixels = 0;
    let latest: any = null;

    if (strokes.length) {
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
      erasedPixels = data.erasedPixels ?? 0;
      latest = data;
    }

    if (areaDirty.value) {
      areaSaving.value = true;
      try {
        const { code, msg, data } = await dogMapSetForbiddenAreas(
          row.id,
          activeAreas.value
        );
        if (code !== SUCCESS || !data) {
          message(msg || "禁止区域保存失败", { type: "error" });
          return;
        }
        savedAreas.value = data.forbiddenAreas ?? [];
        pendingAreas.value = [];
        removedAreaIds.value = [];
        latest = data;
      } finally {
        areaSaving.value = false;
      }
    }

    if (!latest) return;
    meta.value = {
      resolution: latest.resolution,
      origin: latest.origin ?? [],
      width: latest.width,
      height: latest.height
    };
    await drawBase(latest.image, false);
    render();
    const parts: string[] = [];
    if (erasedPixels) parts.push(`擦除 ${erasedPixels} 个像素`);
    if (activeAreas.value.length)
      parts.push(`禁止区域 ${activeAreas.value.length} 块`);
    message(
      `已保存${parts.length ? "：" + parts.join("，") : ""}（map_hash 已更新）`,
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
  const deviceCode = pushTargets.value[0];
  if (!row?.id || pushing.value) return;
  if (!deviceCode) {
    message("该地图没有关联上报设备，也没有分享给其他设备，无法下发到狗体", {
      type: "warning"
    });
    return;
  }
  if (dirtyAny.value) {
    message("还有未保存的擦除 / 禁止区域，请先保存再下发到狗体", {
      type: "warning"
    });
    return;
  }
  pushing.value = true;
  try {
    const { code, msg, data } = await dogMapPushToDevice(row.id, deviceCode);
    if (code !== SUCCESS) {
      message(msg || "下发失败", { type: "error" });
      return;
    }
    // 业务平台会等设备真正写回本体并重新载图后才返回，message 是设备侧结果
    const detail =
      data?.result?.message ?? data?.result?.deviceResult?.message ?? "";
    message(
      detail
        ? `已下发到 ${deviceCode}：${detail}`
        : `已下发到 ${deviceCode}，设备已确认执行`,
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
    "地图已保存。是否立即下发到狗体并重新载图？下发会覆盖本体上的地图栅格（有 map_edit/ 时只改编辑版，原始地图保持不动），禁止区域也会连同编辑记录一起写回，设备需处于空闲状态。",
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
          <el-radio-button value="forbidden" :disabled="!canEdit">
            <el-icon><ForbiddenIcon /></el-icon>
            禁止区域
          </el-radio-button>
        </el-radio-group>

        <el-tooltip
          v-if="tool === 'forbidden'"
          content="在图上按住左键拖出一个矩形 = 禁止区域（栅格会被刷成「占据」，导航规划绕开它）。保存后点「下发到狗体」写回本体，遥控器上也会看到这块红色区域。"
          placement="top"
        >
          <span class="forbidden-hint">
            拖拽画矩形 · 已有 {{ activeAreas.length }} 块
          </span>
        </el-tooltip>

        <el-tooltip
          content="254 = 可通行（擦掉噪点）；128 = 未知/不可通行（本体规划器不会走）"
          placement="top"
        >
          <el-select
            v-model="eraseValue"
            size="small"
            style="width: 150px"
            :disabled="!canEdit"
          >
            <el-option label="擦成可通行 (254)" :value="254" />
            <el-option label="擦成未知 (128)" :value="128" />
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

        <div v-if="activeAreas.length" class="forbidden-list">
          <el-tag
            v-for="(area, index) in activeAreas"
            :key="area.id ?? `new-${index}`"
            type="danger"
            effect="plain"
            size="small"
            closable
            @close="removeArea(area)"
          >
            {{ area.id ? `#${area.id}` : "新" }}
            {{ Math.round(area.width) }}×{{ Math.round(area.height) }}px
          </el-tag>
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
          :disabled="!dirtyAny"
          @click="undo"
          >撤销</el-button
        >
        <el-button
          size="small"
          :icon="useRenderIcon(ResetIcon)"
          :disabled="!dirtyAny"
          @click="discard"
          >放弃</el-button
        >
        <el-button
          size="small"
          type="primary"
          :loading="saving"
          :disabled="!dirtyAny || !canEdit"
          @click="save"
          >保存</el-button
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
        title="橡皮擦与禁止区域只改写编辑版栅格（优先 map_edit/map.pgm），不会改根目录的建图原始地图。"
        description="保存后已下载过该图的设备需要重新同步地图；分辨率、原点与版本号保持不变。"
      />

      <div class="map-meta">
        <span>栅格：{{ hasPackageHint }} px</span>
        <span>分辨率：{{ meta.resolution || "-" }} m/px</span>
        <span>原点：{{ meta.origin.join(", ") || "-" }}</span>
        <span>设备编号：{{ currentMap?.deviceCode || "手工上传" }}</span>
        <span v-if="currentMap?.sharedDeviceCodes?.length">
          共享设备：{{ currentMap.sharedDeviceCodes.join("、") }}
        </span>
        <span>版本：{{ currentMap?.mapVersion || "-" }}</span>
        <span v-if="dirtyAny" class="dirty">有未保存的擦除 / 禁止区域</span>
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
        content="该地图没有关联上报设备，也没有分享给其他设备，无法下发到狗体"
        placement="top"
      >
        <span>
          <el-button
            type="warning"
            :loading="pushing"
            :disabled="!canPush"
            @click="pushToDog"
          >
            下发到狗体（{{ pushTargets[0] || "未关联设备" }}）
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
      font-size: var(--dog-font-sm);
      color: var(--dog-text-secondary);
    }

    .value {
      width: 46px;
      font-family: var(--dog-font-mono);
      font-size: var(--dog-font-xs);
      color: var(--dog-text-secondary);
    }
  }

  .forbidden-hint {
    font-size: var(--dog-font-sm);
    color: var(--el-color-danger);
    white-space: nowrap;
  }

  .forbidden-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    max-width: 380px;
    max-height: 56px;
    overflow-y: auto;
  }

  .spacer {
    flex: 1;
  }
}

.map-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 16px;
  font-size: var(--dog-font-sm);
  color: var(--dog-text-secondary);

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
