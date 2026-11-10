<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { parkApi, mapApi, routeApi, zoneApi } from "@/api/ops";
import type {
  MapVersion,
  Park,
  Route,
  RouteData,
  RouteLeg,
  RoutePoint,
  RouteValidation,
  Zone
} from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import RouteBaseMap from "@/components/RouteBaseMap.vue";
import MapChip from "@/components/MapChip.vue";
import MapBaselineDialog from "@/components/MapBaselineDialog.vue";
import MapPolicyDialog from "@/components/MapPolicyDialog.vue";
import {
  ACTION_LIB,
  AVOID_LEVELS,
  DEVICE_MODELS,
  FAIL_MODES,
  LEG_MODES,
  TRIGGERS
} from "@/utils/constants";
import {
  cloneRouteData,
  computeRouteMetrics,
  formatDuration,
  newLeg,
  newPoint,
  newRouteData,
  parseRouteData
} from "@/utils/format";

interface RouteDraft {
  id: number;
  routeCode: string;
  routeName: string;
  parkId: number | null;
  zoneId: number | null;
  model: string;
  status: string;
  version: string;
  loopMode: string;
  defaultSpeed: number;
  routeDescription: string;
  data: RouteData;
  usedTaskCount: number;
  mapVersionId: string | null;
  mapStatus: string | null;
}

const vueRoute = useRoute();
const routes = ref<Route[]>([]);
const parks = ref<Park[]>([]);
const zones = ref<Zone[]>([]);
const maps = ref<MapVersion[]>([]);
const draft = ref<RouteDraft | null>(null);
const currentId = ref<number | null>(null);
const routeQuery = ref("");
const mode = ref<"add" | "select">("add");
const snap = ref(true);
const selType = ref<"point" | "leg">("point");
const selIdx = ref(0);
const activeTab = ref<"point" | "leg">("point");
const history = ref<string[]>([]);
const dragIndex = ref<number | null>(null);
const justDragged = ref(false);
const canvasRef = ref<SVGSVGElement>();
const simActive = ref(false);
const simIndex = ref(0);
let simTimer = 0;

const routeDialog = ref(false);
const baseDialog = ref(false);
const baselineDialog = ref(false);
const baselineMapId = ref<string | null>(null);
const policyDialog = ref(false);
const policyMode = ref<"policy" | "lineage">("policy");
const actionDialog = ref(false);
const validateVisible = ref(false);
const validation = ref<RouteValidation | null>(null);
const actionKind = ref<"point" | "leg">("point");

const routeForm = reactive({
  routeName: "",
  parkId: 1,
  zoneId: 1,
  model: "X30-Pro",
  loopMode: "闭环",
  defaultSpeed: 1.2,
  status: "草稿",
  routeDescription: "",
  mapVersionId: ""
});

const actionForm = reactive({
  type: "可见光拍照",
  param: "",
  trigger: "到达即执行",
  threshold: "—",
  retry: 1
});

const filteredRoutes = computed(() => {
  const keyword = routeQuery.value.trim().toLowerCase();
  return routes.value.filter(route => {
    const haystack = `${route.routeName}${route.routeCode}${route.parkName}${route.zoneName}`;
    return !keyword || haystack.toLowerCase().includes(keyword);
  });
});

const zonesOfForm = computed(() =>
  zones.value.filter(zone => zone.parkId === routeForm.parkId)
);

const mapsOfForm = computed(() =>
  maps.value.filter(
    item => item.parkId === routeForm.parkId && item.status !== "已归档"
  )
);

const draftMap = computed(() =>
  maps.value.find(item => item.id === draft.value?.mapVersionId) ?? null
);

function mapsOfPark(parkId: number | null) {
  if (!parkId) return [];
  return maps.value.filter(
    item => item.parkId === parkId && item.status !== "已归档"
  );
}

function openBaseline(mapId: string | null | undefined) {
  baselineMapId.value = mapId ?? null;
  baselineDialog.value = true;
}

function openPolicy(mode: "policy" | "lineage") {
  policyMode.value = mode;
  policyDialog.value = true;
}

async function onBaselineUpdated() {
  maps.value = await mapApi.list();
  await load();
}

const metrics = computed(() =>
  draft.value
    ? computeRouteMetrics(draft.value.data, draft.value.defaultSpeed)
    : null
);

const selectedPoint = computed<RoutePoint | null>(() => {
  if (!draft.value || selType.value !== "point") return null;
  return draft.value.data.points[selIdx.value] ?? null;
});

const selectedLeg = computed<RouteLeg | null>(() => {
  if (!draft.value || selType.value !== "leg") return null;
  if (selIdx.value < draft.value.data.legs.length) {
    return draft.value.data.legs[selIdx.value];
  }
  return draft.value.data.returnLeg;
});

const isReturnLeg = computed(
  () => Boolean(draft.value) && selIdx.value >= (draft.value?.data.legs.length ?? 0)
);

const pointActions = computed(() => selectedPoint.value?.actions ?? []);
const legActions = computed(() => selectedLeg.value?.actions ?? []);

const simDog = computed(() => {
  const points = draft.value?.data.points ?? [];
  return points[Math.min(simIndex.value, points.length - 1)] ?? null;
});

const draftParkName = computed(
  () => parks.value.find(park => park.id === draft.value?.parkId)?.parkName ?? "—"
);

const draftZoneName = computed(
  () => zones.value.find(zone => zone.id === draft.value?.zoneId)?.zoneName ?? "—"
);

function actionMeta(type: string) {
  return (
    ACTION_LIB[type] || {
      icon: "cpu",
      tone: "gray",
      param: "—",
      kind: "point" as const
    }
  );
}

function actionOptions(kind: "point" | "leg") {
  return Object.entries(ACTION_LIB)
    .filter(([, meta]) => meta.kind === kind)
    .map(([name]) => name);
}

async function load(selectZoneId?: number) {
  const [routeList, parkList, zoneList, mapList] = await Promise.all([
    routeApi.list(),
    parkApi.list(),
    zoneApi.list(),
    mapApi.list()
  ]);
  routes.value = routeList;
  parks.value = parkList;
  zones.value = zoneList;
  maps.value = mapList;

  const target =
    (currentId.value ? routeList.find(item => item.id === currentId.value) : null) ??
    (selectZoneId ? routeList.find(item => item.zoneId === selectZoneId) : null) ??
    routeList[0];
  if (target) selectRoute(target, false);
}

