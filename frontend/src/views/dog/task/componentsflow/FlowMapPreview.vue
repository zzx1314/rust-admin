<script setup lang="ts">
import type { PropType } from "vue";
import { computed, ref } from "vue";
import { dogMapGetAll, dogMapGetImage } from "@/api/dogMap";
import { dogRouteGetById } from "@/api/dogRoute";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";
import type {
  ActionMarker,
  FlowEdge,
  FlowNode,
  FlowPoint,
  MarkerPlan,
  RouteMap,
  RoutePoint
} from "./flowMapMarkers";
import { layoutMarkerPills, planFlowMarkers } from "./flowMapMarkers";

const props = defineProps({
  visible: { type: Boolean, default: false },
  /** 流程画布节点快照（含任务节点的 routeId / pathString / taskInfo） */
  nodes: { type: Array as PropType<FlowNode[]>, default: () => [] },
  /** 流程画布连线快照，用于还原任务先后顺序与动作归属 */
  edges: { type: Array as PropType<FlowEdge[]>, default: () => [] }
});

const emit = defineEmits(["update:visible"]);

/** 动作类型 → 标记颜色 */
const ACTION_COLORS: Record<string, string> = {
  TAKEOFF: "#67c23a",
  HOVER: "#e6a23c",
  GOTO: "#409eff",
  PHOTO: "#9b59b6",
  LAND: "#f56c6c",
  SERVICE_START_YOLO: "#13c2c2",
  SERVICE_STOP_YOLO: "#13c2c2",
  SERVICE_START_RTSP: "#2f9e44",
  SERVICE_STOP_RTSP: "#2f9e44"
};
const ERROR_COLOR = "#c0392b";
const FALLBACK_COLOR = "#606266";

const mapId = ref<number | null>(null);
const mapOptions = ref<Array<{ label: string; value: number }>>([]);
const canvasRef = ref<HTMLCanvasElement | null>(null);
const resolution = ref(1);
const origin = ref<number[]>([0, 0]);
/** 地图图片放大倍数，与路线规划页保持一致 */
const scale = 2;
const loading = ref(false);
const emptyHint = ref("");

const emptyPlan = (): MarkerPlan => ({
  markers: [],
  rows: [],
  segments: [],
  routeLines: [],
  taskCount: 0
});
const plan = ref<MarkerPlan>(emptyPlan());

/** 图例行（右侧动作列表），带颜色 */
const legendRows = computed(() =>
  plan.value.rows.map(row => ({
    ...row,
    color: row.isError ? ERROR_COLOR : ACTION_COLORS[row.type] || FALLBACK_COLOR
  }))
);

type RouteInfo = { name: string; mapId: number | null; points: RoutePoint[] };
const routeCache = new Map<number, RouteInfo>();

let ctx: CanvasRenderingContext2D | null = null;
const img = new Image();
let imgReady = false;
let project: (point: FlowPoint) => { x: number; y: number } = point => ({
  x: point[0],
  y: point[1]
});

function close() {
  emit("update:visible", false);
}

function colorOf(marker: ActionMarker) {
  if (marker.isError) return ERROR_COLOR;
  return ACTION_COLORS[marker.type] || FALLBACK_COLOR;
}

function toRoutePoint(raw: any, index: number): RoutePoint | null {
  if (Array.isArray(raw) && raw.length >= 2) {
    const x = Number(raw[0]);
    const y = Number(raw[1]);
    if (Number.isFinite(x) && Number.isFinite(y)) {
      return { wpId: index + 1, x, y };
    }
    return null;
  }
  if (raw && typeof raw === "object") {
    const x = Number(raw.x ?? raw.lon);
    const y = Number(raw.y ?? raw.lat);
    if (Number.isFinite(x) && Number.isFinite(y)) {
      return { wpId: index + 1, x, y };
    }
  }
  return null;
}

