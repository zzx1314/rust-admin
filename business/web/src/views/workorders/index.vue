<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { deviceApi, parkApi, staffApi, workOrderApi } from "@/api/ops";
import type { Device, Park, Staff, WorkOrder, WorkOrderDetail } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import { downloadCsv, initials } from "@/utils/format";

const route = useRoute();
const router = useRouter();

/** 工单生命周期：与后端 `STATUS_FLOW` 一一对应 */
const WO_COLUMNS = [
  { key: "待接单", color: "#0284c7", action: "接单" },
  { key: "处理中", color: "#d97706", action: "提交处理" },
  { key: "待验收", color: "#7c3aed", action: "验收通过" },
  { key: "已完成", color: "#10b981", action: "" }
];
const WO_STATUSES = WO_COLUMNS.map(column => column.key);
const WO_TYPES = ["告警处置", "巡检复核", "设备维护", "地图补采", "临时任务"];
const WO_PRIORITIES = ["紧急", "高", "中", "低"];

const orders = ref<WorkOrder[]>([]);
const staff = ref<Staff[]>([]);
const parks = ref<Park[]>([]);
const devices = ref<Device[]>([]);
const viewMode = ref<"board" | "list">("board");
const statusFilter = ref("全部");
/**
 * 只看我负责的：按当前账号绑定的责任人过滤（`staff.user_id`）。
 * 账号没绑责任人时后端退化成「自己建的」，所以这个开关不会一点就空白。
 */
const mineOnly = ref(false);
const loading = ref(true);
const draggedId = ref<number | null>(null);

const createVisible = ref(false);
const saving = ref(false);
const form = reactive({
  title: "",
  orderType: "告警处置",
  priority: "中",
  staffId: null as number | null,
  parkId: null as number | null,
  deviceId: null as number | null,
  deadline: "",
  description: "",
  source: "手动创建",
  sourceRef: null as string | null,
  alertId: null as number | null
});

const detailVisible = ref(false);
const detail = ref<WorkOrderDetail | null>(null);
const progressNote = ref("");

const counts = computed(() => {
  const result: Record<string, number> = { 全部: orders.value.length };
  for (const status of WO_STATUSES) {
    result[status] = orders.value.filter(order => order.status === status).length;
  }
  return result;
});

const visibleOrders = computed(() =>
  statusFilter.value === "全部"
    ? orders.value
    : orders.value.filter(order => order.status === statusFilter.value)
);

const devicesOfForm = computed(() =>
  form.parkId === null
    ? devices.value
    : devices.value.filter(device => device.parkId === form.parkId)
);

function ordersOf(status: string) {
  return orders.value.filter(order => order.status === status);
}

const prioTone = (priority: string) =>
  ({ 紧急: "p-urgent", 高: "p-high", 中: "p-mid", 低: "p-low" })[priority] ?? "p-low";

/** 时限是否已过期：只对未完成的工单标红 */
function isLate(order: WorkOrder) {
  if (!order.deadline || order.status === "已完成") return false;
  return order.deadline < currentStamp();
}

function currentStamp(): string {
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())} ${pad(now.getHours())}:${pad(now.getMinutes())}`;
}

/** 时限展示：当天只显示 HH:MM，跨天显示 MM-DD HH:MM */
function deadlineLabel(value?: string | null) {
  if (!value) return "—";
  const today = currentStamp().slice(0, 10);
  return value.startsWith(today) ? value.slice(11, 16) : value.slice(5, 16);
}

function logTone(action: string) {
  if (action === "close") return "ok";
  if (action === "submit") return "warn";
  return "";
}

function logIcon(action: string) {
  if (action === "created") return "plus";
  if (action === "close") return "check-circle";
  return "activity";
}

async function load() {
  loading.value = true;
  try {
    orders.value = await workOrderApi.list(mineOnly.value ? { mine: true } : undefined);
  } finally {
    loading.value = false;
  }
}

async function loadOptions() {
  const [staffList, parkList, deviceList] = await Promise.all([
    staffApi.list(),
    parkApi.list(),
    deviceApi.list()
  ]);
  staff.value = staffList;
  parks.value = parkList;
  devices.value = deviceList;
}

function defaultDeadline(): string {
  const target = new Date(Date.now() + 2 * 60 * 60 * 1000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${target.getFullYear()}-${pad(target.getMonth() + 1)}-${pad(target.getDate())}T${pad(target.getHours())}:${pad(target.getMinutes())}`;
}

