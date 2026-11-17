<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { parkApi, shiftApi, staffApi, taskApi, zoneApi } from "@/api/ops";
import type { Park, Shift, Staff, Task, Zone } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import { initials } from "@/utils/format";

const router = useRouter();

/** 班次固定三班：时段与后端 `SLOT_RANGES` 保持一致 */
const SHIFT_SLOTS = [
  { name: "早班", time: "08:00 - 16:00" },
  { name: "中班", time: "16:00 - 00:00" },
  { name: "夜班", time: "00:00 - 08:00" }
];
const WEEK_LABELS = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
const SHIFT_STATUSES = ["已发布", "待确认", "草案"];

const viewMode = ref<"week" | "list">("week");
const weekOffset = ref(0);
const shifts = ref<Shift[]>([]);
const staff = ref<Staff[]>([]);
const parks = ref<Park[]>([]);
const zones = ref<Zone[]>([]);
const tasks = ref<Task[]>([]);
const loading = ref(true);

const shiftDialog = ref(false);
const shiftForm = reactive({
  shiftDate: "",
  slot: "早班",
  staffId: null as number | null,
  status: "已发布",
  parkId: null as number | null,
  zoneId: null as number | null,
  taskId: null as number | null,
  note: ""
});

const detailVisible = ref(false);
const detailShift = ref<Shift | null>(null);

/** 复制排班（上一周 / 上个月 → 当前视图区间） */
const copyVisible = ref(false);
const copyScope = ref<"week" | "month">("week");
const copyStatus = ref("待确认");
const copyMode = ref<"skip" | "replace">("skip");
const copyPreview = ref<Shift[]>([]);
const copyPreviewLoading = ref(false);
const copying = ref(false);
const COPY_STATUSES = ["待确认", "草案", "已发布", "保持原状态"];
const COPY_MODES = [
  { value: "skip", label: "跳过目标时段已有排班" },
  { value: "replace", label: "覆盖目标时段已有排班" }
];

const staffVisible = ref(false);
const ruleVisible = ref(false);
/** 责任人库表单：id 为空是新增，有值是在改这一条 */
const staffForm = reactive({
  id: null as number | null,
  name: "",
  role: "",
  team: "",
  phone: "",
  skills: "",
  status: "在岗"
});
const STAFF_STATUSES = ["在岗", "休息", "请假"];
const savingStaff = ref(false);
const isEditingStaff = computed(() => staffForm.id !== null);

