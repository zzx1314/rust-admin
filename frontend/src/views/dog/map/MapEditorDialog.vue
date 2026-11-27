<script setup lang="ts">
/**
 * MapEditorDialog - 地图预览 / 橡皮擦编辑
 *
 * 预览：后端 `GET /api/dogMap/{id}/image` 优先把包内 map_edit/map.pgm（遥控器
 * 编辑后的地图）转成 PNG，没有编辑版时才用 model.pgm / map.pgm，
 * 这里按栅格原始像素尺寸画到 canvas 上，支持缩放、拖动浏览、**视图旋转**
 * （任意角度：90° 按钮 + 「旋转」工具里按住鼠标拖，只转显示，不改地图数据——
 * 栅格 / yaml / 原点一个字节都不动）。旋转后橡皮擦、禁止区域的落点仍然换算回
 * **栅格像素坐标**：直角旋转下轴对齐矩形转回去还是轴对齐的（`rotation` 保持 0），
 * 非直角时屏幕上拖出的轴对齐框在栅格里是斜矩形，按 `rotation = -视图角度` 记录
 * （后端 `point_in_forbidden` 就是绕 center 转这个角度），遥控器那边照样能读。
 *
 * 编辑：橡皮擦按栅格像素坐标记录笔画，每一笔写入 254（可通行）并提交
 * `POST /api/dogMap/{id}/edit`。后端按笔画把这些像素改写为 254 并重算
 * map_hash；根目录的建图原始 map.pgm 不会被改。
 * 已发布 / 已归档版本不可编辑，需要先派生新版本。
 *
 * 擦除值只有 **254（可通行）** 一种，与遥控器保持一致：遥控器的橡皮擦只会写 254，
 * 保存时按「根图 + 擦除圆」重算栅格，根本表达不了「未知」——平台这边能擦成 128 的话，
 * 现场在遥控器上一保存就被整体冲掉。要标禁行请用「禁止区域」（栅格刷成 0）。
 * 接口仍接受 `128` 与旧 `205`（历史客户端 / 脚本），但本页面不再提供。
 *
 * 禁止区域一多就认不出「哪块是哪块」，所以：
 * - 画布上按列表顺序给每块打**序号**（视图坐标，旋转时不歪）；
 * - 右侧列表行 hover / 点击 → 对应矩形高亮并平移到视口中心，反过来点图上的矩形
 *   也会高亮对应行；
 * - 列表显示**米制**尺寸 / 面积 / 中心坐标（与路线坐标系同一套换算）；
 * - 「删区域」工具：在图上点哪块删哪块，删掉的可以用「撤销」找回。
 */
import { computed, nextTick, ref, watch } from "vue";
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
import ForbiddenIcon from "~icons/ri/forbid-line";
import HandIcon from "~icons/ri/hand";
import ZoomIn from "~icons/ri/zoom-in-line";
import ZoomOut from "~icons/ri/zoom-out-line";
import Fullscreen from "~icons/ri/fullscreen-line";
import RotateLeft from "~icons/ri/anticlockwise-line";
import RotateRight from "~icons/ri/clockwise-line";
import RotateIcon from "~icons/ri/refresh-line";
import UndoIcon from "~icons/ri/arrow-go-back-line";
import ResetIcon from "~icons/ri/restart-line";
import DeleteIcon from "~icons/ri/delete-bin-line";

defineOptions({ name: "MapEditorDialog" });

const emit = defineEmits<{ saved: [row: any]; pushed: [row: any] }>();

type Tool = "pan" | "rotate" | "eraser" | "forbidden" | "remove";

/** `.map-canvas-wrap` 的 margin（px）：画布在滚动内容里的偏移，算旋转吸附点时要用。 */
const CANVAS_MARGIN = 12;

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
/** 橡皮擦写入的灰度值：只擦成可通行，与遥控器一致（不再提供「擦成未知 128」）。 */
const ERASE_VALUE = 254;
/** 画布上的擦除颜色：PNG 是灰度图，按同值上色即可。 */
const ERASE_COLOR = `rgb(${ERASE_VALUE},${ERASE_VALUE},${ERASE_VALUE})`;
const zoom = ref(1);
/**
 * 视图旋转（顺时针，度）：任意角度，仅影响展示与鼠标坐标换算。
 *
 * 90° 按钮一档一档地转到 0 / 90 / 180 / 270，再切「旋转」工具用鼠标拖着微调
 * 到任意角度（拖到 90° 的整数倍附近会自动吸附）——现场常见「地图的北向和
 * 正前方差十几度」，只按 90° 转永远对不齐。
 * 不动地图数据：栅格、`map.yaml`、原点、分辨率、`map_hash` 全部保持原样，
 * 下发到狗体、遥控器、路线坐标系都跟旋转前完全一致。
 */
const rotation = ref(0);
const dirty = ref(false);
const cursor = ref({ x: 0, y: 0, visible: false });

/** 已保存的禁止区域（来自后端；遥控器画的那块也在里面） */
const savedAreas = ref<MapForbiddenArea[]>([]);
/** 本次新画的禁止区域（id = 0，保存时后端按毫秒时间戳补 id） */
const pendingAreas = ref<MapForbiddenArea[]>([]);
/** 本次要撤销的已有区域 id */
const removedAreaIds = ref<number[]>([]);
/** 正在拖拽的矩形（**画布坐标**，屏幕上看着轴对齐；落库时再换算成栅格 + 角度） */
const draftingRect = ref<{
  x0: number;
  y0: number;
  x1: number;
  y1: number;
} | null>(null);
const areaSaving = ref(false);
/** 悬停 / 选中的禁止区域：与列表行同一个键（见 `areaKey`），画布据此高亮 */
const activeAreaKey = ref<string | null>(null);