function selectRoute(route: Route, notify = true) {
  stopSimulation();
  currentId.value = route.id;
  draft.value = {
    id: route.id,
    routeCode: route.routeCode,
    routeName: route.routeName,
    parkId: route.parkId,
    zoneId: route.zoneId,
    model: route.model ?? "X30-Pro",
    status: route.status,
    version: route.version,
    loopMode: route.loopMode,
    defaultSpeed: route.defaultSpeed,
    routeDescription: route.routeDescription ?? "",
    data: parseRouteData(route.routeData),
    usedTaskCount: route.usedTaskCount,
    mapVersionId: route.mapVersionId,
    mapStatus: route.mapStatus
  };
  selType.value = "point";
  selIdx.value = 0;
  activeTab.value = "point";
  history.value = [];
  if (notify) ElMessage.info(`已切换至「${route.routeName}」`);
}

function snapshot() {
  if (!draft.value) return;
  history.value.push(JSON.stringify(draft.value.data));
  if (history.value.length > 20) history.value.shift();
}

function undo() {
  if (!draft.value) return;
  const raw = history.value.pop();
  if (!raw) {
    ElMessage.info("没有可撤销的操作");
    return;
  }
  draft.value.data = JSON.parse(raw) as RouteData;
  selIdx.value = Math.min(selIdx.value, Math.max(0, draft.value.data.points.length - 1));
  ElMessage.success("已撤销上一步操作");
}

function svgCoords(event: MouseEvent | PointerEvent) {
  const svg = canvasRef.value;
  if (!svg) return { x: 0, y: 0 };
  const rect = svg.getBoundingClientRect();
  return {
    x: Math.max(24, Math.min(776, ((event.clientX - rect.left) / rect.width) * 800)),
    y: Math.max(22, Math.min(438, ((event.clientY - rect.top) / rect.height) * 460))
  };
}

function snapPoint(point: { x: number; y: number }) {
  let { x, y } = point;
  if (snap.value) {
    if (Math.abs(y - 210) < 30) y = 210;
    if (Math.abs(x - 430) < 26) x = 430;
  }
  return { x: Math.round(x / 5) * 5, y: Math.round(y / 5) * 5 };
}

function onCanvasClick(event: MouseEvent) {
  if (mode.value !== "add" || !draft.value) return;
  snapshot();
  const position = snapPoint(svgCoords(event));
  const newIndex = draft.value.data.points.length + 1;
  draft.value.data.points.push(newPoint(newIndex, position.x, position.y));
  if (draft.value.data.points.length > 1) {
    draft.value.data.legs.push(newLeg(draft.value.defaultSpeed));
  }
  selType.value = "point";
  activeTab.value = "point";
  selIdx.value = draft.value.data.points.length - 1;
  ElMessage.success(`已添加航点 P${selIdx.value + 1}，并自动生成行进航段`);
}

function onPointDown(event: PointerEvent, index: number) {
  if (!draft.value) return;
  event.stopPropagation();
  event.preventDefault();
  snapshot();
  dragIndex.value = index;
  justDragged.value = false;
  (event.currentTarget as Element).setPointerCapture?.(event.pointerId);
}

function onCanvasMove(event: PointerEvent) {
  if (dragIndex.value === null || !draft.value) return;
  const position = snapPoint(svgCoords(event));
  const point = draft.value.data.points[dragIndex.value];
  if (!point) return;
  point.x = position.x;
  point.y = position.y;
  justDragged.value = true;
}

function onCanvasUp() {
  if (dragIndex.value === null) return;
  const index = dragIndex.value;
  dragIndex.value = null;
  if (!justDragged.value) {
    history.value.pop();
    selectPoint(index);
  } else {
    justDragged.value = false;
  }
}

function selectPoint(index: number) {
  selType.value = "point";
  selIdx.value = index;
  activeTab.value = "point";
}

function selectLeg(index: number) {
  selType.value = "leg";
  selIdx.value = index;
  activeTab.value = "leg";
}

function setTab(tab: "point" | "leg") {
  activeTab.value = tab;
  selType.value = tab;
  selIdx.value = 0;
}

function deletePoint() {
  if (!draft.value) return;
  if (selType.value !== "point") {
    ElMessage.warning("请先点选一个航点");
    return;
  }
  if (draft.value.data.points.length <= 2) {
    ElMessage.error("至少需保留 2 个航点");
    return;
  }
  snapshot();
  const index = selIdx.value;
  draft.value.data.points.splice(index, 1);
  if (draft.value.data.legs.length) {
    const legIndex = Math.max(0, Math.min(index, draft.value.data.legs.length - 1));
    draft.value.data.legs.splice(legIndex, 1);
  }
  const maxLegs = Math.max(0, draft.value.data.points.length - 1);
  if (draft.value.data.legs.length > maxLegs) {
    draft.value.data.legs.length = maxLegs;
  }
  selIdx.value = Math.max(0, index - 1);
  selType.value = "point";
  activeTab.value = "point";
  ElMessage.success("航点及关联航段已删除");
}

function legDistance(index: number) {
  const data = draft.value?.data;
  if (!data) return 0;
  const a = data.points[index];
  const b = data.points[index + 1] ?? (data.loop === "闭环" ? data.points[0] : null);
  if (!a || !b) return 0;
  return Math.round(Math.hypot(b.x - a.x, b.y - a.y) * 1.2);
}

function timelineSteps() {
  const data = draft.value?.data;
  if (!data) return [];
  return data.points.map((point, index) => ({
    point,
    index,
    legActions: data.legs[index]?.actions.length ?? 0,
    isLast: index === data.points.length - 1
  }));
}