function openCreate(preset?: Partial<typeof form>) {
  form.title = preset?.title ?? "";
  form.orderType = preset?.orderType ?? "告警处置";
  form.priority = preset?.priority ?? "中";
  form.staffId = preset?.staffId ?? staff.value.find(item => item.status === "在岗")?.id ?? staff.value[0]?.id ?? null;
  form.parkId = preset?.parkId ?? parks.value[0]?.id ?? null;
  form.deviceId = preset?.deviceId ?? null;
  form.deadline = preset?.deadline ?? defaultDeadline();
  form.description = preset?.description ?? "";
  form.source = preset?.source ?? "手动创建";
  form.sourceRef = preset?.sourceRef ?? null;
  form.alertId = preset?.alertId ?? null;
  createVisible.value = true;
}

async function submitCreate() {
  if (!form.title.trim()) {
    ElMessage.error("请填写工单标题");
    return;
  }
  if (!form.staffId) {
    ElMessage.error("请选择处理责任人");
    return;
  }
  saving.value = true;
  try {
    await workOrderApi.create({
      title: form.title.trim(),
      orderType: form.orderType,
      priority: form.priority,
      staffId: form.staffId,
      parkId: form.parkId,
      deviceId: form.deviceId,
      // datetime-local 给的是 `YYYY-MM-DDTHH:mm`，库里统一存空格分隔
      deadline: form.deadline ? form.deadline.replace("T", " ") : null,
      description: form.description || null,
      source: form.source,
      sourceRef: form.sourceRef,
      alertId: form.alertId
    });
    createVisible.value = false;
    statusFilter.value = "全部";
    ElMessage.success("工单已创建并派发");
    await load();
  } finally {
    saving.value = false;
  }
}

async function openDetail(order: WorkOrder) {
  detail.value = await workOrderApi.get(order.id);
  progressNote.value = "";
  detailVisible.value = true;
}

function nextStatus(order: WorkOrder) {
  const index = WO_STATUSES.indexOf(order.status);
  return index >= 0 && index < WO_STATUSES.length - 1 ? WO_STATUSES[index + 1] : null;
}

/** 详情页主按钮：按状态机推进一步（接单 / 提交 / 验收） */
async function advance() {
  if (!detail.value) return;
  const target = nextStatus(detail.value);
  if (!target) return;
  await changeStatus(detail.value.id, target);
}

async function changeStatus(id: number, status: string) {
  const updated = await workOrderApi.updateStatus(id, { status });
  detail.value = updated;
  ElMessage.success(`${updated.orderCode} 已流转至「${updated.status}」`);
  await load();
  // 导航栏红点跟着刷新
  window.dispatchEvent(new CustomEvent("robodog:workorders-changed"));
}

async function addNote() {
  if (!detail.value) return;
  if (!progressNote.value.trim()) {
    ElMessage.error("请填写处理记录");
    return;
  }
  detail.value = await workOrderApi.addLog(detail.value.id, progressNote.value.trim());
  progressNote.value = "";
  ElMessage.success("已记录处理进展");
}

async function removeOrder(order: WorkOrder) {
  await ElMessageBox.confirm(`确定删除工单 ${order.orderCode} 吗？`, "删除工单", {
    type: "warning"
  });
  await workOrderApi.remove(order.id);
  detailVisible.value = false;
  ElMessage.success("工单已删除");
  await load();
}

function onDragStart(order: WorkOrder) {
  draggedId.value = order.id;
}

async function onDrop(status: string) {
  const id = draggedId.value;
  draggedId.value = null;
  if (!id) return;
  const order = orders.value.find(item => item.id === id);
  if (!order || order.status === status) return;
  await workOrderApi.updateStatus(id, { status });
  ElMessage.success(`${order.orderCode} 已移至「${status}」`);
  await load();
  window.dispatchEvent(new CustomEvent("robodog:workorders-changed"));
}

function exportCsv() {
  const rows: Array<Array<unknown>> = [
    ["工单编号", "标题", "类型", "来源", "优先级", "处理人", "园区", "设备", "时限", "状态", "进度"]
  ];
  for (const order of visibleOrders.value) {
    rows.push([
      order.orderCode,
      order.title,
      order.orderType,
      order.source,
      order.priority,
      order.assignee,
      order.parkName ?? "",
      order.deviceCode ?? "",
      order.deadline ?? "",
      order.status,
      `${order.progress}%`
    ]);
  }
  downloadCsv(`工单列表-${currentStamp().slice(0, 10)}.csv`, rows);
  ElMessage.success("工单列表已导出");
}

