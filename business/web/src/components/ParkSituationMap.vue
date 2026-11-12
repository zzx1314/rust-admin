<script setup lang="ts">
import { computed } from "vue";
import type { Device, MapRaster, MapVersion } from "@/api/types";
import { frameOf, type MapFrame } from "@/utils/coords";

const props = withDefaults(
  defineProps<{
    devices?: Device[];
    map?: MapVersion | null;
    raster?: MapRaster | null;
  }>(),
  { devices: () => [], map: null, raster: null }
);

defineEmits<{ select: [device: Device] }>();

/** 画布尺寸（与模板 viewBox 保持一致） */
const VIEW_W = 860;
const VIEW_H = 430;
/** 底图四周留白，避免设备点位贴在边框上 */
const PAD = 40;

/** 把设备上报的 `"x, y"`（地图米）解析成坐标 */
function parsePosition(raw?: string | null): { x: number; y: number } | null {
  if (!raw) return null;
  const numbers = raw.match(/-?\d+(?:\.\d+)?/g);
  if (!numbers || numbers.length < 2) return null;
  const x = Number.parseFloat(numbers[0]);
  const y = Number.parseFloat(numbers[1]);
  return Number.isFinite(x) && Number.isFinite(y) ? { x, y } : null;
}

/** 画布内的绘图区 */
const area = computed(() => ({
  x: PAD,
  y: PAD,
  width: VIEW_W - PAD * 2,
  height: VIEW_H - PAD * 2
}));

/** 真实地图坐标 → 画布像素（与 utils/coords 的公式一致，只是画布尺寸不同） */
function frameLayout(frame: MapFrame) {
  const worldW = Math.max(frame.width * frame.resolution, 0.001);
  const worldH = Math.max(frame.height * frame.resolution, 0.001);
  const scale = Math.min(area.value.width / worldW, area.value.height / worldH);
  const drawW = worldW * scale;
  const drawH = worldH * scale;
  return {
    scale,
    drawW,
    drawH,
    rectX: area.value.x + (area.value.width - drawW) / 2,
    rectY: area.value.y + (area.value.height - drawH) / 2,
    worldW,
    worldH
  };
}

function projectMeters(
  frame: MapFrame,
  layout: ReturnType<typeof frameLayout>,
  point: { x: number; y: number }
) {
  const left = layout.rectX + (point.x - frame.originX) * layout.scale;
  const bottom = layout.rectY + layout.drawH - (point.y - frame.originY) * layout.scale;
  return {
    x: Math.min(VIEW_W - 16, Math.max(16, left)),
    y: Math.min(VIEW_H - 16, Math.max(16, bottom))
  };
}

/** 没有绑定时按设备坐标范围归一化，至少能看出相对位置 */
function normalizedProjector(points: Array<{ x: number; y: number }>) {
  if (points.length < 2) {
    return () => ({ x: VIEW_W / 2, y: VIEW_H / 2 });
  }
  let minX = Number.POSITIVE_INFINITY;
  let minY = Number.POSITIVE_INFINITY;
  let maxX = Number.NEGATIVE_INFINITY;
  let maxY = Number.NEGATIVE_INFINITY;
  for (const point of points) {
    minX = Math.min(minX, point.x);
    maxX = Math.max(maxX, point.x);
    minY = Math.min(minY, point.y);
    maxY = Math.max(maxY, point.y);
  }
  const spanX = maxX - minX;
  const spanY = maxY - minY;
  return (point: { x: number; y: number }) => {
    const fx = spanX > 0.001 ? (point.x - minX) / spanX : 0.5;
    // 地图 y 向上、画布 y 向下
    const fy = spanY > 0.001 ? 1 - (point.y - minY) / spanY : 0.5;
    return {
      x: area.value.x + 24 + fx * (area.value.width - 48),
      y: area.value.y + 24 + fy * (area.value.height - 48)
    };
  };
}

const markers = computed(() => {
  const frame = frameOf(props.map);
  const layout = frameLayout(frame);
  const parsed = props.devices
    .map(device => ({ device, point: parsePosition(device.position) }))
    .filter((item): item is { device: Device; point: { x: number; y: number } } =>
      Boolean(item.point)
    );

  const project = frame.schematic
    ? normalizedProjector(parsed.map(item => item.point))
    : (point: { x: number; y: number }) => projectMeters(frame, layout, point);

  // 没有上报坐标的设备按圆周均匀摆放，避免全部叠在一个点上
  const missing = props.devices.length - parsed.length;
  let missingIndex = 0;
  return props.devices.map(device => {
    const point = parsePosition(device.position);
    let position: { x: number; y: number };
    if (point) {
      position = project(point);
    } else {
      const angle = missing > 1 ? (missingIndex / missing) * Math.PI * 2 : Math.PI / 2;
      missingIndex += 1;
      position = {
        x: VIEW_W / 2 + Math.cos(angle) * 150,
        y: VIEW_H / 2 + Math.sin(angle) * 80
      };
    }
    return { ...device, ...position };
  });
});

