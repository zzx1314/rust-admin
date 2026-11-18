<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { deviceApi, mapApi, parkApi, routeApi, taskApi, zoneApi } from "@/api/ops";
import type { Device, MapVersion, Park, Route, Task, TaskEvent, Zone } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import ProgressBar from "@/components/ProgressBar.vue";
import TaskLiveMonitor from "@/components/TaskLiveMonitor.vue";
import {
  DISPATCH_POLICIES,
  END_ACTIONS,
  EXECUTE_MODES,
  TASK_LEVELS,
  TASK_STATUSES,
  TASK_TYPES,
  endActionLabel
} from "@/utils/constants";
import { parseRouteData } from "@/utils/format";

const router = useRouter();
const tasks = ref<Task[]>([]);
const devices = ref<Device[]>([]);
const parks = ref<Park[]>([]);
const zones = ref<Zone[]>([]);
const routes = ref<Route[]>([]);
/** 地图版本：任务执行监控要把设备位置画到「路线所用的那张地图」上 */
const maps = ref<MapVersion[]>([]);
const stats = ref<{ total: number; counts: Record<string, number>; doneRate: number }>({
  total: 0,
  counts: {},
  doneRate: 0
});
const viewMode = ref<"board" | "table">("board");
/** 首屏加载中：没有任务时要区分「还没拉完」和「真的没有」 */
const loading = ref(true);
const activeStatus = ref("全部");
const draggedId = ref<number | null>(null);
const taskDialog = ref(false);
const editingTaskId = ref<number | null>(null);
const isEditing = computed(() => editingTaskId.value !== null);
const taskForm = reactive({
  taskName: "",
  taskType: "定时巡检",
  executeMode: "周期任务",
  /** 结束动作（返航开关）：空 = 跟随路线（闭环自动返航） */
  endTask: "",
  level: "常规",
  parkId: 1,
  zoneId: null as number | null,
  deviceId: 1,
  routeId: null as number | null,
  planStart: "09:30",
  planEnd: "10:10",
  /** 执行日：1=周一 … 7=周日；空数组 = 每天 */
  scheduleDays: [] as number[],
  dispatchPolicy: "按路线顺序执行",
  description: ""
});

/** 周期任务的执行日选项（与后端 `schedule_days` 一一对应） */
const WEEKDAYS = [
  { value: 1, label: "一" },
  { value: 2, label: "二" },
  { value: 3, label: "三" },
  { value: 4, label: "四" },
  { value: 5, label: "五" },
  { value: 6, label: "六" },
  { value: 7, label: "日" }
];

/** 只有周期任务按时间窗调度；立即执行 / 临时下发 建好就发 */
const isPeriodic = computed(() => taskForm.executeMode === "周期任务");
const detailVisible = ref(false);
const detailTask = ref<Task | null>(null);
/** 任务事件流：定位未就绪、下发失败等可读原因都在这里 */
const detailEvents = ref<TaskEvent[]>([]);
/** 实时监控弹窗（只看视频 + 地图，不打开整个详情） */
const monitorVisible = ref(false);
const monitorTaskId = ref<number | null>(null);

/**
 * 看板列（顺序即执行生命周期）。
 *
 * 后端合法状态是「待执行 / 执行中 / 已暂停 / 已完成 / 异常」；少一个列就会让该状态的
 * 任务在看板上凭空消失（列表视图却能看到）。额外再加一层兜底：库里出现未登记的状态
 * （历史数据 / 后续新增枚举）也会自动生成一列，不再静默丢数据。
 */
const BASE_COLUMNS = [
  { key: "待执行", color: "#94a3b8" },
  { key: "执行中", color: "#3b82f6" },
  { key: "已暂停", color: "#f59e0b" },
  { key: "已完成", color: "#10b981" },
  { key: "异常", color: "#ef4444" }
];

const columns = computed(() => {
  const known = new Set(BASE_COLUMNS.map(column => column.key));
  const extra = Array.from(new Set(tasks.value.map(task => task.taskStatus)))
    .filter(status => status && !known.has(status))
    .sort();
  return [...BASE_COLUMNS, ...extra.map(key => ({ key, color: "#a855f7" }))];
});

const filteredTasks = computed(() =>
  activeStatus.value === "全部"
    ? tasks.value
    : tasks.value.filter(task => task.taskStatus === activeStatus.value)
);

const zonesOfPark = computed(() =>
  zones.value.filter(zone => zone.parkId === taskForm.parkId)
);

/**
 * 巡检路线按园区过滤：后端会校验「路线园区 = 任务园区」，
 * 下拉里列出其他园区的路线只会让人选了之后才报错。
 */
const routesOfForm = computed(() =>
  routes.value.filter(route => route.parkId === taskForm.parkId)
);

/** 还没有任何巡检路线时，任务只能按临时路径下发 */
const routesEmpty = computed(() => routes.value.length === 0);

function goRoutes() {
  router.push("/routes");
}

// 切园区后，原来选中的路线属于别的园区就清掉（回到「临时路径」）
watch(
  () => taskForm.parkId,
  () => {
    if (!zonesOfPark.value.some(zone => zone.id === taskForm.zoneId)) {
      taskForm.zoneId = zonesOfPark.value[0]?.id ?? null;
    }
    if (taskForm.routeId !== null && !routesOfForm.value.some(route => route.id === taskForm.routeId)) {
      taskForm.routeId = routesOfForm.value[0]?.id ?? null;
    }
  }
);

