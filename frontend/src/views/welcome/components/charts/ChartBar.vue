<script setup lang="ts">
import { type PropType, computed, nextTick, ref, watch } from "vue";
import { useDark, useECharts } from "@pureadmin/utils";

const props = defineProps({
  mapData: {
    type: Array as PropType<Array<number>>,
    default: () => []
  },
  modelData: {
    type: Array as PropType<Array<number>>,
    default: () => []
  },
  mediaData: {
    type: Array as PropType<Array<number>>,
    default: () => []
  },
  configData: {
    type: Array as PropType<Array<number>>,
    default: () => []
  }
});

const { isDark } = useDark();

const theme = computed(() => (isDark.value ? "dark" : "light"));

const chartRef = ref();
const { setOptions } = useECharts(chartRef, {
  theme
});

watch(
  () => props,
  async () => {
    await nextTick(); // 确保DOM更新完成后再执行
    setOptions({
      container: ".bar-card",
      color: ["#3b82f6", "#26ce83", "#41b6ff", "#BA53FFFF"],
      tooltip: {
        trigger: "axis",
        axisPointer: {
          type: "none"
        }
      },
      grid: {
        top: "20px",
        left: "50px",
        right: 0
      },
      legend: {
        data: ["地图数量", "模型数量", "媒体数量", "参数配置"],
        textStyle: {
          color: "#606266",
          fontSize: "0.875rem"
        },
        bottom: 0
      },
      xAxis: [
        {
          type: "category",
          data: ["周一", "周二", "周三", "周四", "周五", "周六", "周日"],
          axisLabel: {
            fontSize: "0.875rem"
          },
          axisPointer: {
            type: "shadow"
          }
        }
      ],
      yAxis: [
        {
          type: "value",
          axisLabel: {
            fontSize: "0.875rem"
          },
          splitLine: {
            show: false // 去网格线
          }
          // name: "单位: 个"
        }
      ],
      series: [
        {
          name: "地图数量",
          type: "bar",
          barWidth: 10,
          itemStyle: {
            color: "#3b82f6",
            borderRadius: [10, 10, 0, 0]
          },
          data: props.mapData
        },
        {
          name: "模型数量",
          type: "bar",
          barWidth: 10,
          itemStyle: {
            color: "#26ce83",
            borderRadius: [10, 10, 0, 0]
          },
          data: props.modelData
        },
        {
          name: "媒体数量",
          type: "bar",
          barWidth: 10,
          itemStyle: {
            color: "#41b6ff",
            borderRadius: [10, 10, 0, 0]
          },
          data: props.mediaData
        },
        {
          name: "参数配置",
          type: "bar",
          barWidth: 10,
          itemStyle: {
            color: "#BA53FFFF",
            borderRadius: [10, 10, 0, 0]
          },
          data: props.configData
        }
      ]
    });
  },
  {
    deep: true,
    immediate: true
  }
);
</script>

<template>
  <div ref="chartRef" style="width: 100%; height: 330px" />
</template>