async function persist(silent = true) {
  if (!draft.value) return null;
  if (draft.value.data.loop === "闭环" && !draft.value.data.returnLeg) {
    draft.value.data.returnLeg = {
      mode: "沿道路",
      speed: draft.value.defaultSpeed,
      avoid: "标准",
      actions: [{ type: "静默通行", param: "返航不采集" }]
    };
  }
  if (draft.value.data.loop !== "闭环") {
    draft.value.data.returnLeg = null;
  }
  const updated = await routeApi.update(draft.value.id, {
    routeName: draft.value.routeName,
    parkId: draft.value.parkId,
    zoneId: draft.value.zoneId,
    model: draft.value.model,
    status: draft.value.status,
    version: draft.value.version,
    loopMode: draft.value.data.loop,
    defaultSpeed: draft.value.defaultSpeed,
    routeDescription: draft.value.routeDescription,
    routeData: cloneRouteData(draft.value.data),
    mapVersionId: draft.value.mapVersionId
  });
  draft.value.status = updated.status;
  draft.value.version = updated.version;
  draft.value.usedTaskCount = updated.usedTaskCount;
  const index = routes.value.findIndex(item => item.id === updated.id);
  if (index >= 0) routes.value[index] = updated;
  if (!silent) ElMessage.success("路线已保存");
  return updated;
}

async function saveRoute() {
  await persist(false);
}

async function optimizeRoute() {
  if (!draft.value) return;
  if (draft.value.data.points.length < 3) {
    ElMessage.info("航点不足 3 个，无需优化");
    return;
  }
  await persist(true);
  const updated = await routeApi.optimize(draft.value.id);
  draft.value.data = parseRouteData(updated.routeData);
  draft.value.status = updated.status;
  draft.value.version = updated.version;
  const index = routes.value.findIndex(item => item.id === updated.id);
  if (index >= 0) routes.value[index] = updated;
  ElMessage.success("已按最近邻算法重排航点，缩短总里程");
}

async function validateRoute() {
  if (!draft.value) return;
  await persist(true);
  validation.value = await routeApi.validate(draft.value.id);
  validateVisible.value = true;
}

async function publishRoute() {
  if (!draft.value) return;
  await persist(true);
  if (draft.value.data.points.length < 2) {
    ElMessage.error("航点不足，无法发布");
    return;
  }
  const updated = await routeApi.publish(draft.value.id);
  draft.value.status = updated.status;
  draft.value.version = updated.version;
  const index = routes.value.findIndex(item => item.id === updated.id);
  if (index >= 0) routes.value[index] = updated;
  ElMessage.success(`${updated.routeCode} 已发布为 ${updated.version}，可被任务引用`);
}

function openNewRoute() {
  Object.assign(routeForm, {
    routeName: "",
    parkId: parks.value[0]?.id ?? 1,
    zoneId: zones.value[0]?.id ?? 1,
    model: "X30-Pro",
    loopMode: "闭环",
    defaultSpeed: 1.2,
    status: "草稿",
    routeDescription: "",
    mapVersionId: mapsOfPark(parks.value[0]?.id ?? 1)[0]?.id ?? ""
  });
  routeDialog.value = true;
}

async function createRoute() {
  if (!routeForm.routeName.trim()) {
    ElMessage.warning("请填写路线名称");
    return;
  }
  if (!routeForm.mapVersionId) {
    ElMessage.warning("请选择地图基线版本；未绑定基线的园区不能编排路线");
    return;
  }
  const created = await routeApi.create({
    ...routeForm,
    routeData: newRouteData(routeForm.loopMode)
  });
  routeDialog.value = false;
  currentId.value = created.id;
  await load();
  mode.value = "add";
  ElMessage.success(`路线 ${created.routeCode} 已创建，请点击画布添加航点`);
}

function openBaseDialog() {
  if (!draft.value) return;
  Object.assign(routeForm, {
    routeName: draft.value.routeName,
    parkId: draft.value.parkId ?? parks.value[0]?.id ?? 1,
    zoneId: draft.value.zoneId ?? zones.value[0]?.id ?? 1,
    model: draft.value.model,
    loopMode: draft.value.data.loop,
    defaultSpeed: draft.value.defaultSpeed,
    status: draft.value.status,
    routeDescription: draft.value.routeDescription,
    mapVersionId: draft.value.mapVersionId ?? ""
  });
  baseDialog.value = true;
}

async function saveBaseInfo() {
  if (!draft.value) return;
  snapshot();
  draft.value.routeName = routeForm.routeName || draft.value.routeName;
  draft.value.parkId = routeForm.parkId;
  draft.value.zoneId = routeForm.zoneId;
  draft.value.model = routeForm.model;
  draft.value.defaultSpeed = Number(routeForm.defaultSpeed) || 1.2;
  draft.value.status = routeForm.status;
  draft.value.routeDescription = routeForm.routeDescription;
  draft.value.data.loop = routeForm.loopMode;
  draft.value.loopMode = routeForm.loopMode;
  if (routeForm.loopMode === "闭环" && !draft.value.data.returnLeg) {
    draft.value.data.returnLeg = {
      mode: "沿道路",
      speed: draft.value.defaultSpeed,
      avoid: "标准",
      actions: [{ type: "静默通行", param: "返航不采集" }]
    };
  }
  if (routeForm.loopMode !== "闭环") {
    draft.value.data.returnLeg = null;
  }
  baseDialog.value = false;
  await persist(false);
}

function openActionDialog(kind: "point" | "leg") {
  if (kind === "point" && !selectedPoint.value) {
    ElMessage.warning("请先点选一个航点");
    return;
  }
  if (kind === "leg" && !selectedLeg.value) {
    ElMessage.warning("请先点选一段航段");
    return;
  }
  actionKind.value = kind;
  const type = actionOptions(kind)[0];
  const meta = actionMeta(type);
  Object.assign(actionForm, {
    type,
    param: meta.param,
    trigger: meta.trigger ?? "到达即执行",
    threshold: meta.threshold ?? "—",
    retry: 1
  });
  actionDialog.value = true;
}

function syncActionDefaults() {
  const meta = actionMeta(actionForm.type);
  actionForm.param = meta.param;
  actionForm.trigger = meta.trigger ?? "到达即执行";
  actionForm.threshold = meta.threshold ?? "—";
}

function saveAction() {
  const target =
    actionKind.value === "point" ? selectedPoint.value?.actions : selectedLeg.value?.actions;
  if (!target) return;
  snapshot();
  target.push({
    type: actionForm.type,
    param: actionForm.param,
    ...(actionKind.value === "point"
      ? {
          trigger: actionForm.trigger,
          threshold: actionForm.threshold,
          retry: Number(actionForm.retry) || 1
        }
      : {})
  });
  actionDialog.value = false;
  ElMessage.success(`已添加动作：${actionForm.type}`);
}