function countOf(status: string) {
  return status === "全部" ? stats.value.total : stats.value.counts[status] ?? 0;
}

function tasksOf(status: string) {
  // 与状态筛选保持一致：看板也服从顶部筛选（全部时就是所有任务）
  return filteredTasks.value.filter(task => task.taskStatus === status);
}

async function load() {
  loading.value = true;
  try {
    const [taskList, statsData, deviceList, parkList, zoneList, routeList, mapList] =
      await Promise.all([
        taskApi.list(),
        taskApi.stats(),
        deviceApi.list(),
        parkApi.list(),
        zoneApi.list(),
        routeApi.list(),
        mapApi.list()
      ]);
    tasks.value = taskList;
    stats.value = statsData;
    devices.value = deviceList;
    parks.value = parkList;
    zones.value = zoneList;
    routes.value = routeList;
    maps.value = mapList;
  } catch (error) {
    // 接口失败时也要把页面渲染出来（空数据 + 空状态），不能只剩一片空白
    console.error(error);
    ElMessage.error("任务数据加载失败，请稍后重试");
  } finally {
    loading.value = false;
  }
}

/** `1,2,3` → `[1,2,3]`；非法 / 空值返回空数组（= 每天） */
function parseScheduleDays(raw?: string | null): number[] {
  return (raw ?? "")
    .split(",")
    .map(item => Number(item.trim()))
    .filter(day => Number.isInteger(day) && day >= 1 && day <= 7);
}

/** 执行日的人话：每天 / 工作日 / 周一、周三 */
function daysLabel(raw?: string | null): string {
  const days = parseScheduleDays(raw);
  if (!days.length || days.length === 7) return "每天";
  if (days.join(",") === "1,2,3,4,5") return "工作日";
  return days.map(day => `周${WEEKDAYS.find(item => item.value === day)?.label ?? "?"}`).join("、");
}

/**
 * 计划时间的人话：`工作日 09:30-10:10`。
 *
 * 立即执行 / 临时下发不看时间窗，这里直接标「立即」，避免运维以为它还要等到点。
 */
function scheduleSummary(task: Task): string {
  const range = task.planStart && task.planEnd ? `${task.planStart}-${task.planEnd}` : "—";
  if (task.executeMode !== "周期任务") return `立即执行 · ${range}`;
  return `${daysLabel(task.scheduleDays)} ${range}`;
}

/** 表单提交体：时间 / 执行日由选择器产出，后端再校验一次 */
function formPayload() {
  return {
    ...taskForm,
    // 空数组 = 每天：这里必须送空串（`null` 表示「保持原值」，清不掉已选的执行日）
    scheduleDays: taskForm.scheduleDays.join(",")
  };
}

function tagTone(type?: string | null) {
  const map: Record<string, string> = {
    定时巡检: "blue",
    应急处置: "orange",
    定点拍照: "purple",
    夜巡任务: "purple",
    设备维护: "orange",
    地图采集: "teal"
  };
  return map[type || ""] || "";
}

async function changeStatus(task: Task, status: string) {
  if (task.taskStatus === status) return;
  // 「执行中」不是纯状态改动：必须走 dispatch 把航点 / 地图真正推给设备
  if (status === "执行中") {
    await startTask(task);
    return;
  }
  const progress = status === "已完成" ? 100 : status === "待执行" ? 0 : task.progress;
  await taskApi.updateStatus(task.id, status, progress);
  await load();
  ElMessage.info(`${task.taskCode} 已移动至「${status}」`);
}

function onDragStart(task: Task) {
  draggedId.value = task.id;
}

async function onDrop(status: string) {
  const task = tasks.value.find(item => item.id === draggedId.value);
  draggedId.value = null;
  if (task) await changeStatus(task, status);
}

async function startTask(task: Task) {
  // dispatch 会真正下发 task_dispatch；设备离线时接口报错，状态不会被改成假「执行中」
  await taskApi.dispatch(task.id);
  await load();
  ElMessage.success(`${task.taskName} 已下发执行`);
  // 下发成功顺手打开实时监控：执行中要看的就是画面和地图位置
  openMonitor(task);
}

/**
 * 恢复已暂停的任务：走 `PUT /tasks/{id}/status`，由服务端判断
 * 「本体已受理过（有 robot_task_id）→ 5006 恢复」还是「没真正下发过 → 重新下发」。
 */
async function resumeTask(task: Task) {
  await taskApi.updateStatus(task.id, "执行中");
  await load();
  ElMessage.success(`${task.taskName} 已恢复执行`);
}

async function pauseTask(task: Task) {
  await taskApi.updateStatus(task.id, "已暂停", task.progress);
  await load();
  ElMessage.info(`${task.taskName} 已暂停`);
}

/**
 * 取消任务：把本体上挂着的任务真正收尾（服务端下发 `task_control cancel`）。
 *
 * 为什么需要：暂停只是 `5004`，本体上的导航任务会一直挂着占住导航能力，
 * 之后重新下发会被回 13897 capability busy。取消走「异常」终态，
 * 服务端 `control_action` 会把它翻译成 `cancel`（客户端用真实 task_id 发 5002）。
 */
async function cancelTask(task: Task) {
  await ElMessageBox.confirm(
    `确定取消「${task.taskName}」吗？会立即终止本体上正在执行/暂停的任务。`,
    "取消任务",
    { type: "warning", confirmButtonText: "取消任务", cancelButtonText: "返回" }
  );
  await taskApi.updateStatus(task.id, "异常", task.progress);
  await load();
  ElMessage.success(`${task.taskName} 已取消`);
}