function iso(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

const todayIso = iso(new Date());

/** 周视图的 7 天：默认本周，`weekOffset` 前后翻页 */
const weekDays = computed(() => {
  const base = new Date();
  base.setHours(0, 0, 0, 0);
  const mondayOffset = (base.getDay() + 6) % 7;
  base.setDate(base.getDate() - mondayOffset + weekOffset.value * 7);
  return WEEK_LABELS.map((weekday, index) => {
    const date = new Date(base);
    date.setDate(base.getDate() + index);
    const day = iso(date);
    return {
      date: day,
      weekday,
      label: `${date.getMonth() + 1}-${String(date.getDate()).padStart(2, "0")}`,
      today: day === todayIso
    };
  });
});

const weekStart = computed(() => weekDays.value[0].date);
const weekEnd = computed(() => weekDays.value[6].date);
const weekLabel = computed(() => {
  const start = weekDays.value[0].label;
  const end = weekDays.value[6].label;
  return weekOffset.value === 0 ? `本周 ${start} 至 ${end}` : `${start} 至 ${end}`;
});

/** 周视图格子：日期 + 班次 → 排班列表 */
function shiftsOfCell(date: string, slot: string) {
  return shifts.value.filter(shift => shift.shiftDate === date && shift.slot === slot);
}

const shiftStats = computed(() => {
  const count = (status: string) =>
    shifts.value.filter(shift => shift.status === status).length;
  return {
    total: shifts.value.length,
    published: count("已发布"),
    pending: count("待确认") + count("草案"),
    owners: new Set(shifts.value.map(shift => shift.ownerName)).size
  };
});

/** 责任人负载：≥5 班次高亮提示过载 */
const staffLoad = computed(() =>
  staff.value.map(item => {
    const count = shifts.value.filter(shift => shift.ownerName === item.name).length;
    return {
      ...item,
      weekShifts: count,
      percent: Math.min(100, Math.round((count / 6) * 100)),
      overloaded: count >= 5
    };
  })
);

const zonesOfForm = computed(() =>
  zones.value.filter(zone => zone.parkId === shiftForm.parkId)
);
const tasksOfForm = computed(() =>
  shiftForm.parkId === null
    ? tasks.value
    : tasks.value.filter(task => task.parkId === shiftForm.parkId)
);

/**
 * 复制的源 / 目标区间。
 *
 * 周复制：目标 = 当前视图这一周，源 = 上一周（保持同名星期）。
 * 月复制：以当前视图周的**周四**为锚点（跨月周按主月归属），目标 = 该月整月，
 *        源 = 上个月整月（按同一日号对齐，天数不足由后端跳过并回报）。
 */
const copyRanges = computed(() => {
  const anchor = new Date(`${weekDays.value[3].date}T00:00:00`);
  if (copyScope.value === "month") {
    const targetFrom = new Date(anchor.getFullYear(), anchor.getMonth(), 1);
    const targetTo = new Date(anchor.getFullYear(), anchor.getMonth() + 1, 0);
    const sourceFrom = new Date(anchor.getFullYear(), anchor.getMonth() - 1, 1);
    const sourceTo = new Date(anchor.getFullYear(), anchor.getMonth(), 0);
    return {
      source: { from: iso(sourceFrom), to: iso(sourceTo) },
      target: { from: iso(targetFrom), to: iso(targetTo) },
      targetLabel: `${anchor.getFullYear()}-${String(anchor.getMonth() + 1).padStart(2, "0")} 月`
    };
  }
  const from = new Date(`${weekStart.value}T00:00:00`);
  const to = new Date(`${weekEnd.value}T00:00:00`);
  const previousFrom = new Date(from);
  previousFrom.setDate(from.getDate() - 7);
  const previousTo = new Date(to);
  previousTo.setDate(to.getDate() - 7);
  return {
    source: { from: iso(previousFrom), to: iso(previousTo) },
    target: { from: iso(from), to: iso(to) },
    targetLabel: weekOffset.value === 0 ? "本周" : "当前视图周"
  };
});

async function loadCopyPreview() {
  copyPreviewLoading.value = true;
  try {
    copyPreview.value = await shiftApi.list({
      from: copyRanges.value.source.from,
      to: copyRanges.value.source.to
    });
  } finally {
    copyPreviewLoading.value = false;
  }
}

async function openCopy(scope: "week" | "month") {
  copyScope.value = scope;
  copyStatus.value = "待确认";
  copyMode.value = "skip";
  copyVisible.value = true;
  await loadCopyPreview();
}

async function switchCopyScope(scope: "week" | "month") {
  if (copyScope.value === scope) return;
  copyScope.value = scope;
  await loadCopyPreview();
}

async function submitCopy() {
  if (!copyPreview.value.length) {
    ElMessage.info("源区间没有可复制的排班");
    return;
  }
  copying.value = true;
  try {
    const result = await shiftApi.copy({
      sourceFrom: copyRanges.value.source.from,
      sourceTo: copyRanges.value.source.to,
      targetFrom: copyRanges.value.target.from,
      targetTo: copyRanges.value.target.to,
      status: copyStatus.value === "保持原状态" ? null : copyStatus.value,
      mode: copyMode.value
    });
    copyVisible.value = false;
    await load();
    if (result.created === 0 && result.skipped.length) {
      ElMessage.warning(`没有新增排班：${result.skipped[0].reason}`);
      return;
    }
    const replaced = result.replaced ? `，覆盖 ${result.replaced} 条` : "";
    const skipped = result.skipped.length ? `，跳过 ${result.skipped.length} 条` : "";
    ElMessage.success(`已复制 ${result.created} 条排班${replaced}${skipped}`);
  } finally {
    copying.value = false;
  }
}

/** 预览表里的日期显示：MM-DD + 星期 */
function previewDay(value: string) {
  const date = new Date(`${value}T00:00:00`);
  const weekday = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][date.getDay()];
  return `${value.slice(5)} ${weekday}`;
}

async function load() {
  loading.value = true;
  try {
    const [shiftList, staffList] = await Promise.all([
      shiftApi.list({ from: weekStart.value, to: weekEnd.value }),
      staffApi.list({ from: weekStart.value, to: weekEnd.value })
    ]);
    shifts.value = shiftList;
    staff.value = staffList;
  } finally {
    loading.value = false;
  }
}

async function loadOptions() {
  const [parkList, zoneList, taskList] = await Promise.all([
    parkApi.list(),
    zoneApi.list(),
    taskApi.list()
  ]);
  parks.value = parkList;
  zones.value = zoneList;
  tasks.value = taskList;
}

async function loadStaffLibrary() {
  staff.value = await staffApi.list({ from: weekStart.value, to: weekEnd.value });
}

function shiftWeek(delta: number) {
  weekOffset.value += delta;
}

watch(weekOffset, () => {
  void load();
});

