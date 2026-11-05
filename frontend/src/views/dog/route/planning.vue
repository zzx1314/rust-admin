<template>
  <div class="main">
    <el-button type="primary" link :icon="useRenderIcon(Back)" @click="back()">
      返回
    </el-button>
    <div style="margin-top: 10px">
      <el-select
        v-model="mapId"
        placeholder="选择地图"
        clearable
        style="
          width: 200px;
          margin-right: 10px;
          margin-bottom: 10px;
          margin-left: 10px;
        "
        @change="loadMapImage"
      >
        <el-option
          v-for="opt in mapOptions"
          :key="opt.value"
          :label="opt.label"
          :value="opt.value"
        />
      </el-select>
      <el-button type="primary" style="margin-bottom: 10px" @click="savePath">
        保存路径数据
      </el-button>
      <el-button type="danger" style="margin-bottom: 10px" @click="clearPath">
        清除路径
      </el-button>
      <span class="hint">左键添加航点，右键结束绘制</span>
    </div>

    <div class="vertical-container">
      <div class="map-container">
        <canvas
          ref="canvasRef"
          @mousedown="handleMouseDown"
          @mousemove="handleMouseMove"
          @contextmenu.prevent
        />
      </div>
      <div class="info-panel">
        <el-card>
          <template #header>路线信息</template>
          <div class="route-info">
            <p><strong>路线名称：</strong>{{ routeName }}</p>
            <p><strong>路线类型：</strong>{{ routeType }}</p>
            <p><strong>航点数量：</strong>{{ waypoints.length }}</p>
            <p><strong>分辨率：</strong>{{ resolution }} m/px</p>
            <p><strong>原点：</strong>[{{ origin.join(", ") }}]</p>
          </div>
        </el-card>
        <el-card style="margin-top: 10px">
          <template #header>航点列表</template>
          <div class="waypoint-list">
            <div
              v-for="(wp, index) in waypoints"
              :key="index"
              class="waypoint-item"
            >
              <span class="wp-index">{{ index === 0 ? "起" : index }}</span>
              <span class="wp-coords">({{ wp[0] }}, {{ wp[1] }})</span>
            </div>
            <el-empty v-if="waypoints.length === 0" description="暂无航点" />
          </div>
        </el-card>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, nextTick } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useMultiTagsStoreHook } from "@/store/modules/multiTags";
import { dogMapGetAll, dogMapGetImage } from "@/api/dogMap";
import { dogRouteUpdate, dogRouteGetById } from "@/api/dogRoute";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import Back from "~icons/ep/back";

defineOptions({ name: "DogRoutePlanning" });

const route = useRoute();
const router = useRouter();

/**
 * 参数优先取路由 query；从标签页重新打开时 query 会丢失，
 * 此时回退到 sessionStorage 中的上下文。
 */
function resolveContext() {
  const query = route.query || {};
  if (query.id) {
    return {
      id: Number(query.id) || 0,
      name: (query.name as string) || "",
      type: (query.type as string) || ""
    };
  }
  const cached = sessionStorage.getItem("dog-route-planning");
  if (cached) {
    try {
      return JSON.parse(cached) as {
        id: number;
        name: string;
        type: string;
      };
    } catch {
      /* ignore malformed cache */
    }
  }
  return { id: 0, name: "", type: "" };
}

const context = resolveContext();
const routeId = ref(context.id);
const routeName = ref(context.name);
const routeType = ref(context.type);

const mapId = ref<number | null>(null);
const mapOptions = ref<Array<{ label: string; value: number }>>([]);
const canvasRef = ref<HTMLCanvasElement | null>(null);
const waypoints = ref<Array<[number, number]>>([]);
const drawing = ref(true);
const mousePos = ref<{ x: number; y: number } | null>(null);
/** 栅格分辨率（m/px）与原点（世界坐标），来自地图包 model.yaml */
const resolution = ref(1);
const origin = ref<number[]>([0, 0]);
/** 地图图片放大倍数，与参考实现一致 */
const scale = 2;

let ctx: CanvasRenderingContext2D | null = null;
const img = new Image();
let imgReady = false;

function loadMapOptions() {
  dogMapGetAll().then(res => {
    if (res.code === SUCCESS && res.data) {
      mapOptions.value = res.data.map(m => ({
        label: m.mapName || `地图${m.id}`,
        value: m.id
      }));
    }
  });
}