/** 读取航线航点（与后端一致：wpId 为 routeData 数组下标 + 1） */
async function loadRouteInfo(routeId: number): Promise<RouteInfo> {
  const cached = routeCache.get(routeId);
  if (cached) return cached;

  const info: RouteInfo = { name: "", mapId: null, points: [] };
  try {
    const res = await dogRouteGetById(routeId);
    if (res.code === SUCCESS && res.data) {
      info.name = res.data.routeName || "";
      // 航线表的 modelId 存的是「关联地图」id，与路线规划页一致
      const modelId = Number(res.data.modelId);
      if (Number.isFinite(modelId) && modelId > 0) info.mapId = modelId;
      if (res.data.routeData) {
        const parsed = JSON.parse(res.data.routeData);
        if (Array.isArray(parsed)) {
          info.points = parsed
            .map((raw, index) => toRoutePoint(raw, index))
            .filter((point): point is RoutePoint => point !== null);
        }
      }
    }
  } catch {
    // 航线读取失败按无航点处理
  }
  routeCache.set(routeId, info);
  return info;
}

/** 读取任务节点引用的全部航线航点，并顺带得到默认地图 */
async function loadRoutes(nodes: FlowNode[]) {
  const routeIds = new Set<number>();
  nodes.forEach(node => {
    if (node.type !== "task") return;
    const routeId = Number(node.data?.routeId) || 0;
    if (routeId > 0) routeIds.add(routeId);
  });

  const routes: RouteMap = new Map();
  let defaultMapId: number | null = null;
  for (const routeId of routeIds) {
    const info = await loadRouteInfo(routeId);
    routes.set(routeId, info.points);
    if (!defaultMapId && info.mapId) defaultMapId = info.mapId;
  }
  return { routes, defaultMapId };
}

async function loadMapOptions() {
  try {
    const res = await dogMapGetAll();
    if (res.code === SUCCESS && res.data) {
      mapOptions.value = (res.data as any[]).map(item => ({
        label: item.mapName || `地图${item.id}`,
        value: Number(item.id)
      }));
    }
  } catch {
    // 地图列表读取失败不阻塞标记展示
  }
}

/** 汇总画布上的任务 → 航段 → 动作，生成地图标记 */
async function build() {
  loading.value = true;
  routeCache.clear();
  plan.value = emptyPlan();
  emptyHint.value = "";

  await loadMapOptions();
  const { routes, defaultMapId } = await loadRoutes(props.nodes);
  plan.value = planFlowMarkers(props.nodes, props.edges, routes);

  if (!plan.value.taskCount) {
    emptyHint.value = "画布中还没有任务节点，请先在任务节点中选择航线与航线段";
  } else if (!plan.value.rows.length) {
    emptyHint.value = "任务节点下还没有动作节点，暂无可标记的动作";
  }

  // 优先用航线关联的地图，其次沿用上次选择 / 第一张地图
  mapId.value =
    defaultMapId ?? mapId.value ?? mapOptions.value[0]?.value ?? null;

  await loadMapImage();
}

/** 弹窗打开后再解析并绘制（此时 canvas 已挂载） */
async function openMap() {
  try {
    await build();
  } catch {
    message("地图标记生成失败", { type: "error" });
  } finally {
    loading.value = false;
  }
}

/* ------------------------------ 地图绘制 ------------------------------ */

function resetCanvas() {
  const canvas = canvasRef.value;
  if (!canvas) return;
  if (imgReady && img.naturalWidth > 0) {
    canvas.width = img.width * scale;
    canvas.height = img.height * scale;
  } else {
    canvas.width = 900;
    canvas.height = 560;
  }
  ctx = canvas.getContext("2d");
  computeProjection();
}

