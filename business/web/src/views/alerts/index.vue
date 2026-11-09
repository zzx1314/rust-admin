<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import { alertApi } from "@/api/ops";
import type { Alert } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import { useAppStore } from "@/stores/app";
import { dateTime } from "@/utils/format";

const app = useAppStore();
const alerts = ref<Alert[]>([]);
const stats = ref<{ total: number; counts: Record<string, number>; pending: number }>({
  total: 0,
  counts: {},
  pending: 0
});
const filter = ref("全部");
const levelFilter = ref("全部");
const detailVisible = ref(false);
const detailAlert = ref<Alert | null>(null);

const tabs = ["全部", "未处理", "处理中", "已处理"];

const filteredAlerts = computed(() =>
  alerts.value.filter(alert => {
    const matchesStatus = filter.value === "全部" || alert.status === filter.value;
    const matchesLevel = levelFilter.value === "全部" || alert.level === levelFilter.value;
    return matchesStatus && matchesLevel;
  })
);

function countOf(key: string) {
  return key === "全部" ? stats.value.total : stats.value.counts[key] ?? 0;
}

async function load() {
  const [list, statsData] = await Promise.all([alertApi.list(), alertApi.stats()]);
  alerts.value = list;
  stats.value = statsData;
  app.pendingAlerts = statsData.pending;
}

async function handleAlert(alert: Alert) {
  await alertApi.handle(alert.id);
  ElMessage.success(`${alert.alertCode} 已标记为处理完成`);
  await load();
}

async function handleAll() {
  if (!stats.value.pending) {
    ElMessage.info("当前没有未处理告警");
    return;
  }
  await ElMessageBox.confirm("确定将全部告警标记为已处理吗？", "批量处理", {
    type: "warning"
  });
  const count = await alertApi.handleAll();
  ElMessage.success(`已处理 ${count} 条告警`);
  await load();
}

function openDetail(alert: Alert) {
  detailAlert.value = alert;
  detailVisible.value = true;
}

const levelTag = (level: string) => {
  if (level === "严重") return "orange";
  if (level === "警告") return "orange";
  return "";
};

onMounted(load);
</script>

<template>
  <div>
    <PageHeader title="告警中心" subtitle="设备异常、任务超时与环境风险的统一处置">
      <template #actions>
        <button class="btn" @click="handleAll">
          <AppIcon name="check-circle" />全部标记已处理
        </button>
      </template>
    </PageHeader>

    <div class="stat-strip">
      <div
        class="stat-tab"
        :class="{ on: filter === '全部' }"
        @click="filter = '全部'"
      >
        <div class="st-label"><i style="background: #0284c7" />告警总数</div>
        <div class="st-val">{{ countOf("全部") }} <small>条</small></div>
      </div>
      <div
        class="stat-tab"
        :class="{ on: filter === '未处理' }"
        @click="filter = '未处理'"
      >
        <div class="st-label"><i style="background: #ef4444" />未处理</div>
        <div class="st-val">{{ countOf("未处理") }} <small>条</small></div>
      </div>
      <div
        class="stat-tab"
        :class="{ on: filter === '处理中' }"
        @click="filter = '处理中'"
      >
        <div class="st-label"><i style="background: #f59e0b" />处理中</div>
        <div class="st-val">{{ countOf("处理中") }} <small>条</small></div>
      </div>
      <div
        class="stat-tab"
        :class="{ on: filter === '已处理' }"
        @click="filter = '已处理'"
      >
        <div class="st-label"><i style="background: #10b981" />已处理</div>
        <div class="st-val">{{ countOf("已处理") }} <small>条</small></div>
      </div>
    </div>

    <div class="card">
      <div class="card-head">
        <div class="card-title">
          <AppIcon name="bell" />告警记录
          <span class="card-sub">按发生时间倒序</span>
        </div>
        <div class="actions">
          <select v-model="levelFilter" class="select" style="width: 130px">
            <option>全部</option>
            <option>严重</option>
            <option>警告</option>
            <option>提示</option>
          </select>
          <div class="seg">
            <button
              v-for="tab in tabs"
              :key="tab"
              :class="{ on: filter === tab }"
              @click="filter = tab"
            >
              {{ tab }}
            </button>
          </div>
        </div>
      </div>
      <div class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>级别</th>
              <th>告警内容</th>
              <th>关联设备</th>
              <th>所属园区</th>
              <th>发生时间</th>
              <th>状态</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="alert in filteredAlerts" :key="alert.id">
              <td><span class="tag" :class="levelTag(alert.level)">{{ alert.level }}</span></td>
              <td>
                <div class="cell-main">{{ alert.content }}</div>
                <div class="cell-sub mono">{{ alert.alertCode }}</div>
              </td>
              <td class="mono">{{ alert.deviceCode ?? "—" }}</td>
              <td>{{ alert.parkName ?? "—" }}</td>
              <td class="mono muted">{{ dateTime(alert.occurredAt) }}</td>
              <td><StatusBadge :status="alert.status" /></td>
              <td>
                <div class="actions">
                  <button
                    v-if="alert.status !== '已处理'"
                    class="btn sm"
                    @click="handleAlert(alert)"
                  >
                    <AppIcon name="check" />标记处理
                  </button>
                  <button class="btn sm ghost icon" @click="openDetail(alert)">
                    <AppIcon name="eye" />
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!filteredAlerts.length">
              <td colspan="7"><div class="empty-box">暂无告警记录</div></td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="tfoot"><span>共 {{ filteredAlerts.length }} 条记录</span></div>
    </div>

    <el-dialog v-model="detailVisible" title="告警详情" width="560px">
      <template v-if="detailAlert">
        <div class="kv">
          <span>告警级别</span><b><StatusBadge :status="detailAlert.level" /></b>
        </div>
        <div class="kv"><span>告警编号</span><b class="mono">{{ detailAlert.alertCode }}</b></div>
        <div class="kv"><span>告警内容</span><b>{{ detailAlert.content }}</b></div>
        <div class="kv"><span>关联设备</span><b class="mono">{{ detailAlert.deviceCode ?? "—" }}</b></div>
        <div class="kv"><span>所属园区</span><b>{{ detailAlert.parkName ?? "—" }}</b></div>
        <div class="kv"><span>发生时间</span><b class="mono">{{ detailAlert.occurredAt }}</b></div>
        <div class="kv">
          <span>处理状态</span><b><StatusBadge :status="detailAlert.status" /></b>
        </div>
        <div class="kv">
          <span>处理时间</span><b class="mono">{{ detailAlert.handledAt ?? "—" }}</b>
        </div>
      </template>
      <template #footer>
        <button class="btn" @click="detailVisible = false">关闭</button>
        <button
          v-if="detailAlert && detailAlert.status !== '已处理'"
          class="btn primary"
          @click="handleAlert(detailAlert); detailVisible = false"
        >
          <AppIcon name="check" />标记处理
        </button>
      </template>
    </el-dialog>
  </div>
</template>
