<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { Device, MapRaster, MapVersion, Route } from "@/api/types";
import {
  CANVAS_HEIGHT,
  CANVAS_WIDTH,
  frameOf,
  mapToCanvas,
  parseMapPosition
} from "@/utils/coords";
import { parseRouteData } from "@/utils/format";
import RouteBaseMap from "./RouteBaseMap.vue";
import AppIcon from "./AppIcon.vue";

/**
 * 任务执行实时地图
 *
 * 用「路线地图版本」的米制坐标系把三样东西画在同一张图上：
 * 1. ROS 栅格底图（有地图包时）；
 * 2. 路线航点 / 航段（已走过 = 绿色实线，待执行 = 蓝色虚线）；
 * 3. 机器狗实时位置（`devices.position`，每次轮询刷新，靠相邻两帧算朝向）。
 *
 * 位置是设备心跳（默认 10s）上报的地图米制坐标，与下发到本体的
 * `goals[].position` 同一坐标系，所以只要本体载的就是这张图，点位就不会偏。
 */
const props = withDefaults(
  defineProps<{
    device?: Device | null;
    route?: Route | null;
    map?: MapVersion | null;
    raster?: MapRaster | null;
    /** 任务进度 0~100：用来区分「已走过 / 待执行」航段并高亮下一个目标点 */
    progress?: number;
    /** 角标文字，默认用设备编号 */
    label?: string;
  }>(),
  { device: null, route: null, map: null, raster: null, progress: 0, label: "" }
);

/** 当前任务的坐标几何（地图版本 → 米/像素 + 原点） */
const frame = computed(() => frameOf(props.map));

/** 航点（地图米）→ 画布像素；历史画布像素数据不可用时不画，避免点位张冠李戴 */
const points = computed(() => {
  const data = parseRouteData(props.route?.routeData);
  if (!props.route || data.coordFrame !== "map") return [];
  return data.points.map((point, index) => {
    const { x, y } = mapToCanvas(frame.value, point.x, point.y);
    return { index, name: point.name, cx: x, cy: y };
  });
});

const loopClosed = computed(() => {
  const data = parseRouteData(props.route?.routeData);
  return data.loop === "闭环" && Boolean(data.returnLeg) && points.value.length > 2;
});

/** 已完成的航点数（进度百分比 × 航点数） */
const doneCount = computed(() => {
  const total = points.value.length;
  if (!total) return 0;
  const ratio = Math.min(100, Math.max(0, props.progress ?? 0)) / 100;
  return Math.min(total, Math.round(ratio * total));
});

/** 下一个目标航点下标；全部走完为 -1 */
const nextIndex = computed(() =>
  doneCount.value >= points.value.length ? -1 : doneCount.value
);

function legDone(index: number) {
  // 第 index 段 = 第 index → index+1 个航点：下一个目标点之前的那一段才是走完的
  return index + 1 < doneCount.value;
}

/** 栅格底图（只对真实地图包生效；示意坐标系没有图） */
const rasterRect = computed(() => {
  const item = props.raster;
  if (!item || !item.width || !item.height || frame.value.schematic) return null;
  const scale = Math.min(CANVAS_WIDTH / item.width, CANVAS_HEIGHT / item.height);
  const width = item.width * scale;
  const height = item.height * scale;
  return {
    href: item.image,
    x: (CANVAS_WIDTH - width) / 2,
    y: (CANVAS_HEIGHT - height) / 2,
    width,
    height
  };
});

/** 上一帧位置：用来推朝向（心跳只有 x/y，没有朝向字段） */
const lastFix = ref<{ x: number; y: number } | null>(null);
const bearing = ref(0);

watch(
  () => [props.device?.id, props.device?.position],
  () => {
    const fix = parseMapPosition(props.device?.position);
    if (!fix) {
      lastFix.value = null;
      return;
    }
    const prev = lastFix.value;
    if (!prev) {
      lastFix.value = fix;
      return;
    }
    const dx = fix.x - prev.x;
    const dy = fix.y - prev.y;
    // 抖动 / 定位跳变不更新朝向：移动超过 0.15m 才认为是真的在走
    if (Math.hypot(dx, dy) > 0.15) {
      // 地图 y 向上、画布 y 向下：屏幕角度取反
      bearing.value = (Math.atan2(-dy, dx) * 180) / Math.PI;
      lastFix.value = fix;
    }
  },
  { immediate: true }
);

