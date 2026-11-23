<script setup lang="ts">
import { computed, defineAsyncComponent, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { parkApi, mapApi, routeApi, zoneApi } from "@/api/ops";
import type {
  AiCheckType,
  MapRaster,
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
import AiActionPicker from "@/components/backpack/AiActionPicker.vue";

const MapBaselineDialog = defineAsyncComponent(
  () => import("@/components/MapBaselineDialog.vue")
);
const MapPolicyDialog = defineAsyncComponent(
  () => import("@/components/MapPolicyDialog.vue")
);
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
import { canvasToMap, frameOf, mapToCanvas, type MapFrame } from "@/utils/coords";
import { useAiCatalog } from "@/composables/useAiCatalog";

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
const router = useRouter();
const routes = ref<Route[]>([]);
const parks = ref<Park[]>([]);
const zones = ref<Zone[]>([]);
const maps = ref<MapVersion[]>([]);
const draft = ref<RouteDraft | null>(null);
const currentId = ref<number | null>(null);
const showSkeleton = ref(false);
/** 首屏加载中：加载完且没有任何路线时要显式给空状态，不能一直躺在骨架屏里 */
const loading = ref(true);
let skeletonTimer = 0;
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
/** 「保存」旁的时间戳：上一次写库成功的时刻（自动保存成功也刷新），让人知道改动真的落库了 */
const lastSavedAt = ref("");
/** 自动保存的防抖定时器 / 串行队列（见 [`autoSave`]） */
let autoSaveTimer = 0;
let autoSaveChain: Promise<unknown> = Promise.resolve();
/** 程序自己写 draft 时先置上：那种改动不需要再存一趟（见 [`withoutAutoSave`]） */
let autoSaveSuppressed = false;

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
/** 正在修改的动作下标（null = 新建）；对话框就地改参数，写回原位置 */
const actionEditIndex = ref<number | null>(null);

const routeForm = reactive({
  routeName: "",
  parkId: 1,
  zoneId: null as number | null,
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
  retry: 1,
  /** AI 识别：从「实时视频」同一份模型目录里选的模型包 / 类别 / 置信度 */
  scene: "",
  labels: [] as string[],
  aiConf: 0.35,
  /**
   * AI 识别：两类任务。
   * - `exist`  检测是否存在：找到目标就存证/上报，不判数量；
   * - `count`  检测数量是否正确：还要核对数量，不一致（含「一个都没找到」）告警。
   */
  checkType: "exist" as AiCheckType,
  /**
   * AI 识别：检查目标数量（仅 `count` 任务必填）。
   * 与板端 AI 检出的目标数比对，不一致上报数量告警。
   */
  expectCount: null as number | null,
  /**
   * AI 识别：未检出也告警（仅 `exist` 任务）。
   * 适合「必须有」的目标（灭火器、气体钢瓶）：到期没找到就告警；
   * `exist` 默认只在**检出**时告警，适合「不该出现」的目标（消防通道占用、明火）。
   */
  alertWhenMissing: false,
  /**
   * AI 识别：数量**一致**时也通知（仅 `count` 任务）。
   * 只发企业微信群回执，不落告警、不建工单；默认关（数量任务只报不一致）。
   */
  notifyWhenCountOk: false,
  /**
   * 到点开补光灯（背包板 GPIO，走板端 Agent `/api/lights`）。
   * 离开航点 / 任务结束时自动关；计划勾了「仅夜间自动开灯」时只在夜间开。
   */
  lightFill: false,
  /** 到点开 LED 灯带（板端拉起厂商 `test_rgbrun`，循环闪烁） */
  lightLed: false
});

/** 背包 AI 模型目录（场景 + 类别）：仅在配「AI 识别」动作时按需加载 */
const aiCatalog = useAiCatalog();
const isAiAction = computed(
  () => actionForm.type === "AI 识别" && actionKind.value === "point"
);
/** 目录里选中了场景时，执行参数由场景 + 类别自动生成，不用手填 */
const aiParamAuto = computed(
  () => isAiAction.value && !!aiCatalog.taskOf(actionForm.scene)
);

/**
 * 已保存动作是不是「AI 识别」：与后端 `is_ai_action` 同规则
 * （类型名含 `AI` 或 `识别`），只用于在动作卡片上展示任务类型。
 */
function isAiActionOf(action: { type?: string } | null | undefined) {
  const kind = String(action?.type ?? "");
  return kind.toUpperCase().includes("AI") || kind.includes("识别");
}

/** 动作卡片上的任务类型文案：老数据没 `checkType` 时按有无 `expectCount` 推断 */
function actionCheckTypeLabel(action: {
  checkType?: string;
  expectCount?: number | null;
  alertWhenMissing?: boolean;
  notifyWhenCountOk?: boolean;
}) {
  const countMode = action.checkType
    ? action.checkType === "count"
    : action.expectCount != null && Number(action.expectCount) > 0;
  if (countMode) {
    return `数量是否正确（检查 ${action.expectCount ?? "—"} 个，不一致告警${
      action.notifyWhenCountOk ? "，一致也通知" : ""
    }）`;
  }
  // 存在性任务里「找不到也告警」是另一种语义，卡片上必须能看出来，否则现场会以为配错
  return action.alertWhenMissing ? "是否存在（找不到也告警）" : "是否存在";
}

/** 动作卡片上的灯光文案：没配灯返回空串（不占一行） */
function actionLightLabel(action: { lightFill?: boolean; lightLed?: boolean }) {
  const lights = [
    action.lightFill ? "补光灯" : "",
    action.lightLed ? "LED" : ""
  ].filter(Boolean);
  return lights.length ? `到点开${lights.join(" + ")}` : "";
}

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

const raster = ref<MapRaster | null>(null);
const rasterVisible = ref(false);
const rasterCache = new Map<string, MapRaster>();

/**
 * 当前草稿的坐标几何（地图版本 → 米/像素 + 原点）。
 *
 * 航点在 `draft.data.points` 里存的是**地图米**，画布上看到的是换算后的坐标：
 * 两者只在这里转换，模板/拖拽一律不碰原始米制数值。
 */
const mapFrame = computed<MapFrame>(() => frameOf(draftMap.value));

/** 画布坐标（显示用）：由米制航点反算出来 */
const canvasPoints = computed(() =>
  (draft.value?.data.points ?? []).map(point => {
    const { x, y } = mapToCanvas(mapFrame.value, point.x, point.y);
    return { cx: x, cy: y };
  })
);

/** 编辑器里的坐标是否可下发（示意坐标 / 未迁移的历史数据都不行） */
const coordReady = computed(
  () => !mapFrame.value.schematic && draft.value?.data.coordFrame === "map"
);

/**
 * 画布能否铺真实栅格底图。
 *
 * 只要地图版本有地图包（`filePath`）就试一下：`source === "backend"` 是早期只有
 * 「中台地图导入」才有包时的判断，机器人建图回传（`source = robot-mapping`）
 * 同样带包，也被服务端 `GET /api/map-versions/{id}/raster` 支持。
 * 只按来源判断会让真机地图退回「示意图底图」，画出来跟本体上的图对不上。
 */
const hasRaster = computed(() => Boolean(draftMap.value?.filePath));

const rasterRect = computed(() => {
  const item = raster.value;
  if (!item || !item.width || !item.height) return null;
  const scale = Math.min(800 / item.width, 460 / item.height);
  const width = item.width * scale;
  const height = item.height * scale;
  return {
    x: (800 - width) / 2,
    y: (460 - height) / 2,
    width,
    height
  };
});

async function loadRaster(mapId: string | null) {
  raster.value = null;
  if (!mapId) return;
  const cached = rasterCache.get(mapId);
  if (cached) {
    raster.value = cached;
    return;
  }
  const item = maps.value.find(map => map.id === mapId);
  if (!item || !item.filePath) return;
  try {
    const data = await mapApi.raster(mapId);
    rasterCache.set(mapId, data);
    raster.value = data;
  } catch {
    raster.value = null;
  }
}

function mapsOfPark(parkId: number | null) {
  if (!parkId) return [];
  return maps.value.filter(
    item => item.parkId === parkId && item.status !== "已归档"
  );
}

/** 有可用地图基线（且至少有一张未归档地图）的园区，空状态下用来给出可行引导 */
const parksWithBaseline = computed(() =>
  parks.value.filter(park => park.mapVersionId && mapsOfPark(park.id).length > 0)
);

/** 新建路线时优先选一个「真的有地图基线」的园区，否则下拉里全是空的 */
function preferredPark() {
  return parksWithBaseline.value[0] ?? parks.value[0] ?? null;
}

function goParks() {
  router.push("/parks");
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
  const point = points[Math.min(simIndex.value, points.length - 1)];
  if (!point) return null;
  const { x, y } = mapToCanvas(mapFrame.value, point.x, point.y);
  return { x, y };
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
  loading.value = true;
  try {
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
  } finally {
    loading.value = false;
    showSkeleton.value = false;
  }
}

function selectRoute(route: Route, notify = true) {
  // 在 400 ms 防抖窗口里就切了路线：先把上一条改完的动作写回它自己（persist 里的
  // 数据快照在第一个 await 之前取，随后才换 draft，不会写到新路线头上）
  if (autoSaveTimer) {
    window.clearTimeout(autoSaveTimer);
    autoSaveTimer = 0;
    void persist(true).catch(error => console.warn("切换路线前的补存失败", error));
  }
  stopSimulation();
  currentId.value = route.id;
  // 换草稿是「加载」不是「用户编辑」，不能触发自动保存
  withoutAutoSave(() => {
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
  });
  selType.value = "point";
  selIdx.value = 0;
  activeTab.value = "point";
  history.value = [];
  rasterVisible.value = Boolean(
    maps.value.find(item => item.id === route.mapVersionId)?.filePath
  );
  void loadRaster(route.mapVersionId);
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
  // 撤销也要落库：动作改动刚被自动保存过，不回写就会出现「页面已撤销、平台还是新参数」
  autoSave("撤销");
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
  // 画布像素 → 地图米：落库的始终是米制坐标
  const target = canvasToMap(mapFrame.value, position.x, position.y);
  const newIndex = draft.value.data.points.length + 1;
  draft.value.data.points.push(newPoint(newIndex, round3(target.x), round3(target.y)));
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
  // 拖拽也走「画布像素 → 地图米」，避免把像素写回米制字段
  const target = canvasToMap(mapFrame.value, position.x, position.y);
  point.x = round3(target.x);
  point.y = round3(target.y);
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

function round3(value: number) {
  return Math.round(value * 1000) / 1000;
}

function legDistance(index: number) {
  const data = draft.value?.data;
  if (!data) return 0;
  const a = data.points[index];
  const b = data.points[index + 1] ?? (data.loop === "闭环" ? data.points[0] : null);
  if (!a || !b) return 0;
  // 航点已是米制：直接求平面距离
  return Math.round(Math.hypot(b.x - a.x, b.y - a.y));
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
  // 下面两段是服务端要求的规范化（闭环必须有返航段）：属于「补数据」不是用户改动，
  // 不能让它再触发一次自动保存
  withoutAutoSave(() => {
    if (!draft.value) return;
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
  });
  if (!draft.value) return null;
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
  // 回写只回给「请求发出去时的那条草稿」：发出后用户可能已经切到别的路线了，
  // 无脑写 draft 会把 A 的版本号/状态写到 B 头上
  if (draft.value?.id === updated.id) {
    draft.value.status = updated.status;
    draft.value.version = updated.version;
    draft.value.usedTaskCount = updated.usedTaskCount;
  }
  const index = routes.value.findIndex(item => item.id === updated.id);
  if (index >= 0) routes.value[index] = updated;
  stampSaved();
  if (!silent) ElMessage.success("路线已保存");
  return updated;
}

/** 记下「刚刚写过库」的时刻（保存按钮旁的小字） */
function stampSaved() {
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  lastSavedAt.value = `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
}

/**
 * 动作改完就自己存盘，不用再记得点「保存」。
 *
 * 只给「新增 / 修改 / 删除航点动作」用：动作是任务的实质内容（AI 识别配的场景、
 * 类别、检查数量都写在这里），配完忘了点「保存」就等于白配 —— 任务下发时用的
 * 还是老口径，站到航点上才发现识别跑的是上一版的参数。
 *
 * 400 ms 合并 + 串行：连点几下「添加」「删除」只会发一次 PUT，也不会出现
 * 后发的请求先到、把新改动覆盖回旧数据的静默丢改。失败不重试也不自己弹 toast
 * （`src/api/http.ts` 已经弹过原因了），「已保存 HH:mm:ss」小字不会跟着说谎。
 */
function autoSave(reason: string) {
  window.clearTimeout(autoSaveTimer);
  autoSaveTimer = window.setTimeout(() => {
    autoSaveTimer = 0;
    autoSaveChain = autoSaveChain.then(async () => {
      if (!draft.value) return;
      try {
        await persist(true);
      } catch (error) {
        console.warn(`自动保存失败（${reason}）`, error);
      }
    });
  }, 400);
}

/** 程序自己改 draft（加载路线 / 顺序优化 / 规范化补字段）时，别把这次改动当成用户编辑去存 */
function withoutAutoSave(run: () => void) {
  const previous = autoSaveSuppressed;
  autoSaveSuppressed = true;
  try {
    run();
  } finally {
    autoSaveSuppressed = previous;
  }
}

/**
 * 属性面板（航点 / 航段属性）是直接 `v-model` 到 `draft.data` 上的，没有统一入口，
 * 所以靠**深度监听草稿数据**兜住：名字、坐标、停留时长、机身偏航、半径、容错、
 * 航段方式/速度/避障……改完就自动落库（含画布拖点、新增/删除航点）。
 *
 * `flush: "sync"` 是必须的：回调要跟着这次改动**同步**执行，[`withoutAutoSave`]
 * 的抑制标记才盖得住；默认的异步 flush 会拖到标记复位之后才跑到。
 */
watch(
  () => draft.value?.data,
  () => {
    if (autoSaveSuppressed) return;
    autoSave("属性修改");
  },
  { deep: true, flush: "sync" }
);

async function saveRoute() {
  // 正等着自动保存的改动就一起在这次里写，不必再补一次 PUT
  window.clearTimeout(autoSaveTimer);
  autoSaveTimer = 0;
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
  // 服务端回来的重排结果是「结果渲染」，不是用户编辑：不再自动存一趟
  withoutAutoSave(() => {
    if (!draft.value) return;
    draft.value.data = parseRouteData(updated.routeData);
  });
  if (!draft.value) return;
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
  const park = preferredPark();
  const parkId = park?.id ?? 1;
  Object.assign(routeForm, {
    routeName: "",
    parkId,
    // 区域必须属于所选园区，否则选中的是别的园区的区域
    zoneId: zones.value.find(zone => zone.parkId === parkId)?.id ?? null,
    model: "X30-Pro",
    loopMode: "闭环",
    defaultSpeed: 1.2,
    status: "草稿",
    routeDescription: "",
    mapVersionId: mapsOfPark(parkId)[0]?.id ?? ""
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
  // data 上的改动（循环方式 + 返航段）紧接着就 persist(false)，不用再等自动保存
  withoutAutoSave(() => {
    if (!draft.value) return;
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
  });
  baseDialog.value = false;
  await persist(false);
}

/** 从阈值文案里还原置信度（`置信度 ≥ 0.35` → 0.35），改 AI 动作时回填滑杆 */
function parseConfidence(threshold?: string | null) {
  const match = /(\d*\.?\d+)/.exec(String(threshold ?? ""));
  const value = match ? Number(match[1]) : NaN;
  return Number.isFinite(value) && value > 0 && value <= 1 ? value : 0.35;
}

/** 打开动作对话框：`index` 有值 = 修改已配置的那条，就地覆盖；否则新建 */
function openActionDialog(kind: "point" | "leg", index: number | null = null) {
  if (kind === "point" && !selectedPoint.value) {
    ElMessage.warning("请先点选一个航点");
    return;
  }
  if (kind === "leg" && !selectedLeg.value) {
    ElMessage.warning("请先点选一段航段");
    return;
  }
  actionKind.value = kind;
  const existing =
    index === null
      ? null
      : (kind === "point" ? selectedPoint.value : selectedLeg.value)?.actions[index] ?? null;
  if (existing) {
    // 修改：把已存的参数整套回填，参数 / 场景 / 类别 / 数量都能在原值上改
    Object.assign(actionForm, {
      type: existing.type,
      param: existing.param ?? "",
      trigger: existing.trigger ?? "到达即执行",
      threshold: existing.threshold ?? "—",
      retry: existing.retry ?? 1,
      scene: existing.scene ?? "",
      labels: [...(existing.labels ?? [])],
      aiConf: parseConfidence(existing.threshold),
      checkType:
        existing.checkType ??
        (existing.expectCount != null && Number(existing.expectCount) > 0 ? "count" : "exist"),
      expectCount: existing.expectCount ?? null,
      alertWhenMissing: existing.alertWhenMissing === true,
      notifyWhenCountOk: existing.notifyWhenCountOk === true,
      lightFill: existing.lightFill === true,
      lightLed: existing.lightLed === true
    });
    actionEditIndex.value = index;
    if (actionForm.type === "AI 识别") void aiCatalog.load();
    actionDialog.value = true;
    return;
  }
  // 新建：按动作类型带默认参数
  const type = actionOptions(kind)[0];
  const meta = actionMeta(type);
  Object.assign(actionForm, {
    type,
    param: meta.param,
    trigger: meta.trigger ?? "到达即执行",
    threshold: meta.threshold ?? "—",
    retry: 1,
    scene: "",
    labels: [],
    aiConf: 0.35,
    checkType: "exist",
    expectCount: null,
    alertWhenMissing: false,
    notifyWhenCountOk: false,
    lightFill: false,
    lightLed: false
  });
  actionEditIndex.value = null;
  actionDialog.value = true;
}

function syncActionDefaults() {
  const meta = actionMeta(actionForm.type);
  actionForm.param = meta.param;
  actionForm.trigger = meta.trigger ?? "到达即执行";
  actionForm.threshold = meta.threshold ?? "—";
  actionForm.scene = "";
  actionForm.labels = [];
  actionForm.aiConf = 0.35;
  actionForm.checkType = "exist";
  actionForm.expectCount = null;
  actionForm.alertWhenMissing = false;
  actionForm.notifyWhenCountOk = false;
  actionForm.lightFill = false;
  actionForm.lightLed = false;
  if (actionForm.type === "AI 识别") void aiCatalog.load();
}

/** 场景 + 类别 → 可读的执行参数（后端仍按 `param` 展示 / 关键词兜底） */
function composeAiParam() {
  const task = aiCatalog.taskOf(actionForm.scene);
  if (!task) return actionForm.param;
  const picked = aiCatalog
    .labelsOf(actionForm.scene)
    .filter(label => actionForm.labels.includes(label.name))
    .map(label => label.zh);
  return `${task.name} · ${picked.length ? picked.join(" / ") : "全部类别"}`;
}

// 选了模型包 / 改了类别或阈值：执行参数自动跟着变（目录不可用时保留手填的关键词）
watch(
  [() => actionForm.scene, () => actionForm.labels, () => actionForm.aiConf],
  () => {
    if (!isAiAction.value || !aiCatalog.taskOf(actionForm.scene)) return;
    actionForm.param = composeAiParam();
  },
  { deep: true }
);

// 打开对话框默认是「可见光拍照」：用户切到 AI 识别时才去读模型目录，不拖慢首屏
watch(
  () => actionForm.type,
  type => {
    if (type === "AI 识别" && actionKind.value === "point") void aiCatalog.load();
  }
);

function saveAction() {
  const target =
    actionKind.value === "point" ? selectedPoint.value?.actions : selectedLeg.value?.actions;
  if (!target) return;
  // 数量任务必须填检查数量，否则板端只能按存在性判定（等于白配）
  const countTask = isAiAction.value && actionForm.checkType === "count";
  const expectCount = Math.max(1, Math.min(999, Number(actionForm.expectCount) || 0));
  if (countTask && !expectCount) {
    ElMessage.error("「检测数量是否正确」必须填写检查目标数量");
    return;
  }
  snapshot();
  // AI 识别：场景 / 类别来自实时视频模型目录，阈值走滑杆（后端按 `threshold` 解析置信度）
  const ai = isAiAction.value;
  const action = {
    type: actionForm.type,
    param: ai && aiParamAuto.value ? composeAiParam() : actionForm.param,
    ...(actionKind.value === "point"
      ? {
          trigger: actionForm.trigger,
          threshold: ai
            ? `置信度 ≥ ${(Number(actionForm.aiConf) || 0.35).toFixed(2)}`
            : actionForm.threshold,
          retry: Number(actionForm.retry) || 1
        }
      : {}),
    ...(ai && actionForm.scene ? { scene: actionForm.scene } : {}),
    ...(ai && actionForm.labels.length ? { labels: [...actionForm.labels] } : {}),
    // 任务类型：存在性任务不写检查数量（后端也按 checkType 判，双保险）
    ...(ai ? { checkType: actionForm.checkType } : {}),
    ...(countTask ? { expectCount } : {}),
    // 数量任务：核对一致也发群消息（只推企业微信，不落告警、不建工单）
    ...(countTask && actionForm.notifyWhenCountOk ? { notifyWhenCountOk: true } : {}),
    // 「未检出也告警」只对存在性任务有意义（数量任务自带「0 ≠ N」告警）
    ...(ai && !countTask && actionForm.alertWhenMissing ? { alertWhenMissing: true } : {}),
    // 灯光只挂在航点动作上（行进任务没有「到点」，也就没有开灯时机）
    ...(actionKind.value === "point" && actionForm.lightFill ? { lightFill: true } : {}),
    ...(actionKind.value === "point" && actionForm.lightLed ? { lightLed: true } : {})
  };
  // 修改：整条替换回原位置（顺序不变，撤销栈里是一条快照）
  const editIndex = actionEditIndex.value;
  if (editIndex !== null && target[editIndex]) {
    target[editIndex] = action as (typeof target)[number];
  } else {
    target.push(action as (typeof target)[number]);
  }
  actionDialog.value = false;
  actionEditIndex.value = null;
  ElMessage.success(`${editIndex !== null ? "已更新" : "已添加"}动作：${actionForm.type}`);
  // 动作是任务的实质内容：配完直接落库，不要求操作员再记得点「保存」
  autoSave(`${editIndex !== null ? "修改" : "新增"}动作 ${actionForm.type}`);
}

function removeAction(kind: "point" | "leg", index: number) {
  const target = kind === "point" ? selectedPoint.value?.actions : selectedLeg.value?.actions;
  if (!target) return;
  snapshot();
  const [removed] = target.splice(index, 1);
  ElMessage.success("动作已删除");
  autoSave(`删除动作 ${removed?.type ?? ""}`.trim());
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
      routeForm.zoneId = zonesOfForm.value[0]?.id ?? null;
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
  skeletonTimer = window.setTimeout(() => {
    if (!draft.value) showSkeleton.value = true;
  }, 120);
});

onBeforeUnmount(() => {
  window.clearInterval(simTimer);
  window.clearTimeout(skeletonTimer);
  // 还在 400ms 防抖窗口里就离开页面：立刻补一次写库，别把最后一次动作改动丢掉
  if (autoSaveTimer) {
    window.clearTimeout(autoSaveTimer);
    autoSaveTimer = 0;
    void persist(true).catch(error => console.warn("离开页面时补存失败", error));
  }
});
</script>

<template>
  <div>
    <PageHeader title="巡检任务规划" subtitle="可视化编排航点、行进航段与每一点/段执行的采集动作">
      <template #actions>
        <button
          class="btn"
          :disabled="!draft?.mapVersionId"
          @click="draft && openBaseline(draft.mapVersionId)"
        >
          <AppIcon name="layers" />地图基线
        </button>
        <button class="btn" :disabled="!draft" @click="validateRoute"><AppIcon name="shield" />校验路线</button>
        <button class="btn" :disabled="!draft" @click="publishRoute"><AppIcon name="send" />发布路线</button>
        <button class="btn primary" @click="openNewRoute"><AppIcon name="plus" />新建路线</button>
      </template>
    </PageHeader>

    <div v-if="!draft && loading" class="route-skeleton" :class="{ visible: showSkeleton }">
      <div class="route-skeleton-bar" />
      <div class="route-skeleton-grid">
        <div class="route-skeleton-card" style="height: 520px" />
        <div class="route-skeleton-card" style="height: 640px" />
        <div class="route-skeleton-card" style="height: 520px" />
      </div>
    </div>

    <!-- 加载完但一条路线都没有：不能只留空白/无限骨架屏，要给空状态 + 下一步 -->
    <div v-else-if="!draft" class="card route-empty">
      <div class="route-empty-icon"><AppIcon name="route" /></div>
      <b>还没有巡检路线</b>
      <p>
        巡检路线把航点、航段与每点/段执行的采集动作编排成可下发的任务模板；<br />
        任务下发时必须引用一条已发布路线（或走“临时路径”）。
      </p>
      <p v-if="!parksWithBaseline.length" class="rt-hint" style="margin: 0 0 4px">
        当前没有可用地图基线的园区，无法编排路线：请先到「园区管理」新增园区并绑定地图
        （设备建图回传 / 中台同步后即可在园区表单里选到）。
      </p>
      <p v-else class="rt-hint" style="margin: 0 0 4px">
        可编排园区：
        <span v-for="park in parksWithBaseline" :key="park.id" class="tag" style="margin-left: 4px">
          {{ park.parkName }}
        </span>
      </p>
      <div class="actions" style="justify-content: center; margin-top: 10px">
        <button class="btn primary" @click="openNewRoute">
          <AppIcon name="plus" />新建路线
        </button>
        <button class="btn" @click="goParks">
          <AppIcon name="park" />去园区管理
        </button>
      </div>
    </div>

    <template v-if="draft">

    <div class="route-summary">
      <div class="rs-main">
        <div class="rs-title">
          <b>{{ draft.routeName }}</b>
          <span class="mono muted">{{ draft.routeCode }}</span>
          <StatusBadge :status="draft.status" />
          <span class="tag">{{ draft.version }}</span>
        </div>
        <div class="rs-meta">
          <span><AppIcon name="park" />{{ draftParkName }} · {{ draftZoneName }}</span>
          <span><AppIcon name="cpu" />{{ draft.model }}</span>
          <span><AppIcon name="route" />{{ draft.data.loop }}</span>
          <span><AppIcon name="clock" />{{ draft.defaultSpeed }} m/s</span>
          <span class="rs-map">
            <MapChip
              :map-id="draft.mapVersionId"
              :status="draftMap?.status"
              :fallback="draft.mapStatus"
            />
          </span>
          <span v-if="draftMap" class="mono muted" style="font-size: 11px">
            {{ draftMap.mapName }}{{ draftMap.sourceId ? ` · 中台 #${draftMap.sourceId}` : "" }}
          </span>
        </div>
      </div>
      <div class="rs-side">
        <div v-if="metrics" class="rs-metrics">
          <span><b>{{ metrics.pointCount }}</b> 航点</span>
          <span><b>{{ (metrics.distanceM / 1000).toFixed(2) }}</b> km</span>
          <span><b>{{ Math.ceil(metrics.durationS / 60) }}</b> min</span>
        </div>
        <button class="btn sm" @click="openBaseDialog">
          <AppIcon name="edit" />编辑基础信息
        </button>
      </div>
    </div>

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
            <button
              v-if="hasRaster"
              class="rt-tool"
              :class="{ on: rasterVisible }"
              :disabled="!raster"
              title="显示 ROS model.pgm 栅格底图"
              @click="rasterVisible = !rasterVisible"
            >
              <AppIcon name="layers" />真实地图
            </button>
            <div class="rt-tool-sep" />
            <button class="rt-tool" @click="undo"><AppIcon name="undo" />撤销</button>
            <button class="rt-tool" @click="optimizeRoute"><AppIcon name="shuffle" />顺序优化</button>
            <button class="rt-tool danger" @click="deletePoint"><AppIcon name="trash" />删除航点</button>
            <button
              class="rt-tool save"
              title="保存任务规划（动作 / 属性的改动都会自动保存）"
              @click="saveRoute"
            >
              <AppIcon name="check" />保存
            </button>
            <span v-if="lastSavedAt" class="rt-save-hint">已保存 {{ lastSavedAt }}</span>
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
              <template v-if="rasterVisible && raster && rasterRect">
                <rect width="800" height="460" fill="#0b1220" />
                <image
                  :href="raster.image"
                  :x="rasterRect.x"
                  :y="rasterRect.y"
                  :width="rasterRect.width"
                  :height="rasterRect.height"
                  preserveAspectRatio="none"
                />
              </template>
              <g id="routeLayer">
                <template v-for="(leg, index) in draft.data.legs" :key="`leg-${index}`">
                  <template v-if="draft.data.points[index] && draft.data.points[index + 1]">
                    <line
                      :x1="canvasPoints[index].cx"
                      :y1="canvasPoints[index].cy"
                      :x2="canvasPoints[index + 1].cx"
                      :y2="canvasPoints[index + 1].cy"
                      :stroke="selType === 'leg' && selIdx === index ? '#f59e0b' : '#0284c7'"
                      :stroke-width="selType === 'leg' && selIdx === index ? 5 : 3"
                      stroke-dasharray="9 7"
                      marker-end="url(#rtarrow)"
                      :opacity="selType === 'leg' && selIdx === index ? 1 : 0.85"
                    />
                    <line
                      class="leg-hit"
                      :x1="canvasPoints[index].cx"
                      :y1="canvasPoints[index].cy"
                      :x2="canvasPoints[index + 1].cx"
                      :y2="canvasPoints[index + 1].cy"
                      @click.stop="selectLeg(index)"
                    />
                    <g
                      style="cursor: pointer"
                      @click.stop="selectLeg(index)"
                    >
                      <rect
                        :x="(canvasPoints[index].cx + canvasPoints[index + 1].cx) / 2 - 20"
                        :y="(canvasPoints[index].cy + canvasPoints[index + 1].cy) / 2 - 25"
                        width="40"
                        height="16"
                        rx="5"
                        :fill="selType === 'leg' && selIdx === index ? '#f59e0b' : '#fff'"
                        :stroke="selType === 'leg' && selIdx === index ? '#f59e0b' : '#cfe3f2'"
                      />
                      <text
                        :x="(canvasPoints[index].cx + canvasPoints[index + 1].cx) / 2"
                        :y="(canvasPoints[index].cy + canvasPoints[index + 1].cy) / 2 - 14"
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
                    :x1="canvasPoints[draft.data.points.length - 1].cx"
                    :y1="canvasPoints[draft.data.points.length - 1].cy"
                    :x2="canvasPoints[0].cx"
                    :y2="canvasPoints[0].cy"
                    :stroke="selType === 'leg' && isReturnLeg ? '#f59e0b' : '#94a3b8'"
                    stroke-width="2.4"
                    stroke-dasharray="4 7"
                    opacity=".85"
                  />
                  <line
                    class="leg-hit"
                    :x1="canvasPoints[draft.data.points.length - 1].cx"
                    :y1="canvasPoints[draft.data.points.length - 1].cy"
                    :x2="canvasPoints[0].cx"
                    :y2="canvasPoints[0].cy"
                    @click.stop="selectLeg(draft.data.legs.length)"
                  />
                  <g style="cursor: pointer" @click.stop="selectLeg(draft.data.legs.length)">
                    <rect
                      :x="(canvasPoints[draft.data.points.length - 1].cx + canvasPoints[0].cx) / 2 - 18"
                      :y="(canvasPoints[draft.data.points.length - 1].cy + canvasPoints[0].cy) / 2 - 10"
                      width="36"
                      height="16"
                      rx="5"
                      fill="#fff"
                      stroke="#cbd5e1"
                    />
                    <text
                      :x="(canvasPoints[draft.data.points.length - 1].cx + canvasPoints[0].cx) / 2"
                      :y="(canvasPoints[draft.data.points.length - 1].cy + canvasPoints[0].cy) / 2 + 1"
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
                    :cx="canvasPoints[index].cx"
                    :cy="canvasPoints[index].cy"
                    r="21"
                    fill="none"
                    stroke="#f59e0b"
                    stroke-width="2"
                    stroke-dasharray="4 3"
                  />
                  <circle
                    :cx="canvasPoints[index].cx"
                    :cy="canvasPoints[index].cy"
                    r="14"
                    fill="#fff"
                    :stroke="
                      index === 0 ? '#0d9488' : index === draft.data.points.length - 1 ? '#0f172a' : '#0284c7'
                    "
                    stroke-width="2.5"
                  />
                  <circle
                    :cx="canvasPoints[index].cx"
                    :cy="canvasPoints[index].cy"
                    r="7"
                    :fill="
                      index === 0 ? '#0d9488' : index === draft.data.points.length - 1 ? '#0f172a' : '#0284c7'
                    "
                  />
                  <text
                    :x="canvasPoints[index].cx"
                    :y="canvasPoints[index].cy + 3.5"
                    font-size="9"
                    fill="#fff"
                    text-anchor="middle"
                    font-weight="700"
                  >
                    {{ index + 1 }}
                  </text>
                  <g :transform="`translate(${canvasPoints[index].cx - 46},${index % 2 ? canvasPoints[index].cy - 38 : canvasPoints[index].cy + 18})`">
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
              · {{ rasterVisible && raster ? "ROS 栅格底图" : "示意图底图" }}
              · 航点存地图米制坐标
            </div>
            <div v-if="draft && !coordReady" class="rt-canvas-warn">
              <AppIcon name="warn" />
              示意坐标系（该地图没有真实 ROS 地图包）：可编排但不能下发，请先发布真实地图基线
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
              <span
                >{{ step.point.actions.length }} 项点动作 · 停留 {{ step.point.dwell }}s · 机身
                {{ step.point.heading }}°</span
              >
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
                  <label>航点坐标（地图米）</label>
                  <input v-model.number="selectedPoint.x" class="input mono" type="number" step="0.1" />
                  <input v-model.number="selectedPoint.y" class="input mono" type="number" step="0.1" />
                </div>
                <div class="rt-field">
                  <label>坐标系</label>
                  <span class="muted" style="font-size: 11px">
                    {{ mapFrame.schematic ? "示意坐标（无真实地图包，不可下发）" : `地图米 · ${mapFrame.resolution} m/px` }}
                  </span>
                </div>
                <div class="rt-field">
                  <label>到达半径</label>
                  <input v-model.number="selectedPoint.radius" class="input" type="number" step="0.1" min="0.1" />
                  <span class="muted" style="font-size: 11px">m</span>
                </div>
                <div class="rt-field">
                  <label>停留时长</label>
                  <input
                    v-model.number="selectedPoint.dwell"
                    class="input"
                    type="number"
                    min="0"
                    step="2"
                    title="到点停留秒数。本体「等待」动作固定 2 s，平台按 2 s 粒度重复下发（6 s → 3 次等待），单点上限 300 s"
                  />
                  <span class="muted" style="font-size: 11px">s（2 s 粒度）</span>
                </div>
                <div class="rt-field">
                  <label>机身偏航角</label>
                  <input
                    v-model.number="selectedPoint.heading"
                    class="input"
                    type="number"
                    min="0"
                    max="360"
                    title="到点后机身转头对准的偏航角（度），0° = 地图 +X 方向；下发为本体路径点的 orientation 四元数"
                  />
                  <span class="muted" style="font-size: 11px">° 到点转头</span>
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
                    <button
                      class="btn sm ghost icon"
                      title="修改"
                      @click="openActionDialog('point', index)"
                    >
                      <AppIcon name="edit" />
                    </button>
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
                    <div v-if="isAiActionOf(action)">
                      检测类型：<b>{{ actionCheckTypeLabel(action) }}</b>
                    </div>
                    <div v-if="actionLightLabel(action)">
                      灯光：<b>{{ actionLightLabel(action) }}</b>
                    </div>
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
                    <button
                      class="btn sm ghost icon"
                      title="修改"
                      @click="openActionDialog('leg', index)"
                    >
                      <AppIcon name="edit" />
                    </button>
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
    </template>

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
          <select v-model="routeForm.zoneId" class="select">
            <option :value="null">— 不指定区域</option>
            <option v-for="zone in zonesOfForm" :key="zone.id" :value="zone.id">{{ zone.zoneName }}</option>
          </select>
          <div v-if="!zonesOfForm.length" class="rt-hint" style="margin: 4px 0 0">
            该园区还未划分巡检区域，可直接编排路线
          </div>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>地图基线版本 <em>*</em> · 创建后冻结引用</label>
          <select v-model="routeForm.mapVersionId" class="select">
            <option value="">该园区无可用基线 · 需先完成建图标定</option>
            <option v-for="map in mapsOfForm" :key="map.id" :value="map.id">
              {{ map.mapName }} · {{ map.id }} · {{ map.status }}
              {{ map.sourceId ? ` · 中台 #${map.sourceId}` : "" }}
            </option>
          </select>
          <div v-if="!mapsOfForm.length" class="rt-hint" style="margin: 6px 0 0">
            该园区还没有地图基线，创建后无法编排路线：请到「园区管理」给园区绑定地图后再来。
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
        航点坐标以所选地图版本的原点与分辨率为基准（存地图米制，1 位小数，画布只是显示换算）；创建后引用冻结，后续地图发新版不会自动改写本路线坐标。没有真实 ROS 地图包的示意基线只能编排、不能下发任务。创建后进入画布点击添加航点。
      </div>
      <template #footer>
        <button class="btn" @click="routeDialog = false"><AppIcon name="close" />取消</button>
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
          <select v-model="routeForm.zoneId" class="select">
            <option :value="null">— 不指定区域</option>
            <option v-for="zone in zonesOfForm" :key="zone.id" :value="zone.id">{{ zone.zoneName }}</option>
          </select>
          <div v-if="!zonesOfForm.length" class="rt-hint" style="margin: 4px 0 0">
            该园区还未划分巡检区域，可直接编排路线
          </div>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>地图基线（冻结引用）</label>
          <div style="display: flex; align-items: center; gap: 10px; min-height: 34px">
            <MapChip
              :map-id="draft?.mapVersionId ?? null"
              :status="draftMap?.status"
              :fallback="draft?.mapStatus ?? null"
            />
            <span class="muted" style="font-size: 11px">
              创建后引用冻结，不随园区当前基线变化
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
        <button class="btn" @click="baseDialog = false"><AppIcon name="close" />取消</button>
        <button class="btn primary" @click="saveBaseInfo"><AppIcon name="check" />保存</button>
      </template>
    </el-dialog>

    <el-dialog
      v-model="actionDialog"
      :title="
        actionEditIndex !== null
          ? actionKind === 'point'
            ? '修改航点动作'
            : '修改行进任务'
          : actionKind === 'point'
            ? '添加航点动作'
            : '添加行进任务'
      "
      width="560px"
    >
      <div class="form-grid">
        <div class="field" style="grid-column: 1 / -1">
          <label>动作类型 <em>*</em></label>
          <select v-model="actionForm.type" class="select" @change="syncActionDefaults">
            <option v-for="type in actionOptions(actionKind)" :key="type">{{ type }}</option>
          </select>
        </div>

        <!-- AI 识别：场景 + 类别 + 阈值直接复用「实时视频」的 AI 模型目录 -->
        <div v-if="isAiAction" class="field" style="grid-column: 1 / -1">
          <AiActionPicker
            v-model:scene="actionForm.scene"
            v-model:labels="actionForm.labels"
            v-model:conf="actionForm.aiConf"
            :tasks="aiCatalog.tasks.value"
            :loading="aiCatalog.loading.value"
            :error="aiCatalog.error.value"
            :live="aiCatalog.live.value"
            @retry="aiCatalog.load(true)"
          />
        </div>

        <div class="field" style="grid-column: 1 / -1">
          <label>
            执行参数
            <span v-if="aiParamAuto" class="muted">（由所选场景与类别自动生成）</span>
            <span v-else-if="isAiAction" class="muted">（可填模型场景关键词，如「烟火 / 人员」）</span>
          </label>
          <input
            v-model="actionForm.param"
            class="input"
            :readonly="aiParamAuto"
            :placeholder="isAiAction ? '烟火检测 · 火焰 / 烟雾' : ''"
          />
        </div>
        <template v-if="actionKind === 'point'">
          <div class="field">
            <label>触发时机</label>
            <select v-model="actionForm.trigger" class="select">
              <option v-for="trigger in TRIGGERS" :key="trigger">{{ trigger }}</option>
            </select>
          </div>
          <div v-if="!isAiAction" class="field">
            <label>告警阈值</label>
            <input v-model="actionForm.threshold" class="input" />
          </div>
          <div v-else class="field">
            <label>告警阈值</label>
            <input
              class="input mono"
              :value="`置信度 ≥ ${(Number(actionForm.aiConf) || 0.35).toFixed(2)}`"
              readonly
              title="由上方「置信度阈值」滑杆决定"
            />
          </div>
          <!-- AI 识别：两类任务（只判「有没有」/ 判「数量对不对」） -->
          <div v-if="isAiAction" class="field">
            <label>检测类型</label>
            <select v-model="actionForm.checkType" class="select" title="决定到点是只判「能否找到目标」，还是要核对数量">
              <option value="exist">检测是否存在</option>
              <option value="count">检测数量是否正确</option>
            </select>
          </div>
          <!-- 检查目标数量：只有数量任务需要，且必填 -->
          <div v-if="isAiAction && actionForm.checkType === 'count'" class="field">
            <label>检查目标数量 <em>*</em></label>
            <input
              v-model.number="actionForm.expectCount"
              class="input mono"
              type="number"
              min="1"
              max="999"
              placeholder="如 3"
              title="板端把 AI 检出的目标数与它比对，不一致（含一个都没找到）上报数量告警"
            />
            <label
              class="chk"
              title="勾上后：核对一致也发一条企业微信群消息作为巡检回执，但不落告警、不建工单"
            >
              <input v-model="actionForm.notifyWhenCountOk" type="checkbox" />
              数量一致也通知
            </label>
          </div>
          <!-- 存在性任务的另一半语义：“应该有却没有”也要告警 -->
          <div v-if="isAiAction && actionForm.checkType === 'exist'" class="field">
            <label>未检出时</label>
            <label class="chk" title="勾上后：窗口正常到期却一个目标都没找到，也上报告警（含自动派单）">
              <input v-model="actionForm.alertWhenMissing" type="checkbox" />
              未找到目标也告警
            </label>
          </div>
          <!-- 航点灯光：补光灯 / LED 灯带（与 AI 识别无关，不配 AI 也能只开灯） -->
          <div class="field" style="grid-column: 1 / -1">
            <label>
              到点开灯
              <span class="muted">（背包板上的补光灯 / LED 灯带，离开航点与任务结束时自动关）</span>
            </label>
            <div style="display: flex; gap: 18px; flex-wrap: wrap">
              <label class="chk" title="板端拉高补光灯 GPIO；与摄像头触发脚同号或 FILL_LIGHT_GPIO=-1 时板端会拒绝，日志里有原因">
                <input v-model="actionForm.lightFill" type="checkbox" />
                开补光灯
              </label>
              <label class="chk" title="板端拉起厂商 LED 程序（循环闪烁，没有常亮接口）">
                <input v-model="actionForm.lightLed" type="checkbox" />
                开 LED 灯带
              </label>
            </div>
            <div class="rt-hint" style="margin: 4px 0 0">
              计划里勾了<b>「仅夜间自动开灯」</b>时，只有夜间时段（板端默认 18:00-06:00）到达该航点才真的开灯，
              白天到达由板端直接跳过；补光灯与 LED 各自独立配置。
            </div>
          </div>
          <div class="field">
            <label>失败重试次数</label>
            <input v-model.number="actionForm.retry" class="input" type="number" min="0" max="5" />
          </div>
          <div v-if="isAiAction" class="rt-hint" style="margin: 0; grid-column: 1 / -1">
            到点后平台用<b>与实时视频同一套 AI 模型</b>开识别窗口：场景 = 模型包，类别 = 勾选的标签，
            阈值 = 置信度下限；检出目标写入「告警中心」。命中帧与 result.json 会落到板端
            detect_jobs/计划ID/pt航点下标/（计划 ID = 业务任务号）。检测类型二选一：
            <b>检测是否存在</b>（找到就存证/上报，勾上「未找到目标也告警」后反过来——
            适合灭火器、气体钢瓶这类「必须有」的东西）、
            <b>检测数量是否正确</b>（再核对数量，不一致——含一个都没找到——上报数量告警；
            适合「必须有 N 个」的点；勾上「数量一致也通知」后核对一致也会发一条群消息回执，
            但不落告警、不建工单）。
            窗口时长跟着「停留时长」走（停留 + 8s，最短 10s、最长 300s，离开航点立即结束），
            不额外占用停留时长。模型目录读不到时可以手填场景关键词，后端仍有关键词兜底。
          </div>
        </template>
        <div v-else class="field" style="grid-column: 1 / -1">
          <div class="rt-hint" style="margin: 0">该动作在两航点之间移动过程中持续执行，不占用停留时长。</div>
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="actionDialog = false"><AppIcon name="close" />取消</button>
        <button class="btn primary" @click="saveAction">
          <AppIcon name="check" />{{ actionEditIndex !== null ? "保存修改" : "添加" }}
        </button>
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
          校验通过后即可发布为正式版本；已发布的路线模板可被「计划管理」引用，下发时会自动展开为航点序列与动作指令集。
        </div>
      </template>
      <template #footer>
        <button class="btn primary" @click="validateVisible = false">知道了</button>
      </template>
    </el-dialog>

    <MapBaselineDialog
      v-if="baselineDialog"
      v-model="baselineDialog"
      :map-id="baselineMapId"
      @updated="onBaselineUpdated"
      @data-source="openPolicy('lineage')"
    />
    <MapPolicyDialog v-if="policyDialog" v-model="policyDialog" :mode="policyMode" />
  </div>
</template>
