<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import type { EChartsOption } from "echarts";
import { ElMessage } from "element-plus";
import { dashboardApi, mapApi } from "@/api/ops";
import type { DashboardSummary, MapRaster, MapVersion } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import KpiCard from "@/components/KpiCard.vue";
import EChart from "@/components/EChart.vue";
import ParkSituationMap from "@/components/ParkSituationMap.vue";
import AppIcon from "@/components/AppIcon.vue";
import { downloadCsv, timeOnly } from "@/utils/format";

const router = useRouter();
const summary = ref<DashboardSummary | null>(null);
const mapCard = ref<HTMLDivElement>();
const maps = ref<MapVersion[]>([]);
const raster = ref<MapRaster | null>(null);
const selectedParkId = ref<number | null>(null);
const rasterCache = new Map<string, MapRaster>();
let rasterToken = 0;
let timer = 0;

/** 态势图园区下拉：后端已按登录人收敛（自己创建 + 自己设备所在园区） */
const parkOptions = computed(() => summary.value?.parks ?? []);

const selectedPark = computed(
  () => parkOptions.value.find(park => park.id === selectedParkId.value) ?? null
);

/** 当前园区下、当前登录人管辖的设备 */
const selectedDevices = computed(() => {
  if (selectedParkId.value === null) return [];
  return (summary.value?.mapDevices ?? []).filter(
    device => device.parkId === selectedParkId.value
  );
});

const selectedMap = computed(() => {
  const park = selectedPark.value;
  if (!park) return null;
  return (
    maps.value.find(item => item.id === park.mapVersionId) ??
    maps.value.find(item => item.parkId === park.id && item.status !== "已归档") ??
    null
  );
});

const onlineInPark = computed(
  () =>
    selectedDevices.value.filter(
      device => device.status === "执行任务" || device.status === "在线待命"
    ).length
);

const parkSubtitle = computed(() => {
  const park = selectedPark.value;
  if (!park) return "暂无园区";
  const parts: string[] = [];
  if (park.zoneCount) parts.push(`${park.zoneCount} 个区域`);
  if (park.deviceCount) parts.push(`${park.deviceCount} 台设备`);
  if (park.parkAddress) parts.push(park.parkAddress);
  return parts.length ? parts.join(" · ") : park.status;
});

const donutGradient = computed(() => {
  const data = summary.value?.deviceStatus ?? [];
  const total = data.reduce((sum, item) => sum + item.value, 0);
  if (!total) return "#e2e8f0 0% 100%";
  let accumulated = 0;
  return data
    .map(item => {
      const from = ((accumulated / total) * 100).toFixed(2);
      accumulated += item.value;
      const to = ((accumulated / total) * 100).toFixed(2);
      return `${item.color} ${from}% ${to}%`;
    })
    .join(",");
});

const trendOption = computed<EChartsOption>(() => {
  const trend = summary.value?.trend;
  return {
    tooltip: { trigger: "axis" },
    legend: { show: false },
    grid: { left: 42, right: 18, top: 24, bottom: 30 },
    xAxis: {
      type: "category",
      data: trend?.days ?? [],
      axisLine: { lineStyle: { color: "#e2e8f0" } },
      axisTick: { show: false },
      axisLabel: { color: "#94a3b8", fontSize: 10 }
    },
    yAxis: {
      type: "value",
      splitLine: { lineStyle: { color: "#eef2f7" } },
      axisLabel: { color: "#b3c0cf", fontSize: 10 }
    },
    series: [
      {
        name: "计划",
        type: "bar",
        data: trend?.plan ?? [],
        barWidth: 12,
        itemStyle: { color: "#dbeafe", borderRadius: [3, 3, 0, 0] }
      },
      {
        name: "完成",
        type: "bar",
        data: trend?.done ?? [],
        barWidth: 12,
        itemStyle: { color: "#0284c7", borderRadius: [3, 3, 0, 0] },
        label: {
          show: true,
          position: "top",
          color: "#0284c7",
          fontSize: 9,
          fontWeight: 700
        }
      }
    ]
  };
});