const canvasRef = ref<HTMLCanvasElement>();
const viewportRef = ref<HTMLDivElement>();
const areaRowsRef = ref<HTMLDivElement>();

let ctx: CanvasRenderingContext2D | null = null;
let baseImage: HTMLImageElement | null = null;
/** 已提交但尚未保存的笔画（栅格像素坐标） */
let strokes: MapEditStroke[] = [];
let drawing = false;
let currentStroke: MapEditStroke | null = null;
let panState: { x: number; y: number; left: number; top: number } | null = null;
/**
 * 拖拽旋转手势：`pin` 是手势开始时地图中心在**视口里**的屏幕坐标（整个拖动
 * 过程把它钉住，否则 canvas 外接矩形一变大地图就跳），`startAngle` 是按下时
 * 鼠标绕着它的角度。
 */
let rotateState: {
  startRotation: number;
  startAngle: number | null;
  pin: { x: number; y: number };
} | null = null;

const canEdit = computed(() =>
  EDITABLE_STATUS.includes(currentMap.value?.mapStatus ?? "")
);
/**
 * 旋转后画布的外接矩形（任意角度下宽高都变）：
 * `offsetX / offsetY` 是「栅格原点在旋转后画布里的位置」，`cos / sin` 是旋转矩阵。
 */
const viewBox = computed(() => {
  const width = meta.value.width;
  const height = meta.value.height;
  const rad = (rotation.value * Math.PI) / 180;
  const cos = Math.cos(rad);
  const sin = Math.sin(rad);
  if (!width || !height) {
    return { width: 0, height: 0, offsetX: 0, offsetY: 0, cos, sin };
  }
  // 四个角绕原点旋转后的范围：min = 平移量，max - min = 画布尺寸
  const xs = [0, width * cos, -height * sin, width * cos - height * sin];
  const ys = [0, width * sin, height * cos, width * sin + height * cos];
  const minX = Math.min(...xs);
  const minY = Math.min(...ys);
  // 90° 的整数倍时 cos/sin 带浮点误差，减 1e-6 再 ceil 才不会多出 1px
  const span = (from: number, to: number) =>
    Math.max(1, Math.ceil(to - from - 1e-6));
  return {
    width: span(minX, Math.max(...xs)),
    height: span(minY, Math.max(...ys)),
    offsetX: -minX,
    offsetY: -minY,
    cos,
    sin
  };
});
/** 旋转后画布的像素尺寸（任意角度都按外接矩形算）。 */
const viewWidth = computed(() => viewBox.value.width);
const viewHeight = computed(() => viewBox.value.height);
/** 状态行里显示的角度：-180~180，一位小数（拖动时不会满屏小数）。 */
const rotationText = computed(() => {
  const value = Math.round(rotation.value * 10) / 10;
  return value > 180 ? value - 360 : value;
});
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

/** 列表行的稳定键：已保存的用 id，本次新画的用下标（与保存时的排序无关） */
function areaKey(area: MapForbiddenArea, index: number) {
  return area.id ? `id-${area.id}` : `new-${index}`;
}

function formatLength(value: number) {
  return value >= 10 ? value.toFixed(1) : value.toFixed(2);
}

/** 尺寸 / 面积：有分辨率就换米，没有就退回像素（手工上传的包可能没有 yaml）。 */
function areaSizeText(area: MapForbiddenArea) {
  const resolution = meta.value.resolution;
  if (!resolution) {
    return `${Math.round(area.width)}×${Math.round(area.height)} px`;
  }
  const width = area.width * resolution;
  const height = area.height * resolution;
  return `${formatLength(width)}×${formatLength(height)} m（${formatLength(
    width * height
  )} m²）`;
}

/**
 * 中心坐标（米）：与本体 / 路线坐标系一致——PGM 行 0 在图顶，米制 y 要翻过来
 * （同一套换算见 `business/server/src/handlers/maps/clearance.rs::cell`）。
 */
function areaCenterText(area: MapForbiddenArea) {
  const resolution = meta.value.resolution;
  const origin = meta.value.origin;
  if (!resolution || origin.length < 2) {
    return `中心 (${Math.round(area.center.x)}, ${Math.round(
      area.center.y
    )}) px`;
  }
  const x = origin[0] + area.center.x * resolution;
  const y =
    origin[1] + ((meta.value.height || 0) - 1 - area.center.y) * resolution;
  return `中心 (${x.toFixed(1)}, ${y.toFixed(1)}) m`;
}

/** 画布序号 + 右侧列表共用的行数据（顺序 = 画布上的编号） */
const areaRows = computed(() =>
  activeAreas.value.map((area, index) => ({
    area,
    index,
    key: areaKey(area, index),
    no: index + 1,
    sizeText: areaSizeText(area),
    centerText: areaCenterText(area)
  }))
);