function openShiftDialog(preset?: { date?: string; slot?: string }) {
  shiftForm.shiftDate = preset?.date ?? weekDays.value.find(day => day.today)?.date ?? weekStart.value;
  shiftForm.slot = preset?.slot ?? "早班";
  shiftForm.staffId = staff.value.find(item => item.status === "在岗")?.id ?? staff.value[0]?.id ?? null;
  shiftForm.status = "已发布";
  shiftForm.parkId = parks.value[0]?.id ?? null;
  shiftForm.zoneId = null;
  shiftForm.taskId = null;
  shiftForm.note = "";
  shiftDialog.value = true;
}

watch(
  () => shiftForm.parkId,
  () => {
    if (!zonesOfForm.value.some(zone => zone.id === shiftForm.zoneId)) {
      shiftForm.zoneId = zonesOfForm.value[0]?.id ?? null;
    }
    if (shiftForm.taskId !== null && !tasksOfForm.value.some(task => task.id === shiftForm.taskId)) {
      shiftForm.taskId = null;
    }
  }
);

async function submitShift() {
  if (!shiftForm.shiftDate) {
    ElMessage.error("请选择排班日期");
    return;
  }
  if (!shiftForm.staffId) {
    ElMessage.error("请选择责任人");
    return;
  }
  await shiftApi.create({
    shiftDate: shiftForm.shiftDate,
    slot: shiftForm.slot,
    staffId: shiftForm.staffId,
    status: shiftForm.status,
    parkId: shiftForm.parkId,
    zoneId: shiftForm.zoneId,
    taskId: shiftForm.taskId,
    note: shiftForm.note || null
  });
  shiftDialog.value = false;
  ElMessage.success("排班已添加");
  await load();
}

function openDetail(shift: Shift) {
  detailShift.value = shift;
  detailVisible.value = true;
}

async function setDetailStatus(status: string) {
  if (!detailShift.value) return;
  const updated = await shiftApi.update(detailShift.value.id, { status });
  detailShift.value = updated;
  ElMessage.success(`排班已更新为「${status}」`);
  await load();
}

async function removeShift(shift: Shift) {
  await ElMessageBox.confirm(`确定删除排班 ${shift.shiftCode} 吗？`, "删除排班", {
    type: "warning"
  });
  await shiftApi.remove(shift.id);
  detailVisible.value = false;
  ElMessage.success("排班已删除");
  await load();
}

/** 排班 → 工单：责任人 / 园区自动预填，减少一次重复录入 */
function createOrderFromShift(shift: Shift) {
  detailVisible.value = false;
  router.push({
    path: "/workorders",
    query: {
      create: "1",
      title: `${shift.taskName ?? "巡检异常"} · 现场处置`,
      staffId: String(shift.staffId ?? ""),
      parkId: String(shift.parkId ?? ""),
      orderType: "巡检复核"
    }
  });
}

function staffCodeOf(id: number | null) {
  return staff.value.find(item => item.id === id)?.staffCode ?? "";
}

function resetStaffForm() {
  staffForm.id = null;
  staffForm.name = "";
  staffForm.role = "";
  staffForm.team = "";
  staffForm.phone = "";
  staffForm.skills = "";
  staffForm.status = "在岗";
}

function editStaff(item: Staff) {
  staffForm.id = item.id;
  staffForm.name = item.name;
  staffForm.role = item.role ?? "";
  staffForm.team = item.team ?? "";
  staffForm.phone = item.phone ?? "";
  staffForm.skills = item.skills ?? "";
  staffForm.status = item.status;
}

/** 新增 / 修改共用：空串表示清空该字段（后端按传入值覆盖） */
async function saveStaff() {
  if (!staffForm.name.trim()) {
    ElMessage.error("请填写责任人姓名");
    return;
  }
  savingStaff.value = true;
  try {
    const payload = {
      name: staffForm.name.trim(),
      role: staffForm.role,
      team: staffForm.team,
      phone: staffForm.phone,
      skills: staffForm.skills,
      status: staffForm.status
    };
    if (staffForm.id === null) {
      await staffApi.create(payload);
      ElMessage.success("责任人已加入责任人库");
    } else {
      await staffApi.update(staffForm.id, payload);
      ElMessage.success("责任人信息已更新");
    }
    resetStaffForm();
    // 改名会同步排班 / 工单里的姓名，周视图要一起刷新
    await Promise.all([loadStaffLibrary(), load()]);
  } finally {
    savingStaff.value = false;
  }
}