const kpis = computed(() => {
  const data = summary.value;
  if (!data) return [];
  return [
    {
      label: "在线设备",
      value: data.kpis.onlineDevices.value,
      unit: `/ ${data.kpis.onlineDevices.total} 台`,
      icon: "dog",
      color: "#0284c7",
      soft: "#e6f4fd",
      trend: data.kpis.onlineDevices.value
        ? `+${data.kpis.onlineDevices.value}`
        : "0",
      up: true,
      foot: data.kpis.onlineDevices.foot,
      spark: [12, 15, 14, 18, 17, 21, data.kpis.onlineDevices.value]
    },
    {
      label: "今日巡检任务",
      value: data.kpis.todayTasks.value,
      unit: "项",
      icon: "task",
      color: "#7c3aed",
      soft: "#f1ecfe",
      trend: `${data.kpis.todayTasks.rate}%`,
      up: true,
      foot: data.kpis.todayTasks.foot,
      spark: [9, 11, 10, 14, 13, 16, data.kpis.todayTasks.value]
    },
    {
      label: "在管园区",
      value: data.kpis.parks.value,
      unit: `个 / ${data.kpis.parks.zones} 区域`,
      icon: "park",
      color: "#0d9488",
      soft: "#e2f8f5",
      trend: "持平",
      up: true,
      foot: data.kpis.parks.foot,
      spark: [2, 2, 3, 3, 3, 3, data.kpis.parks.value]
    },
    {
      label: "待处理告警",
      value: data.kpis.pendingAlerts.value,
      unit: "条",
      icon: "bell",
      color: "#dc2626",
      soft: "#fdeced",
      trend: data.kpis.pendingAlerts.value > 0 ? "待处置" : "已清零",
      up: data.kpis.pendingAlerts.value === 0,
      foot: data.kpis.pendingAlerts.foot,
      spark: [6, 5, 5, 4, 3, 3, data.kpis.pendingAlerts.value]
    }
  ];
});

const eventIcon = (type: string) => {
  if (type === "success") return "check-circle";
  if (type === "warning" || type === "status") return "warn";
  if (type === "running") return "play";
  if (type === "progress") return "activity";
  if (type === "charging") return "charge";
  if (type === "command" || type === "teleop") return "send";
  if (type === "alert") return "bell";
  if (type === "route") return "route";
  if (type === "binding") return "pin";
  if (type === "device_event") return "cpu";
  if (type === "created") return "plus";
  return "task";
};

const eventTone = (type: string) => {
  if (type === "success") return "ok";
  if (type === "warning" || type === "status" || type === "alert") return "warn";
  return "";
};

async function loadRaster(map: MapVersion | null) {
  const token = ++rasterToken;
  if (!map || map.source !== "backend" || !map.filePath) {
    raster.value = null;
    return;
  }
  const cached = rasterCache.get(map.id);
  if (cached) {
    raster.value = cached;
    return;
  }
  try {
    const data = await mapApi.raster(map.id);
    rasterCache.set(map.id, data);
    if (token === rasterToken) raster.value = data;
  } catch {
    // 地图包损坏 / 文件丢失时退回示意底图，不能让整张态势图报错
    if (token === rasterToken) raster.value = null;
  }
}

watch(selectedMap, map => {
  void loadRaster(map);
});

async function load() {
  summary.value = await dashboardApi.summary();
  try {
    maps.value = await mapApi.list();
  } catch {
    // 地图列表只是态势图底图，失败时保留上一次结果，不影响驾驶舱主数据
  }
  // 保持用户当前的园区选择；园区被删除 / 取消分配后自动回落到第一个
  const stillExists = parkOptions.value.some(park => park.id === selectedParkId.value);
  if (!stillExists) {
    selectedParkId.value = parkOptions.value[0]?.id ?? null;
  }
}