onMounted(async () => {
  await Promise.all([load(), loadOptions()]);
  // 从排班详情 / 告警中心「转工单」进来时直接打开新建弹窗并预填
  if (route.query.create === "1") {
    const staffId = Number(route.query.staffId);
    const parkId = Number(route.query.parkId);
    const deviceId = Number(route.query.deviceId);
    const alertId = Number(route.query.alertId);
    openCreate({
      title: String(route.query.title ?? ""),
      orderType: String(route.query.orderType ?? "巡检复核"),
      staffId: Number.isFinite(staffId) && staffId > 0 ? staffId : null,
      parkId: Number.isFinite(parkId) && parkId > 0 ? parkId : null,
      deviceId: Number.isFinite(deviceId) && deviceId > 0 ? deviceId : null,
      source: String(route.query.source ?? "手动创建"),
      sourceRef: route.query.sourceRef ? String(route.query.sourceRef) : null,
      description: String(route.query.description ?? ""),
      alertId: Number.isFinite(alertId) && alertId > 0 ? alertId : null,
      priority: String(route.query.priority ?? "中")
    });
    // 预填参数用完就清掉：刷新页面不该再弹一次新建窗口
    void router.replace({ path: "/workorders" });
  }
  // 从告警中心「查看工单」/ 企业微信消息里的链接进来时直接打开这条工单详情
  const focus = Number(route.query.focus);
  if (Number.isFinite(focus) && focus > 0) {
    const target = orders.value.find(order => order.id === focus);
    if (target) await openDetail(target);
    void router.replace({ path: "/workorders" });
  }
});
</script>