const rasterRect = computed(() => {
  const raster = props.raster;
  if (!raster || !raster.width || !raster.height) return null;
  const frame = frameOf(props.map);
  if (frame.schematic) return null;
  const layout = frameLayout(frame);
  return {
    href: raster.image,
    x: layout.rectX,
    y: layout.rectY,
    width: layout.drawW,
    height: layout.drawH
  };
});

const hasRaster = computed(() => Boolean(rasterRect.value));

function toneOf(device: Device): string {
  if (device.status === "故障") return "#ef4444";
  if (device.status === "执行任务") return "#0284c7";
  if (device.status === "充电中") return "#f59e0b";
  if (device.status === "在线待命") return "#10b981";
  return "#94a3b8";
}

function labelColor(device: Device): string {
  if (device.status === "故障") return "#dc2626";
  return "#0f172a";
}

function tooltip(device: Device): string {
  const parts = [device.deviceId, device.status];
  if (device.zoneName) parts.push(device.zoneName);
  if (device.bodyOnline) parts.push(`电量 ${device.battery}%`);
  if (device.position) parts.push(`坐标 ${device.position}`);
  return parts.join(" · ");
}
</script>

<template>
  <svg viewBox="0 0 860 430" role="img" aria-label="园区实时态势地图">
    <defs>
      <pattern id="grid" width="34" height="34" patternUnits="userSpaceOnUse">
        <path d="M34 0H0V34" fill="none" stroke="#e8eef5" stroke-width="1" />
      </pattern>
      <linearGradient id="situation-bg" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0" stop-color="#f8fbfe" />
        <stop offset="1" stop-color="#eef4f9" />
      </linearGradient>
    </defs>

    <rect width="860" height="430" fill="url(#situation-bg)" />
    <rect width="860" height="430" fill="url(#grid)" />

    <!-- 真实 ROS 地图底图：设备点位与底图共用同一套米制坐标 -->
    <image
      v-if="rasterRect"
      :href="rasterRect.href"
      :x="rasterRect.x"
      :y="rasterRect.y"
      :width="rasterRect.width"
      :height="rasterRect.height"
      preserveAspectRatio="none"
      style="image-rendering: pixelated"
    />

    <!-- 没有地图包时的示意底图（不标具体建筑名，避免把示意当真实园区） -->
    <g v-else>
      <rect
        x="24"
        y="22"
        width="812"
        height="386"
        rx="14"
        fill="none"
        stroke="#cbd5e1"
        stroke-width="2"
        stroke-dasharray="9 7"
      />
      <path d="M40 210 H820" stroke="#e6edf5" stroke-width="28" stroke-linecap="round" />
      <path d="M430 32 V398" stroke="#e6edf5" stroke-width="24" stroke-linecap="round" />
      <g fill="#e4ebf3" stroke="#d3dfea" stroke-width="1.2" opacity=".85">
        <rect x="76" y="62" width="150" height="86" rx="6" />
        <rect x="268" y="58" width="120" height="96" rx="6" />
        <rect x="92" y="262" width="132" height="94" rx="6" />
        <rect x="268" y="268" width="122" height="88" rx="6" />
        <rect x="478" y="58" width="140" height="92" rx="6" />
        <rect x="482" y="262" width="96" height="94" rx="6" />
        <rect x="626" y="276" width="120" height="76" rx="6" />
      </g>
      <text
        x="430"
        y="412"
        font-size="10.5"
        fill="#94a3b8"
        text-anchor="middle"
        letter-spacing=".5"
      >
        示意底图 · 园区地图包未绑定或不可用
      </text>
    </g>

    <g
      v-for="device in markers"
      :key="device.id"
      :transform="`translate(${device.x},${device.y})`"
      style="cursor: pointer"
      @click="$emit('select', device)"
    >
      <title>{{ tooltip(device) }}</title>
      <circle
        r="14"
        :fill="toneOf(device)"
        opacity=".25"
        :class="{ 'map-pulse': device.status === '执行任务' }"
      />
      <circle r="11" fill="#fff" :stroke="toneOf(device)" stroke-width="2" />
      <circle r="5" :fill="toneOf(device)" />
      <g transform="translate(-23,-38)">
        <rect width="48" height="17" rx="5" :fill="labelColor(device)" opacity=".9" />
        <text x="24" y="12" font-size="9.5" fill="#fff" text-anchor="middle" font-weight="600">
          {{ device.deviceId }}
        </text>
      </g>
    </g>

    <text
      v-if="!markers.length"
      x="430"
      y="220"
      font-size="12"
      fill="#94a3b8"
      text-anchor="middle"
    >
      该园区暂无可展示设备
    </text>
  </svg>
</template>

<style scoped>
.map-pulse {
  animation: mapPulse 2s infinite;
}

@keyframes mapPulse {
  0% {
    opacity: 0.35;
    transform: scale(1);
  }
  70% {
    opacity: 0;
    transform: scale(1.8);
  }
  100% {
    opacity: 0;
    transform: scale(1);
  }
}
</style>