/** 加载地图包渲染出的栅格地图（PNG base64 + 分辨率 + 原点） */
function loadMapImage() {
  if (!mapId.value) {
    imgReady = false;
    resetCanvas();
    drawFallback();
    return;
  }
  dogMapGetImage(mapId.value).then(res => {
    if (res.code !== SUCCESS || !res.data) {
      message(res.msg || "地图加载失败", { type: "error" });
      imgReady = false;
      drawFallback();
      return;
    }
    const data = res.data;
    resolution.value = data.resolution > 0 ? data.resolution : 1;
    origin.value = data.origin?.length ? data.origin : [0, 0];
    img.src = data.image;
    img.onload = () => {
      imgReady = true;
      if (!canvasRef.value) return;
      canvasRef.value.width = img.width * scale;
      canvasRef.value.height = img.height * scale;
      ctx = canvasRef.value.getContext("2d");
      redraw();
    };
  });
}

function worldToPixel([wx, wy]: [number, number]) {
  const height = imgReady ? img.height : 500 / scale;
  return {
    x: ((wx - origin.value[0]) / resolution.value) * scale,
    y: (height - (wy - origin.value[1]) / resolution.value) * scale
  };
}

function pixelToWorld(p: { x: number; y: number }): [number, number] {
  const height = imgReady ? img.height : 500 / scale;
  const wx = origin.value[0] + (p.x / scale) * resolution.value;
  const wy = origin.value[1] + (height - p.y / scale) * resolution.value;
  return [Number(wx.toFixed(2)), Number(wy.toFixed(2))];
}

function getMousePos(e: MouseEvent) {
  const rect = canvasRef.value!.getBoundingClientRect();
  return {
    x: e.clientX - rect.left,
    y: e.clientY - rect.top
  };
}

function resetCanvas() {
  if (!canvasRef.value) return;
  canvasRef.value.width = 800;
  canvasRef.value.height = 500;
  ctx = canvasRef.value.getContext("2d");
}

/** 未选择地图时画一层淡网格，方便先放航点 */
function drawFallback() {
  if (!ctx || !canvasRef.value || imgReady) return;
  const w = canvasRef.value.width;
  const h = canvasRef.value.height;
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#fafafa";
  ctx.fillRect(0, 0, w, h);
  ctx.strokeStyle = "#e5e5e5";
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
  ctx.fillStyle = "#999";
  ctx.font = "13px Arial";
  ctx.fillText("请先选择地图（地图包需包含 model.pgm / model.yaml）", 12, 22);
  redrawWaypoints();
}