function removeAction(kind: "point" | "leg", index: number) {
  const target = kind === "point" ? selectedPoint.value?.actions : selectedLeg.value?.actions;
  if (!target) return;
  snapshot();
  target.splice(index, 1);
  ElMessage.success("动作已删除");
}

function startSimulation() {
  if (simActive.value) {
    stopSimulation();
    ElMessage.info("已停止模拟");
    return;
  }
  const points = draft.value?.data.points ?? [];
  if (points.length < 2) {
    ElMessage.error("至少需要 2 个航点才能模拟");
    return;
  }
  simActive.value = true;
  simIndex.value = 0;
  simTimer = window.setInterval(() => {
    const total = draft.value?.data.points.length ?? 0;
    if (simIndex.value >= total - 1) {
      stopSimulation();
      ElMessage.success(`${draft.value?.routeCode} 模拟执行完成，共 ${total} 个航点`);
      return;
    }
    simIndex.value += 1;
  }, 900);
}

function stopSimulation() {
  window.clearInterval(simTimer);
  simActive.value = false;
  simIndex.value = 0;
}

async function planFromZone(zoneId: number) {
  const target = routes.value.find(item => item.zoneId === zoneId);
  if (!target) {
    ElMessage.info("该区域暂未关联巡检路线");
    return;
  }
  selectRoute(target);
}

watch(
  () => vueRoute.query.zoneId,
  value => {
    const zoneId = Number(value);
    if (zoneId) planFromZone(zoneId);
  }
);

watch(
  () => vueRoute.query.routeId,
  value => {
    const routeId = Number(value);
    if (!routeId) return;
    currentId.value = routeId;
    load();
  }
);

watch(
  () => routeForm.parkId,
  () => {
    if (!zonesOfForm.value.some(zone => zone.id === routeForm.zoneId)) {
      routeForm.zoneId = zonesOfForm.value[0]?.id ?? 1;
    }
    const options = mapsOfForm.value;
    if (!options.some(map => map.id === routeForm.mapVersionId)) {
      routeForm.mapVersionId = options[0]?.id ?? "";
    }
  }
);

onMounted(() => {
  const routeId = Number(vueRoute.query.routeId);
  if (routeId) currentId.value = routeId;
  const zoneId = Number(vueRoute.query.zoneId);
  load(zoneId || undefined);
});

onBeforeUnmount(() => window.clearInterval(simTimer));
</script>