<template>
  <div>
    <PageHeader title="工单管理" subtitle="把异常与人工任务派发给责任人，跟踪处理全过程">
      <template #actions>
        <button class="btn" @click="exportCsv">
          <AppIcon name="download" />导出
        </button>
        <button class="btn primary" @click="openCreate()">
          <AppIcon name="plus" />新建工单
        </button>
      </template>
    </PageHeader>

    <div class="stat-strip">
      <div
        v-for="tab in [{ key: '全部', color: '#0284c7' }, ...WO_COLUMNS.map(column => ({ key: column.key, color: column.color }))]"
        :key="tab.key"
        class="stat-tab"
        :class="{ on: statusFilter === tab.key }"
        @click="statusFilter = tab.key"
      >
        <div class="st-label"><i :style="{ background: tab.color }" />{{ tab.key === "全部" ? "全部工单" : tab.key }}</div>
        <div class="st-val">{{ counts[tab.key] ?? 0 }} <small>单</small></div>
      </div>
    </div>

    <div class="card-head" style="background: transparent; border: none; padding: 0 0 12px">
      <div class="card-title" />
      <div class="seg" style="margin-right: 8px">
        <button
          :class="{ on: mineOnly }"
          title="只看派给「我」（当前账号绑定的责任人）的单。账号未绑定责任人时按「自己建的」算"
          @click="mineOnly = !mineOnly; void load()"
        >
          只看我负责的
        </button>
      </div>
      <div class="seg">
        <button
          v-for="key in ['全部', ...WO_STATUSES]"
          :key="key"
          :class="{ on: statusFilter === key }"
          @click="statusFilter = key"
        >
          {{ key }}
        </button>
        <span style="width: 8px" />
        <button :class="{ on: viewMode === 'board' }" @click="viewMode = 'board'">
          <AppIcon name="board" />看板
        </button>
        <button :class="{ on: viewMode === 'list' }" @click="viewMode = 'list'">
          <AppIcon name="list" />列表
        </button>
      </div>
    </div>

    <div v-if="viewMode === 'board'" class="kanban">
      <div
        v-for="column in WO_COLUMNS"
        :key="column.key"
        class="kcol"
        :data-wo-col="column.key"
        @dragover.prevent
        @drop="onDrop(column.key)"
      >
        <div class="kcol-head">
          <b><i :style="{ background: column.color }" />{{ column.key }}</b>
          <span class="kcol-count">{{ ordersOf(column.key).length }}</span>
        </div>
        <div
          v-for="order in ordersOf(column.key)"
          :key="order.id"
          class="wo-card"
          draggable="true"
          @dragstart="onDragStart(order)"
          @click="openDetail(order)"
        >
          <div class="wo-top">
            <div style="min-width: 0">
              <div class="wo-id">{{ order.orderCode }} · {{ order.orderType }}</div>
              <div class="wo-title">{{ order.title }}</div>
            </div>
            <span class="prio" :class="prioTone(order.priority)"><i />{{ order.priority }}</span>
          </div>
          <div class="wo-meta">
            <span><AppIcon name="pin" />{{ order.parkName ?? "未指定园区" }}</span>
            <span>
              <AppIcon name="dog" />
              <span class="mono">{{ order.deviceCode ?? "—" }}</span>
              <span>· 来源 {{ order.source }}</span>
            </span>
            <span>
              <AppIcon name="clock" />创建 {{ deadlineLabel(order.createdAt) }} · 时限
              <b class="wo-deadline" :class="{ late: isLate(order) }" style="font-weight: 600">
                {{ deadlineLabel(order.deadline) }}
              </b>
            </span>
          </div>
          <div class="tc-prog">
            <div class="bar" :class="{ ok: order.progress >= 100 }">
              <i :style="{ width: `${order.progress}%` }" />
            </div>
            <span class="num" style="font-size: 11px; color: var(--muted)">{{ order.progress }}%</span>
          </div>
          <div class="wo-assignee">
            <span class="sc-ava" :style="{ background: order.staffColor ?? '#94a3b8' }">
              {{ initials(order.assignee) }}
            </span>
            <span class="wa-name">{{ order.assignee }}</span>
            <span style="font-size: 10px; color: var(--muted)">{{ order.staffTeam ?? "" }}</span>
          </div>
        </div>
        <div
          v-if="!ordersOf(column.key).length"
          style="text-align: center; padding: 24px 0; color: var(--muted); font-size: 11px"
        >
          {{ loading ? "加载中…" : "暂无工单" }}
        </div>
      </div>
    </div>

    <div v-else class="card">
      <div class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>工单编号</th>
              <th>标题 / 来源</th>
              <th>优先级</th>
              <th>处理人</th>
              <th>园区 / 设备</th>
              <th>时限</th>
              <th>状态</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="order in visibleOrders" :key="order.id">
              <td class="mono">{{ order.orderCode }}</td>
              <td>
                <div class="cell-main">{{ order.title }}</div>
                <div class="cell-sub">{{ order.orderType }} · 来源 {{ order.source }}</div>
              </td>
              <td>
                <span class="prio" :class="prioTone(order.priority)"><i />{{ order.priority }}</span>
              </td>
              <td>
                <div class="row-flex">
                  <span class="sc-ava" :style="{ background: order.staffColor ?? '#94a3b8' }">
                    {{ initials(order.assignee) }}
                  </span>
                  <div>
                    <div class="cell-main">{{ order.assignee }}</div>
                    <div class="cell-sub">{{ order.staffTeam ?? "" }}</div>
                  </div>
                </div>
              </td>
              <td>
                {{ order.parkName ?? "—" }}
                <div class="cell-sub mono">{{ order.deviceCode ?? "—" }}</div>
              </td>
              <td class="mono" :class="{ late: isLate(order) }">
                {{ deadlineLabel(order.deadline) }}
              </td>
              <td><StatusBadge :status="order.status" /></td>
              <td>
                <div class="actions">
                  <button class="btn sm" @click="openDetail(order)">
                    <AppIcon name="eye" />详情
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!visibleOrders.length">
              <td colspan="8">
                <div class="empty-box">{{ loading ? "加载中…" : "暂无工单" }}</div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="tfoot"><span>显示 {{ visibleOrders.length }} 条工单</span></div>
    </div>

    <!-- 新建工单 -->
    <el-dialog v-model="createVisible" title="新建工单" width="640px">
      <div class="form-grid">
        <div class="field" style="grid-column: 1 / -1">
          <label>工单标题 <em>*</em></label>
          <input v-model="form.title" class="input" placeholder="例如：RD-009 电池温度异常处置" />
        </div>
        <div class="field">
          <label>工单类型</label>
          <select v-model="form.orderType" class="select">
            <option v-for="type in WO_TYPES" :key="type" :value="type">{{ type }}</option>
          </select>
        </div>
        <div class="field">
          <label>优先级</label>
          <select v-model="form.priority" class="select">
            <option v-for="priority in WO_PRIORITIES" :key="priority" :value="priority">
              {{ priority }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>处理责任人 <em>*</em></label>
          <select v-model="form.staffId" class="select">
            <option v-for="item in staff" :key="item.id" :value="item.id">
              {{ item.name }}（{{ item.role ?? "运维" }}）
            </option>
          </select>
        </div>
        <div class="field">
          <label>所属园区</label>
          <select v-model="form.parkId" class="select">
            <option :value="null">未指定</option>
            <option v-for="park in parks" :key="park.id" :value="park.id">
              {{ park.parkName }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>关联设备</label>
          <select v-model="form.deviceId" class="select">
            <option :value="null">未指定</option>
            <option v-for="device in devicesOfForm" :key="device.id" :value="device.id">
              {{ device.deviceId }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>处理时限</label>
          <input v-model="form.deadline" class="input" type="datetime-local" />
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>问题描述</label>
          <textarea
            v-model="form.description"
            class="textarea"
            placeholder="描述异常现象、处理要求与验收标准"
          />
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <div class="link-hint">
            工单创建后进入「待接单」，由责任人接单 → 处理 → 提交验收 → 验收通过后关闭。
            来源为告警的工单会保留原始告警编号。
          </div>
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="createVisible = false">取消</button>
        <button class="btn primary" :disabled="saving" @click="submitCreate">
          <AppIcon name="send" />派发工单
        </button>
      </template>
    </el-dialog>

    <!-- 工单详情 -->
    <el-dialog
      v-model="detailVisible"
      title="工单详情"
      width="720px"
      class="wo-detail-dialog"
    >
      <template v-if="detail">
        <div
          style="display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; margin-bottom: 12px"
        >
          <div>
            <div style="font-size: 15px; font-weight: 700; line-height: 1.4">{{ detail.title }}</div>
            <div style="font-size: 11px; color: var(--muted); margin-top: 4px">
              {{ detail.orderCode }} · {{ detail.orderType }} · 来源 {{ detail.source }} · 创建于
              {{ detail.createdAt }}
            </div>
          </div>
          <span class="prio" :class="prioTone(detail.priority)"><i />{{ detail.priority }}</span>
        </div>
        <div class="kv"><span>当前状态</span><b><StatusBadge :status="detail.status" /></b></div>
        <div class="kv">
          <span>处理责任人</span>
          <b>{{ detail.assignee }}{{ detail.staffTeam ? ` · ${detail.staffTeam}` : "" }}</b>
        </div>
        <div class="kv">
          <span>所属园区 / 设备</span>
          <b>{{ detail.parkName ?? "—" }} / <span class="mono">{{ detail.deviceCode ?? "—" }}</span></b>
        </div>
        <div class="kv">
          <span>处理时限</span>
          <b class="mono" :class="{ 'wo-deadline late': isLate(detail) }">
            {{ detail.deadline ?? "—" }}
          </b>
        </div>
        <div class="kv"><span>完成进度</span><b>{{ detail.progress }}%</b></div>
        <div class="bar" :class="{ ok: detail.progress >= 100 }" style="margin: 10px 0 14px">
          <i :style="{ width: `${detail.progress}%` }" />
        </div>
        <div class="rt-hint" style="margin-bottom: 14px">{{ detail.description ?? "暂无问题描述" }}</div>

        <div class="card-head" style="padding: 0 0 10px; border: none">
          <div class="card-title">
            <AppIcon name="activity" />处理轨迹
            <span class="card-sub">共 {{ detail.logs.length }} 条记录</span>
          </div>
        </div>
        <div class="timeline">
          <div v-for="log in detail.logs" :key="log.id" class="tl-item">
            <div class="tl-dot" :class="logTone(log.action)">
              <AppIcon :name="logIcon(log.action)" />
            </div>
            <div class="tl-body">
              <b>{{ log.content }}</b>
              <p>操作人：{{ log.actor }}　<time>{{ log.createdAt }}</time></p>
            </div>
          </div>
        </div>

        <div class="wo-log-form">
          <input
            v-model="progressNote"
            class="input"
            placeholder="补充一条处理进展（例如：已更换充电触点，观察一个充放电周期）"
            @keydown.enter="addNote"
          />
          <button class="btn" @click="addNote"><AppIcon name="edit" />记录</button>
        </div>
      </template>
      <template #footer>
        <button v-if="detail" class="btn" @click="removeOrder(detail)">
          <AppIcon name="trash" />删除
        </button>
        <button class="btn" @click="detailVisible = false">关闭</button>
        <button
          v-if="detail && nextStatus(detail)"
          class="btn primary"
          @click="advance"
        >
          <AppIcon name="check" />{{ WO_COLUMNS.find(column => column.key === detail?.status)?.action }}
        </button>
      </template>
    </el-dialog>
  </div>
</template>

<style scoped>
.wo-detail-dialog :deep(.el-dialog__body) {
  max-height: 72vh;
  overflow-y: auto;
}
</style>
