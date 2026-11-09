<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import * as echarts from "echarts";

const props = withDefaults(
  defineProps<{ option: echarts.EChartsOption; height?: string }>(),
  { height: "260px" }
);

const container = ref<HTMLDivElement>();
let chart: echarts.ECharts | null = null;
let observer: ResizeObserver | null = null;

function render() {
  if (!container.value) return;
  if (!chart) {
    chart = echarts.init(container.value);
  }
  chart.setOption(props.option, true);
}

onMounted(() => {
  render();
  if (container.value) {
    observer = new ResizeObserver(() => chart?.resize());
    observer.observe(container.value);
  }
});

watch(() => props.option, render, { deep: true });

onBeforeUnmount(() => {
  observer?.disconnect();
  chart?.dispose();
  chart = null;
});
</script>

<template>
  <div ref="container" :style="{ width: '100%', height }" />
</template>
