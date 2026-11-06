<script setup lang="ts">
import { computed, onMounted, ref, type Component } from "vue";
import Segmented, { type OptionsType } from "@/components/ReSegmented";
import DogStatCard from "@/components/DogUI/DogStatCard.vue";
import DogStatusBadge from "@/components/DogUI/DogStatusBadge.vue";
import { ChartBar, ChartLine } from "./components/charts";
import DocNumBer from "./components/charts/DocNumBer.vue";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import {
  statisticsAll,
  statisticsAllType,
  statisticsDocNumber
} from "@/api/dogStatistics";
import { dogDeviceGetAll } from "@/api/dogDevice";
import Refresh from "~icons/ep/refresh";
import RiRobot2Line from "~icons/ri/robot-2-line";
import RiTaskLine from "~icons/ri/task-line";
import RiBox3Line from "~icons/ri/box-3-line";
import RiRouteLine from "~icons/ri/route-line";

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
  deviceCount: number[];
  modelCount: number[];
  routeCount: number[];
  taskCount: number[];
}

/** 与后端 `get_all_statistics` 返回顺序一致：设备 / 任务 / 模型 / 路线。 */
const KPI_ICONS: Component[] = [
  RiRobot2Line,
  RiTaskLine,
  RiBox3Line,
  RiRouteLine
];

const kpis = ref<KpiCard[]>([]);
const weekly = ref<WeeklyStatistics[]>([
  { deviceCount: [], modelCount: [], routeCount: [], taskCount: [] },
  { deviceCount: [], modelCount: [], routeCount: [], taskCount: [] }
]);
const deviceTypes = ref<{ xdata: string[]; ydata: number[] }>({
  xdata: [],
  ydata: []
});
const fleet = ref({ total: 0, online: 0, offline: 0 });
const loading = ref(true);
const refreshing = ref(false);
const loadError = ref("");
const updatedAt = ref<Date | null>(null);

const onlineRate = computed(() =>
  fleet.value.total
    ? Math.round((fleet.value.online / fleet.value.total) * 100)
    : 0
);

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
  const [summary, typeStats, devices] = await Promise.allSettled([
    statisticsAll(),
    statisticsAllType(),
    dogDeviceGetAll()
  ]);

  if (summary.status === "fulfilled" && summary.value?.data) {
    kpis.value = summary.value.data.map((item: any, index: number) => ({
      label: item.name,
      value: Number(item.value) || 0,
      percent: item.percent ?? "",
      data: (item.data ?? []).map((value: any) => Number(value)),
      accent: item.color || "var(--dog-accent)",
      icon: KPI_ICONS[index] ?? RiRobot2Line
    }));
  }

  if (typeStats.status === "fulfilled" && typeStats.value?.data) {
    weekly.value = typeStats.value.data;
  }

  if (devices.status === "fulfilled" && devices.value?.data) {
    const list: Array<{ status?: string }> = devices.value.data;
    const online = list.filter(item => item.status === "online").length;
    fleet.value = {
      total: list.length,
      online,
      offline: list.length - online
    };
  }

  try {
    const typeResult = await statisticsDocNumber();
    if (typeResult?.data) {
      deviceTypes.value = typeResult.data;
    }
  } catch {
    deviceTypes.value = { xdata: [], ydata: [] };
  }

  const failed = [summary, typeStats, devices].some(
    item => item.status === "rejected"
  );
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
    <div class="dashboard__grid">
      <section class="dog-card bar-card">
        <div class="dog-card__head">
          <div>
            <h2 class="dog-card__title">数量统计</h2>
            <p class="dog-card__hint">设备、模型、路线与任务的每日新增数量</p>
          </div>
          <Segmented v-model="curWeek" :options="optionsBasis" />
        </div>
        <ChartBar
          :device-data="weeklyChart.deviceCount"
          :model-data="weeklyChart.modelCount"
          :route-data="weeklyChart.routeCount"
          :task-data="weeklyChart.taskCount"
        />
      </section>

      <section class="dog-card">
        <div class="dog-card__head">
          <div>
            <h2 class="dog-card__title">设备类型分布</h2>
            <p class="dog-card__hint">按型号聚合的在管设备数量</p>
          </div>
          <div class="dashboard__fleet">
            <span class="dashboard__legend-item">
              <DogStatusBadge status="online" live />
              <b class="dog-num">{{ fleet.online }}</b>
              台
            </span>
            <span class="dashboard__legend-item">
              <DogStatusBadge status="offline" />
              <b class="dog-num">{{ fleet.offline }}</b>
              台
            </span>
            <span class="dashboard__rate">
              <span class="dashboard__rate-label">在线率</span>
              <b class="dog-num">
                {{ onlineRate }}<span class="dashboard__rate-unit">%</span>
              </b>
            </span>
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
        <div
          class="dashboard__meter"
          role="progressbar"
          :aria-valuenow="onlineRate"
          aria-valuemin="0"
          aria-valuemax="100"
          :aria-label="`设备在线率 ${onlineRate}%，共 ${fleet.total} 台设备`"
        >
          <span
            class="dashboard__meter-fill"
            :style="{ width: `${onlineRate}%` }"
          />
        </div>
        <DocNumBer
          v-if="deviceTypes.xdata.length"
          :group-names="deviceTypes.xdata"
          :online-nums="deviceTypes.ydata"
        />
        <el-empty
          v-else
          :image-size="72"
          description="暂无设备数据，新增设备后即可看到分布"
        />
      </section>
    </div>
  </div>
</template>

<style scoped lang="scss">
/* 首页单屏优先：区块间距略小于通用 .dog-page */
.dashboard {
  gap: var(--dog-space-3);
  padding-top: 0;
  padding-bottom: 0;
}

/* 在线率摘要（并入「设备类型分布」卡头，不额外占行） */
.dashboard__fleet {
  display: flex;
  flex-wrap: wrap;
  gap: var(--dog-space-2) var(--dog-space-3);
  align-items: center;
}

.dashboard__legend-item {
  display: inline-flex;
  gap: 6px;
  align-items: center;
  font-size: 13px;
  color: var(--dog-text-secondary);
  white-space: nowrap;
}

.dashboard__rate {
  display: inline-flex;
  gap: 4px;
  align-items: baseline;
  margin: 0;
  font-size: 15px;
  font-weight: 700;
  line-height: 1.2;
  color: var(--dog-text-primary);
  white-space: nowrap;
}

.dashboard__rate-label {
  font-size: 12px;
  font-weight: 400;
  color: var(--dog-text-muted);
}

.dashboard__rate-unit {
  margin-left: 1px;
  font-size: 12px;
  font-weight: 500;
  color: var(--dog-text-muted);
}

.dashboard__meter {
  height: 6px;
  margin-bottom: var(--dog-space-2);
  overflow: hidden;
  background: var(--dog-surface-muted);
  border-radius: var(--dog-radius-pill);
}

.dashboard__meter-fill {
  display: block;
  width: 0;
  height: 100%;
  background: linear-gradient(
    90deg,
    var(--dog-status-online-bg),
    var(--dog-status-online)
  );
  border-radius: inherit;
  transition: width var(--dog-duration-slow) var(--dog-ease);
}

/* 指标卡 */
.dashboard__kpis {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--dog-space-4);
}

/* 图表区 */
.dashboard__grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(360px, 1fr));
  gap: var(--dog-space-3);
}

.dashboard :deep(.el-empty) {
  padding: var(--dog-space-5) 0;
}
</style>