function redraw() {
  if (!ctx || !canvasRef.value) return;
  if (imgReady) {
    ctx.clearRect(0, 0, canvasRef.value.width, canvasRef.value.height);
    ctx.drawImage(img, 0, 0, canvasRef.value.width, canvasRef.value.height);
    // 世界坐标原点标记
    const zero = worldToPixel([0, 0]);
    ctx.fillStyle = "#ee0d51";
    ctx.beginPath();
    ctx.arc(zero.x, zero.y, 5, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = "#4982ee";
    ctx.font = "13px Arial";
    ctx.fillText("(0,0)", zero.x + 8, zero.y - 8);
    redrawWaypoints();
  } else {
    drawFallback();
  }
}

function redrawWaypoints() {
  if (!ctx) return;

  // 路径连线
  if (waypoints.value.length > 1) {
    ctx.strokeStyle = "green";
    ctx.lineWidth = 2;
    ctx.beginPath();
    const first = worldToPixel(waypoints.value[0]);
    ctx.moveTo(first.x, first.y);
    for (let i = 1; i < waypoints.value.length; i++) {
      const pt = worldToPixel(waypoints.value[i]);
      ctx.lineTo(pt.x, pt.y);
    }
    ctx.stroke();
  }

  // 航点
  waypoints.value.forEach((wp, index) => {
    const pt = worldToPixel(wp);

    ctx.beginPath();
    ctx.arc(pt.x, pt.y, 6, 0, Math.PI * 2);
    ctx.fillStyle = "black";
    ctx.fill();

    ctx.beginPath();
    ctx.arc(pt.x, pt.y, 5, 0, Math.PI * 2);
    ctx.fillStyle = "yellow";
    ctx.fill();

    ctx.fillStyle = "red";
    ctx.font = "13px Arial";
    ctx.textAlign = "center";
    ctx.fillText(`${index + 1}`, pt.x, pt.y - 10);
    ctx.textAlign = "start";
  });

  // 跟随鼠标的虚线预览
  if (drawing.value && mousePos.value && waypoints.value.length > 0) {
    const last = worldToPixel(waypoints.value[waypoints.value.length - 1]);
    ctx.strokeStyle = "rgba(0,0,255,0.5)";
    ctx.setLineDash([5, 5]);
    ctx.beginPath();
    ctx.moveTo(last.x, last.y);
    ctx.lineTo(mousePos.value.x, mousePos.value.y);
    ctx.stroke();
    ctx.setLineDash([]);
  }
}

function handleMouseDown(e: MouseEvent) {
  if (e.button === 0) {
    waypoints.value.push(pixelToWorld(getMousePos(e)));
    drawing.value = true;
    redraw();
  } else if (e.button === 2) {
    drawing.value = false;
    mousePos.value = null;
    redraw();
  }
}

function handleMouseMove(e: MouseEvent) {
  if (!drawing.value || waypoints.value.length === 0) return;
  mousePos.value = getMousePos(e);
  redraw();
}

function savePath() {
  if (waypoints.value.length === 0) {
    message("请先绘制路径", { type: "warning" });
    return;
  }
  dogRouteUpdate(routeId.value, {
    routeData: JSON.stringify(waypoints.value),
    modelId: mapId.value
  }).then(res => {
    if (res.code === SUCCESS) {
      message("保存成功！", { type: "success" });
    } else {
      message(res.msg || "保存失败！", { type: "error" });
    }
  });
}

function clearPath() {
  waypoints.value = [];
  drawing.value = true;
  mousePos.value = null;
  redraw();
}

function restoreRoute(dataStr: string) {
  try {
    const parsed = JSON.parse(dataStr);
    if (Array.isArray(parsed)) {
      waypoints.value = parsed as Array<[number, number]>;
      drawing.value = false;
    }
  } catch {
    console.warn("routeData 解析失败");
  }
}

function back() {
  const multiTagsStore = useMultiTagsStoreHook();
  router.push({ path: "/dog/route/index" });
  nextTick(() => {
    multiTagsStore.handleTags("splice", "/dog/route/planning/index");
  });
}

/** 从服务端拉取路线，用于刷新/从标签页重新打开时恢复上下文 */
async function loadRoute() {
  if (!routeId.value) return;
  const res = await dogRouteGetById(routeId.value);
  if (res.code !== SUCCESS || !res.data) return;
  const record = res.data;
  routeName.value = record.routeName || routeName.value;
  routeType.value = record.routeType || routeType.value;
  if (record.modelId) {
    mapId.value = record.modelId;
    loadMapImage();
  }
  if (record.routeData) restoreRoute(record.routeData);
}

onMounted(() => {
  loadMapOptions();

  nextTick(() => {
    resetCanvas();
    drawFallback();
    loadRoute();
  });
});
</script>

<style scoped lang="scss">
.main {
  position: relative;
  width: 100%;
  height: 100%;
}

.hint {
  margin-left: 8px;
  font-size: 13px;
  color: var(--el-text-color-secondary);
}

.vertical-container {
  display: flex;
  flex-direction: row;
  height: calc(100vh - 130px);
}

.map-container {
  display: flex;
  flex: 1;
  align-items: center;
  justify-content: center;
  margin: 10px;
  overflow: auto;
  border: 1px solid var(--el-border-color);
  border-radius: 4px;
}

.info-panel {
  flex: 0 0 300px;
  padding: 10px;
  overflow-y: auto;
}

canvas {
  display: block;
}

.route-info p {
  margin: 8px 0;
  font-size: 14px;
}

.waypoint-list {
  max-height: 300px;
  overflow-y: auto;
}

.waypoint-item {
  display: flex;
  align-items: center;
  padding: 6px 0;
  font-size: 13px;
  border-bottom: 1px solid var(--el-border-color-lighter);
}

.wp-index {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  margin-right: 8px;
  font-size: 12px;
  color: #fff;
  background: var(--el-color-primary);
  border-radius: 50%;
}

.wp-coords {
  color: var(--el-text-color-regular);
}
</style>