/** 有地图时按栅格分辨率 / 原点换算；无地图时按坐标范围自适应铺满画布 */
function computeProjection() {
  const canvas = canvasRef.value;
  if (imgReady && canvas) {
    project = ([wx, wy]) => ({
      x: ((wx - origin.value[0]) / resolution.value) * scale,
      y: (img.height - (wy - origin.value[1]) / resolution.value) * scale
    });
    return;
  }

  const w = canvas?.width || 900;
  const h = canvas?.height || 560;
  const all: FlowPoint[] = [
    ...plan.value.routeLines.flat(),
    ...plan.value.segments.flatMap(segment => segment.points),
    ...plan.value.markers.map(marker => marker.point)
  ];
  if (!all.length) {
    project = ([x, y]) => ({ x: w / 2 + x, y: h / 2 - y });
    return;
  }

  const xs = all.map(point => point[0]);
  const ys = all.map(point => point[1]);
  const minX = Math.min(...xs);
  const maxX = Math.max(...xs);
  const minY = Math.min(...ys);
  const maxY = Math.max(...ys);
  const spanX = maxX - minX || 1;
  const spanY = maxY - minY || 1;
  const margin = 70;
  const k = Math.min((w - margin * 2) / spanX, (h - margin * 2) / spanY);
  const offsetX = (w - spanX * k) / 2;
  const offsetY = (h - spanY * k) / 2;

  project = ([x, y]) => ({
    x: offsetX + (x - minX) * k,
    y: h - offsetY - (y - minY) * k
  });
}

async function loadMapImage() {
  if (!mapId.value) {
    imgReady = false;
    resetCanvas();
    redraw();
    return;
  }
  try {
    const res = await dogMapGetImage(mapId.value);
    if (res.code !== SUCCESS || !res.data?.image) {
      imgReady = false;
      resetCanvas();
      redraw();
      return;
    }
    const data = res.data;
    resolution.value = data.resolution > 0 ? data.resolution : 1;
    origin.value = data.origin?.length ? data.origin : [0, 0];
    await new Promise<void>(resolve => {
      img.onload = () => resolve();
      img.onerror = () => resolve();
      img.src = data.image;
    });
    imgReady = img.naturalWidth > 0;
  } catch {
    imgReady = false;
  }
  resetCanvas();
  redraw();
}

function drawRoundRect(
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number
) {
  context.beginPath();
  context.moveTo(x + r, y);
  context.lineTo(x + w - r, y);
  context.quadraticCurveTo(x + w, y, x + w, y + r);
  context.lineTo(x + w, y + h - r);
  context.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
  context.lineTo(x + r, y + h);
  context.quadraticCurveTo(x, y + h, x, y + h - r);
  context.lineTo(x, y + r);
  context.quadraticCurveTo(x, y, x + r, y);
  context.closePath();
}

function drawFallbackGrid(w: number, h: number) {
  if (!ctx) return;
  ctx.fillStyle = "#fafafa";
  ctx.fillRect(0, 0, w, h);
  ctx.strokeStyle = "#e8e8e8";
  ctx.lineWidth = 0.5;
  for (let x = 0; x <= w; x += 40) {
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, h);
    ctx.stroke();
  }
  for (let y = 0; y <= h; y += 40) {
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(w, y);
    ctx.stroke();
  }
  ctx.fillStyle = "#909399";
  ctx.font = "13px Arial";
  ctx.fillText("未选择地图，航点坐标按范围自适应显示", 12, 22);
}

function drawRouteLines() {
  if (!ctx) return;
  plan.value.routeLines.forEach(line => {
    if (line.length < 2) return;
    ctx!.strokeStyle = "rgba(144, 147, 153, 0.55)";
    ctx!.lineWidth = 1.5;
    ctx!.beginPath();
    line.forEach((point, index) => {
      const p = project(point);
      if (index === 0) ctx!.moveTo(p.x, p.y);
      else ctx!.lineTo(p.x, p.y);
    });
    ctx!.stroke();
  });
}

