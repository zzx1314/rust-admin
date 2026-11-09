<script setup lang="ts">
import { computed } from "vue";
import AppIcon from "./AppIcon.vue";

const props = withDefaults(
  defineProps<{
    label: string;
    value: number | string;
    unit?: string;
    icon?: string;
    color?: string;
    soft?: string;
    trend?: string;
    up?: boolean;
    foot?: string;
    spark?: number[];
  }>(),
  {
    unit: "",
    icon: "dog",
    color: "#0284c7",
    soft: "#e6f4fd",
    trend: "",
    up: true,
    foot: "",
    spark: () => []
  }
);

const sparkPoints = computed(() => {
  const data = props.spark;
  if (data.length < 2) return "";
  const max = Math.max(...data);
  const min = Math.min(...data);
  const span = max - min || 1;
  return data
    .map((value, index) => {
      const x = (index * (70 / (data.length - 1)) + 2).toFixed(1);
      const y = (26 - ((value - min) / span) * 20).toFixed(1);
      return `${x},${y}`;
    })
    .join(" ");
});

const lastPoint = computed(() => {
  const data = props.spark;
  if (data.length < 2) return null;
  const max = Math.max(...data);
  const min = Math.min(...data);
  const span = max - min || 1;
  return {
    x: 72,
    y: Number(
      (26 - ((data[data.length - 1] - min) / span) * 20).toFixed(1)
    )
  };
});
</script>

<template>
  <div class="card kpi" :style="{ '--kpi-c': color, '--kpi-soft': soft }">
    <div class="kpi-top">
      <span class="kpi-icon"><AppIcon :name="icon" /></span>
      <span v-if="trend" class="trend" :class="up ? 'up' : 'down'">
        <AppIcon :name="up ? 'trend-up' : 'trend-down'" />{{ trend }}
      </span>
    </div>
    <div class="kpi-label">{{ label }}</div>
    <div class="kpi-value">
      {{ value }} <small>{{ unit }}</small>
    </div>
    <div class="kpi-foot">{{ foot }}</div>
    <svg v-if="sparkPoints" class="spark" viewBox="0 0 74 30">
      <polyline
        :points="sparkPoints"
        fill="none"
        :stroke="color"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
      <circle
        v-if="lastPoint"
        :cx="lastPoint.x"
        :cy="lastPoint.y"
        r="2.6"
        :fill="color"
      />
    </svg>
  </div>
</template>
