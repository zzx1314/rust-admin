<script setup lang="ts">
import { ref, computed, watch, type Ref, nextTick } from "vue";
import { useDark, useECharts, type EchartOptions } from "@pureadmin/utils";
import * as echarts from "echarts/core";

const { isDark } = useDark();

const theme: EchartOptions["theme"] = computed(() => {
  return isDark.value ? "dark" : "light";
});

const barChartRef = ref<HTMLDivElement | null>(null);
const { setOptions, resize } = useECharts(barChartRef as Ref<HTMLDivElement>, {
  theme
});

const props = defineProps({
  groupNames: {
    type: Array<string>,
    default: () => []
  },
  onlineNums: {
    type: Array<number>,
    default: () => []
  }
});

watch(
  () => props,
  async () => {
    await nextTick(); // 确保DOM更新完成后再执行
    setOptions(
      {
        tooltip: {
          trigger: "axis",
          axisPointer: {
            type: "shadow"
          }
        },
        grid: {
          bottom: "20px",
          right: "10px"
        },
        legend: {
          //@ts-expect-error
          right: true,
          data: ["设备数量"]
        },
        xAxis: [
          {
            type: "category",
            axisTick: {
              alignWithLabel: true
            },
            axisLabel: {
              interval: 0
            },
            data: props.groupNames,
            triggerEvent: true
          }
        ],
        yAxis: [
          {
            type: "value",
            triggerEvent: true
          }
        ],
        series: [
          {
            name: "设备数量",
            type: "bar",
            barWidth: "5%",
            itemStyle: {
              color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
                {
                  offset: 0,
                  color: "#3ce6ab"
                },
                {
                  offset: 1,
                  color: "#3ce675"
                }
              ])
            },
            data: props.onlineNums
          }
        ],
        addTooltip: true
      },
      {
        name: "click",
        callback: params => {
          console.log("click", params);
        }
      }
    );
  },
  {
    deep: true,
    immediate: true
  }
);
</script>

<template>
  <div ref="barChartRef" style="width: 100%; height: 365px" />
</template>