const dog = computed(() => {
  const fix = parseMapPosition(props.device?.position);
  if (!fix) return null;
  const { x, y } = mapToCanvas(frame.value, fix.x, fix.y);
  return {
    x,
    y,
    label: props.label || props.device?.deviceId || "",
    // 留 8px 容差：贴在边缘的点位仍然画出来
    offMap:
      x < -8 || y < -8 || x > CANVAS_WIDTH + 8 || y > CANVAS_HEIGHT + 8
  };
});

/** 地图与本体当前载图是否一致：不一致时坐标再准也画不到同一张图上 */
const mapMismatch = computed(() => {
  const active = props.device?.activeMapId?.trim();
  const target = props.map?.id;
  return Boolean(active && target && active !== target);
});

/** 画不出点位时给一句可读原因（运维最常踩的三个坑都在这里） */
const note = computed(() => {
  if (!props.device) return "任务未关联执行设备，无法显示实时位置";
  if (mapMismatch.value) {
    return `本体当前载图 ${props.device?.activeMapId} 与路线地图 ${props.map?.id} 不一致，点位可能对不上`;
  }
  if (!dog.value) return "设备尚未上报位置（定位未收敛或设备客户端离线）";
  if (dog.value.offMap) return "设备位置不在这张地图的范围内（可能被搬动或载错图）";
  if (!points.value.length) {
    if (props.route) return "该路线航点不是地图米制坐标（历史数据），地图只显示设备实时位置";
    return "任务使用临时路径，地图只显示设备实时位置";
  }
  if (!frame.value.schematic && !rasterRect.value) {
    return "地图栅格底图未加载，仅显示坐标系内的相对位置";
  }
  return "";
});
</script>

<template>
  <div class="tlm">
    <svg viewBox="0 0 800 460" role="img" aria-label="任务执行实时地图">
      <defs>
        <marker id="tlm-arrow" markerWidth="9" markerHeight="9" refX="6" refY="4.5" orient="auto">
          <path d="M1 1 7 4.5 1 8" fill="none" stroke="#0284c7" stroke-width="1.6" />
        </marker>
        <pattern id="tlm-grid" width="32" height="32" patternUnits="userSpaceOnUse">
          <path d="M32 0H0V32" fill="none" stroke="#e9eff6" stroke-width="1" />
        </pattern>
      </defs>

      <!-- 底图：真实栅格 > 示意底图（无地图包时的相对位置参考） -->
      <template v-if="rasterRect">
        <rect width="800" height="460" fill="#0b1220" />
        <image
          :href="rasterRect.href"
          :x="rasterRect.x"
          :y="rasterRect.y"
          :width="rasterRect.width"
          :height="rasterRect.height"
          preserveAspectRatio="none"
        />
      </template>
      <template v-else-if="frame.schematic">
        <RouteBaseMap />
      </template>
      <template v-else>
        <rect width="800" height="460" fill="#f8fafc" />
        <rect width="800" height="460" fill="url(#tlm-grid)" />
      </template>

      <!-- 路线：已走过 = 绿色实线，待执行 = 蓝色虚线 -->
      <g v-if="points.length">
        <template v-for="(point, index) in points" :key="`leg-${index}`">
          <line
            v-if="points[index + 1]"
            :x1="point.cx"
            :y1="point.cy"
            :x2="points[index + 1].cx"
            :y2="points[index + 1].cy"
            :stroke="legDone(index) ? '#10b981' : '#0284c7'"
            :stroke-width="legDone(index) ? 3.4 : 3"
            :stroke-dasharray="legDone(index) ? '' : '9 7'"
            :marker-end="legDone(index) ? undefined : 'url(#tlm-arrow)'"
            :opacity="legDone(index) ? 0.95 : 0.85"
          />
        </template>
        <line
          v-if="loopClosed"
          :x1="points[points.length - 1].cx"
          :y1="points[points.length - 1].cy"
          :x2="points[0].cx"
          :y2="points[0].cy"
          :stroke="doneCount >= points.length ? '#10b981' : '#94a3b8'"
          stroke-width="2.4"
          stroke-dasharray="4 7"
          opacity=".85"
        />

        <g v-for="point in points" :key="`wp-${point.index}`">
          <title>P{{ point.index + 1 }} {{ point.name }}</title>
          <circle
            v-if="point.index === nextIndex"
            :cx="point.cx"
            :cy="point.cy"
            r="20"
            fill="none"
            stroke="#f59e0b"
            stroke-width="2"
            stroke-dasharray="4 3"
            class="tlm-next"
          />
          <circle
            :cx="point.cx"
            :cy="point.cy"
            r="12"
            fill="#fff"
            :stroke="
              point.index === nextIndex
                ? '#f59e0b'
                : point.index < doneCount
                  ? '#10b981'
                  : '#0284c7'
            "
            stroke-width="2.4"
          />
          <circle
            :cx="point.cx"
            :cy="point.cy"
            r="5.6"
            :fill="
              point.index === nextIndex
                ? '#f59e0b'
                : point.index < doneCount
                  ? '#10b981'
                  : '#0284c7'
            "
          />
          <text
            :x="point.cx"
            :y="point.cy + 3.2"
            font-size="8.5"
            fill="#fff"
            text-anchor="middle"
            font-weight="700"
          >
            {{ point.index + 1 }}
          </text>
        </g>
      </g>

      <!-- 机器狗实时位置 -->
      <g
        v-if="dog && !dog.offMap"
        :transform="`translate(${dog.x},${dog.y})`"
        class="tlm-dog"
      >
        <title>{{ dog.label }} · 实时位置</title>
        <circle r="17" fill="#0284c7" opacity=".18" class="tlm-dog-pulse" />
        <g :transform="`rotate(${bearing})`">
          <path d="M17 0 L2 7.5 L5.5 0 L2 -7.5 Z" fill="#0284c7" opacity=".9" />
        </g>
        <circle r="9.5" fill="#fff" stroke="#0284c7" stroke-width="2.4" />
        <circle r="3.6" fill="#0284c7" />
        <g transform="translate(-30,-36)">
          <rect width="60" height="17" rx="5" fill="#0f172a" opacity=".88" />
          <text x="30" y="12" font-size="9.5" fill="#fff" text-anchor="middle" font-weight="600">
            {{ dog.label }}
          </text>
        </g>
      </g>

      <text
        v-if="!points.length && !dog"
        x="400"
        y="230"
        font-size="12"
        fill="#94a3b8"
        text-anchor="middle"
      >
        暂无可展示的航点与设备位置
      </text>
    </svg>

    <div class="tlm-legend">
      <span><i style="background: #10b981" />已走过</span>
      <span><i style="background: #0284c7" />待执行</span>
      <span><i style="background: #f59e0b" />下一目标</span>
      <span><i style="background: #0284c7; border-radius: 50%" />机器狗</span>
    </div>

    <div v-if="note" class="tlm-note">
      <AppIcon name="warn" />
      <span>{{ note }}</span>
    </div>
  </div>