function drawSegments() {
  if (!ctx) return;
  plan.value.segments.forEach(segment => {
    if (segment.points.length > 1) {
      ctx!.strokeStyle = "green";
      ctx!.lineWidth = 2.5;
      ctx!.beginPath();
      segment.points.forEach((point, index) => {
        const p = project(point);
        if (index === 0) ctx!.moveTo(p.x, p.y);
        else ctx!.lineTo(p.x, p.y);
      });
      ctx!.stroke();
    }

    // 航点
    segment.points.forEach((point, index) => {
      const p = project(point);
      ctx!.beginPath();
      ctx!.arc(p.x, p.y, 6, 0, Math.PI * 2);
      ctx!.fillStyle = "black";
      ctx!.fill();
      ctx!.beginPath();
      ctx!.arc(p.x, p.y, 4, 0, Math.PI * 2);
      ctx!.fillStyle = "yellow";
      ctx!.fill();

      // 航点号（与后端一致：wpId = routeData 下标 + 1）
      ctx!.fillStyle = "#c0392b";
      ctx!.font = "bold 12px Arial";
      ctx!.textAlign = "center";
      ctx!.fillText(`${segment.ids[index] ?? index + 1}`, p.x, p.y - 10);
      ctx!.textAlign = "start";

      if (index === 0) {
        ctx!.fillStyle = "#303133";
        ctx!.font = "bold 12px Arial";
        ctx!.fillText(segment.label, p.x + 10, p.y + 18);
      }
    });
  });
}

/**
 * 绘制动作标记：同一航段 / 同一航点上的多个动作竖排错开（带引线），
 * 不同锚点的标签也会互相避让，避免动作被覆盖。
 */
function drawMarkers() {
  const canvas = canvasRef.value;
  if (!ctx || !canvas) return;

  // 1. 按画布字体测量每个动作标签的尺寸
  const boxHeight = 24;
  ctx.font = "13px Arial";
  const pending = plan.value.markers.map(marker => {
    const anchor = project(marker.point);
    return {
      marker,
      anchorX: anchor.x,
      anchorY: anchor.y,
      width: ctx!.measureText(marker.action).width + 28,
      height: boxHeight
    };
  });
  const pills = layoutMarkerPills(pending, {
    width: canvas.width,
    height: canvas.height
  });
  if (!pills.length) return;

  // 2. 锚点引线（虚线）
  ctx.strokeStyle = "rgba(96, 98, 102, 0.5)";
  ctx.lineWidth = 1;
  ctx.setLineDash([3, 3]);
  pills.forEach(pill => {
    const targetX = pill.side === "right" ? pill.x : pill.x + pill.width;
    const targetY = pill.y + pill.height / 2;
    ctx!.beginPath();
    ctx!.moveTo(pill.anchorX, pill.anchorY);
    ctx!.lineTo(targetX, targetY);
    ctx!.stroke();
  });
  ctx.setLineDash([]);

  // 3. 动作标签
  pills.forEach(pill => {
    const color = colorOf(pill.marker);
    const centerY = pill.y + pill.height / 2;

    ctx!.fillStyle = "rgba(255, 255, 255, 0.94)";
    drawRoundRect(ctx!, pill.x, pill.y, pill.width, pill.height, 6);
    ctx!.fill();
    ctx!.strokeStyle = color;
    ctx!.lineWidth = 1.5;
    ctx!.stroke();

    ctx!.beginPath();
    ctx!.arc(pill.x + 10, centerY, 3.5, 0, Math.PI * 2);
    ctx!.fillStyle = color;
    ctx!.fill();

    ctx!.fillStyle = "#303133";
    ctx!.fillText(pill.marker.action, pill.x + 18, centerY + 4.5);
  });

  // 4. 锚点圆点（同一锚点只画一次，最后绘制避免被标签挡住）
  const anchors = new Map<string, { x: number; y: number; color: string }>();
  pills.forEach(pill => {
    const key = `${pill.anchorX.toFixed(1)},${pill.anchorY.toFixed(1)}`;
    if (!anchors.has(key)) {
      anchors.set(key, {
        x: pill.anchorX,
        y: pill.anchorY,
        color: colorOf(pill.marker)
      });
    }
  });
  anchors.forEach(anchor => {
    ctx!.beginPath();
    ctx!.arc(anchor.x, anchor.y, 7, 0, Math.PI * 2);
    ctx!.fillStyle = "#ffffff";
    ctx!.fill();
    ctx!.beginPath();
    ctx!.arc(anchor.x, anchor.y, 5, 0, Math.PI * 2);
    ctx!.fillStyle = anchor.color;
    ctx!.fill();
  });
}

