<script setup lang="ts">
import { computed, onMounted, ref, type Component } from "vue";
import Segmented, { type OptionsType } from "@/components/ReSegmented";
import DogStatCard from "@/components/DogUI/DogStatCard.vue";
import { ChartBar, ChartLine } from "./components/charts";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import { statisticsAll, statisticsAllType } from "@/api/dogStatistics";
import Refresh from "~icons/ep/refresh";
import RiMap2Line from "~icons/ri/map-2-line";
import RiBox3Line from "~icons/ri/box-3-line";
import RiImageLine from "~icons/ri/image-line";
import RiSettings4Line from "~icons/ri/settings-4-line";

defineOptions({
  name: "Welcome"
});

interface KpiCard {
  label: string;
  value: number;
  percent: string;
  data: number[];
  accent: string;
  icon: Component;
}

interface WeeklyStatistics {
  mapCount: number[];
  modelCount: number[];
  mediaCount: number[];
  configCount: number[];
}

/** 与后端 `get_all_statistics` 返回顺序一致：地图 / 模型 / 媒体 / 参数配置。 */
const KPI_ICONS: Component[] = [
  RiMap2Line,
  RiBox3Line,
  RiImageLine,
  RiSettings4Line
];

const kpis = ref<KpiCard[]>([]);
const weekly = ref<WeeklyStatistics[]>([
  { mapCount: [], modelCount: [], mediaCount: [], configCount: [] },
  { mapCount: [], modelCount: [], mediaCount: [], configCount: [] }
]);
const loading = ref(true);
const refreshing = ref(false);
const loadError = ref("");
const updatedAt = ref<Date | null>(null);

const updatedText = computed(() =>
  updatedAt.value
    ? updatedAt.value.toLocaleTimeString("zh-CN", {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hour12: false
      })
    : "暂无数据"
);

/** 统计接口失败时保留其余区块，只在顶部提示，避免整页空白。 */
async function loadDashboard() {
  loadError.value = "";
  const [summary, typeStats] = await Promise.allSettled([
    statisticsAll(),
    statisticsAllType()
  ]);

  if (summary.status === "fulfilled" && summary.value?.data) {
    kpis.value = summary.value.data.map((item: any, index: number) => ({
      label: item.name,
      value: Number(item.value) || 0,
      percent: item.percent ?? "",
      data: (item.data ?? []).map((value: any) => Number(value)),
      accent: item.color || "var(--dog-accent)",
      icon: KPI_ICONS[index] ?? RiBox3Line
    }));
  }

  if (typeStats.status === "fulfilled" && typeStats.value?.data) {
    weekly.value = typeStats.value.data;
  }

  const failed = [summary, typeStats].some(item => item.status === "rejected");
  if (failed) {
    loadError.value = "部分统计数据加载失败，可稍后点击「刷新」重试。";
  }

  loading.value = false;
  updatedAt.value = new Date();
}

async function refresh() {
  refreshing.value = true;
  try {
    await loadDashboard();
  } finally {
    refreshing.value = false;
  }
}

onMounted(loadDashboard);

const curWeek = ref(1);
const optionsBasis: Array<OptionsType> = [{ label: "上周" }, { label: "本周" }];

const weeklyChart = computed(
  () => weekly.value[curWeek.value] ?? weekly.value[0]
);
</script>

<template>
  <div class="dog-page dashboard">
    <el-alert
      v-if="loadError"
      :title="loadError"
      type="warning"
      show-icon
      :closable="false"
    />

    <!-- 核心指标 -->
    <section class="dashboard__kpis" aria-label="核心指标">
      <template v-if="loading && !kpis.length">
        <div v-for="index in 4" :key="index" class="dog-card">
          <el-skeleton :rows="2" animated />
        </div>
      </template>
      <DogStatCard
        v-for="card in kpis"
        :key="card.label"
        class="line-card"
        :label="card.label"
        :value="card.value"
        :percent="card.percent"
        :icon="card.icon"
        :accent="card.accent"
      >
        <template v-if="card.data.length > 1" #chart>
          <ChartLine :color="card.accent" :data="card.data" />
        </template>
      </DogStatCard>
    </section>

    <!-- 图表 -->
    <section class="dog-card bar-card">
      <div class="dog-card__head">
        <div>
          <h2 class="dog-card__title">数量统计</h2>
          <p class="dog-card__hint">地图、模型、媒体与参数配置的每日新增数量</p>
        </div>
        <div class="dashboard__actions">
          <Segmented v-model="curWeek" :options="optionsBasis" />
          <el-button
            circle
            size="small"
            aria-label="刷新统计数据"
            :title="`数据更新于 ${updatedText}`"
            :icon="useRenderIcon(Refresh)"
            :loading="refreshing"
            @click="refresh"
          />
        </div>
      </div>
      <ChartBar
        :map-data="weeklyChart.mapCount"
        :model-data="weeklyChart.modelCount"
        :media-data="weeklyChart.mediaCount"
        :config-data="weeklyChart.configCount"
      />
    </section>
  </div>
</template>

<style scoped lang="scss">
/* 首页单屏优先：区块间距略小于通用 .dog-page */
.dashboard {
  gap: var(--dog-space-3);
  padding-top: 0;
  padding-bottom: 0;
}

.dashboard__actions {
  display: flex;
  gap: var(--dog-space-2);
  align-items: center;
}

/* 指标卡 */
.dashboard__kpis {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--dog-space-4);
}
</style>