async function removeTask(task: Task) {
  await ElMessageBox.confirm(`确定删除任务「${task.taskName}」吗？`, "删除任务", {
    type: "warning"
  });
  await taskApi.remove(task.id);
  ElMessage.success("任务已删除");
  await load();
}

function openCreate() {
  editingTaskId.value = null;
  const parkId = parks.value[0]?.id ?? 1;
  Object.assign(taskForm, {
    taskName: "",
    taskType: "定时巡检",
    executeMode: "周期任务",
    endTask: "",
    level: "常规",
    parkId,
    zoneId: zones.value.find(zone => zone.parkId === parkId)?.id ?? null,
    deviceId: devices.value[0]?.id ?? 1,
    routeId: routes.value.find(route => route.parkId === parkId)?.id ?? null,
    planStart: "09:30",
    planEnd: "10:10",
    scheduleDays: [],
    dispatchPolicy: "按路线顺序执行",
    description: ""
  });
  taskDialog.value = true;
}

/// 仅「待执行」的任务允许修改，其余状态需先在看板中回到待执行
function openEdit(task: Task) {
  if (task.taskStatus !== "待执行") {
    ElMessage.warning("仅「待执行」的任务可以修改");
    return;
  }
  editingTaskId.value = task.id;
  Object.assign(taskForm, {
    taskName: task.taskName ?? "",
    taskType: task.taskType ?? "定时巡检",
    executeMode: task.executeMode ?? "周期任务",
    endTask: task.endTask ?? "",
    level: task.level ?? "常规",
    parkId: task.parkId ?? parks.value[0]?.id ?? 1,
    zoneId: task.zoneId ?? null,
    deviceId: task.deviceId ?? devices.value[0]?.id ?? 1,
    routeId: task.routeId ?? null,
    planStart: task.planStart ?? "09:30",
    planEnd: task.planEnd ?? "10:10",
    scheduleDays: parseScheduleDays(task.scheduleDays),
    dispatchPolicy: task.dispatchPolicy ?? "按路线顺序执行",
    description: task.description ?? ""
  });
  taskDialog.value = true;
}

async function saveTask() {
  if (!taskForm.taskName.trim()) {
    ElMessage.warning("请填写任务名称");
    return;
  }
  if (!taskForm.deviceId) {
    ElMessage.warning("请选择执行设备");
    return;
  }
  // 周期任务的时间窗就是调度依据：必须选完整时间段（后端同样会拒）
  if (isPeriodic.value) {
    if (!taskForm.planStart || !taskForm.planEnd) {
      ElMessage.warning("请选择计划时间段");
      return;
    }
    if (taskForm.planStart === taskForm.planEnd) {
      ElMessage.warning("计划开始与结束时间不能相同");
      return;
    }
  }
  const editingId = editingTaskId.value;
  if (editingId !== null) {
    await taskApi.update(editingId, formPayload());
    taskDialog.value = false;
    editingTaskId.value = null;
    ElMessage.success("任务已更新");
    await load();
    return;
  }
  await taskApi.create(formPayload());
  taskDialog.value = false;
  viewMode.value = "board";
  ElMessage.success(
    isPeriodic.value ? "任务已创建，将在配置的时间段内自动下发" : "任务已创建并下发"
  );
  await load();
}

async function openDetail(task: Task) {
  detailTask.value = task;
  detailEvents.value = [];
  detailVisible.value = true;
  await refreshDetail(task.id);
}

/** 拉取详情（任务行 + 事件流）；任务已被删除时关掉弹窗 */
async function refreshDetail(taskId: number) {
  try {
    detailEvents.value = await taskApi.events(taskId);
  } catch {
    // 事件流取不到不影响详情主体（例如旧任务没有事件记录）
  }
}

function editFromDetail() {
  const task = detailTask.value;
  if (!task) return;
  detailVisible.value = false;
  openEdit(task);
}

const detailRoute = computed(() => {
  const routeId = detailTask.value?.routeId;
  if (!routeId) return null;
  return routes.value.find(route => route.id === routeId) ?? null;
});

/** 任务对应的执行设备（实时位置 / 电量 / 定位状态），来自设备列表轮询 */
function deviceOf(task: Task | null): Device | null {
  if (!task?.deviceId) return null;
  return devices.value.find(device => device.id === task.deviceId) ?? null;
}

function routeOf(task: Task | null): Route | null {
  if (!task?.routeId) return null;
  return routes.value.find(route => route.id === task.routeId) ?? null;
}

/**
 * 任务执行要看的图：路线冻结的地图 → 本体当前载图 → 园区地图基线。
 *
 * 优先路线地图：下发到本体的航点坐标就是按这张图换算的，拿别的图会把点位画歪。
 */
function mapOf(task: Task | null): MapVersion | null {
  if (!task) return null;
  const park = parks.value.find(item => item.id === task.parkId);
  const candidates = [
    routeOf(task)?.mapVersionId,
    deviceOf(task)?.activeMapId,
    park?.mapVersionId
  ];
  for (const id of candidates) {
    if (!id) continue;
    const found = maps.value.find(map => map.id === id);
    if (found) return found;
  }
  return null;
}

/** 监控弹窗里的任务：始终从轮询结果取最新一行，进度 / 阶段才是实时的 */
const monitorTask = computed(
  () => tasks.value.find(task => task.id === monitorTaskId.value) ?? null
);

function openMonitor(task: Task) {
  monitorTaskId.value = task.id;
  monitorVisible.value = true;
}