<template>
  <div v-if="draft">
    <PageHeader title="巡检路线规划" subtitle="可视化编排航点、行进航段与每一点/段执行的采集动作">
      <template #actions>
        <button class="btn" :disabled="!draft.mapVersionId" @click="openBaseline(draft.mapVersionId)">
          <AppIcon name="layers" />地图基线
        </button>
        <button class="btn" @click="validateRoute"><AppIcon name="shield" />校验路线</button>
        <button class="btn" @click="publishRoute"><AppIcon name="send" />发布路线</button>
        <button class="btn primary" @click="openNewRoute"><AppIcon name="plus" />新建路线</button>
      </template>
    </PageHeader>

    <div class="route-layout">
      <div>
        <div class="rp">
          <div class="rp-head">
            <b><AppIcon name="route" />路线模板</b>
            <span class="tag blue">{{ routes.length }} 条</span>
          </div>
          <div class="rp-body">
            <div class="rt-search">
              <AppIcon name="search" />
              <input v-model="routeQuery" class="input" placeholder="搜索路线 / 园区" />
            </div>
            <div
              v-for="route in filteredRoutes"
              :key="route.id"
              class="rt-item"
              :class="{ on: route.id === currentId }"
              @click="selectRoute(route)"
            >
              <b>{{ route.routeName }}</b>
              <div class="rt-sub">{{ route.parkName }} · {{ route.zoneName }}</div>
              <div style="display: flex; align-items: center; gap: 6px; margin-top: 8px">
                <StatusBadge :status="route.status" />
                <span class="tag">{{ route.version }}</span>
              </div>
              <div class="rt-meta">
                <span><AppIcon name="waypoint" />{{ route.pointCount }} 点</span>
                <span><AppIcon name="route" />{{ (route.distanceM / 1000).toFixed(2) }} km</span>
                <span><AppIcon name="clock" />{{ Math.ceil(route.durationS / 60) }} min</span>
              </div>
              <div class="rt-sub" style="margin-top: 7px">
                {{ route.usedTaskCount ? `被 ${route.usedTaskCount} 个任务引用` : "暂无任务引用" }}
                · {{ route.updatedAt.slice(5, 16) }} 更新
              </div>
            </div>
            <div v-if="!filteredRoutes.length" class="empty-box" style="padding: 24px 0">
              未找到路线
            </div>
            <button class="rt-new" @click="openNewRoute">
              <AppIcon name="plus" />新建路线模板
            </button>
          </div>
        </div>

        <div class="rp" style="margin-top: 14px">
          <div class="rp-head">
            <b><AppIcon name="settings" />路线基础信息</b>
            <button class="btn sm ghost icon" title="编辑" @click="openBaseDialog">
              <AppIcon name="edit" />
            </button>
          </div>
          <div class="rp-body" style="padding: 6px 14px 12px">
            <div class="kv"><span>路线编号</span><b class="mono">{{ draft.routeCode }}</b></div>
            <div class="kv"><span>所属园区</span><b>{{ draftParkName }}</b></div>
            <div class="kv">
              <span>地图基线（冻结引用）</span>
              <b style="display: flex; align-items: center; gap: 8px">
                <MapChip
                  :map-id="draft.mapVersionId"
                  :status="draftMap?.status"
                  :fallback="draft.mapStatus"
                />
                <button
                  v-if="draft.mapVersionId"
                  class="btn sm ghost icon"
                  title="查看基线详情"
                  @click="openBaseline(draft.mapVersionId)"
                >
                  <AppIcon name="layers" />
                </button>
              </b>
            </div>
            <div class="kv"><span>巡检区域</span><b>{{ draftZoneName }}</b></div>
            <div class="kv"><span>适用机型</span><b>{{ draft.model }}</b></div>
            <div class="kv"><span>闭环方式</span><b>{{ draft.data.loop }}</b></div>
            <div class="kv"><span>默认速度</span><b>{{ draft.defaultSpeed }} m/s</b></div>
            <div class="kv"><span>版本 / 状态</span><b>{{ draft.version }} · {{ draft.status }}</b></div>
            <div class="rt-hint" style="margin-top: 10px">{{ draft.routeDescription }}</div>
          </div>
        </div>
      </div>

      <div>
        <div class="rp">
          <div class="rt-toolbar">
            <button class="rt-tool" :class="{ on: mode === 'add' }" @click="mode = 'add'">
              <AppIcon name="plus" />添加航点
            </button>
            <button class="rt-tool" :class="{ on: mode === 'select' }" @click="mode = 'select'">
              <AppIcon name="eye" />选择
            </button>
            <button class="rt-tool" :class="{ on: snap }" title="吸附到园区道路" @click="snap = !snap">
              <AppIcon name="magnet" />道路吸附
            </button>
            <div class="rt-tool-sep" />
            <button class="rt-tool" @click="undo"><AppIcon name="undo" />撤销</button>
            <button class="rt-tool" @click="optimizeRoute"><AppIcon name="shuffle" />顺序优化</button>
            <button class="rt-tool danger" @click="deletePoint"><AppIcon name="trash" />删除航点</button>
            <button class="rt-tool" @click="saveRoute"><AppIcon name="check" />保存</button>
            <div class="sep" />
            <button class="rt-tool" :class="{ on: simActive }" @click="startSimulation">
              <AppIcon :name="simActive ? 'pause' : 'play'" />{{ simActive ? "停止模拟" : "模拟执行" }}
            </button>
          </div>

          <div class="rt-canvas">
            <svg
              ref="canvasRef"
              viewBox="0 0 800 460"
              :class="{ 'select-mode': mode === 'select' }"
              role="img"
              aria-label="巡检路线编辑画布"
              @click="onCanvasClick"
              @pointermove="onCanvasMove"
              @pointerup="onCanvasUp"
              @pointercancel="onCanvasUp"
            >
              <RouteBaseMap />
              <g id="routeLayer">
                <template v-for="(leg, index) in draft.data.legs" :key="`leg-${index}`">
                  <template v-if="draft.data.points[index] && draft.data.points[index + 1]">
                    <line
                      :x1="draft.data.points[index].x"
                      :y1="draft.data.points[index].y"
                      :x2="draft.data.points[index + 1].x"
                      :y2="draft.data.points[index + 1].y"
                      :stroke="selType === 'leg' && selIdx === index ? '#f59e0b' : '#0284c7'"
                      :stroke-width="selType === 'leg' && selIdx === index ? 5 : 3"
                      stroke-dasharray="9 7"
                      marker-end="url(#rtarrow)"
                      :opacity="selType === 'leg' && selIdx === index ? 1 : 0.85"
                    />
                    <line
                      class="leg-hit"
                      :x1="draft.data.points[index].x"
                      :y1="draft.data.points[index].y"
                      :x2="draft.data.points[index + 1].x"
                      :y2="draft.data.points[index + 1].y"
                      @click.stop="selectLeg(index)"
                    />
                    <g
                      style="cursor: pointer"
                      @click.stop="selectLeg(index)"
                    >
                      <rect
                        :x="(draft.data.points[index].x + draft.data.points[index + 1].x) / 2 - 20"
                        :y="(draft.data.points[index].y + draft.data.points[index + 1].y) / 2 - 25"
                        width="40"
                        height="16"
                        rx="5"
                        :fill="selType === 'leg' && selIdx === index ? '#f59e0b' : '#fff'"
                        :stroke="selType === 'leg' && selIdx === index ? '#f59e0b' : '#cfe3f2'"
                      />
                      <text
                        :x="(draft.data.points[index].x + draft.data.points[index + 1].x) / 2"
                        :y="(draft.data.points[index].y + draft.data.points[index + 1].y) / 2 - 14"
                        font-size="9.5"
                        :fill="selType === 'leg' && selIdx === index ? '#fff' : '#0369a1'"
                        text-anchor="middle"
                        font-weight="700"
                      >
                        {{ leg.actions.length }} 项
                      </text>
                    </g>
                  </template>
                </template>

                <template
                  v-if="
                    draft.data.loop === '闭环' &&
                    draft.data.returnLeg &&
                    draft.data.points.length > 2
                  "
                >
                  <line
                    :x1="draft.data.points[draft.data.points.length - 1].x"
                    :y1="draft.data.points[draft.data.points.length - 1].y"
                    :x2="draft.data.points[0].x"
                    :y2="draft.data.points[0].y"
                    :stroke="selType === 'leg' && isReturnLeg ? '#f59e0b' : '#94a3b8'"
                    stroke-width="2.4"
                    stroke-dasharray="4 7"
                    opacity=".85"
                  />
                  <line
                    class="leg-hit"
                    :x1="draft.data.points[draft.data.points.length - 1].x"
                    :y1="draft.data.points[draft.data.points.length - 1].y"
                    :x2="draft.data.points[0].x"
                    :y2="draft.data.points[0].y"
                    @click.stop="selectLeg(draft.data.legs.length)"
                  />
                  <g style="cursor: pointer" @click.stop="selectLeg(draft.data.legs.length)">
                    <rect
                      :x="(draft.data.points[draft.data.points.length - 1].x + draft.data.points[0].x) / 2 - 18"
                      :y="(draft.data.points[draft.data.points.length - 1].y + draft.data.points[0].y) / 2 - 10"
                      width="36"
                      height="16"
                      rx="5"
                      fill="#fff"
                      stroke="#cbd5e1"
                    />
                    <text
                      :x="(draft.data.points[draft.data.points.length - 1].x + draft.data.points[0].x) / 2"
                      :y="(draft.data.points[draft.data.points.length - 1].y + draft.data.points[0].y) / 2 + 1"
                      font-size="9.5"
                      fill="#64748b"
                      text-anchor="middle"
                      font-weight="700"
                    >
                      返航
                    </text>
                  </g>
                </template>

                <g
                  v-for="(point, index) in draft.data.points"
                  :key="`point-${index}`"
                  class="wp-hit"
                  style="cursor: pointer"
                  @pointerdown="onPointDown($event, index)"
                  @click.stop="selectPoint(index)"
                >
                  <circle
                    v-if="selType === 'point' && selIdx === index"
                    :cx="point.x"
                    :cy="point.y"
                    r="21"
                    fill="none"
                    stroke="#f59e0b"
                    stroke-width="2"
                    stroke-dasharray="4 3"
                  />
                  <circle
                    :cx="point.x"
                    :cy="point.y"
                    r="14"
                    fill="#fff"
                    :stroke="
                      index === 0 ? '#0d9488' : index === draft.data.points.length - 1 ? '#0f172a' : '#0284c7'
                    "
                    stroke-width="2.5"
                  />
                  <circle
                    :cx="point.x"
                    :cy="point.y"
                    r="7"
                    :fill="
                      index === 0 ? '#0d9488' : index === draft.data.points.length - 1 ? '#0f172a' : '#0284c7'
                    "
                  />
                  <text
                    :x="point.x"
                    :y="point.y + 3.5"
                    font-size="9"
                    fill="#fff"
                    text-anchor="middle"
                    font-weight="700"
                  >
                    {{ index + 1 }}
                  </text>
                  <g :transform="`translate(${point.x - 46},${index % 2 ? point.y - 38 : point.y + 18})`">
                    <rect
                      width="92"
                      height="18"
                      rx="5"
                      fill="#fff"
                      :stroke="selType === 'point' && selIdx === index ? '#f59e0b' : '#e2e8f0'"
                    />
                    <text
                      x="46"
                      y="12.5"
                      font-size="9.5"
                      fill="#334155"
                      text-anchor="middle"
                      font-weight="600"
                    >
                      {{ point.name.length > 9 ? `${point.name.slice(0, 9)}…` : point.name }}
                    </text>
                  </g>
                </g>

                <text
                  v-if="!draft.data.points.length"
                  x="400"
                  y="230"
                  font-size="14"
                  fill="#94a3b8"
                  text-anchor="middle"
                >
                  点击画布添加第一个航点
                </text>

                <g v-if="simActive && simDog" :transform="`translate(${simDog.x},${simDog.y})`" class="sim-dog">
                  <circle r="15" fill="#06b6d4" opacity=".22" />
                  <circle r="9" fill="#fff" stroke="#0284c7" stroke-width="2.5" />
                  <circle r="4" fill="#0284c7" />
                </g>
              </g>
            </svg>
            <div class="rt-canvas-tag">
              <AppIcon name="info" />
              {{ mode === "add" ? "点击画布新增航点 · 拖拽航点调整位置" : "点选航点或航段编辑属性" }}
              · 点击段标签配置行进任务
            </div>
            <div class="rt-legend">
              <span><i style="background: #0d9488" />起点</span>
              <span><i style="background: #0284c7" />途经航点</span>
              <span><i style="background: #0f172a" />终点</span>
              <span><i style="background: #f59e0b" />当前选中</span>
            </div>
          </div>

          <div v-if="metrics" class="rt-metrics">
            <div class="rt-metric"><span>航点数量</span><b>{{ metrics.pointCount }} <small>个</small></b></div>
            <div class="rt-metric"><span>预计里程</span><b>{{ (metrics.distanceM / 1000).toFixed(2) }} <small>km</small></b></div>
            <div class="rt-metric"><span>预计耗时</span><b>{{ Math.ceil(metrics.durationS / 60) }} <small>min</small></b></div>
            <div class="rt-metric"><span>动作总数</span><b>{{ metrics.actionCount }} <small>项</small></b></div>
            <div class="rt-metric"><span>预估耗电</span><b>{{ metrics.batteryPct }} <small>%</small></b></div>
          </div>
        </div>

        <div class="rp" style="margin-top: 14px">
          <div class="rp-head">
            <b><AppIcon name="activity" />执行时序</b>
            <span class="card-sub">到达航点 → 执行点动作 → 行进段持续任务 → 下一航点</span>
          </div>
          <div class="rt-timeline">
            <div
              v-for="step in timelineSteps()"
              :key="step.index"
              class="rt-step"
              :class="{
                active:
                  (simActive && simIndex === step.index) ||
                  (!simActive && selType === 'point' && selIdx === step.index)
              }"
              style="cursor: pointer"
              @click="selectPoint(step.index)"
            >
              <div class="dot">{{ step.index + 1 }}</div>
              <b>{{ step.point.name }}</b>
              <span>{{ step.point.actions.length }} 项点动作 · 停留 {{ step.point.dwell }}s</span>
              <span v-if="!step.isLast" style="color: #0284c7">
                ↓ 段 {{ step.index + 1 }}：{{ step.legActions }} 项行进任务
              </span>
              <span v-else-if="draft.data.loop === '闭环'" style="color: #64748b">↓ 闭环返航</span>
            </div>
            <div v-if="!draft.data.points.length" class="empty-box" style="padding: 18px 0">
              暂无航点，请在画布上添加
            </div>
          </div>
        </div>
      </div>

      <div>
        <div class="rp rt-prop-panel">
          <div class="rp-head">
            <b><AppIcon name="settings" />属性配置</b>
            <span class="tag blue">{{ draft.routeCode }}</span>
          </div>
          <div class="rt-tabs">
            <button class="rt-tab" :class="{ on: activeTab === 'point' }" @click="setTab('point')">
              <AppIcon name="waypoint" />航点属性
            </button>
            <button class="rt-tab" :class="{ on: activeTab === 'leg' }" @click="setTab('leg')">
              <AppIcon name="route" />航段任务
            </button>
          </div>

          <div class="rt-prop">
            <template v-if="activeTab === 'point'">
              <template v-if="selectedPoint">
                <div class="rt-sec-title">
                  <span>航点 P{{ selIdx + 1 }} · 导航参数</span><AppIcon name="waypoint" />
                </div>
                <div class="rt-field">
                  <label>航点名称</label>
                  <input v-model="selectedPoint.name" class="input" />
                </div>
                <div class="rt-field">
                  <label>坐标 (x,y)</label>
                  <input
                    class="input mono"
                    :value="`${Math.round(selectedPoint.x)}, ${Math.round(selectedPoint.y)}`"
                    readonly
                  />
                </div>
                <div class="rt-field">
                  <label>到达半径</label>
                  <input v-model.number="selectedPoint.radius" class="input" type="number" step="0.1" min="0.1" />
                  <span class="muted" style="font-size: 11px">m</span>
                </div>
                <div class="rt-field">
                  <label>停留时长</label>
                  <input v-model.number="selectedPoint.dwell" class="input" type="number" min="0" />
                  <span class="muted" style="font-size: 11px">s</span>
                </div>
                <div class="rt-field">
                  <label>云台朝向</label>
                  <input v-model.number="selectedPoint.heading" class="input" type="number" min="0" max="360" />
                  <span class="muted" style="font-size: 11px">°</span>
                </div>
                <div class="rt-field">
                  <label>必经点</label>
                  <label class="chk"><input v-model="selectedPoint.must" type="checkbox" /> 不可跳过</label>
                </div>
                <div class="rt-field">
                  <label>异常策略</label>
                  <select v-model="selectedPoint.onFail" class="select">
                    <option v-for="mode in FAIL_MODES" :key="mode">{{ mode }}</option>
                  </select>
                </div>

                <div class="rt-sec-title">
                  <span>该点执行动作（{{ pointActions.length }}）</span><AppIcon name="task" />
                </div>
                <div v-for="(action, index) in pointActions" :key="`pa-${index}`" class="act-card">
                  <div class="act-top">
                    <span class="act-ico" :class="actionMeta(action.type).tone">
                      <AppIcon :name="actionMeta(action.type).icon" />
                    </span>
                    <b>{{ action.type }}</b>
                    <button class="btn sm ghost icon" title="删除" @click="removeAction('point', index)">
                      <AppIcon name="trash" />
                    </button>
                  </div>
                  <div class="act-kv">
                    <div>参数：<b>{{ action.param || "—" }}</b></div>
                    <div>
                      触发：<b>{{ action.trigger || "到达即执行" }}</b> · 阈值：<b>{{ action.threshold || "—" }}</b>
                    </div>
                    <div>失败：<b>重试 {{ action.retry || 1 }} 次</b></div>
                  </div>
                </div>
                <div v-if="!pointActions.length" class="empty-box" style="padding: 16px 0">
                  尚未配置动作
                </div>
                <button class="act-add" @click="openActionDialog('point')">
                  <AppIcon name="plus" />添加航点动作
                </button>
                <div class="rt-hint">
                  点动作在到达该航点后按序执行；阈值超限时自动创建告警并关联本航点编号，便于回溯定位。
                </div>
              </template>
              <div v-else class="empty-box">请在画布中点选一个航点</div>
            </template>

            <template v-else>
              <template v-if="selectedLeg">
                <div class="rt-sec-title">
                  <span>{{ isReturnLeg ? "闭环返航段" : `航段 S${selIdx + 1} · P${selIdx + 1} → P${selIdx + 2}` }}</span>
                  <AppIcon name="route" />
                </div>
                <div class="rt-field">
                  <label>段长</label>
                  <input class="input mono" :value="`${isReturnLeg ? legDistance(draft.data.points.length - 1) : legDistance(selIdx)} m`" readonly />
                </div>
                <div class="rt-field">
                  <label>过渡方式</label>
                  <select v-model="selectedLeg.mode" class="select">
                    <option v-for="mode in LEG_MODES" :key="mode">{{ mode }}</option>
                  </select>
                </div>
                <div class="rt-field">
                  <label>段内速度</label>
                  <input v-model.number="selectedLeg.speed" class="input" type="number" step="0.1" min="0.2" />
                  <span class="muted" style="font-size: 11px">m/s</span>
                </div>
                <div class="rt-field">
                  <label>避障等级</label>
                  <select v-model="selectedLeg.avoid" class="select">
                    <option v-for="level in AVOID_LEVELS" :key="level">{{ level }}</option>
                  </select>
                </div>

                <div class="rt-sec-title">
                  <span>行进中持续任务（{{ legActions.length }}）</span><AppIcon name="activity" />
                </div>
                <div v-for="(action, index) in legActions" :key="`la-${index}`" class="act-card">
                  <div class="act-top">
                    <span class="act-ico" :class="actionMeta(action.type).tone">
                      <AppIcon :name="actionMeta(action.type).icon" />
                    </span>
                    <b>{{ action.type }}</b>
                    <button class="btn sm ghost icon" title="删除" @click="removeAction('leg', index)">
                      <AppIcon name="trash" />
                    </button>
                  </div>
                  <div class="act-kv">
                    <div>参数：<b>{{ action.param || "—" }}</b></div>
                    <div>类型：<b>行进过程中持续执行</b></div>
                  </div>
                </div>
                <div v-if="!legActions.length" class="empty-box" style="padding: 16px 0">
                  该航段静默通行
                </div>
                <button class="act-add" @click="openActionDialog('leg')">
                  <AppIcon name="plus" />添加行进任务
                </button>
                <div class="rt-hint">
                  段任务在两点之间移动期间持续执行（如全程录像、连续测温、周界扫描），与航点瞬时动作互补。
                </div>
              </template>
              <div v-else class="empty-box">请在画布中点选一段航段</div>
            </template>
          </div>
        </div>
      </div>
    </div>

    <el-dialog v-model="routeDialog" title="新建巡检路线" width="600px">
      <div class="form-grid">
        <div class="field" style="grid-column: 1 / -1">
          <label>路线名称 <em>*</em></label>
          <input v-model="routeForm.routeName" class="input" placeholder="例如：B区立体仓库巡检" />
        </div>
        <div class="field">
          <label>所属园区</label>
          <select v-model.number="routeForm.parkId" class="select">
            <option v-for="park in parks" :key="park.id" :value="park.id">{{ park.parkName }}</option>
          </select>
        </div>
        <div class="field">
          <label>巡检区域</label>
          <select v-model.number="routeForm.zoneId" class="select">
            <option v-for="zone in zonesOfForm" :key="zone.id" :value="zone.id">{{ zone.zoneName }}</option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>地图基线版本 <em>*</em> · 创建后冻结引用</label>
          <select v-model="routeForm.mapVersionId" class="select">
            <option value="">该园区无可用基线 · 需先完成建图标定</option>
            <option v-for="map in mapsOfForm" :key="map.id" :value="map.id">
              {{ map.id }} · {{ map.status }} · {{ map.frameId }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>适用机型</label>
          <select v-model="routeForm.model" class="select">
            <option v-for="model in DEVICE_MODELS" :key="model">{{ model }}</option>
          </select>
        </div>
        <div class="field">
          <label>闭环方式</label>
          <select v-model="routeForm.loopMode" class="select">
            <option>闭环</option>
            <option>往返</option>
            <option>单向</option>
          </select>
        </div>
        <div class="field">
          <label>默认速度 (m/s)</label>
          <input v-model.number="routeForm.defaultSpeed" class="input" type="number" step="0.1" min="0.2" />
        </div>
        <div class="field">
          <label>初始状态</label>
          <select v-model="routeForm.status" class="select">
            <option>草稿</option>
            <option>已发布</option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>路线说明</label>
          <textarea v-model="routeForm.routeDescription" class="textarea" rows="3" placeholder="描述覆盖点位与巡检目标" />
        </div>
      </div>
      <div class="rt-hint">
        航点坐标以所选地图版本的 frame_id 与分辨率为基准，创建后引用冻结；后续地图发新版不会自动改写本路线坐标。创建后进入画布点击添加航点。
      </div>
      <template #footer>
        <button class="btn" @click="routeDialog = false">取消</button>
        <button class="btn primary" @click="createRoute"><AppIcon name="check" />创建路线</button>
      </template>
    </el-dialog>

    <el-dialog v-model="baseDialog" title="编辑路线基础信息" width="600px">
      <div class="form-grid">
        <div class="field" style="grid-column: 1 / -1">
          <label>路线名称</label>
          <input v-model="routeForm.routeName" class="input" />
        </div>
        <div class="field">
          <label>所属园区</label>
          <select v-model.number="routeForm.parkId" class="select">
            <option v-for="park in parks" :key="park.id" :value="park.id">{{ park.parkName }}</option>
          </select>
        </div>
        <div class="field">
          <label>巡检区域</label>
          <select v-model.number="routeForm.zoneId" class="select">
            <option v-for="zone in zonesOfForm" :key="zone.id" :value="zone.id">{{ zone.zoneName }}</option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>地图基线（冻结引用）</label>
          <div style="display: flex; align-items: center; gap: 10px; min-height: 34px">
            <MapChip
              :map-id="draft.mapVersionId"
              :status="draftMap?.status"
              :fallback="draft.mapStatus"
            />
            <span class="muted" style="font-size: 11px">
              {{ draftMap?.frameId ?? "创建后引用冻结，不随园区当前基线变化" }}
            </span>
          </div>
        </div>
        <div class="field">
          <label>适用机型</label>
          <select v-model="routeForm.model" class="select">
            <option v-for="model in DEVICE_MODELS" :key="model">{{ model }}</option>
          </select>
        </div>
        <div class="field">
          <label>闭环方式</label>
          <select v-model="routeForm.loopMode" class="select">
            <option>闭环</option>
            <option>往返</option>
            <option>单向</option>
          </select>
        </div>
        <div class="field">
          <label>默认速度 (m/s)</label>
          <input v-model.number="routeForm.defaultSpeed" class="input" type="number" step="0.1" min="0.2" />
        </div>
        <div class="field">
          <label>状态</label>
          <select v-model="routeForm.status" class="select">
            <option>草稿</option>
            <option>已发布</option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>路线说明</label>
          <textarea v-model="routeForm.routeDescription" class="textarea" rows="3" />
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="baseDialog = false">取消</button>
        <button class="btn primary" @click="saveBaseInfo"><AppIcon name="check" />保存</button>
      </template>
    </el-dialog>

    <el-dialog
      v-model="actionDialog"
      :title="actionKind === 'point' ? '添加航点动作' : '添加行进任务'"
      width="560px"
    >
      <div class="form-grid">
        <div class="field" style="grid-column: 1 / -1">
          <label>动作类型 <em>*</em></label>
          <select v-model="actionForm.type" class="select" @change="syncActionDefaults">
            <option v-for="type in actionOptions(actionKind)" :key="type">{{ type }}</option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>执行参数</label>
          <input v-model="actionForm.param" class="input" />
        </div>
        <template v-if="actionKind === 'point'">
          <div class="field">
            <label>触发时机</label>
            <select v-model="actionForm.trigger" class="select">
              <option v-for="trigger in TRIGGERS" :key="trigger">{{ trigger }}</option>
            </select>
          </div>
          <div class="field">
            <label>告警阈值</label>
            <input v-model="actionForm.threshold" class="input" />
          </div>
          <div class="field">
            <label>失败重试次数</label>
            <input v-model.number="actionForm.retry" class="input" type="number" min="0" max="5" />
          </div>
        </template>
        <div v-else class="field" style="grid-column: 1 / -1">
          <div class="rt-hint" style="margin: 0">该动作在两航点之间移动过程中持续执行，不占用停留时长。</div>
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="actionDialog = false">取消</button>
        <button class="btn primary" @click="saveAction"><AppIcon name="check" />添加</button>
      </template>
    </el-dialog>

    <el-dialog v-model="validateVisible" title="路线校验结果" width="680px">
      <template v-if="validation">
        <div class="rt-val-item" style="border: 0; padding-bottom: 12px">
          <AppIcon name="info" />
          <span class="muted">
            {{ validation.routeCode }} · {{ validation.routeName }} ·
            {{ validation.passed }}/{{ validation.total }} 项通过 · 预计里程
            {{ (validation.metrics.distanceM / 1000).toFixed(2) }} km · 预计耗时
            {{ formatDuration(validation.metrics.durationS) }} · 动作
            {{ validation.metrics.actionCount }} 项 · 预估耗电
            {{ validation.metrics.batteryPct }}%
          </span>
        </div>
        <div v-for="item in validation.items" :key="item.title" class="rt-val-item">
          <AppIcon :name="item.ok ? 'check-circle' : 'warn'" />
          <span :style="{ flex: 1, color: item.ok ? '#059669' : '#d97706' }">{{ item.title }}</span>
          <span class="muted" style="text-align: right">{{ item.description }}</span>
        </div>
        <div class="rt-hint">
          校验通过后即可发布为正式版本；已发布的路线模板可被「任务管理」引用，下发时会自动展开为航点序列与动作指令集。
        </div>
      </template>
      <template #footer>
        <button class="btn primary" @click="validateVisible = false">知道了</button>
      </template>
    </el-dialog>

    <MapBaselineDialog
      v-model="baselineDialog"
      :map-id="baselineMapId"
      @updated="onBaselineUpdated"
      @data-source="openPolicy('lineage')"
    />
    <MapPolicyDialog v-model="policyDialog" :mode="policyMode" />
  </div>
</template>
