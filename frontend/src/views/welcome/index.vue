<script setup lang="ts">
import { ref, onMounted } from "vue";
import ReCol from "@/components/ReCol";
import { useDark } from "./utils";
import { ReNormalCountTo } from "@/components/ReCountTo";
import { ChartBar, ChartLine, ChartRound } from "./components/charts";
import Segmented, { type OptionsType } from "@/components/ReSegmented";
import DocNumBer from "@/views/welcome/components/charts/DocNumBer.vue";
import {
  statisticsAll,
  statisticsAllType,
  statisticsDocNumber
} from "@/api/dogStatistics";
import RiBloggerLine from "~icons/ri/blogger-line";
import Tools from "~icons/ep/tools";
import Box from "~icons/ep/box";
import Smile from "~icons/ri/star-smile-line";

defineOptions({
  name: "Welcome"
});

const chartData = ref([]);
const barChartData = ref([
  {
    deviceCount: [],
    modelCount: [],
    routeCount: [],
    taskCount: []
  },
  {
    deviceCount: [],
    modelCount: [],
    routeCount: [],
    taskCount: []
  }
]);
const docChatData = ref({
  xdata: [],
  ydata: []
});

onMounted(() => {
  statisticsAll().then(res => {
    res.data[0].icon = RiBloggerLine;
    res.data[1].icon = Tools;
    res.data[2].icon = Box;
    res.data[3].icon = Smile;
    chartData.value.push(...res.data);
    for (let i = 0; i < 4; i++) {
      chartData.value[i].value = Number(res.data[i].value);
      chartData.value[i].data = res.data[i].data.map((d: any) => Number(d));
    }
  });
  statisticsAllType().then(res => {
    if (res.data.length > 0) {
      barChartData.value = res.data;
    }
  });
  statisticsDocNumber().then(res => {
    docChatData.value = res.data;
  });
});

const { isDark } = useDark();

let curWeek = ref(1);
const optionsBasis: Array<OptionsType> = [{ label: "上周" }, { label: "本周" }];
</script>

<template>
  <div>
    <el-row :gutter="24" justify="space-around">
      <re-col
        v-for="(item, index) in chartData"
        :key="index"
        v-motion
        class="mb-4.5"
        :value="6"
        :md="12"
        :sm="12"
        :xs="24"
        :initial="{ opacity: 0, y: 100 }"
        :enter="{
          opacity: 1,
          y: 0,
          transition: { delay: 80 * (index + 1) }
        }"
      >
        <el-card class="line-card" shadow="never">
          <div class="flex justify-between">
            <span class="text-md font-medium">{{ item.name }}</span>
            <div
              class="w-8 h-8 flex justify-center items-center rounded-md"
              :style="{
                backgroundColor: isDark ? 'transparent' : item.bgColor
              }"
            >
              <IconifyIconOffline
                :icon="item.icon"
                :color="item.color"
                width="18"
                height="18"
              />
            </div>
          </div>
          <div class="flex justify-between items-start mt-3">
            <div class="w-1/2">
              <ReNormalCountTo
                :duration="item.duration"
                :fontSize="'1.6em'"
                :startVal="100"
                :endVal="item.value"
              />
              <p class="font-medium text-green-500">{{ item.percent }}</p>
            </div>
            <ChartLine
              v-if="item.data.length > 1"
              class="w-1/2!"
              :color="item.color"
              :data="item.data"
            />
            <ChartRound v-else class="w-1/2!" />
          </div>
        </el-card>
      </re-col>

      <re-col
        v-motion
        class="mb-4.5"
        :value="12"
        :xs="24"
        :initial="{ opacity: 0, y: 100 }"
        :enter="{ opacity: 1, y: 0, transition: { delay: 400 } }"
      >
        <el-card class="bar-card" shadow="never">
          <div class="flex justify-between">
            <span class="text-md font-medium">数量统计</span>
            <Segmented v-model="curWeek" :options="optionsBasis" />
          </div>
          <div class="flex justify-between items-start mt-3">
            <ChartBar
              :device-data="barChartData[curWeek].deviceCount"
              :model-data="barChartData[curWeek].modelCount"
              :route-data="barChartData[curWeek].routeCount"
              :task-data="barChartData[curWeek].taskCount"
            />
          </div>
        </el-card>
      </re-col>

      <re-col
        v-motion
        class="mb-4.5"
        :value="12"
        :xs="24"
        :initial="{ opacity: 0, y: 100 }"
        :enter="{ opacity: 1, y: 0, transition: { delay: 480 } }"
      >
        <el-card shadow="never">
          <div class="flex justify-between">
            <span class="text-md font-medium">设备统计</span>
          </div>
          <div class="flex justify-between items-start mt-4">
            <DocNumBer
              :group-names="docChatData.xdata"
              :online-nums="docChatData.ydata"
            />
          </div>
        </el-card>
      </re-col>
    </el-row>
  </div>
</template>

<style lang="scss" scoped>
:deep(.el-card) {
  --el-card-border-color: none;

  .el-progress--line {
    width: 85%;
  }

  .el-progress-bar__innerText {
    font-size: 15px;
  }

  .el-scrollbar__bar {
    display: none;
  }

  .el-timeline-item {
    margin: 0 6px;
  }
}

.main-content {
  margin: 20px 20px 0 !important;
}
</style>