/** 执行中的任务：能看实时监控（视频走背包前置，地图位置由轮询驱动） */
function isRunning(task: Task) {
  return task.taskStatus === "执行中" || task.taskStatus === "已暂停";
}

const detailSequence = computed(() => {
  const route = detailRoute.value;
  if (!route) return [];
  const data = parseRouteData(route.routeData);
  return data.points.map((point, index) => {
    const leg = data.legs[index];
    return {
      name: `P${index + 1} ${point.name}`,
      actions: point.actions.map(action => action.type).join(" / ") || "无动作",
      leg: leg
        ? leg.actions.map(action => action.type).join(" / ") || "静默通行"
        : route.loopMode === "闭环"
          ? "闭环返航"
          : "—"
    };
  });
});

function openRoutePlanner() {
  if (!detailRoute.value) return;
  detailVisible.value = false;
  router.push({ path: "/routes", query: { routeId: detailRoute.value.id } });
}

async function sendFromDetail(command: string) {
  const task = detailTask.value;
  if (!task) return;
  if (command === "start") {
    await taskApi.dispatch(task.id);
    ElMessage.success("任务已下发执行");
  } else if (command === "resume") {
    await taskApi.updateStatus(task.id, "执行中");
    ElMessage.success("任务已恢复执行");
  } else {
    await taskApi.updateStatus(task.id, "已暂停", task.progress);
    ElMessage.info("任务已暂停");
  }
  detailVisible.value = false;
  await load();
  // 「开始执行 / 重新下发」后直接进监控：和看板上的执行按钮保持一致
  if (command === "start") openMonitor(task);
}

function clickStat(status: string) {
  activeStatus.value = status;
  viewMode.value = "table";
}

function clickStatusBadge(status: string) {
  activeStatus.value = status;
}

/** 自动刷新周期（毫秒）：任务进度由设备 5s 上报一次，页面同步即可 */
const REFRESH_MS = 5000;
const lastRefreshed = ref("");
const liveRefreshing = ref(false);
let refreshTimer = 0;

function stampNow() {
  lastRefreshed.value = new Date().toLocaleTimeString("zh-CN", { hour12: false });
}

/**
 * 轮询任务数据（看板 / 列表 / 详情 / 监控共用）。
 *
 * 除了 `tasks` + `stats`，每轮也拉一次 `devices`：任务监控要的实时位置 / 电量 /
 * 定位状态都在设备行上（`position` 由心跳 10s 上报），这样才能在任务页看到
 * 机器狗走到哪了。视频不走这里（统一由背包前置 `dogcam` 提供，与实时视频同源），
 * 设备/园区/路线/地图这类台账只在建任务时用，不重复拉。
 */
async function refreshLive() {
  // 页面在后台 / 编辑弹窗打开时先不抢刷新，避免把用户正在改的上下文顶掉
  if (document.hidden || taskDialog.value || liveRefreshing.value) return;
  liveRefreshing.value = true;
  try {
    const [taskList, statsData, deviceList] = await Promise.all([
      taskApi.list(),
      taskApi.stats(),
      deviceApi.list()
    ]);
    tasks.value = taskList;
    stats.value = statsData;
    devices.value = deviceList;
    stampNow();
    // 监控弹窗里的任务被删了就直接关掉，避免停在一份过期数据上
    if (monitorVisible.value && monitorTaskId.value !== null) {
      if (!taskList.some(item => item.id === monitorTaskId.value)) {
        monitorVisible.value = false;
        monitorTaskId.value = null;
      }
    }
    if (detailVisible.value && detailTask.value) {
      const fresh = taskList.find(item => item.id === detailTask.value?.id);
      if (!fresh) {
        // 任务被删了（或不在当前账号可见范围）：关掉弹窗
        detailVisible.value = false;
        detailTask.value = null;
        return;
      }
      detailTask.value = fresh;
      await refreshDetail(fresh.id);
    }
  } catch {
    // 自动刷新失败不弹错（避免网络抖动时刷屏），下次轮询会自己恢复
  } finally {
    liveRefreshing.value = false;
  }
}

onMounted(async () => {
  await load();
  stampNow();
  refreshTimer = window.setInterval(() => void refreshLive(), REFRESH_MS);
});

onBeforeUnmount(() => {
  window.clearInterval(refreshTimer);
});</script>

