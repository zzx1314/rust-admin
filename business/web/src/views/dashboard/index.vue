<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import type { EChartsOption } from "echarts";
import { ElMessage } from "element-plus";
import { dashboardApi } from "@/api/ops";
import type { DashboardSummary } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import KpiCard from "@/components/KpiCard.vue";
import EChart from "@/components/EChart.vue";
import ParkSituationMap from "@/components/ParkSituationMap.vue";
import AppIcon from "@/components/AppIcon.vue";
import { downloadCsv, timeOnly } from "@/utils/format";

const router = useRouter();
const summary = ref<DashboardSummary | null>(null);
const mapCard = ref<HTMLDivElement>();
let timer = 0;

const donutGradient = computed(() => {
  const data = summary.value?.deviceStatus ?? [];
  const total = data.reduce((sum, item) => sum + item.value, 0) || 1;
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
  if (type === "warning") return "warn";
  if (type === "running") return "play";
  if (type === "charging") return "charge";
  if (type === "command") return "send";
  if (type === "alert") return "bell";
  if (type === "route") return "route";
  return "task";
};

const eventTone = (type: string) => {
  if (type === "success") return "ok";
  if (type === "warning") return "warn";
  return "";
};

async function load() {
  summary.value = await dashboardApi.summary();
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
            <span class="card-sub">张江智造园区 · 一期</span>
          </div>
          <button class="btn sm" @click="fullscreen">
            <AppIcon name="maximize" />全屏
          </button>
        </div>
        <div class="map-stage">
          <ParkSituationMap :devices="summary.mapDevices" />
          <div class="map-badge">
            <AppIcon name="activity" /><b>{{ summary.kpis.onlineDevices.value }}</b>
            台设备在线作业
          </div>
          <div class="map-legend">
            <span><i style="background: #0284c7" />巡检中</span>
            <span><i style="background: #0d9488" />在线待命</span>
            <span><i style="background: #ef4444" />异常设备</span>
            <span><i style="background: #06b6d4" />规划路线</span>
          </div>
        </div>
        <div class="map-devices">
          <div
            v-for="device in summary.mapDevices.slice(0, 5)"
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
                      : '#10b981'
              }"
            />
            <b>{{ device.deviceId }}</b>
            <span class="muted">{{ device.zoneName }} · {{ device.bodyOnline ? `${device.battery}%` : "—" }}</span>
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
