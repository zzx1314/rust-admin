<script setup lang="ts">
import { computed } from "vue";
import type { Device } from "@/api/types";

const props = withDefaults(
  defineProps<{ devices?: Device[]; highlight?: string | null }>(),
  { devices: () => [], highlight: null }
);

const knownPositions: Record<string, { x: number; y: number }> = {
  "RD-001": { x: 238, y: 196 },
  "RD-021": { x: 596, y: 236 },
  "RD-011": { x: 330, y: 352 },
  "RD-009": { x: 548, y: 150 },
  "RD-002": { x: 420, y: 258 },
  "RD-003": { x: 168, y: 132 },
  "RD-007": { x: 700, y: 300 },
  "RD-015": { x: 520, y: 320 },
  "RD-018": { x: 760, y: 96 },
  "RD-024": { x: 250, y: 96 }
};

const markers = computed(() =>
  props.devices.slice(0, 8).map((device, index) => {
    const known = knownPositions[device.deviceId];
    const angle = (index / Math.max(1, props.devices.length)) * Math.PI * 2;
    return {
      ...device,
      x: known?.x ?? 300 + Math.cos(angle) * 220,
      y: known?.y ?? 220 + Math.sin(angle) * 120
    };
  })
);

function toneOf(device: Device): string {
  if (device.status === "故障") return "#ef4444";
  if (device.status === "执行任务") return "#0284c7";
  if (device.status === "在线待命") return "#10b981";
  return "#94a3b8";
}

function labelColor(device: Device): string {
  if (device.status === "故障") return "#dc2626";
  return "#0f172a";
}
</script>

<template>
  <svg viewBox="0 0 860 430" role="img" aria-label="园区实时态势地图">
    <defs>
      <pattern id="grid" width="34" height="34" patternUnits="userSpaceOnUse">
        <path d="M34 0H0V34" fill="none" stroke="#e8eef5" stroke-width="1" />
      </pattern>
      <linearGradient id="grass" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0" stop-color="#eaf6ec" />
        <stop offset="1" stop-color="#dcefe1" />
      </linearGradient>
      <linearGradient id="bld" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#dbe4ef" />
        <stop offset="1" stop-color="#c9d6e6" />
      </linearGradient>
      <marker id="arrow" markerWidth="8" markerHeight="8" refX="4" refY="4" orient="auto">
        <path d="M1 1 7 4 1 7" fill="none" stroke="#0284c7" stroke-width="1.4" />
      </marker>
    </defs>
    <rect width="860" height="430" fill="#f7fafc" />
    <rect width="860" height="430" fill="url(#grid)" />
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
    <rect x="44" y="42" width="210" height="120" rx="12" fill="url(#grass)" />
    <rect x="600" y="256" width="216" height="132" rx="12" fill="url(#grass)" />
    <path d="M52 330 q70-40 150-10 q60 25 130 5 v60 H52z" fill="url(#grass)" opacity=".75" />
    <path
      d="M660 52 q70-16 130 6 q20 40-16 62 q-70 18-118-8 q-24-30 4-60z"
      fill="#dbeafe"
      stroke="#bfdbfe"
    />
    <text x="722" y="94" font-size="11" fill="#60a5fa" text-anchor="middle">景观水系</text>
    <path d="M40 210 H820" stroke="#e2e8f0" stroke-width="34" stroke-linecap="round" />
    <path d="M40 210 H820" stroke="#fff" stroke-width="2" stroke-dasharray="12 12" opacity=".9" />
    <path d="M430 32 V398" stroke="#e2e8f0" stroke-width="30" stroke-linecap="round" />
    <path d="M430 32 V398" stroke="#fff" stroke-width="2" stroke-dasharray="12 12" opacity=".9" />
    <path
      d="M40 210 q180 0 240 -60 H600"
      stroke="#e2e8f0"
      stroke-width="18"
      fill="none"
      stroke-linecap="round"
    />
    <g fill="url(#bld)" stroke="#b6c6d9" stroke-width="1.4">
      <rect x="76" y="62" width="150" height="86" rx="6" />
      <rect x="268" y="58" width="120" height="96" rx="6" />
      <rect x="92" y="262" width="132" height="94" rx="6" />
      <rect x="268" y="268" width="122" height="88" rx="6" />
      <rect x="478" y="58" width="140" height="92" rx="6" />
      <rect x="482" y="262" width="96" height="94" rx="6" />
      <rect x="626" y="276" width="120" height="76" rx="6" />
    </g>
    <g font-size="11" fill="#7b8ea6" font-weight="600" text-anchor="middle">
      <text x="151" y="110">A 生产车间</text>
      <text x="328" y="110">B 立体仓库</text>
      <text x="158" y="315">D 装卸平台</text>
      <text x="329" y="315">E 冷链仓</text>
      <text x="548" y="110">G 研发楼</text>
      <text x="530" y="315">F 配电房</text>
      <text x="686" y="320">充电站</text>
    </g>
    <g font-size="11" fill="#94a3b8" font-weight="700" letter-spacing="1">
      <text x="52" y="186">A 区 · 生产</text>
      <text x="470" y="186">G 区 · 研发</text>
      <text x="52" y="392">D 区 · 物流</text>
      <text x="470" y="392">H 区 · 停车</text>
      <text x="640" y="240">C 区 · 周界</text>
    </g>
    <path
      d="M120 176 H350 V236 H620 V180 H780"
      fill="none"
      stroke="#0284c7"
      stroke-width="2.6"
      stroke-dasharray="9 9"
      opacity=".85"
      marker-end="url(#arrow)"
    />
    <path
      d="M120 244 H330 V370 H700"
      fill="none"
      stroke="#06b6d4"
      stroke-width="2.2"
      stroke-dasharray="7 8"
      opacity=".7"
    />
    <g transform="translate(688,300)">
      <rect x="-13" y="-13" width="26" height="26" rx="7" fill="#fff" stroke="#0d9488" stroke-width="1.6" />
      <path d="M2-7-6 2h5l-1 7 8-9h-5z" fill="#0d9488" />
    </g>
    <g
      v-for="device in markers"
      :key="device.id"
      :transform="`translate(${device.x},${device.y})`"
      style="cursor: pointer"
    >
      <circle
        r="14"
        :fill="toneOf(device)"
        opacity=".25"
        :class="{ 'map-pulse': device.status === '执行任务' }"
      />
      <circle r="11" fill="#fff" :stroke="toneOf(device)" stroke-width="2" />
      <circle r="5" :fill="toneOf(device)" />
      <g transform="translate(-23,-38)">
        <rect
          width="48"
          height="17"
          rx="5"
          :fill="labelColor(device)"
          :stroke="highlight === device.deviceId ? '#f59e0b' : 'transparent'"
          stroke-width="1.5"
          opacity=".9"
        />
        <text x="24" y="12" font-size="9.5" fill="#fff" text-anchor="middle" font-weight="600">
          {{ device.deviceId }}
        </text>
      </g>
    </g>
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