<template>
  <div>
    <PageHeader title="计划管理" subtitle="巡检计划的创建、下发、执行跟踪与看板协作">
      <template #actions>
        <button class="btn primary" @click="openCreate">
          <AppIcon name="plus" />新建计划
        </button>
      </template>
    </PageHeader>

    <div class="stat-strip">
      <div
        class="stat-tab"
        :class="{ on: activeStatus === '全部' }"
        @click="clickStat('全部')"
      >
        <div class="st-label"><i style="background: #0284c7" />计划总数</div>
        <div class="st-val">{{ stats.total }} <small>项</small></div>
      </div>
      <div
        v-for="column in columns"
        :key="column.key"
        class="stat-tab"
        :class="{ on: activeStatus === column.key }"
        @click="clickStat(column.key)"
      >
        <div class="st-label"><i :style="{ background: column.color }" />{{ column.key }}</div>
        <div class="st-val">{{ countOf(column.key) }} <small>项</small></div>
      </div>
    </div>

    <div class="toolbar">
      <span v-if="activeStatus !== '全部'" class="tag blue">
        当前筛选：{{ activeStatus }}
        <button
          style="border: 0; background: transparent; color: inherit; cursor: pointer"
          @click="activeStatus = '全部'"
        >
          ×
        </button>
      </span>
      <div class="sep" />
      <div class="seg">
        <button :class="{ on: viewMode === 'board' }" @click="viewMode = 'board'">
          <AppIcon name="board" />看板视图
        </button>
        <button :class="{ on: viewMode === 'table' }" @click="viewMode = 'table'">
          <AppIcon name="list" />列表视图
        </button>
      </div>
      <span class="muted live-hint" title="任务进度由设备每 5s 上报，页面同步轮询">
        <i class="live-dot" />实时 {{ REFRESH_MS / 1000 }}s
        <template v-if="lastRefreshed"> · 上次 {{ lastRefreshed }}</template>
      </span>
    </div>

    <div v-if="!tasks.length && !loading" class="card tasks-empty">
      <div class="tasks-empty-icon"><AppIcon name="task" /></div>
      <b>还没有巡检任务</b>
      <p>
        任务把巡检路线（或临时航点）下发给指定设备，按计划时间执行；<br />
        在下方看板里可以直接新建、下发和跟踪进度。
      </p>
      <p v-if="routesEmpty" class="rt-hint" style="margin: 0 0 4px">
        当前还没有巡检路线：可以先到「巡检任务」编排一条，或直接新建计划用
        “临时路径”下发。
      </p>
      <div class="actions" style="justify-content: center; margin-top: 10px">
        <button class="btn primary" @click="openCreate">
          <AppIcon name="plus" />新建计划
        </button>
        <button class="btn" @click="goRoutes">
          <AppIcon name="route" />去编排路线
        </button>
      </div>
    </div>

    <div v-else-if="viewMode === 'board'" class="kanban">
      <div
        v-for="column in columns"
        :key="column.key"
        class="kcol"
        :data-status="column.key"
        @dragover.prevent
        @drop="onDrop(column.key)"
      >
        <div class="kcol-head">
          <b><i :style="{ background: column.color }" />{{ column.key }}</b>
          <span class="kcol-count">{{ tasksOf(column.key).length }}</span>
        </div>
        <div
          v-for="task in tasksOf(column.key)"
          :key="task.id"
          class="task-card"
          draggable="true"
          @dragstart="onDragStart(task)"
        >
          <div class="tc-top">
            <span class="tag" :class="tagTone(task.taskType)">{{ task.taskType }}</span>
            <span
              v-if="task.level === '紧急' || task.level === '重要'"
              class="tag orange"
            >
              {{ task.level }}
            </span>
          </div>
          <div class="tc-name">{{ task.taskName }}</div>
          <div class="tc-info">
            <span><AppIcon name="dog" />{{ task.deviceCode }} · {{ task.executeMode }}</span>
            <span><AppIcon name="pin" />{{ task.parkName }} / {{ task.zoneName }}</span>
            <span><AppIcon name="clock" />{{ scheduleSummary(task) }}</span>
            <span>
              <AppIcon name="route" />路线
              {{ task.routeCode ?? "—" }} · {{ task.routeId ? "已编排" : "临时路径" }}
            </span>
            <span v-if="isRunning(task)" :title="`实时位置（地图米）：${deviceOf(task)?.position || '未上报'}`">
              <AppIcon name="waypoint" />{{ deviceOf(task)?.position || "位置未上报" }}
            </span>
          </div>
          <div class="tc-prog">
            <ProgressBar
              :value="task.progress"
              :tone="task.taskStatus === '异常' ? 'danger' : task.taskStatus === '已完成' ? 'ok' : ''"
            />
          </div>
          <div class="tc-foot">
            <span class="time mono">{{ task.taskCode }}</span>
            <div class="actions">
              <button
                v-if="task.taskStatus === '待执行'"
                class="btn sm ghost icon"
                title="立即执行"
                @click="startTask(task)"
              >
                <AppIcon name="play" />
              </button>
              <button
                v-if="task.taskStatus === '已暂停'"
                class="btn sm ghost icon"
                title="恢复执行"
                @click="resumeTask(task)"
              >
                <AppIcon name="play" />
              </button>
              <button
                v-if="task.taskStatus === '待执行'"
                class="btn sm ghost icon"
                title="编辑"
                @click="openEdit(task)"
              >
                <AppIcon name="edit" />
              </button>
              <button
                v-if="isRunning(task)"
                class="btn sm ghost icon"
                title="实时监控（视频 + 地图位置）"
                @click="openMonitor(task)"
              >
                <AppIcon name="video" />
              </button>
              <button
                v-if="task.taskStatus === '执行中'"
                class="btn sm ghost icon"
                title="暂停"
                @click="pauseTask(task)"
              >
                <AppIcon name="pause" />
              </button>
              <button
                v-if="task.taskStatus === '执行中' || task.taskStatus === '已暂停'"
                class="btn sm ghost icon"
                title="取消任务（终止本体上的任务）"
                @click="cancelTask(task)"
              >
                <AppIcon name="close" />
              </button>
              <button class="btn sm ghost icon" title="详情" @click="openDetail(task)">
                <AppIcon name="eye" />
              </button>
              <button
                class="btn sm ghost icon"
                title="删除"
                style="color: #dc2626"
                @click="removeTask(task)"
              >
                <AppIcon name="trash" />
              </button>
            </div>
          </div>
        </div>
        <div v-if="!tasksOf(column.key).length" class="empty-box" style="padding: 24px 0">
          暂无任务
        </div>
      </div>
    </div>

    <div v-else class="card">
      <div class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>计划编号</th>
              <th>计划名称</th>
              <th>类型</th>
              <th>执行设备</th>
              <th>关联路线</th>
              <th>园区 / 区域</th>
              <th>计划时间</th>
              <th>状态</th>
              <th>进度</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="task in filteredTasks" :key="task.id">
              <td class="mono cell-main">{{ task.taskCode }}</td>
              <td>
                <div class="cell-main">{{ task.taskName }}</div>
                <div class="cell-sub">{{ task.executeMode }}</div>
              </td>
              <td><span class="tag" :class="tagTone(task.taskType)">{{ task.taskType }}</span></td>
              <td class="mono">{{ task.deviceCode }}</td>
              <td>
                <div class="cell-main mono">{{ task.routeCode ?? "—" }}</div>
                <div class="cell-sub">{{ task.routeName ?? "临时路径" }}</div>
              </td>
              <td>
                {{ task.parkName }}
                <div class="cell-sub">{{ task.zoneName }}</div>
              </td>
              <td class="mono muted">{{ scheduleSummary(task) }}</td>
              <td>
                <StatusBadge :status="task.taskStatus" @click="clickStatusBadge(task.taskStatus)" />
              </td>
              <td style="min-width: 120px">
                <ProgressBar
                  :value="task.progress"
                  :tone="task.taskStatus === '异常' ? 'danger' : ''"
                />
              </td>
              <td>
                <div class="actions">
                  <button
                    v-if="isRunning(task)"
                    class="btn sm ghost icon"
                    title="实时监控（视频 + 地图位置）"
                    @click="openMonitor(task)"
                  >
                    <AppIcon name="video" />
                  </button>
                  <button
                    v-if="task.taskStatus === '待执行'"
                    class="btn sm ghost icon"
                    title="编辑"
                    @click="openEdit(task)"
                  >
                    <AppIcon name="edit" />
                  </button>
                  <button class="btn sm ghost icon" title="详情" @click="openDetail(task)">
                    <AppIcon name="eye" />
                  </button>
                  <button
                    class="btn sm ghost icon"
                    style="color: #dc2626"
                    @click="removeTask(task)"
                  >
                    <AppIcon name="trash" />
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!filteredTasks.length">
              <td colspan="10"><div class="empty-box">没有符合条件的任务</div></td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="tfoot"><span>共 {{ filteredTasks.length }} 项任务</span></div>
    </div>

    <el-dialog
      v-model="taskDialog"
      :title="isEditing ? '编辑巡检任务' : '新建巡检任务'"
      width="680px"
    >
      <div class="form-grid">
        <div class="field" style="grid-column: 1 / -1">
          <label>计划名称 <em>*</em></label>
          <input v-model="taskForm.taskName" class="input" placeholder="例如：A区上午例行巡检" />
        </div>
        <div class="field">
          <label>计划类型</label>
          <select v-model="taskForm.taskType" class="select">
            <option v-for="type in TASK_TYPES" :key="type">{{ type }}</option>
          </select>
        </div>
        <div class="field">
          <label>执行模式</label>
          <select v-model="taskForm.executeMode" class="select">
            <option v-for="mode in EXECUTE_MODES" :key="mode">{{ mode }}</option>
          </select>
        </div>
        <div class="field">
          <label>跑完去哪（返航开关）</label>
          <select v-model="taskForm.endTask" class="select">
            <option v-for="item in END_ACTIONS" :key="item.value" :value="item.value">
              {{ item.label }}
            </option>
          </select>
          <div class="rt-hint" style="margin: 4px 0 0">
            默认跟随路线：闭环路线跑完自动返回起点。「原地待命」会停在最后一个航点，
            需要人再去处理；「返回充电点」要求地图已设置回充点。
          </div>
        </div>
        <div class="field">
          <label>执行园区</label>
          <select v-model.number="taskForm.parkId" class="select">
            <option v-for="park in parks" :key="park.id" :value="park.id">{{ park.parkName }}</option>
          </select>
        </div>
        <div class="field">
          <label>巡检区域</label>
          <select v-model="taskForm.zoneId" class="select">
            <option :value="null">— 不指定区域</option>
            <option v-for="zone in zonesOfPark" :key="zone.id" :value="zone.id">
              {{ zone.zoneName }}
            </option>
          </select>
          <div v-if="!zonesOfPark.length" class="rt-hint" style="margin: 4px 0 0">
            该园区还未划分巡检区域，可直接下发
          </div>
        </div>
        <div class="field">
          <label>执行设备</label>
          <select v-model.number="taskForm.deviceId" class="select">
            <option v-for="device in devices" :key="device.id" :value="device.id">
              {{ device.deviceId }} · {{ device.model }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>巡检路线</label>
          <select v-model="taskForm.routeId" class="select">
            <option :value="null">— 临时路径（不引用路线模板）</option>
            <option v-for="route in routesOfForm" :key="route.id" :value="route.id">
              {{ route.routeCode }} · {{ route.routeName }} · {{ route.status }}
            </option>
          </select>
          <div v-if="routesEmpty" class="rt-hint" style="margin: 4px 0 0">
            还没有任何巡检路线：任务会按临时路径下发；
            <button
              style="border: 0; background: transparent; color: var(--primary); cursor: pointer; padding: 0"
              @click="taskDialog = false; goRoutes()"
            >
              先去编排路线
            </button>
          </div>
          <div v-else-if="!routesOfForm.length" class="rt-hint" style="margin: 4px 0 0">
            该园区还没有巡检路线（共 {{ routes.length }} 条在其它园区）；任务可以走临时路径，
            或
            <button
              style="border: 0; background: transparent; color: var(--primary); cursor: pointer; padding: 0"
              @click="taskDialog = false; goRoutes()"
            >
              去编排
            </button>
          </div>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>计划时间段 <em v-if="isPeriodic">*</em></label>
          <div class="time-range" :class="{ disabled: !isPeriodic }">
            <el-time-select
              v-model="taskForm.planStart"
              :disabled="!isPeriodic"
              start="00:00"
              step="00:05"
              end="23:55"
              placeholder="开始"
            />
            <span class="time-sep">-</span>
            <el-time-select
              v-model="taskForm.planEnd"
              :disabled="!isPeriodic"
              start="00:05"
              step="00:05"
              end="23:59"
              placeholder="结束"
            />
          </div>
          <div class="rt-hint" style="margin: 4px 0 0">
            <template v-if="!isPeriodic">「{{ taskForm.executeMode }}」创建后立即下发，不需要计划时间</template>
            <template v-else-if="taskForm.planEnd && taskForm.planStart && taskForm.planEnd < taskForm.planStart">
              跨零点执行：{{ taskForm.planStart }} 开始，次日 {{ taskForm.planEnd }} 结束
            </template>
            <template v-else>只在时间段内到点自动下发，设备就绪后不会提前跑</template>
          </div>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>执行日</label>
          <div class="weekday-row" :class="{ disabled: !isPeriodic }">
            <button
              v-for="day in WEEKDAYS"
              :key="day.value"
              type="button"
              class="weekday-chip"
              :class="{ on: taskForm.scheduleDays.includes(day.value) }"
              :disabled="!isPeriodic"
              @click="
                taskForm.scheduleDays = taskForm.scheduleDays.includes(day.value)
                  ? taskForm.scheduleDays.filter(item => item !== day.value)
                  : [...taskForm.scheduleDays, day.value].sort()
              "
            >
              {{ day.label }}
            </button>
            <span class="rt-hint" style="margin-left: 6px">
              {{ taskForm.scheduleDays.length ? daysLabel(taskForm.scheduleDays.join(",")) : "不选 = 每天" }}
            </span>
          </div>
        </div>
        <div class="field">
          <label>优先级</label>
          <select v-model="taskForm.level" class="select">
            <option v-for="level in TASK_LEVELS" :key="level">{{ level }}</option>
          </select>
        </div>
        <div class="field">
          <label>下发策略</label>
          <select v-model="taskForm.dispatchPolicy" class="select">
            <option v-for="policy in DISPATCH_POLICIES" :key="policy">{{ policy }}</option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>任务说明</label>
          <textarea v-model="taskForm.description" class="textarea" rows="3" placeholder="选填" />
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="taskDialog = false"><AppIcon name="close" />取消</button>
        <button class="btn primary" @click="saveTask">
          <AppIcon name="check" />{{ isEditing ? "保存修改" : "创建任务" }}
        </button>
      </template>
    </el-dialog>

    <el-dialog
      v-model="detailVisible"
      class="task-live-dialog"
      title="任务详情"
      width="min(1080px, 94vw)"
      top="5vh"
      destroy-on-close
    >
      <template v-if="detailTask">
        <div class="live-hint" style="margin-bottom: 10px">
          <i class="live-dot" />详情实时更新（每 {{ REFRESH_MS / 1000 }}s）
          <template v-if="lastRefreshed"> · 上次 {{ lastRefreshed }}</template>
        </div>

        <div class="rt-sec-title">
          <span>实时视频 / 地图位置</span><AppIcon name="video" />
        </div>
        <TaskLiveMonitor
          :task="detailTask"
          :device="deviceOf(detailTask)"
          :route="detailRoute"
          :map="mapOf(detailTask)"
          :refreshed-at="lastRefreshed"
        />

        <div class="rt-sec-title" style="margin-top: 16px">
          <span>任务信息</span><AppIcon name="task" />
        </div>
        <div class="kv"><span>任务名称</span><b>{{ detailTask.taskName }}</b></div>
        <div class="kv">
          <span>任务类型 / 模式</span><b>{{ detailTask.taskType }} / {{ detailTask.executeMode }}</b>
        </div>
        <div class="kv">
          <span>跑完去哪</span><b>{{ endActionLabel(detailTask.endTask, detailRoute?.loopMode) }}</b>
        </div>
        <div class="kv"><span>执行设备</span><b class="mono">{{ detailTask.deviceCode }}</b></div>
        <div class="kv">
          <span>园区 / 区域</span><b>{{ detailTask.parkName }} / {{ detailTask.zoneName }}</b>
        </div>
        <div class="kv">
          <span>计划时间</span><b class="mono">{{ scheduleSummary(detailTask) }}</b>
        </div>
        <div class="kv">
          <span>最近自动下发</span>
          <b class="mono">{{ detailTask.lastDispatchedOn || "—" }}</b>
        </div>
        <div class="kv">
          <span>关联路线</span>
          <b>{{ detailRoute ? `${detailRoute.routeCode} · ${detailRoute.routeName}` : "未关联（临时路径）" }}</b>
        </div>
        <div class="kv"><span>下发策略</span><b>{{ detailTask.dispatchPolicy }}</b></div>
        <div class="kv"><span>执行状态</span><b><StatusBadge :status="detailTask.taskStatus" /></b></div>
        <div class="kv"><span>完成进度</span><b>{{ detailTask.progress }}%</b></div>
        <div class="bar" style="margin-top: 12px">
          <i :style="{ width: `${detailTask.progress}%` }" />
        </div>

        <div class="rt-sec-title" style="margin-top: 18px">
          <span>下发后的航点执行序列</span><AppIcon name="route" />
        </div>
        <div v-if="detailSequence.length" class="act-kv" style="font-size: 11.5px; gap: 7px">
          <div v-for="step in detailSequence" :key="step.name">
            <b>{{ step.name }}</b> · {{ step.actions }}
            <span v-if="step.leg" style="color: #0284c7"> → 行进：{{ step.leg }}</span>
          </div>
        </div>
        <div v-else class="empty-box" style="padding: 20px 0">
          该任务使用临时路径，未绑定路线模板
        </div>

        <div class="rt-sec-title" style="margin-top: 18px">
          <span>任务事件</span><AppIcon name="clock" />
        </div>
        <div v-if="detailEvents.length" class="act-kv" style="font-size: 11.5px; gap: 7px">
          <div v-for="event in detailEvents" :key="event.id">
            <b class="mono muted">{{ event.createdAt.slice(5, 16) }}</b> · {{ event.title }}
            <div v-if="event.description" style="margin-top: 2px; line-height: 1.6">
              {{ event.description }}
            </div>
          </div>
        </div>
        <div v-else class="empty-box" style="padding: 16px 0">暂无事件记录</div>

        <div class="actions" style="margin-top: 16px">
          <button
            v-if="detailTask.taskStatus === '待执行'"
            class="btn sm"
            @click="editFromDetail"
          >
            <AppIcon name="edit" />编辑任务
          </button>
          <button
            v-if="detailRoute"
            class="btn sm"
            @click="openRoutePlanner"
          >
            <AppIcon name="route" />打开路线编排
          </button>
          <button
            v-if="detailTask.taskStatus !== '执行中' && detailTask.taskStatus !== '已暂停'"
            class="btn primary sm"
            @click="sendFromDetail('start')"
          >
            <AppIcon name="play" />开始执行
          </button>
          <button
            v-if="detailTask.taskStatus === '已暂停'"
            class="btn primary sm"
            title="本体已受理的任务走 5006 恢复，否则重新下发"
            @click="sendFromDetail('resume')"
          >
            <AppIcon name="play" />恢复执行
          </button>
          <button
            v-if="detailTask.taskStatus === '执行中'"
            class="btn sm"
            @click="sendFromDetail('pause')"
          >
            <AppIcon name="pause" />暂停任务
          </button>
          <button
            v-if="detailTask.taskStatus === '执行中'"
            class="btn sm ghost"
            title="向设备重新下发一次任务（卡在执行中或需要重跑时使用）"
            @click="sendFromDetail('start')"
          >
            <AppIcon name="play" />重新下发
          </button>
        </div>
      </template>
    </el-dialog>
    <el-dialog
      v-model="monitorVisible"
      class="task-live-dialog"
      title="任务实时监控"
      width="min(1180px, 96vw)"
      top="4vh"
      destroy-on-close
    >
      <template v-if="monitorTask">
        <div class="live-hint" style="margin-bottom: 10px">
          <i class="live-dot" />{{ monitorTask.taskCode }} · {{ monitorTask.taskName }}
          <span class="muted">
            {{ isRunning(monitorTask) ? "执行中" : monitorTask.taskStatus }} · 每
            {{ REFRESH_MS / 1000 }}s 刷新
          </span>
          <template v-if="lastRefreshed">
            <span class="muted"> · 上次 {{ lastRefreshed }}</span>
          </template>
        </div>
        <TaskLiveMonitor
          :task="monitorTask"
          :device="deviceOf(monitorTask)"
          :route="routeOf(monitorTask)"
          :map="mapOf(monitorTask)"
          :refreshed-at="lastRefreshed"
        />
      </template>
      <div v-else class="empty-box" style="padding: 24px 0">任务已不存在</div>
    </el-dialog>
  </div>
</template>

<style scoped>
/* 详情 / 监控内容较长（视频 + 地图 + 事件流）：弹窗主体自己滚动，
   标题栏固定，避免整页跟着滚、关闭按钮被顶出屏幕 */
.task-live-dialog :deep(.el-dialog__body) {
  max-height: 76vh;
  overflow: auto;
}

/* 计划时间段：两颗时间选择器 + 分隔符。用选择器而不是输入框，
   时间窗是调度依据，手填一个 `9点` 就等于这条计划永远不跑 */
.time-range {
  display: flex;
  align-items: center;
  gap: 6px;
}
.time-range :deep(.el-select) {
  flex: 0 1 220px;
  min-width: 120px;
}
.time-range .time-sep {
  color: var(--text-3, #94a3b8);
}
.time-range.disabled {
  opacity: 0.55;
}

/* 执行日：周一到周日的开关片 */
.weekday-row {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
}
.weekday-row.disabled {
  opacity: 0.55;
}
.weekday-chip {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  border: 1px solid var(--line, #e2e8f0);
  background: var(--bg-1, #fff);
  color: var(--text-2, #475569);
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s ease;
}
.weekday-chip:hover:not(:disabled) {
  border-color: var(--primary, #2563eb);
  color: var(--primary, #2563eb);
}
.weekday-chip.on {
  background: var(--primary, #2563eb);
  border-color: var(--primary, #2563eb);
  color: #fff;
}
.weekday-chip:disabled {
  cursor: not-allowed;
}
</style>