</template>

<style scoped>
.tlm {
  position: relative;
  border-radius: 10px;
  overflow: hidden;
  border: 1px solid var(--line);
  background: #f8fafc;
  line-height: 0;
}

.tlm svg {
  display: block;
  width: 100%;
  height: auto;
  aspect-ratio: 800 / 460;
}

.tlm-legend {
  position: absolute;
  left: 8px;
  bottom: 8px;
  display: flex;
  gap: 10px;
  padding: 4px 8px;
  border-radius: 6px;
  background: rgb(255 255 255 / 88%);
  border: 1px solid var(--line-2, #e2e8f0);
  font-size: 10px;
  line-height: 1.4;
  color: var(--muted);
}

.tlm-legend span {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.tlm-legend i {
  width: 8px;
  height: 8px;
  border-radius: 2px;
  display: inline-block;
}

.tlm-note {
  position: absolute;
  top: 8px;
  left: 8px;
  right: 8px;
  display: flex;
  align-items: flex-start;
  gap: 6px;
  padding: 5px 8px;
  border-radius: 6px;
  background: rgb(255 251 235 / 94%);
  border: 1px solid #fde68a;
  color: #92400e;
  font-size: 11px;
  line-height: 1.5;
}

.tlm-note .ic {
  width: 13px;
  height: 13px;
  flex: none;
  margin-top: 1px;
}

.tlm-dog-pulse {
  animation: tlmPulse 2.2s ease-out infinite;
  transform-origin: center;
  transform-box: fill-box;
}

.tlm-next {
  animation: tlmSpin 9s linear infinite;
  transform-origin: center;
  transform-box: fill-box;
}

@keyframes tlmPulse {
  0% {
    opacity: 0.28;
    transform: scale(0.9);
  }
  70% {
    opacity: 0;
    transform: scale(1.8);
  }
  100% {
    opacity: 0;
    transform: scale(0.9);
  }
}

@keyframes tlmSpin {
  to {
    transform: rotate(360deg);
  }
}
</style>