async function removeStaff(item: Staff) {
  await ElMessageBox.confirm(
    `确定移除 ${item.name}（${item.staffCode}）吗？历史排班与工单会保留姓名，但不再关联责任人库。`,
    "移除责任人",
    { type: "warning" }
  );
  await staffApi.remove(item.id);
  ElMessage.success(`${item.name} 已移除`);
  if (staffForm.id === item.id) resetStaffForm();
  await Promise.all([loadStaffLibrary(), load()]);
}

const statusTone = (status: string) =>
  status === "已发布" ? "ok" : status === "待确认" ? "warn" : "neutral";

onMounted(async () => {
  await Promise.all([load(), loadOptions()]);
});
</script>

<template>
  <div>
    <PageHeader title="排班管理" subtitle="在制定巡检计划时指定责任人，按班次统筹人力覆盖">
      <template #actions>
        <button class="btn" @click="staffVisible = true">
          <AppIcon name="user" />责任人库
        </button>
        <button class="btn primary" @click="openShiftDialog()">
          <AppIcon name="plus" />新建排班
        </button>
      </template>
    </PageHeader>

    <div class="shift-bar">
      <AppIcon name="shift" />
      <span class="sb-txt">
        <b>排班是计划层：</b>为每一条巡检计划指定「谁负责」，按周排布早 / 中 / 夜班。排班决定责任归属，
        工单负责具体任务的派发与闭环 —— 派单时会优先推荐当班责任人。
      </span>
      <div class="sb-week">
        <button class="btn sm ghost icon" title="上一周" @click="shiftWeek(-1)">
          <AppIcon name="chevron-left" />
        </button>
        <b>{{ weekLabel }}</b>
        <button class="btn sm ghost icon" title="下一周" @click="shiftWeek(1)">
          <AppIcon name="chevron-right" />
        </button>
      </div>
      <button class="btn sm" @click="openCopy('week')">
        <AppIcon name="shuffle" />复制上一周
      </button>
      <button class="btn sm" @click="openCopy('month')">
        <AppIcon name="calendar" />复制上个月
      </button>
      <button class="btn sm" @click="ruleVisible = true">
        <AppIcon name="info" />排班规则
      </button>
    </div>

    <div class="stat-strip">
      <div class="stat-tab on">
        <div class="st-label"><i style="background: #0284c7" />本周班次</div>
        <div class="st-val">{{ shiftStats.total }} <small>个</small></div>
      </div>
      <div class="stat-tab">
        <div class="st-label"><i style="background: #10b981" />已发布</div>
        <div class="st-val">{{ shiftStats.published }} <small>个</small></div>
      </div>
      <div class="stat-tab">
        <div class="st-label"><i style="background: #f59e0b" />待确认 / 草案</div>
        <div class="st-val">{{ shiftStats.pending }} <small>个</small></div>
      </div>
      <div class="stat-tab">
        <div class="st-label"><i style="background: #7c3aed" />在岗责任人</div>
        <div class="st-val">{{ shiftStats.owners }} <small>人</small></div>
      </div>
    </div>

    <div class="card" style="margin-bottom: 16px">
      <div class="card-head">
        <div class="card-title">
          <AppIcon name="calendar" />本周排班表
          <span class="card-sub">{{ weekStart }} 至 {{ weekEnd }}</span>
        </div>
        <div class="seg">
          <button :class="{ on: viewMode === 'week' }" @click="viewMode = 'week'">
            <AppIcon name="board" />周视图
          </button>
          <button :class="{ on: viewMode === 'list' }" @click="viewMode = 'list'">
            <AppIcon name="list" />列表
          </button>
        </div>
      </div>

      <div v-if="viewMode === 'week'" style="padding: 16px">
        <div class="week-grid">
          <div class="wg-h">班次</div>
          <div
            v-for="day in weekDays"
            :key="day.date"
            class="wg-h"
            :class="{ today: day.today }"
          >
            {{ day.weekday }}<small>{{ day.label }}</small>
          </div>
          <template v-for="shiftSlot in SHIFT_SLOTS" :key="shiftSlot.name">
            <div class="wg-slot">
              <b>{{ shiftSlot.name }}</b><small>{{ shiftSlot.time }}</small>
            </div>
            <div
              v-for="day in weekDays"
              :key="`${day.date}-${shiftSlot.name}`"
              class="wg-cell"
              :class="{ 'empty-cell': !shiftsOfCell(day.date, shiftSlot.name).length }"
              @click="openShiftDialog({ date: day.date, slot: shiftSlot.name })"
            >
              <div
                v-for="shift in shiftsOfCell(day.date, shiftSlot.name)"
                :key="shift.id"
                class="shift-chip"
                :class="{ off: shift.status === '草案' }"
                :title="`${shift.parkName ?? ''} / ${shift.taskName ?? ''}`"
                @click.stop="openDetail(shift)"
              >
                <span
                  class="sc-ava"
                  :style="{ background: shift.staffColor ?? '#94a3b8' }"
                >{{ initials(shift.ownerName) }}</span>
                <div style="min-width: 0">
                  <div class="sc-name">{{ shift.ownerName }}</div>
                  <div class="sc-task">{{ shift.taskName || "未关联计划" }}</div>
                </div>
              </div>
              <div class="wg-add"><AppIcon name="plus" />添加</div>
            </div>
          </template>
        </div>
      </div>

      <div v-else class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>排班编号</th>
              <th>日期</th>
              <th>班次 / 时段</th>
              <th>责任人</th>
              <th>园区 / 区域</th>
              <th>关联计划</th>
              <th>负责设备</th>
              <th>状态</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="shift in shifts" :key="shift.id">
              <td class="mono">{{ shift.shiftCode }}</td>
              <td>
                {{ shift.shiftDate }}
                <div class="cell-sub">
                  {{ weekDays.find(day => day.date === shift.shiftDate)?.weekday ?? "" }}
                </div>
              </td>
              <td>
                <span class="tag blue">{{ shift.slot }}</span>
                <div class="cell-sub mono">{{ shift.timeRange }}</div>
              </td>
              <td>
                <div class="row-flex">
                  <span
                    class="sc-ava"
                    :style="{ background: shift.staffColor ?? '#94a3b8' }"
                  >{{ initials(shift.ownerName) }}</span>
                  <div>
                    <div class="cell-main">{{ shift.ownerName }}</div>
                    <div class="cell-sub">{{ shift.staffRole ?? "" }}</div>
                  </div>
                </div>
              </td>
              <td>
                {{ shift.parkName ?? "—" }}
                <div class="cell-sub">{{ shift.zoneName ?? "" }}</div>
              </td>
              <td>{{ shift.taskName ?? "—" }}</td>
              <td class="mono">{{ shift.deviceCount }} 台</td>
              <td>
                <span class="badge" :class="statusTone(shift.status)">
                  <i />{{ shift.status }}
                </span>
              </td>
              <td>
                <div class="actions">
                  <button class="btn sm ghost icon" @click="openDetail(shift)">
                    <AppIcon name="eye" />
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!shifts.length">
              <td colspan="9">
                <div class="empty-box">{{ loading ? "加载中…" : "暂无排班记录" }}</div>
              </td>
            </tr>
          </tbody>
        </table>
        <div class="tfoot"><span>共 {{ shifts.length }} 条排班</span></div>
      </div>
    </div>

    <div class="grid g3">
      <div class="card">
        <div class="card-head">
          <div class="card-title">
            <AppIcon name="user" />责任人负载
            <span class="card-sub">本周班次分布</span>
          </div>
        </div>
        <div class="card-body">
          <div v-for="item in staffLoad" :key="item.id" class="staff-load">
            <span class="sc-ava" :style="{ background: item.color }">
              {{ initials(item.name) }}
            </span>
            <div class="sl-info">
              <b>{{ item.name }} <span class="tag">{{ item.role ?? "—" }}</span></b>
              <small>{{ item.team ?? "—" }} · {{ item.weekShifts }} 个班次 · 在办 {{ item.openOrderCount }} 单</small>
            </div>
            <div class="load-bar">
              <i :class="{ high: item.overloaded }" :style="{ width: `${item.percent}%` }" />
            </div>
          </div>
          <div v-if="!staffLoad.length" class="empty-box">责任人库为空</div>
        </div>
      </div>
      <div class="card" style="grid-column: span 2">
        <div class="card-head">
          <div class="card-title">
            <AppIcon name="info" />排班说明
            <span class="card-sub">排班与计划、工单的协作关系</span>
          </div>
        </div>
        <div class="card-body">
          <div class="link-hint" style="margin-bottom: 10px">
            <b>计划 → 排班：</b>新建巡检计划时选择责任人，系统按周生成排班格子，直观看到每个人在哪个时段负责哪个区域。
            同一责任人同一时段重复排班会触发冲突提示。
          </div>
          <div class="link-hint" style="margin-bottom: 10px">
            <b>排班 → 工单：</b>当班责任人收到告警或巡检异常时，可从排班详情直接创建工单，责任人自动预填，减少派单环节。
          </div>
          <div class="link-hint">
            <b>负载与替班：</b>右侧负载条显示每人班次数，接近上限（≥5）时高亮提示，便于及时调整或安排替班，避免人力过载。
          </div>
        </div>
      </div>
    </div>

    <!-- 复制排班：上一周 / 上个月 → 当前视图区间 -->
    <el-dialog v-model="copyVisible" title="复制排班" width="680px">
      <div class="form-grid">
        <div class="field" style="grid-column: 1 / -1">
          <label>复制来源</label>
          <div class="seg">
            <button :class="{ on: copyScope === 'week' }" @click="switchCopyScope('week')">
              <AppIcon name="shuffle" />上一周
            </button>
            <button :class="{ on: copyScope === 'month' }" @click="switchCopyScope('month')">
              <AppIcon name="calendar" />上个月
            </button>
          </div>
        </div>
        <div class="field">
          <label>源区间</label>
          <div class="copy-range mono">
            {{ copyRanges.source.from }} 至 {{ copyRanges.source.to }}
            <span class="tag">{{ copyPreview.length }} 条排班</span>
          </div>
        </div>
        <div class="field">
          <label>目标区间</label>
          <div class="copy-range mono">
            {{ copyRanges.target.from }} 至 {{ copyRanges.target.to }}
            <span class="tag blue">{{ copyRanges.targetLabel }}</span>
          </div>
        </div>
        <div class="field">
          <label>复制后的状态</label>
          <select v-model="copyStatus" class="select">
            <option v-for="status in COPY_STATUSES" :key="status" :value="status">
              {{ status }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>目标时段已有排班时</label>
          <select v-model="copyMode" class="select">
            <option v-for="mode in COPY_MODES" :key="mode.value" :value="mode.value">
              {{ mode.label }}
            </option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>待复制内容</label>
          <div class="copy-preview">
            <table class="table">
              <thead>
                <tr>
                  <th>日期</th>
                  <th>班次</th>
                  <th>责任人</th>
                  <th>关联计划</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="shift in copyPreview" :key="shift.id">
                  <td class="mono">{{ previewDay(shift.shiftDate) }}</td>
                  <td><span class="tag blue">{{ shift.slot }}</span></td>
                  <td>
                    <div class="row-flex">
                      <span
                        class="sc-ava"
                        :style="{ background: shift.staffColor ?? '#94a3b8' }"
                      >{{ initials(shift.ownerName) }}</span>
                      <span class="cell-main">{{ shift.ownerName }}</span>
                    </div>
                  </td>
                  <td>{{ shift.taskName ?? "—" }}</td>
                </tr>
                <tr v-if="!copyPreview.length">
                  <td colspan="4">
                    <div class="empty-box">
                      {{ copyPreviewLoading ? "加载中…" : "源区间没有排班可复制" }}
                    </div>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <div class="link-hint">
            <b>对齐规则：</b>周复制保持同名星期（周一 → 周一）；月复制按同一日号（5 号 → 5 号），
            目标月天数不足的日期会跳过。复制默认生成「待确认」状态，确认无误后再改为「已发布」。
          </div>
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="copyVisible = false">取消</button>
        <button
          class="btn primary"
          :disabled="copying || !copyPreview.length"
          @click="submitCopy"
        >
          <AppIcon name="check" />开始复制
        </button>
      </template>
    </el-dialog>

    <!-- 新建排班 -->
    <el-dialog v-model="shiftDialog" title="新建排班" width="640px">
      <div class="form-grid">
        <div class="field">
          <label>日期 <em>*</em></label>
          <input v-model="shiftForm.shiftDate" class="input" type="date" />
        </div>
        <div class="field">
          <label>班次 <em>*</em></label>
          <select v-model="shiftForm.slot" class="select">
            <option v-for="shiftSlot in SHIFT_SLOTS" :key="shiftSlot.name" :value="shiftSlot.name">
              {{ shiftSlot.name }}（{{ shiftSlot.time }}）
            </option>
          </select>
        </div>
        <div class="field">
          <label>责任人 <em>*</em></label>
          <select v-model="shiftForm.staffId" class="select">
            <option v-for="item in staff" :key="item.id" :value="item.id">
              {{ item.name }}（{{ item.role ?? "运维" }}）
            </option>
          </select>
        </div>
        <div class="field">
          <label>发布状态</label>
          <select v-model="shiftForm.status" class="select">
            <option v-for="status in SHIFT_STATUSES" :key="status" :value="status">
              {{ status }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>所属园区</label>
          <select v-model="shiftForm.parkId" class="select">
            <option :value="null">未指定</option>
            <option v-for="park in parks" :key="park.id" :value="park.id">
              {{ park.parkName }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>巡检区域</label>
          <select v-model="shiftForm.zoneId" class="select">
            <option :value="null">未指定</option>
            <option v-for="zone in zonesOfForm" :key="zone.id" :value="zone.id">
              {{ zone.zoneName }}
            </option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>关联巡检计划</label>
          <select v-model="shiftForm.taskId" class="select">
            <option :value="null">临时计划（不关联任务）</option>
            <option v-for="task in tasksOfForm" :key="task.id" :value="task.id">
              {{ task.taskName }}（{{ task.taskCode }}）
            </option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>备注</label>
          <input v-model="shiftForm.note" class="input" placeholder="例如：替班 / 临时调整" />
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <div class="link-hint">
            选择责任人后，系统会在该日期 / 班次的排班格中占位。若该责任人同一时段已有排班，将提示冲突。
          </div>
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="shiftDialog = false">取消</button>
        <button class="btn primary" @click="submitShift">
          <AppIcon name="check" />保存排班
        </button>
      </template>
    </el-dialog>

    <!-- 排班详情 -->
    <el-dialog v-model="detailVisible" title="排班详情" width="560px">
      <template v-if="detailShift">
        <div
          style="
            display: flex;
            align-items: center;
            gap: 12px;
            padding: 12px;
            border: 1px solid var(--line);
            border-radius: 10px;
            margin-bottom: 12px;
          "
        >
          <span
            class="sc-ava"
            style="width: 34px; height: 34px; font-size: 14px"
            :style="{ background: detailShift.staffColor ?? '#94a3b8' }"
          >{{ initials(detailShift.ownerName) }}</span>
          <div style="flex: 1">
            <div style="font-size: 14px; font-weight: 700">{{ detailShift.ownerName }}</div>
            <div style="font-size: 11px; color: var(--muted)">
              {{ detailShift.staffRole ?? "—" }} · {{ detailShift.staffTeam ?? "—" }}
            </div>
          </div>
          <StatusBadge :status="detailShift.status" />
        </div>
        <div class="kv"><span>排班编号</span><b class="mono">{{ detailShift.shiftCode }}</b></div>
        <div class="kv"><span>排班日期</span><b>{{ detailShift.shiftDate }}</b></div>
        <div class="kv"><span>班次 / 时段</span><b>{{ detailShift.slot }} · {{ detailShift.timeRange }}</b></div>
        <div class="kv">
          <span>园区 / 区域</span>
          <b>{{ detailShift.parkName ?? "—" }} / {{ detailShift.zoneName ?? "—" }}</b>
        </div>
        <div class="kv"><span>关联巡检计划</span><b>{{ detailShift.taskName ?? "—" }}</b></div>
        <div class="kv"><span>负责设备</span><b class="mono">{{ detailShift.deviceCount }} 台</b></div>
        <div class="kv">
          <span>技能标签</span>
          <b>{{ staff.find(item => item.id === detailShift?.staffId)?.skills ?? "—" }}</b>
        </div>
        <div class="link-hint" style="margin-top: 12px">
          该计划由 <b>{{ detailShift.ownerName }}</b> 负责执行与结果确认。若执行中发现异常，可据此排班直接派发工单。
        </div>
        <div style="display: flex; align-items: center; gap: 8px; margin-top: 14px">
          <span style="font-size: 11.5px; color: var(--muted)">状态调整</span>
          <div class="seg">
            <button
              v-for="status in SHIFT_STATUSES"
              :key="status"
              :class="{ on: detailShift.status === status }"
              @click="setDetailStatus(status)"
            >
              {{ status }}
            </button>
          </div>
        </div>
      </template>
      <template #footer>
        <button v-if="detailShift" class="btn" @click="removeShift(detailShift)">
          <AppIcon name="trash" />删除
        </button>
        <button class="btn" @click="detailVisible = false">关闭</button>
        <button
          v-if="detailShift"
          class="btn primary"
          @click="createOrderFromShift(detailShift)"
        >
          <AppIcon name="ticket" />据此派发工单
        </button>
      </template>
    </el-dialog>

    <!-- 责任人库：新增 / 修改 / 删除，排班与工单共用 -->
    <el-dialog
      v-model="staffVisible"
      title="责任人库"
      width="900px"
      @closed="resetStaffForm"
    >
      <div class="link-hint" style="margin-bottom: 10px">
        共 {{ staff.length }} 名责任人。排班决定谁负责哪个时段，工单派发时从该库中选择处理人。
        修改姓名会同步历史排班 / 工单里的显示名；还有今天及以后的排班或在办工单时不允许移除。
      </div>
      <div class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>姓名</th>
              <th>角色</th>
              <th>班组</th>
              <th>联系电话</th>
              <th>状态</th>
              <th>本周班次</th>
              <th>在办工单</th>
              <th>技能</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="item in staff" :key="item.id">
              <td>
                <div class="row-flex">
                  <span class="sc-ava" :style="{ background: item.color }">
                    {{ initials(item.name) }}
                  </span>
                  <div>
                    <div class="cell-main">{{ item.name }}</div>
                    <div class="cell-sub mono">{{ item.staffCode }}</div>
                  </div>
                </div>
              </td>
              <td>{{ item.role || "—" }}</td>
              <td>{{ item.team || "—" }}</td>
              <td class="mono muted">{{ item.phone || "—" }}</td>
              <td><StatusBadge :status="item.status" /></td>
              <td class="mono">{{ item.shiftCount }} 班</td>
              <td class="mono">{{ item.openOrderCount }} 单</td>
              <td><span class="tag">{{ item.skills || "—" }}</span></td>
              <td>
                <div class="actions">
                  <button class="btn sm" @click="editStaff(item)">
                    <AppIcon name="edit" />修改
                  </button>
                  <button class="btn sm ghost icon" title="移除责任人" @click="removeStaff(item)">
                    <AppIcon name="trash" />
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!staff.length">
              <td colspan="9">
                <div class="empty-box">责任人库为空，先在下方添加处理人</div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <div class="staff-form">
        <div class="staff-form-head">
          <b>{{ isEditingStaff ? "修改责任人" : "新增责任人" }}</b>
          <span v-if="isEditingStaff" class="mono muted">{{ staffCodeOf(staffForm.id) }}</span>
        </div>
        <div class="staff-form-grid">
          <input v-model="staffForm.name" class="input" placeholder="姓名 *" />
          <input v-model="staffForm.role" class="input" placeholder="角色（如 运维工程师）" />
          <input v-model="staffForm.team" class="input" placeholder="班组" />
          <input v-model="staffForm.phone" class="input" placeholder="联系电话" />
          <input v-model="staffForm.skills" class="input" placeholder="技能（如 红外测温 / 应急处置）" />
          <select v-model="staffForm.status" class="select">
            <option v-for="status in STAFF_STATUSES" :key="status" :value="status">
              {{ status }}
            </option>
          </select>
        </div>
        <div class="staff-form-actions">
          <button v-if="isEditingStaff" class="btn" @click="resetStaffForm">取消修改</button>
          <button class="btn primary" :disabled="savingStaff" @click="saveStaff">
            <AppIcon :name="isEditingStaff ? 'check' : 'plus'" />
            {{ isEditingStaff ? "保存修改" : "加入责任人库" }}
          </button>
        </div>
      </div>
    </el-dialog>

    <!-- 排班规则 -->
    <el-dialog v-model="ruleVisible" title="排班规则" width="600px">
      <div class="policy-flow">
        <div class="policy-step">
          <span class="policy-n">1</span>
          <div style="flex: 1">
            <b>计划驱动排班</b>
            <p>每条巡检计划在计划层指定责任人，按早 / 中 / 夜班落到周视图格子里。</p>
          </div>
          <span class="tag blue">计划</span>
        </div>
        <div class="policy-step">
          <span class="policy-n">2</span>
          <div style="flex: 1">
            <b>冲突与负载校验</b>
            <p>同一责任人同一时段重复排班会被拦截；负载条显示每人班次数，≥5 班次高亮提示过载。</p>
          </div>
          <span class="tag">校验</span>
        </div>
        <div class="policy-step">
          <span class="policy-n">3</span>
          <div style="flex: 1">
            <b>排班转工单</b>
            <p>当班责任人发现异常后，从排班详情直接派发工单，责任人自动预填，避免重复录入。</p>
          </div>
          <span class="tag blue">执行</span>
        </div>
        <div class="policy-step">
          <span class="policy-n">4</span>
          <div style="flex: 1">
            <b>工单闭环</b>
            <p>工单经接单 → 处理 → 验收 → 关闭，全过程留痕，结果与告警、设备联动。</p>
          </div>
          <span class="tag">闭环</span>
        </div>
      </div>
    </el-dialog>
  </div>
</template>

<style scoped>
.copy-range {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 34px;
  color: var(--text-2);
}
.copy-preview {
  max-height: 220px;
  overflow-y: auto;
  border: 1px solid var(--line-2);
  border-radius: 10px;
}
.copy-preview .table th {
  position: sticky;
  top: 0;
  z-index: 1;
}
.staff-form {
  margin-top: 14px;
  padding-top: 14px;
  border-top: 1px dashed var(--line);
}
.staff-form-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 10px;
  font-size: 12.5px;
}
.staff-form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr 1fr;
  gap: 8px;
}
.staff-form-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 10px;
}
@media (max-width: 900px) {
  .staff-form-grid {
    grid-template-columns: 1fr 1fr;
  }
}
</style>