const brushStyle = computed(() => {
  const size = brushRadius.value * 2 * zoom.value;
  // canvas 是旋转后的坐标系，光标圆要按视图坐标定位（否则 90° 后会跑偏）
  const point = rasterToCanvas(cursor.value);
  return {
    left: `${point.x * zoom.value - size / 2}px`,
    top: `${point.y * zoom.value - size / 2}px`,
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
  activeAreaKey.value = null;
  draftingRect.value = null;
  drawing = false;
  currentStroke = null;
  panState = null;
  rotateState = null;
  tool.value = "pan";
  zoom.value = 1;
  rotation.value = 0;
  brushRadius.value = 8;
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
    activeAreaKey.value = null;
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

// ---- 视图旋转（只转显示，不动地图数据）------------------------------------
//
// 画布始终是「旋转后」的坐标系（外接矩形，任意角度下宽高都变），底图、未保存
// 笔画、禁止区域都画在同一个 ctx 变换下；鼠标坐标再逆变换回栅格像素坐标提交给
// 后端。90° 的整数倍下轴对齐矩形转回去仍是轴对齐的，禁止区域自身 `rotation`
// 保持 0；非直角时屏幕上拖出的框在栅格里是斜的，按 -视图角度 记录。

/** 把 ctx 设成「栅格坐标 → 旋转后画布坐标」的变换。 */
function applyViewTransform(context: CanvasRenderingContext2D) {
  const box = viewBox.value;
  context.translate(box.offsetX, box.offsetY);
  if (box.sin || box.cos !== 1) {
    context.rotate((rotation.value * Math.PI) / 180);
  }
}

/** 栅格像素坐标 → 旋转后画布坐标。 */
function rasterToCanvas(point: { x: number; y: number }) {
  const box = viewBox.value;
  return {
    x: point.x * box.cos - point.y * box.sin + box.offsetX,
    y: point.x * box.sin + point.y * box.cos + box.offsetY
  };
}

/** 旋转后画布坐标 → 栅格像素坐标（`rasterToCanvas` 的逆变换，不裁剪）。 */
function canvasToRasterRaw(point: { x: number; y: number }) {
  const box = viewBox.value;
  const dx = point.x - box.offsetX;
  const dy = point.y - box.offsetY;
  return {
    x: dx * box.cos + dy * box.sin,
    y: -dx * box.sin + dy * box.cos
  };
}

/** 某个栅格坐标是否落在地图上（旋转后画布的四角有空区，不能拿它当边缘）。 */
function inRaster(point: { x: number; y: number }) {
  return (
    point.x >= 0 &&
    point.y >= 0 &&
    point.x <= meta.value.width &&
    point.y <= meta.value.height
  );
}

/** 旋转后画布坐标 → 栅格像素坐标，越界时裁到地图边上。 */
function canvasToRaster(point: { x: number; y: number }) {
  const raw = canvasToRasterRaw(point);
  const clamp = (value: number, max: number) =>
    Math.min(Math.max(value, 0), max);
  return {
    x: clamp(raw.x, meta.value.width),
    y: clamp(raw.y, meta.value.height)
  };
}

/**
 * 设置视图角度（顺时针，度）：任意值都收拢到 0~360，只动显示、不改地图数据。
 * 拖动浏览时保留 zoom / 滚动位置，不重新适应窗口（否则每转一下图就跳一下）。
 */
function setRotation(deg: number) {
  const value = Number(deg);
  if (!Number.isFinite(value)) return;
  let next = Math.round(value * 10) / 10;
  next = ((next % 360) + 360) % 360;
  if (next === rotation.value) return;
  rotation.value = next;
}

/** 拖到 0 / 90 / 180 / 270 附近（±2.5°）自动吸附，方便把图拿正。 */
function snapAngle(deg: number) {
  const nearest = Math.round(deg / 90) * 90;
  return Math.abs(deg - nearest) <= 2.5 ? nearest : deg;
}

/** 栅格中心在旋转后画布里的位置（rotate 以它为中心）。 */
function rasterCenterInCanvas() {
  return rasterToCanvas({
    x: meta.value.width / 2,
    y: meta.value.height / 2
  });
}

/** 地图中心当前落在视口的哪个位置（相对 viewport 左上角，px）。 */
function rasterCenterOnScreen() {
  const viewport = viewportRef.value;
  if (!viewport) return null;
  const center = rasterCenterInCanvas();
  return {
    x: center.x * zoom.value + CANVAS_MARGIN - viewport.scrollLeft,
    y: center.y * zoom.value + CANVAS_MARGIN - viewport.scrollTop
  };
}

/** 屏幕上（y 向下）一点的方位角，顺时针为正，度。 */
function angleAround(
  center: { x: number; y: number },
  point: { x: number; y: number }
) {
  return (Math.atan2(point.y - center.y, point.x - center.x) * 180) / Math.PI;
}

/** 「旋转」工具：按住左键绕着地图中心拖 = 手动旋转视图。 */
function startRotateDrag(event: PointerEvent) {
  const canvas = canvasRef.value;
  const viewport = viewportRef.value;
  const pin = rasterCenterOnScreen();
  if (!canvas || !viewport || !pin) return;
  const rect = viewport.getBoundingClientRect();
  const pointer = { x: event.clientX - rect.left, y: event.clientY - rect.top };
  canvas.setPointerCapture(event.pointerId);
  // 在中心附近起手时方位角没有意义（半步就转半圈）：先空着，离开死区再开始计
  const far = Math.hypot(pointer.x - pin.x, pointer.y - pin.y) >= 12;
  rotateState = {
    startRotation: rotation.value,
    startAngle: far ? angleAround(pin, pointer) : null,
    pin
  };
}

function updateRotateDrag(event: PointerEvent) {
  const viewport = viewportRef.value;
  if (!viewport || !rotateState) return;
  const rect = viewport.getBoundingClientRect();
  const pointer = { x: event.clientX - rect.left, y: event.clientY - rect.top };
  if (rotateState.startAngle === null) {
    if (
      Math.hypot(pointer.x - rotateState.pin.x, pointer.y - rotateState.pin.y) <
      12
    ) {
      return;
    }
    rotateState.startAngle = angleAround(rotateState.pin, pointer);
    rotateState.startRotation = rotation.value;
    return;
  }
  let delta = angleAround(rotateState.pin, pointer) - rotateState.startAngle;
  while (delta <= -180) delta += 360;
  while (delta > 180) delta -= 360;
  setRotation(snapAngle(rotateState.startRotation + delta));
}

/** 在栅格坐标系里画东西（底图 / 笔画 / 禁止区域），自动套上视图旋转。 */
function drawOnRaster(
  context: CanvasRenderingContext2D,
  draw: (context: CanvasRenderingContext2D) => void
) {
  context.save();
  applyViewTransform(context);
  draw(context);
  context.restore();
}

/** 按旋转后的外接矩形重排画布尺寸（换宽高会清空 canvas，要重画）。 */
function resizeCanvasForView() {
  const canvas = canvasRef.value;
  if (!canvas || !baseImage) return false;
  const nextWidth = viewWidth.value;
  const nextHeight = viewHeight.value;
  if (canvas.width === nextWidth && canvas.height === nextHeight && ctx) {
    return true;
  }
  canvas.width = nextWidth;
  canvas.height = nextHeight;
  ctx = canvas.getContext("2d");
  return true;
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
      // 栅格自身尺寸（未旋转），meta 里的 width / height 始终是栅格尺寸
      meta.value.width = image.naturalWidth || image.width;
      meta.value.height = image.naturalHeight || image.height;
      baseImage = image;
      resizeCanvasForView();
      if (ctx) {
        ctx.setTransform(1, 0, 0, 1, 0, 0);
        ctx.clearRect(0, 0, canvas.width, canvas.height);
        drawOnRaster(ctx, context => context.drawImage(image, 0, 0));
      }
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

/** 重绘：底图 + 回放未保存的笔画 + 禁止区域（全部在旋转后的视图坐标系下）。 */
function render() {
  const canvas = canvasRef.value;
  if (!canvas || !ctx || !baseImage) return;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  drawOnRaster(ctx, context => {
    context.drawImage(baseImage as HTMLImageElement, 0, 0);
    for (const stroke of strokes) {
      paintStroke(context, stroke);
    }
    paintForbiddenAreas(context);
  });
  // 正在拖的矩形存的是「画布坐标」（屏幕上看着轴对齐），不能套上面的旋转
  paintDraftingRect(ctx);
  // 序号画在最上层（视图坐标，旋转时不歪）
  paintAreaLabels(ctx);
}

/** 画一个禁止区域矩形（`rotation` 绕 center，按弧度，栅格坐标系）。 */
function paintAreaRect(
  context: CanvasRenderingContext2D,
  center: { x: number; y: number },
  width: number,
  height: number,
  rotation = 0,
  dashed = false,
  active = false
) {
  context.save();
  context.translate(center.x, center.y);
  if (rotation) context.rotate(rotation);
  context.fillStyle = active
    ? "rgba(220, 38, 38, 0.6)"
    : "rgba(220, 38, 38, 0.35)";
  context.strokeStyle = "rgba(220, 38, 38, 0.95)";
  context.lineWidth = Math.max(1 / zoom.value, 0.5);
  if (dashed) context.setLineDash([6 / zoom.value, 4 / zoom.value]);
  context.fillRect(-width / 2, -height / 2, width, height);
  context.strokeRect(-width / 2, -height / 2, width, height);
  if (active) {
    // 选中时再加一圈白边：红底图上红框容易看不出来
    context.setLineDash([]);
    context.strokeStyle = "rgba(255, 255, 255, 0.95)";
    context.lineWidth = Math.max(2 / zoom.value, 0.8);
    context.strokeRect(-width / 2, -height / 2, width, height);
  }
  context.restore();
}

/** 画禁止区域（半透明红块 + 边框），按各自的 `rotation` 绕中心转。 */
function paintForbiddenAreas(context: CanvasRenderingContext2D) {
  const list = activeAreas.value;
  for (let index = 0; index < list.length; index++) {
    const area = list[index];
    paintAreaRect(
      context,
      area.center,
      area.width,
      area.height,
      area.rotation ?? 0,
      false,
      areaKey(area, index) === activeAreaKey.value
    );
  }
}

/**
 * 在**视图坐标**里给每块禁止区域打序号（与右侧列表顺序一致）。
 *
 * 不套 `drawOnRaster` 的旋转：数字始终正着看；字号按 zoom 反向缩放，
 * 屏幕上大小恒定（zoom 变化时要重画，见文件末尾的 watch）。
 */
function paintAreaLabels(context: CanvasRenderingContext2D) {
  const list = activeAreas.value;
  if (!list.length) return;
  const basePx = Math.min(Math.max(13 / zoom.value, 7), 40);
  context.save();
  context.textAlign = "center";
  context.textBaseline = "middle";
  context.lineJoin = "round";
  for (let index = 0; index < list.length; index++) {
    const area = list[index];
    if (!inRaster(area.center)) continue;
    const point = rasterToCanvas(area.center);
    const active = areaKey(area, index) === activeAreaKey.value;
    const fontPx = active ? basePx * 1.25 : basePx;
    context.font = `600 ${fontPx}px ui-sans-serif, system-ui, sans-serif`;
    context.lineWidth = Math.max(fontPx / 4, 1);
    context.strokeStyle = "rgba(15, 23, 42, 0.9)";
    context.fillStyle = active ? "#ffffff" : "#fecdd3";
    const label = String(index + 1);
    context.strokeText(label, point.x, point.y);
    context.fillText(label, point.x, point.y);
  }
  context.restore();
}

/** 某个栅格坐标是否落在区域内（与后端 `point_in_forbidden` 同一套几何）。 */
function pointInArea(point: { x: number; y: number }, area: MapForbiddenArea) {
  const dx = point.x - area.center.x;
  const dy = point.y - area.center.y;
  const rotation = area.rotation ?? 0;
  const sin = Math.sin(rotation);
  const cos = Math.cos(rotation);
  // 反向旋转到矩形自身坐标系
  const localX = dx * cos + dy * sin;
  const localY = -dx * sin + dy * cos;
  return (
    Math.abs(localX) <= area.width / 2 && Math.abs(localY) <= area.height / 2
  );
}

/** 命中最上层（后画在上）的那块区域；点空白返回 null。 */
function hitTestArea(point: { x: number; y: number }) {
  const rows = areaRows.value;
  for (let index = rows.length - 1; index >= 0; index--) {
    if (pointInArea(point, rows[index].area)) return rows[index];
  }
  return null;
}

/** 把视口滚到某块区域的中心（列表点击定位用）。 */
function focusArea(area: MapForbiddenArea) {
  const viewport = viewportRef.value;
  if (!viewport) return;
  const point = rasterToCanvas(area.center);
  viewport.scrollLeft =
    point.x * zoom.value + CANVAS_MARGIN - viewport.clientWidth / 2;
  viewport.scrollTop =
    point.y * zoom.value + CANVAS_MARGIN - viewport.clientHeight / 2;
}

/** 列表行 hover / 点击：高亮对应矩形，点击时顺便把它滚到视口中心。 */
function highlightArea(
  row: { key: string; area: MapForbiddenArea },
  focus = false
) {
  activeAreaKey.value = row.key;
  if (focus) focusArea(row.area);
  render();
  // 从画布点中的行可能在列表可视区外：滚到最近处（不抢页面滚动）
  nextTick(() => {
    const key = row.key;
    areaRowsRef.value
      ?.querySelector(`[data-key="${key}"]`)
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  });
}

function clearHighlight() {
  if (!activeAreaKey.value) return;
  activeAreaKey.value = null;
  render();
}

/** 屏幕事件 → 命中的区域行（不在图上 / 没命中返回 null）。 */
function areaRowAtEvent(event: MouseEvent | PointerEvent) {
  const canvas = canvasRef.value;
  if (!canvas) return null;
  const rect = canvas.getBoundingClientRect();
  if (!rect.width || !rect.height) return null;
  const view = {
    x: ((event.clientX - rect.left) / rect.width) * canvas.width,
    y: ((event.clientY - rect.top) / rect.height) * canvas.height
  };
  const point = canvasToRasterRaw(view);
  if (!inRaster(point)) return null;
  return hitTestArea(point);
}

/** 浏览模式点图：选中那块（与右侧列表联动）。 */
function selectAreaAt(event: PointerEvent) {
  const row = areaRowAtEvent(event);
  if (row) highlightArea(row);
  else clearHighlight();
}

/** 「删区域」点图：删掉点到的那块（保存后才会改栅格）。 */
function removeAreaAt(event: PointerEvent) {
  const row = areaRowAtEvent(event);
  if (!row) {
    message("这里没有禁止区域", { type: "info" });
    return;
  }
  removeArea(row.area);
  message(`已删除区域 ${row.no}（点「保存」后栅格才会还原）`, { type: "info" });
}

/** 画正在拖拽的矩形（画布坐标，屏幕上一定是轴对齐的）。 */
function paintDraftingRect(context: CanvasRenderingContext2D) {
  const rect = draftingRect.value;
  if (!rect) return;
  paintAreaRect(
    context,
    { x: (rect.x0 + rect.x1) / 2, y: (rect.y0 + rect.y1) / 2 },
    Math.abs(rect.x1 - rect.x0),
    Math.abs(rect.y1 - rect.y0),
    0,
    true
  );
}

/**
 * 把屏幕上拖出的轴对齐矩形（画布坐标）换算成禁止区域（栅格坐标）。
 *
 * 视图顺时针转了 θ，屏幕上轴对齐的框落到栅格里就是逆时针转了 θ 的斜矩形：
 * `rotation = -θ`（弧度），90° 的整数倍时就是 0 / ±90°，和以前一样。
 * 角度收拢到 (-90°, 90°]：同一矩形转 180° 还是它自己，宽高不用换。
 */
function createAreaFromDraft(rect: {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}): MapForbiddenArea | null {
  const width = Math.abs(rect.x1 - rect.x0);
  const height = Math.abs(rect.y1 - rect.y0);
  if (width < 2 || height < 2) return null;
  const center = canvasToRaster({
    x: (rect.x0 + rect.x1) / 2,
    y: (rect.y0 + rect.y1) / 2
  });
  // 归一到 (-90°, 90°]：同一矩形转 180° 就是它自己，宽高不用动
  let angle = -rotation.value;
  while (angle <= -90) angle += 180;
  while (angle > 90) angle -= 180;
  return {
    center: {
      x: Math.round(center.x * 100) / 100,
      y: Math.round(center.y * 100) / 100
    },
    width: Math.round(width * 100) / 100,
    height: Math.round(height * 100) / 100,
    rotation: Math.round(((angle * Math.PI) / 180) * 10000) / 10000
  };
}

function paintStroke(context: CanvasRenderingContext2D, stroke: MapEditStroke) {
  const radius = stroke.radius ?? brushRadius.value;
  context.save();
  context.fillStyle = ERASE_COLOR;
  context.strokeStyle = ERASE_COLOR;
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
  if (!viewport || !viewWidth.value || !viewHeight.value) return;
  // 弹窗还在过渡时容器尺寸可能是 0：此时保持 100%，别把图缩成一个点
  if (viewport.clientWidth < 32 || viewport.clientHeight < 32) {
    zoom.value = 1;
    return;
  }
  const scale = Math.min(
    (viewport.clientWidth - 24) / viewWidth.value,
    (viewport.clientHeight - 24) / viewHeight.value,
    1
  );
  zoom.value = Math.max(scale, 0.02);
}

/** 旋转视图（90° 一档）。只动显示，不改地图数据。 */
function rotateBy(step: number) {
  if (!baseImage) return;
  setRotation(rotation.value + step);
}

// 角度一变（按钮 / 拖动）：外接矩形会变，重建 canvas 再把底图、笔画、禁止区域
// 重画回原位；zoom 与滚动位置保留，只转图不跳画面。
watch(rotation, () => {
  if (!baseImage) return;
  resizeCanvasForView();
  render();
  // 拖动旋转时把地图中心钉在手势开始时的屏幕位置（画布 CSS 尺寸要下一帧才刷新）
  if (rotateState) nextTick(applyRotatePin);
});

// 序号字号按 zoom 反向缩放（屏幕上恒定大小）：缩放后要重画一次
watch(zoom, () => {
  if (baseImage) render();
});

/** 旋转后外接矩形会往外撑：把地图中心钉回手势开始时那个屏幕位置。 */
function applyRotatePin() {
  const viewport = viewportRef.value;
  if (!viewport || !rotateState) return;
  const center = rasterCenterInCanvas();
  viewport.scrollLeft =
    center.x * zoom.value + CANVAS_MARGIN - rotateState.pin.x;
  viewport.scrollTop =
    center.y * zoom.value + CANVAS_MARGIN - rotateState.pin.y;
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

/** 屏幕坐标 → 旋转后画布坐标（canvas 像素，不经过旋转逆变换） */
function toCanvasPoint(event: PointerEvent) {
  const canvas = canvasRef.value;
  if (!canvas) return { x: 0, y: 0 };
  const rect = canvas.getBoundingClientRect();
  if (!rect.width || !rect.height) return { x: 0, y: 0 };
  return {
    x: ((event.clientX - rect.left) / rect.width) * canvas.width,
    y: ((event.clientY - rect.top) / rect.height) * canvas.height
  };
}

function onPointerDown(event: PointerEvent) {
  if (event.button !== 0) return;
  // 旋转的起手交给 viewport（canvas 外面那块深色区域也能转，且事件会冒泡上来）
  if (tool.value === "rotate") return;
  // 点选 / 点删在**按下**时处理：平移会给 viewport 设 pointer capture，
  // 随后那次 click 的 target 不一定还是 canvas，挂在 click 上会漏。
  if (tool.value === "pan" || tool.value === "remove") {
    if (tool.value === "remove") {
      if (canEdit.value) removeAreaAt(event);
    } else {
      // 已发布 / 已归档的图也允许点一下看是哪块
      selectAreaAt(event);
    }
    return;
  }
  if (!canEdit.value) return;
  if (tool.value === "forbidden") {
    const canvas = canvasRef.value;
    if (!canvas) return;
    // 旋转后画布四角是空的：在空区起手不画框
    const view = toCanvasPoint(event);
    if (!inRaster(canvasToRasterRaw(view))) return;
    canvas.setPointerCapture(event.pointerId);
    draftingRect.value = {
      x0: view.x,
      y0: view.y,
      x1: view.x,
      y1: view.y
    };
    render();
    return;
  }
  if (tool.value !== "eraser") return;
  const canvas = canvasRef.value;
  if (!canvas) return;
  // 旋转后画布四角是空的：在空区起手不落笔
  const view = toCanvasPoint(event);
  const raw = canvasToRasterRaw(view);
  if (!inRaster(raw)) return;
  canvas.setPointerCapture(event.pointerId);
  drawing = true;
  const point = canvasToRaster(view);
  currentStroke = { points: [point], radius: brushRadius.value };
  strokes.push(currentStroke);
  dirty.value = true;
  if (ctx) {
    const stroke = currentStroke;
    drawOnRaster(ctx, context => paintStroke(context, stroke));
  }
}

function onPointerMove(event: PointerEvent) {
  if (tool.value === "rotate") {
    updateRotateDrag(event);
    return;
  }
  const view = toCanvasPoint(event);
  const point = canvasToRaster(view);
  cursor.value = {
    x: point.x,
    y: point.y,
    // 鼠标在旋转后画布的空角上时，光标圆不该被拉到地图边缘
    visible: tool.value === "eraser" && inRaster(canvasToRasterRaw(view))
  };
  if (tool.value === "forbidden" && draftingRect.value) {
    draftingRect.value = {
      ...draftingRect.value,
      x1: view.x,
      y1: view.y
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
    // 只补画新增的一段，避免每次重绘整张图（要套上视图旋转，否则 90° 后会画错位置）
    const radius = currentStroke.radius ?? brushRadius.value;
    const from = last;
    const to = next;
    drawOnRaster(ctx, context => {
      context.fillStyle = ERASE_COLOR;
      context.strokeStyle = ERASE_COLOR;
      context.lineWidth = radius * 2;
      context.lineCap = "round";
      context.beginPath();
      context.moveTo(from.x, from.y);
      context.lineTo(to.x, to.y);
      context.stroke();
    });
  }
}

function onPointerUp(event: PointerEvent) {
  if (tool.value === "rotate") {
    // 事件会从 canvas 冒泡到 viewport，这里只收尾一次
    if (rotateState) {
      rotateState = null;
      canvasRef.value?.releasePointerCapture?.(event.pointerId);
    }
    return;
  }
  if (tool.value === "forbidden" && draftingRect.value) {
    const rect = draftingRect.value;
    draftingRect.value = null;
    canvasRef.value?.releasePointerCapture?.(event.pointerId);
    const area = createAreaFromDraft(rect);
    if (area) pendingAreas.value.push(area);
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

/** 拖动平移（浏览模式）/ 手动旋转视图（旋转模式） */
function onViewportPointerDown(event: PointerEvent) {
  if (event.button !== 0) return;
  if (tool.value === "rotate") {
    // 旋转只动视图、不改地图数据：已发布 / 已归档的图也允许转
    startRotateDrag(event);
    return;
  }
  if (tool.value !== "pan") return;
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
  // 优先撤销还没保存的禁止区域，再撤销「刚删掉的已有区域」，最后撤销笔画
  if (pendingAreas.value.length) {
    pendingAreas.value.pop();
    render();
    return;
  }
  if (removedAreaIds.value.length) {
    removedAreaIds.value.pop();
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
  activeAreaKey.value = null;
  render();
}

/** 一次删掉本次视图里的全部区域（保存后才落盘，撤销 / 放弃都能退回）。 */
function clearAllAreas() {
  if (!canEdit.value || !areaRows.value.length) return;
  ElMessageBox.confirm(
    `确定要删除全部 ${areaRows.value.length} 块禁止区域？点「保存」后栅格才会按建图原图还原（没保存前可用「放弃」整体退回）。`,
    "清空禁止区域",
    { type: "warning", confirmButtonText: "全部删除", cancelButtonText: "取消" }
  )
    .then(() => {
      pendingAreas.value = [];
      removedAreaIds.value = savedAreas.value
        .map(area => area.id)
        .filter((id): id is number => typeof id === "number");
      activeAreaKey.value = null;
      render();
    })
    .catch(() => {});
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
      const { code, msg, data } = await dogMapErase(row.id, strokes);
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
        activeAreaKey.value = null;
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
          <el-radio-button value="remove" :disabled="!canEdit">
            <el-icon><DeleteIcon /></el-icon>
            删区域
          </el-radio-button>
          <el-radio-button value="rotate">
            <el-icon><RotateIcon /></el-icon>
            旋转
          </el-radio-button>
        </el-radio-group>

        <el-tooltip
          v-if="tool === 'rotate'"
          content="在图上按住左键绕着中心拖动 = 手动旋转视图。拖到 0 / 90 / 180 / 270 附近会自动吸附；也可以直接用右侧的 ±90° 按钮。只转显示，不改地图数据。"
          placement="top"
        >
          <span class="rotate-hint">
            按住拖动旋转 · 当前 {{ rotationText }}°
          </span>
        </el-tooltip>

        <el-tooltip
          v-if="tool === 'forbidden'"
          content="在图上按住左键拖出一个矩形 = 禁止区域（栅格会被刷成「占据」，导航规划绕开它）。在图上点已有的红框可以选中它（与右侧列表联动）。保存后点「下发到狗体」写回本体，遥控器上也会看到这块红色区域。"
          placement="top"
        >
          <span class="forbidden-hint">
            拖拽画矩形 · 已有 {{ activeAreas.length }} 块
          </span>
        </el-tooltip>

        <el-tooltip
          v-if="tool === 'remove'"
          content="在图上点一下已有的红色区域就能删掉它（也可以点右侧列表行的「删除」）。保存后栅格才会按建图原图还原；删错了用「撤销」找回。"
          placement="top"
        >
          <span class="forbidden-hint">
            在图上点哪块删哪块 · 已有 {{ activeAreas.length }} 块
          </span>
        </el-tooltip>

        <el-tooltip
          v-if="tool === 'eraser'"
          content="橡皮擦只把像素擦成可通行 (254)，与遥控器一致；要标禁行请用「禁止区域」。"
          placement="top"
        >
          <span class="erase-hint">擦成可通行 (254)</span>
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

        <div v-if="activeAreas.length" class="forbidden-count">
          禁止区域 {{ activeAreas.length }} 块（画布上标着序号，右侧可定位 /
          删除）
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
          <el-button
            size="small"
            :icon="useRenderIcon(RotateLeft)"
            title="逆时针旋转 90°（只转视图，不改地图）"
            @click="rotateBy(-90)"
          />
          <el-button
            size="small"
            :icon="useRenderIcon(RotateRight)"
            title="顺时针旋转 90°（只转视图，不改地图）"
            @click="rotateBy(90)"
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
        <span v-if="rotation" class="rotated">
          视图已旋转 {{ rotationText }}°（不影响地图数据）
        </span>
        <span>设备编号：{{ currentMap?.deviceCode || "手工上传" }}</span>
        <span v-if="currentMap?.sharedDeviceCodes?.length">
          共享设备：{{ currentMap.sharedDeviceCodes.join("、") }}
        </span>
        <span>版本：{{ currentMap?.mapVersion || "-" }}</span>
        <span v-if="dirtyAny" class="dirty">有未保存的擦除 / 禁止区域</span>
      </div>

      <div class="map-body">
        <div
          ref="viewportRef"
          class="map-viewport"
          :class="{
            'is-pan': tool === 'pan',
            'is-rotate': tool === 'rotate',
            'is-erase': tool === 'eraser' && canEdit,
            'is-remove': tool === 'remove' && canEdit
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
                width: `${viewWidth * zoom}px`,
                height: `${viewHeight * zoom}px`
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

        <div v-if="areaRows.length" class="area-panel">
          <div class="area-panel-head">
            <span>禁止区域 {{ areaRows.length }} 块</span>
            <el-button
              size="small"
              type="danger"
              link
              :disabled="!canEdit"
              @click="clearAllAreas"
              >清空</el-button
            >
          </div>
          <div ref="areaRowsRef" class="area-rows">
            <div
              v-for="row in areaRows"
              :key="row.key"
              :data-key="row.key"
              class="area-row"
              :class="{ active: row.key === activeAreaKey }"
              @mouseenter="highlightArea(row)"
              @mouseleave="clearHighlight"
              @click="highlightArea(row, true)"
            >
              <span class="no">{{ row.no }}</span>
              <span class="text">
                <span class="size">{{ row.sizeText }}</span>
                <span class="muted">{{ row.centerText }}</span>
              </span>
              <el-button
                size="small"
                type="danger"
                link
                :disabled="!canEdit"
                @click.stop="removeArea(row.area)"
                >删除</el-button
              >
            </div>
          </div>
          <div class="area-panel-foot">
            删除后需点「保存」才会按建图原图还原该块栅格；没保存可用「放弃」整体退回。
          </div>
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

  .erase-hint {
    font-size: var(--dog-font-sm);
    color: var(--dog-text-secondary);
    white-space: nowrap;
  }

  .forbidden-count {
    font-size: var(--dog-font-sm);
    color: var(--el-color-danger);
    white-space: nowrap;
  }

  .spacer {
    flex: 1;
  }

  .rotate-hint {
    font-size: var(--dog-font-sm);
    color: var(--el-color-primary);
    white-space: nowrap;
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

  .rotated {
    color: var(--el-color-primary);
  }
}

.map-body {
  display: flex;
  gap: 10px;
  align-items: stretch;
  min-height: 0;
}

.area-panel {
  display: flex;
  flex: 0 0 268px;
  flex-direction: column;
  max-height: 62vh;
  border: 1px solid var(--el-border-color);
  border-radius: 6px;

  .area-panel-head {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: space-between;
    padding: 6px 8px;
    font-size: var(--dog-font-sm);
    font-weight: 600;
    border-bottom: 1px solid var(--el-border-color-lighter);
  }

  .area-rows {
    flex: 1;
    overflow-y: auto;
  }

  .area-row {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 5px 8px;
    cursor: pointer;
    border-bottom: 1px solid var(--el-border-color-lighter);

    &:hover {
      background-color: var(--el-color-danger-light-9);
    }

    &.active {
      background-color: var(--el-color-danger-light-8);
      box-shadow: inset 3px 0 0 var(--el-color-danger);
    }

    .no {
      display: inline-flex;
      flex: none;
      align-items: center;
      justify-content: center;
      width: 20px;
      height: 20px;
      font-size: var(--dog-font-xs);
      font-weight: 600;
      color: #fff;
      background-color: var(--el-color-danger);
      border-radius: 50%;
    }

    .text {
      display: flex;
      flex: 1;
      flex-direction: column;
      min-width: 0;
      font-size: var(--dog-font-xs);
      line-height: 1.35;
      color: var(--dog-text-primary);

      .muted {
        color: var(--dog-text-secondary);
      }
    }
  }

  .area-panel-foot {
    flex: none;
    padding: 6px 8px;
    font-size: var(--dog-font-xs);
    line-height: 1.4;
    color: var(--dog-text-secondary);
    border-top: 1px solid var(--el-border-color-lighter);
  }
}

.map-viewport {
  position: relative;
  flex: 1;
  min-width: 0;
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

  &.is-remove {
    cursor: crosshair;
  }

  &.is-rotate {
    cursor: grab;

    &:active {
      cursor: grabbing;
    }
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