function go(path: string) {
  router.push(path);
}

function exportReport() {
  const data = summary.value;
  if (!data) return;
  downloadCsv("robodog-daily-report.csv", [
    ["指标", "数值"],
    ["在线设备", `${data.kpis.onlineDevices.value}/${data.kpis.onlineDevices.total}`],
    ["任务总数", data.kpis.todayTasks.value],
    ["任务完成率", `${data.kpis.todayTasks.rate}%`],
    ["在管园区", data.kpis.parks.value],
    ["巡检区域", data.kpis.parks.zones],
    ["待处理告警", data.kpis.pendingAlerts.value],
    [],
    ["日期", ...data.trend.days],
    ["计划", ...data.trend.plan],
    ["完成", ...data.trend.done]
  ]);
  ElMessage.success("运行日报已导出");
}

async function fullscreen() {
  if (!mapCard.value) return;
  if (document.fullscreenElement) {
    await document.exitFullscreen();
    return;
  }
  await mapCard.value.requestFullscreen();
}

onMounted(async () => {
  await load();
  timer = window.setInterval(load, 10000);
});

onBeforeUnmount(() => window.clearInterval(timer));
</script>

<template>
  <div v-if="summary">
    <PageHeader title="总览驾驶舱" subtitle="实时掌握园区、设备与巡检任务的运行态势">
      <template #actions>
        <span class="live-chip"><i />数据实时更新</span>
        <button class="btn" @click="exportReport">
          <AppIcon name="download" />导出日报
        </button>
      </template>
    </PageHeader>

    <div class="grid kpis">
      <KpiCard v-for="kpi in kpis" :key="kpi.label" v-bind="kpi" />
    </div>

    <div class="grid row-2 mt18">
      <div ref="mapCard" class="card map-card">
        <div class="card-head">
          <div class="card-title">
            <AppIcon name="pin" />园区实时态势
            <select
              v-if="parkOptions.length"
              v-model.number="selectedParkId"
              class="select park-select"
              aria-label="选择园区"
            >
              <option v-for="park in parkOptions" :key="park.id" :value="park.id">
                {{ park.parkName }}
              </option>
            </select>
            <span class="card-sub">{{ parkSubtitle }}</span>
          </div>
          <button class="btn sm" @click="fullscreen">
            <AppIcon name="maximize" />全屏
          </button>
        </div>
        <div class="map-stage">
          <ParkSituationMap
            :devices="selectedDevices"
            :map="selectedMap"
            :raster="raster"
            @select="go('/devices')"
          />
          <div class="map-badge">
            <AppIcon name="activity" /><b>{{ onlineInPark }}</b>
            台设备在线作业
          </div>
          <div class="map-legend">
            <span><i style="background: #0284c7" />巡检中</span>
            <span><i style="background: #10b981" />在线待命</span>
            <span><i style="background: #f59e0b" />充电中</span>
            <span><i style="background: #ef4444" />故障</span>
            <span><i style="background: #94a3b8" />离线</span>
          </div>
        </div>
        <div class="map-devices">
          <div
            v-for="device in selectedDevices.slice(0, 8)"
            :key="device.id"
            class="map-chip"
            @click="go('/devices')"
          >
            <i
              class="dot"
              :style="{
                background:
                  device.status === '故障'
                    ? '#ef4444'
                    : device.status === '执行任务'
                      ? '#0284c7'
                      : device.status === '充电中'
                        ? '#f59e0b'
                        : device.status === '在线待命'
                          ? '#10b981'
                          : '#94a3b8'
              }"
            />
            <b>{{ device.deviceId }}</b>
            <span class="muted">
              {{ device.zoneName || "未分配区域" }} ·
              {{ device.bodyOnline ? `${device.battery}%` : "—" }}
            </span>
          </div>
          <div v-if="selectedDevices.length > 8" class="map-chip" @click="go('/devices')">
            <span class="muted">还有 {{ selectedDevices.length - 8 }} 台设备</span>
          </div>
          <div v-if="!selectedDevices.length" class="muted" style="font-size: 12px">
            当前园区暂无归你管辖的设备
          </div>
        </div>
      </div>

      <div>
        <div class="card">
          <div class="card-head">
            <div class="card-title">
              <AppIcon name="cpu" />设备状态分布
              <span class="card-sub">共 {{ summary.totals.devices }} 台</span>
            </div>
            <button class="btn sm ghost" @click="go('/devices')">
              查看全部 <AppIcon name="chevron-right" />
            </button>
          </div>
          <div class="card-body">
            <div class="donut-wrap">
              <div class="donut" :style="{ background: `conic-gradient(${donutGradient})` }">
                <div class="donut-center">
                  <b>{{ summary.totals.devices }}</b><span>设备总数</span>
                </div>
              </div>
              <div class="legend">
                <div v-for="item in summary.deviceStatus" :key="item.name" class="legend-row">
                  <i :style="{ background: item.color }" />
                  <span>{{ item.name }}</span>
                  <b>{{ item.value }} 台</b>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div class="card mt18">
          <div class="card-head">
            <div class="card-title">
              <AppIcon name="bell" />实时告警
              <span class="card-sub">{{ summary.kpis.pendingAlerts.value }} 条待处理</span>
            </div>
            <button class="btn sm ghost" @click="go('/alerts')">
              告警中心 <AppIcon name="chevron-right" />
            </button>
          </div>
          <div class="card-body" style="padding-top: 12px">
            <div v-for="alert in summary.alerts" :key="alert.id" class="alert-item">
              <div
                class="alert-ico"
                :class="
                  alert.level === '严重' ? 'danger' : alert.level === '警告' ? 'warn' : 'info'
                "
              >
                <AppIcon :name="alert.level === '提示' ? 'info' : 'warn'" />
              </div>
              <div class="alert-txt">
                <b>{{ alert.content }}</b>
                <span>{{ alert.parkName }} · {{ alert.deviceCode }}</span>
              </div>
              <div class="alert-time">{{ timeOnly(alert.occurredAt) }}</div>
            </div>
            <div v-if="!summary.alerts.length" class="empty-box">暂无告警记录</div>
          </div>
        </div>
      </div>
    </div>

    <div class="grid row-2 mt18">
      <div class="card">
        <div class="card-head">
          <div class="card-title">
            <AppIcon name="activity" />近 7 日任务趋势
            <span class="card-sub">计划 vs 完成</span>
          </div>
          <div>
            <span class="tag blue">● 计划</span>
            <span class="tag teal">● 完成</span>
          </div>
        </div>
        <div class="card-body">
          <EChart :option="trendOption" height="210px" />
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title">
            <AppIcon name="clock" />最新动态
            <span class="card-sub">实时事件流</span>
          </div>
        </div>
        <div class="card-body">
          <div class="timeline">
            <div v-for="event in summary.timeline" :key="event.id" class="tl-item">
              <div class="tl-dot" :class="eventTone(event.eventType)">
                <AppIcon :name="eventIcon(event.eventType)" />
              </div>
              <div class="tl-body">
                <b>{{ event.title }}</b>
                <p>{{ event.description }}</p>
                <time>{{ timeOnly(event.createdAt) }}</time>
              </div>
            </div>
          </div>
          <div v-if="!summary.timeline.length" class="empty-box">暂无动态</div>
        </div>
      </div>
    </div>
  </div>

  <div v-else class="empty-box">正在加载驾驶舱数据…</div>
</template>

<style scoped>
.park-select {
  height: 28px;
  width: auto;
  min-width: 0;
  padding: 0 8px;
  font-size: 11.5px;
  border-radius: 7px;
  margin-left: 2px;
}
</style>