function redraw() {
  const canvas = canvasRef.value;
  if (!ctx || !canvas) return;
  const w = canvas.width;
  const h = canvas.height;
  ctx.clearRect(0, 0, w, h);
  if (imgReady) {
    ctx.drawImage(img, 0, 0, w, h);
  } else {
    drawFallbackGrid(w, h);
  }
  drawRouteLines();
  drawSegments();
  drawMarkers();
}
</script>

<template>
  <el-dialog
    :model-value="visible"
    title="流程航线地图"
    width="82%"
    top="6vh"
    append-to-body
    destroy-on-close
    @opened="openMap"
    @update:model-value="close"
  >
    <div class="flow-map">
      <div class="toolbar">
        <el-select
          v-model="mapId"
          placeholder="选择地图"
          clearable
          style="width: 220px"
          @change="loadMapImage"
        >
          <el-option
            v-for="opt in mapOptions"
            :key="opt.value"
            :label="opt.label"
            :value="opt.value"
          />
        </el-select>
        <span class="hint">
          航点来自任务节点所选航线，动作按所属航线段标记（GOTO 标记在目标航点）
        </span>
      </div>

      <div v-loading="loading" class="content">
        <div class="map-container">
          <canvas ref="canvasRef" />
        </div>
        <div class="legend-panel">
          <el-empty
            v-if="!legendRows.length"
            :description="emptyHint || '暂无动作标记'"
            :image-size="80"
          />
          <template v-else>
            <div class="legend-title">动作标记（{{ legendRows.length }}）</div>
            <el-scrollbar class="legend-list">
              <div
                v-for="(row, index) in legendRows"
                :key="index"
                class="legend-item"
              >
                <span class="legend-dot" :style="{ background: row.color }" />
                <div class="legend-text">
                  <div class="legend-head">
                    <el-tag size="small" type="info">{{ row.task }}</el-tag>
                    <span
                      class="legend-action"
                      :class="{ 'is-error': row.isError }"
                    >
                      {{ row.action }}
                    </span>
                  </div>
                  <div v-if="row.desc" class="legend-desc">{{ row.desc }}</div>
                </div>
              </div>
            </el-scrollbar>
          </template>
        </div>
      </div>
    </div>
    <template #footer>
      <el-button @click="close">关闭</el-button>
    </template>
  </el-dialog>
</template>

<style scoped lang="scss">
.flow-map {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  align-items: center;
}

.hint {
  font-size: 12px;
  color: var(--el-text-color-secondary);
}

.content {
  display: flex;
  gap: 10px;
  height: 62vh;
}

.map-container {
  display: flex;
  flex: 1;
  align-items: center;
  justify-content: center;
  overflow: auto;
  background: #fff;
  border: 1px solid var(--el-border-color);
  border-radius: 4px;
}

canvas {
  display: block;
}

.legend-panel {
  display: flex;
  flex: 0 0 320px;
  flex-direction: column;
  padding: 8px;
  overflow: hidden;
  border: 1px solid var(--el-border-color);
  border-radius: 4px;
}

.legend-title {
  margin-bottom: 6px;
  font-size: 13px;
  font-weight: 600;
  color: var(--el-text-color-primary);
}

.legend-list {
  flex: 1;
  height: 100%;
}

.legend-item {
  display: flex;
  gap: 8px;
  padding: 6px 2px;
  border-bottom: 1px solid var(--el-border-color-lighter);
}

.legend-dot {
  flex-shrink: 0;
  width: 10px;
  height: 10px;
  margin-top: 5px;
  border-radius: 50%;
}

.legend-text {
  flex: 1;
  min-width: 0;
}

.legend-head {
  display: flex;
  gap: 6px;
  align-items: center;
}

.legend-action {
  font-size: 13px;
  font-weight: 600;
  color: var(--el-text-color-primary);
}

.legend-action.is-error {
  color: #c0392b;
}

.legend-desc {
  margin-top: 2px;
  font-size: 12px;
  line-height: 1.4;
  color: var(--el-text-color-secondary);
  word-break: break-all;
}
</style>
