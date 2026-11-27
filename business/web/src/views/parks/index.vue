<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useRouter } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { parkApi, mapApi, zoneApi } from "@/api/ops";
import type { MapVersion, Park, Zone } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import MiniMap from "@/components/MiniMap.vue";
import MapChip from "@/components/MapChip.vue";
import MapBaselineDialog from "@/components/MapBaselineDialog.vue";
import MapPolicyDialog from "@/components/MapPolicyDialog.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import { PARK_TYPES, ZONE_TYPES } from "@/utils/constants";

const router = useRouter();
const parks = ref<Park[]>([]);
const zones = ref<Zone[]>([]);
const maps = ref<MapVersion[]>([]);
const search = ref("");
const parkStatus = ref("全部");
const viewMode = ref<"card" | "table">("card");
const zoneParkFilter = ref<number | null>(null);
const zoneCard = ref<HTMLDivElement>();

const parkDialog = ref(false);
const editingPark = ref<Park | null>(null);
const parkForm = reactive({
  parkCode: "",
  parkName: "",
  parkDescription: "",
  parkAddress: "",
  area: 0,
  ownerName: "",
  parkType: "生产制造",
  status: "运行中",
  color: "#0284c7",
  mapVersionId: ""
});

const baselineDialog = ref(false);
const baselineMapId = ref<string | null>(null);
const policyDialog = ref(false);
const policyMode = ref<"policy" | "lineage">("policy");

const zoneDialog = ref(false);
const editingZone = ref<Zone | null>(null);
const zoneForm = reactive({
  parkId: 1,
  zoneName: "",
  zoneType: "生产区",
  frequency: "每 2 小时",
  status: "正常"
});

const filteredParks = computed(() => {
  const keyword = search.value.trim().toLowerCase();
  return parks.value.filter(park => {
    const matchesKeyword =
      !keyword ||
      `${park.parkName}${park.parkCode}${park.ownerName ?? ""}`.toLowerCase().includes(keyword);
    const matchesStatus = parkStatus.value === "全部" || park.status === parkStatus.value;
    return matchesKeyword && matchesStatus;
  });
});

const filteredZones = computed(() =>
  zoneParkFilter.value
    ? zones.value.filter(zone => zone.parkId === zoneParkFilter.value)
    : zones.value
);

const totalArea = computed(() =>
  parks.value.reduce((sum, park) => sum + park.area, 0).toFixed(1)
);
const totalZones = computed(() => zones.value.length);
const totalDevices = computed(() =>
  parks.value.reduce((sum, park) => sum + park.deviceCount, 0)
);
const completionRate = computed(() => {
  const total = parks.value.reduce((sum, park) => sum + park.taskCount, 0);
  const done = parks.value.reduce((sum, park) => sum + park.doneCount, 0);
  return total ? Math.round((done / total) * 100) : 0;
});

function rateOf(park: Park) {
  return park.taskCount ? Math.round((park.doneCount / park.taskCount) * 100) : 0;
}

function mapOf(mapId: string | null | undefined) {
  return maps.value.find(item => item.id === mapId) ?? null;
}

/**
 * 园区名下**绑定的全部地图**：归属本园区的（`map_versions.park_id`，可以有很多版本）
 * 加上园区当前基线。基线可能是别的园区的图（共用同一张基线），这时它不在
 * `parkId === park.id` 里，只按归属筛就会「明明绑着却看不到」。
 * 园区基线只有一个（路线未指定地图时的兜底），这里列的是园区可用的整批地图，
 * 新建巡检路线时能任选一张（与 `routes/index.vue::mapsOfPark` 同一口径）。
 */
function mapsBoundToPark(parkId: number | null | undefined) {
  if (!parkId) return [];
  const baselineId = parks.value.find(park => park.id === parkId)?.mapVersionId ?? null;
  const own = maps.value.filter(item => item.parkId === parkId);
  const baseline = baselineId ? maps.value.find(item => item.id === baselineId) : null;
  const list =
    baseline && !own.some(item => item.id === baseline.id) ? [baseline, ...own] : own;
  return [...list].sort((a, b) => {
    if (a.id === baselineId) return -1;
    if (b.id === baselineId) return 1;
    return (b.publishedAt ?? b.createdAt).localeCompare(a.publishedAt ?? a.createdAt);
  });
}

/** 卡片 / 表格上提示的地图数量：不含已归档（归档图只在弹窗里作为历史版本可见） */
function usableMapCount(parkId: number | null | undefined) {
  return mapsBoundToPark(parkId).filter(item => item.status !== "已归档").length;
}

const mapsDialog = ref(false);
const mapsParkId = ref<number | null>(null);
const mapsParkName = computed(
  () => parks.value.find(park => park.id === mapsParkId.value)?.parkName ?? ""
);
const parkBoundMaps = computed(() => mapsBoundToPark(mapsParkId.value));

function openParkMaps(park: Park) {
  mapsParkId.value = park.id;
  mapsDialog.value = true;
}

function isCurrentBaseline(parkId: number | null, mapId: string) {
  return parks.value.find(park => park.id === parkId)?.mapVersionId === mapId;
}

/** 编辑弹窗里「设为基线」：只改表单，确认保存时才生效（后端拦未发布的图） */
function pickBaseline(map: MapVersion) {
  if (map.status !== "已发布" && map.id !== parkForm.mapVersionId) return;
  parkForm.mapVersionId = map.id;
}

/**
 * 把某张地图设为园区基线：打开编辑弹窗并把地图预选好，由用户确认保存。
 * 不做静默保存——后端只接受「中台已发布」的图（`resolve_park_map`），
 * 让保存动作把 400 原因直接显示出来。
 */
function useAsBaseline(map: MapVersion) {
  const park = parks.value.find(item => item.id === mapsParkId.value);
  if (!park) return;
  mapsDialog.value = false;
  openEditPark(park);
  parkForm.mapVersionId = map.id;
}

/**
 * 地图状态由中台统一管理（草稿 / 部分标定 / 已发布 / 已归档），业务平台只照搬：
 * 园区基线只能选「中台已发布」的图。未发布的图仅在「当前就是本园区基线」时保留，
 * 用来显示现状（保存时提交同一个 id 不会被拒），否则会把正在用的基线从下拉里弄消失。
 */
function selectableBaseline(item: MapVersion) {
  return item.status === "已发布" || item.id === parkForm.mapVersionId;
}

/**
 * 编辑弹窗里展示的「本园区绑定的地图」：归属本园区的全部地图 + 当前基线。
 *
 * 下拉只展示一张「当前基线」是不够的：园区名下往往有多张地图（多个建图版本），
 * 用户编完园区就去建路线，看不到清单就以为园区只能绑一张。
 */
const editParkMaps = computed(() => mapsBoundToPark(editingPark.value?.id));

/** 下拉候选：本园区名下全部未归档图（未发布的仍然列出来，只是 `:disabled`，不再凭空消失） */
const activeMapOptions = computed(() =>
  editParkMaps.value.filter(item => item.status !== "已归档")
);

/**
 * 设备地图：机器人回传的地图先只跟设备挂钩（`parkId` 为空），
 * 设备绑定人登录后在这里挑一张绑为本园区基线（选中即认领到本园区）。
 */
const deviceMapOptions = computed(() =>
  maps.value.filter(item => !item.parkId && item.status !== "已归档")
);

/**
 * 已归属**其他园区**的设备地图。
 *
 * 不同园区可以共用同一张地图基线：设备先绑了别的园区（或老图绑错园区）时，
 * 本园区也能直接选它，保存只写 `parks.map_version_id`，**不改变图的归属园区**。
 * 只列带 `deviceCode` 的机器人地图：演示 / 历史地图没有采集设备，不该被挂到业务园区上。
 * 选项里不再展示「归属园区 / 上传人」——一张图可以同时被多个园区当基线，
 * 写个单一归属只会误导人（详见 `resolve_park_map`）。
 */
const otherParkMapOptions = computed(() => {
  const parkId = editingPark.value?.id;
  // 当前基线（可能正是共用别园区的图）已经在 `activeMapOptions` 里展示，
  // 这里排除掉，否则同一个 `value` 会出现两次、下拉里一串重名。
  const baselineId = parkForm.mapVersionId;
  return maps.value.filter(
    item =>
      Boolean(item.deviceCode) &&
      item.parkId != null &&
      item.parkId !== parkId &&
      item.id !== baselineId &&
      item.status !== "已归档"
  );
});

const boundParkCount = computed(
  () => parks.value.filter(park => park.mapVersionId).length
);

async function load() {
  const [parkList, zoneList, mapList] = await Promise.all([
    parkApi.list(),
    zoneApi.list(),
    mapApi.list()
  ]);
  parks.value = parkList;
  zones.value = zoneList;
  maps.value = mapList;
  if (!zoneForm.parkId && parkList.length) zoneForm.parkId = parkList[0].id;
}

function openBaseline(mapId: string | null | undefined) {
  baselineMapId.value = mapId ?? null;
  baselineDialog.value = true;
}

function openPolicy(mode: "policy" | "lineage") {
  policyMode.value = mode;
  policyDialog.value = true;
}

function openCreatePark() {
  editingPark.value = null;
  Object.assign(parkForm, {
    parkCode: "",
    parkName: "",
    parkDescription: "",
    parkAddress: "",
    area: 0,
    ownerName: "",
    parkType: "生产制造",
    status: "运行中",
    color: "#0284c7",
    mapVersionId: ""
  });
  parkDialog.value = true;
}

function openEditPark(park: Park) {
  editingPark.value = park;
  Object.assign(parkForm, {
    parkCode: park.parkCode,
    parkName: park.parkName,
    parkDescription: park.parkDescription ?? "",
    parkAddress: park.parkAddress ?? "",
    area: park.area,
    ownerName: park.ownerName ?? "",
    parkType: park.parkType ?? "生产制造",
    status: park.status,
    color: park.color,
    mapVersionId: park.mapVersionId ?? ""
  });
  parkDialog.value = true;
}

async function savePark() {
  if (!parkForm.parkName.trim() || !parkForm.area) {
    ElMessage.warning("请填写园区名称与面积");
    return;
  }
  // 基线只能是业务库里已有的地图（设备回传 / 中台下发），且必须中台已发布：
  // 不再提供「backend 地图库导入」，见 `resolve_park_map`。
  const payload = { ...parkForm, area: Number(parkForm.area) };
  if (editingPark.value) {
    await parkApi.update(editingPark.value.id, payload);
    ElMessage.success("园区信息已更新");
  } else {
    await parkApi.create(payload);
    ElMessage.success(`园区「${parkForm.parkName}」已创建`);
  }
  parkDialog.value = false;
  await load();
}

async function removePark(park: Park) {
  await ElMessageBox.confirm(`确定删除园区「${park.parkName}」及其区域吗？`, "删除园区", {
    type: "warning"
  });
  await parkApi.remove(park.id);
  ElMessage.success("园区已删除");
  await load();
}

function showZones(park: Park) {
  zoneParkFilter.value = park.id;
  zoneCard.value?.scrollIntoView({ behavior: "smooth", block: "center" });
  ElMessage.info(`已定位 ${park.parkName} 的区域清单`);
}

function openCreateZone() {
  editingZone.value = null;
  Object.assign(zoneForm, {
    parkId: parks.value[0]?.id ?? 1,
    zoneName: "",
    zoneType: "生产区",
    frequency: "每 2 小时",
    status: "正常"
  });
  zoneDialog.value = true;
}

function openEditZone(zone: Zone) {
  editingZone.value = zone;
  Object.assign(zoneForm, {
    parkId: zone.parkId,
    zoneName: zone.zoneName,
    zoneType: zone.zoneType ?? "生产区",
    frequency: zone.frequency ?? "每 2 小时",
    status: zone.status
  });
  zoneDialog.value = true;
}

async function saveZone() {
  if (!zoneForm.zoneName.trim()) {
    ElMessage.warning("请填写区域名称");
    return;
  }
  if (editingZone.value) {
    await zoneApi.update(editingZone.value.id, { ...zoneForm });
    ElMessage.success("巡检区域已更新");
  } else {
    await zoneApi.create({ ...zoneForm });
    ElMessage.success("巡检区域已创建");
  }
  zoneDialog.value = false;
  await load();
}

async function removeZone(zone: Zone) {
  await ElMessageBox.confirm(`确定删除区域「${zone.zoneName}」吗？`, "删除区域", {
    type: "warning"
  });
  await zoneApi.remove(zone.id);
  ElMessage.success("巡检区域已删除");
  await load();
}

function planZone(zone: Zone) {
  router.push({ path: "/routes", query: { zoneId: zone.id, parkId: zone.parkId } });
  ElMessage.info(`已打开「${zone.zoneName}」的巡检路线编排`);
}

onMounted(load);
</script>

<template>
  <div>
    <PageHeader title="园区管理" subtitle="维护园区基础信息、区域划分与巡检范围">
      <template #actions>
        <button class="btn primary" @click="openCreatePark">
          <AppIcon name="plus" />新增园区
        </button>
      </template>
    </PageHeader>

    <div class="map-policy-bar">
      <div class="mp-txt">
        <b>地图基线约束</b> · 园区建档时绑定坐标系，路线创建时冻结版本引用。当前
        {{ boundParkCount }}/{{ parks.length }} 个园区已标定，未绑定的园区仅作资产台账，禁止编排巡检路线。
      </div>
      <button class="btn sm" @click="openPolicy('lineage')">
        <AppIcon name="cpu" />数据来源
      </button>
      <button class="btn sm" @click="openPolicy('policy')">
        <AppIcon name="info" />绑定时机说明
      </button>
    </div>

    <div class="stat-strip">
      <div class="stat-tab on">
        <div class="st-label"><AppIcon name="park" />在管园区</div>
        <div class="st-val">{{ parks.length }} <small>个</small></div>
      </div>
      <div class="stat-tab">
        <div class="st-label"><AppIcon name="layers" />覆盖面积</div>
        <div class="st-val">{{ totalArea }} <small>万㎡</small></div>
      </div>
      <div class="stat-tab">
        <div class="st-label"><AppIcon name="zone" />巡检区域</div>
        <div class="st-val">{{ totalZones }} <small>个</small></div>
      </div>
      <div class="stat-tab">
        <div class="st-label"><AppIcon name="dog" />接入设备</div>
        <div class="st-val">{{ totalDevices }} <small>台</small></div>
      </div>
      <div class="stat-tab">
        <div class="st-label"><AppIcon name="shield" />今日完成率</div>
        <div class="st-val">{{ completionRate }} <small>%</small></div>
      </div>
    </div>

    <div class="toolbar">
      <div class="search">
        <AppIcon name="search" />
        <input
          v-model="search"
          class="input"
          placeholder="搜索园区名称 / 编号 / 负责人"
        />
      </div>
      <select v-model="parkStatus" class="select" style="width: 150px">
        <option>全部</option>
        <option>运行中</option>
        <option>部分维护</option>
        <option>停用</option>
      </select>
      <div class="sep" />
      <div class="seg">
        <button :class="{ on: viewMode === 'card' }" @click="viewMode = 'card'">
          <AppIcon name="grid" />卡片
        </button>
        <button :class="{ on: viewMode === 'table' }" @click="viewMode = 'table'">
          <AppIcon name="list" />表格
        </button>
      </div>
    </div>

    <div v-if="viewMode === 'card'" class="grid g3">
      <div v-for="park in filteredParks" :key="park.id" class="card park-card">
        <div class="park-thumb">
          <MiniMap :color="park.color" />
          <span class="thumb-tag">{{ park.parkCode }}</span>
        </div>
        <div class="park-body">
          <div class="park-name">
            <b>{{ park.parkName }}</b>
            <StatusBadge :status="park.status" />
          </div>
          <div class="park-meta">
            {{ park.parkDescription }} · 面积 {{ park.area }} 万㎡
          </div>
          <div class="park-stats">
            <div class="park-stat"><b>{{ park.zoneCount }}</b><span>巡检区域</span></div>
            <div class="park-stat"><b>{{ park.deviceCount }}</b><span>接入设备</span></div>
            <div class="park-stat"><b>{{ park.taskCount }}</b><span>今日任务</span></div>
          </div>
          <div class="park-prog-label">
            <span>今日巡检完成率</span>
            <b>{{ park.doneCount }}/{{ park.taskCount }} · {{ rateOf(park) }}%</b>
          </div>
          <div class="bar" :class="rateOf(park) === 100 ? 'ok' : ''">
            <i :style="{ width: `${rateOf(park)}%` }" />
          </div>
          <div class="park-mapline">
            <MapChip
              :map-id="park.mapVersionId"
              :status="mapOf(park.mapVersionId)?.status"
              :fallback="park.mapStatus"
            />
            <button
              class="btn sm ghost"
              :disabled="!park.mapVersionId"
              @click="openBaseline(park.mapVersionId)"
            >
              <AppIcon name="layers" />地图基线
            </button>
            <button class="btn sm ghost" @click="openParkMaps(park)">
              <AppIcon name="grid" />绑定地图 {{ usableMapCount(park.id) }}
            </button>
          </div>
          <div class="park-foot">
            <div class="park-owner">
              <span class="avatar">{{ (park.ownerName || "待").slice(0, 1) }}</span>
              负责人 {{ park.ownerName || "待分配" }}
            </div>
            <div class="actions">
              <button class="btn sm" @click="showZones(park)">查看区域</button>
              <button class="btn sm ghost icon" title="编辑" @click="openEditPark(park)">
                <AppIcon name="edit" />
              </button>
              <button
                class="btn sm ghost icon"
                title="删除"
                style="color: #dc2626"
                @click="removePark(park)"
              >
                <AppIcon name="trash" />
              </button>
            </div>
          </div>
        </div>
      </div>
      <div v-if="!filteredParks.length" class="card empty-box" style="grid-column: 1 / -1">
        未找到匹配的园区
      </div>
    </div>

    <div v-else class="card">
      <div class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>园区名称</th>
              <th>面积</th>
              <th>区域数</th>
              <th>设备数</th>
              <th>今日巡检</th>
              <th>负责人</th>
              <th>地图（基线 / 全部）</th>
              <th>状态</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="park in filteredParks" :key="park.id">
              <td>
                <div class="row-flex">
                  <span class="row-icon"><AppIcon name="park" /></span>
                  <div>
                    <div class="cell-main">{{ park.parkName }}</div>
                    <div class="cell-sub mono">{{ park.parkCode }}</div>
                  </div>
                </div>
              </td>
              <td>{{ park.area }} 万㎡</td>
              <td>{{ park.zoneCount }} 个</td>
              <td>{{ park.deviceCount }} 台</td>
              <td>{{ park.doneCount }}/{{ park.taskCount }}</td>
              <td>{{ park.ownerName }}</td>
              <td>
                <div class="park-mapline" style="margin: 0; padding-top: 0; border-top: none">
                  <MapChip
                    :map-id="park.mapVersionId"
                    :status="mapOf(park.mapVersionId)?.status"
                    :fallback="park.mapStatus"
                  />
                  <button
                    class="btn sm ghost icon"
                    title="地图基线"
                    :disabled="!park.mapVersionId"
                    @click="openBaseline(park.mapVersionId)"
                  >
                    <AppIcon name="layers" />
                  </button>
                  <button
                    class="btn sm ghost"
                    :title="`查看 ${park.parkName} 绑定的全部地图`"
                    @click="openParkMaps(park)"
                  >
                    全部 {{ usableMapCount(park.id) }}
                  </button>
                </div>
              </td>
              <td><StatusBadge :status="park.status" /></td>
              <td>
                <div class="actions">
                  <button class="btn sm" @click="showZones(park)">区域</button>
                  <button class="btn sm ghost icon" @click="openEditPark(park)">
                    <AppIcon name="edit" />
                  </button>
                  <button
                    class="btn sm ghost icon"
                    style="color: #dc2626"
                    @click="removePark(park)"
                  >
                    <AppIcon name="trash" />
                  </button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="tfoot"><span>共 {{ filteredParks.length }} 个园区</span></div>
    </div>

    <div ref="zoneCard" class="card mt18">
      <div class="card-head">
        <div class="card-title">
          <AppIcon name="zone" />巡检区域清单
          <span class="card-sub">
            {{ zoneParkFilter ? `已筛选园区 · ${filteredZones.length} 个区域` : "按园区划分的最小巡检单元" }}
          </span>
        </div>
        <div class="actions">
          <button
            v-if="zoneParkFilter"
            class="btn sm ghost"
            @click="zoneParkFilter = null"
          >
            显示全部
          </button>
          <button class="btn sm" @click="openCreateZone">
            <AppIcon name="plus" />新增区域
          </button>
        </div>
      </div>
      <div class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>区域名称 / 所属园区</th>
              <th>区域类型</th>
              <th>绑定设备</th>
              <th>巡检频次</th>
              <th>状态</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="zone in filteredZones" :key="zone.id">
              <td>
                <div class="cell-main">{{ zone.zoneName }}</div>
                <div class="cell-sub">{{ zone.parkName }}</div>
              </td>
              <td><span class="tag blue">{{ zone.zoneType }}</span></td>
              <td>{{ zone.deviceCount }} 台</td>
              <td>{{ zone.frequency }}</td>
              <td><StatusBadge :status="zone.status" /></td>
              <td>
                <div class="actions">
                  <button class="btn sm" @click="planZone(zone)">巡检计划</button>
                  <button class="btn sm ghost icon" @click="openEditZone(zone)">
                    <AppIcon name="edit" />
                  </button>
                  <button
                    class="btn sm ghost icon"
                    style="color: #dc2626"
                    @click="removeZone(zone)"
                  >
                    <AppIcon name="trash" />
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!filteredZones.length">
              <td colspan="6"><div class="empty-box">暂无巡检区域</div></td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="tfoot"><span>共 {{ filteredZones.length }} 个巡检区域</span></div>
    </div>

    <el-dialog
      v-model="parkDialog"
      :title="editingPark ? '编辑园区' : '新增园区'"
      width="580px"
    >
      <div class="form-grid">
        <div class="field">
          <label>园区名称 <em>*</em></label>
          <input v-model="parkForm.parkName" class="input" placeholder="例如：松江智造园区" />
        </div>
        <div class="field">
          <label>园区编号</label>
          <input
            v-model="parkForm.parkCode"
            class="input"
            :readonly="Boolean(editingPark)"
            placeholder="留空自动生成"
          />
        </div>
        <div class="field">
          <label>占地面积（万㎡） <em>*</em></label>
          <input v-model.number="parkForm.area" class="input" type="number" min="0" step="0.1" />
        </div>
        <div class="field">
          <label>负责人</label>
          <input v-model="parkForm.ownerName" class="input" placeholder="请输入姓名" />
        </div>
        <div class="field">
          <label>运行状态</label>
          <select v-model="parkForm.status" class="select">
            <option>运行中</option>
            <option>部分维护</option>
            <option>停用</option>
          </select>
        </div>
        <div class="field">
          <label>园区类型</label>
          <select v-model="parkForm.parkType" class="select">
            <option v-for="type in PARK_TYPES" :key="type">{{ type }}</option>
          </select>
        </div>
        <div class="field">
          <label>主题色</label>
          <input v-model="parkForm.color" class="input" type="color" style="height: 34px; padding: 3px" />
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>地图基线绑定 <em>*</em></label>
          <select v-model="parkForm.mapVersionId" class="select">
            <option value="">待标定 · 已登记坐标系，建图采集中（不可编排路线）</option>
            <option value="__none__">暂不绑定 · 仅资产台账（不可编排路线）</option>
            <option
              v-for="map in activeMapOptions"
              :key="map.id"
              :value="map.id"
              :disabled="!selectableBaseline(map)"
            >
              {{ map.mapName }} · {{ map.id }} · {{ map.status }}
              {{ map.status === "已发布" ? "" : "（未发布·仅保持现状）" }} ·
              {{ map.parkId !== editingPark?.id ? "共用其他园区的图 · " : "" }}{{
                map.sourceId ? `中台 #${map.sourceId}` : "无中台记录"
              }}
            </option>
            <optgroup
              v-if="deviceMapOptions.length"
              label="我的设备地图 · 未归属园区（选中即绑定到本园区）"
            >
              <option
                v-for="map in deviceMapOptions"
                :key="map.id"
                :value="map.id"
                :disabled="!selectableBaseline(map)"
              >
                {{ map.mapName }} · {{ map.id }} · {{ map.status }}
                {{ map.status === "已发布" ? "" : "（未发布·仅保持现状）" }} ·
                {{ map.sourceId ? `中台 #${map.sourceId}` : "无中台记录" }} ·
                {{ map.deviceCode ? `设备 ${map.deviceCode}` : "设备未知" }}
              </option>
            </optgroup>
            <optgroup
              v-if="otherParkMapOptions.length"
              label="我的设备地图 · 已有其他园区在用（可共用同一张）"
            >
              <option
                v-for="map in otherParkMapOptions"
                :key="map.id"
                :value="map.id"
                :disabled="!selectableBaseline(map)"
              >
                {{ map.mapName }} · {{ map.id }} · {{ map.status }}
                {{ map.status === "已发布" ? "" : "（未发布·仅保持现状）" }} ·
                {{ map.sourceId ? `中台 #${map.sourceId}` : "无中台记录" }} ·
                {{ map.deviceCode ? `设备 ${map.deviceCode}` : "设备未知" }}
              </option>
            </optgroup>
          </select>

          <!-- 园区名下可以有多张地图（多个建图版本）：只给一个基线下拉看不出家底 -->
          <div v-if="editingPark" class="bound-maps">
            <div class="bm-head">
              <b>本园区绑定的地图</b>
              <span class="tag blue">{{ editParkMaps.length }} 张</span>
              <span class="bm-hint">
                基线只有一个（路线未指定地图时的兜底）；路线创建时可选本园区任意一张
              </span>
              <button class="btn sm ghost" @click="openParkMaps(editingPark)">
                <AppIcon name="grid" />查看全部
              </button>
            </div>
            <div v-if="editParkMaps.length" class="bm-list">
              <div
                v-for="map in editParkMaps"
                :key="map.id"
                class="bm-row"
                :class="{ on: parkForm.mapVersionId === map.id }"
              >
                <div class="bm-name">
                  <b>{{ map.mapName }}</b>
                  <span class="bm-id">{{ map.id }}</span>
                </div>
                <span class="bm-meta">{{ map.version }}</span>
                <StatusBadge :status="map.status" />
                <span class="bm-meta">{{ map.deviceCode ? `设备 ${map.deviceCode}` : "设备未登记" }}</span>
                <span class="bm-meta">{{ map.routeCount }} 条路线</span>
                <span v-if="parkForm.mapVersionId === map.id" class="tag teal"
                  >当前基线</span
                >
                <span v-else-if="map.parkId !== editingPark?.id" class="tag orange">共用</span>
                <button
                  class="btn sm ghost"
                  :disabled="parkForm.mapVersionId === map.id || map.status !== '已发布'"
                  :title="
                    map.status !== '已发布'
                      ? '只有中台「已发布」的地图能当园区基线'
                      : '设为园区基线（保存后生效）'
                  "
                  @click="pickBaseline(map)"
                >
                  设为基线
                </button>
              </div>
            </div>
            <div v-else class="rt-hint" style="margin-top: 8px">
              该园区还没有绑定任何地图：先在「设备管理」同步地图，或在新建路线时选一张图（自动归属到本园区）。
            </div>
          </div>
        </div>
        <div class="field">
          <label>园区地址</label>
          <input v-model="parkForm.parkAddress" class="input" placeholder="选填" />
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>备注说明</label>
          <textarea
            v-model="parkForm.parkDescription"
            class="textarea"
            rows="3"
            placeholder="选填，描述园区范围与巡检要求"
          />
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="parkDialog = false"><AppIcon name="close" />取消</button>
        <button class="btn primary" @click="savePark">
          <AppIcon name="check" />保存
        </button>
      </template>
    </el-dialog>

    <el-dialog
      v-model="zoneDialog"
      :title="editingZone ? '编辑巡检区域' : '新增巡检区域'"
      width="520px"
    >
      <div class="form-grid">
        <div class="field">
          <label>区域名称 <em>*</em></label>
          <input v-model="zoneForm.zoneName" class="input" placeholder="例如：I区 · 新车间" />
        </div>
        <div class="field">
          <label>所属园区</label>
          <select v-model.number="zoneForm.parkId" class="select">
            <option v-for="park in parks" :key="park.id" :value="park.id">
              {{ park.parkName }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>区域类型</label>
          <select v-model="zoneForm.zoneType" class="select">
            <option v-for="type in ZONE_TYPES" :key="type">{{ type }}</option>
          </select>
        </div>
        <div class="field">
          <label>巡检频次</label>
          <select v-model="zoneForm.frequency" class="select">
            <option>每 1 小时</option>
            <option>每 2 小时</option>
            <option>每 3 小时</option>
            <option>每 4 小时</option>
            <option>每 6 小时</option>
            <option>维护中</option>
          </select>
        </div>
        <div class="field">
          <label>状态</label>
          <select v-model="zoneForm.status" class="select">
            <option>正常</option>
            <option>告警</option>
            <option>维护中</option>
          </select>
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="zoneDialog = false"><AppIcon name="close" />取消</button>
        <button class="btn primary" @click="saveZone">
          <AppIcon name="check" />保存
        </button>
      </template>
    </el-dialog>

    <el-dialog v-model="mapsDialog" :title="`${mapsParkName} · 绑定的地图`" width="780px">
      <div class="rt-hint" style="margin: 0 0 10px">
        园区基线只有一个（路线未指定地图时的兜底）；同一个园区名下的地图可以有多个版本，
        新建巡检路线时能在这里任选一张，路线创建即冻结引用。
        已归档的历史版本仍列出来（灰色），但不能再用于新建路线。
      </div>
      <div class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>地图名称 / 编号</th>
              <th>版本</th>
              <th>状态</th>
              <th>采集设备</th>
              <th>引用路线</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="map in parkBoundMaps" :key="map.id">
              <td>
                <div class="cell-main">
                  {{ map.mapName }}
                  <span v-if="isCurrentBaseline(mapsParkId, map.id)" class="tag teal">当前基线</span>
                  <span v-else-if="map.parkId !== mapsParkId" class="tag orange">共用其他园区的图</span>
                </div>
                <div class="cell-sub mono">{{ map.id }}</div>
              </td>
              <td>{{ map.version }}</td>
              <td><StatusBadge :status="map.status" /></td>
              <td>{{ map.deviceCode || "—" }}</td>
              <td>{{ map.routeCount }} 条</td>
              <td>
                <div class="actions">
                  <button class="btn sm ghost" @click="openBaseline(map.id)">查看</button>
                  <button
                    class="btn sm"
                    :disabled="isCurrentBaseline(mapsParkId, map.id) || map.status !== '已发布'"
                    :title="
                      map.status !== '已发布'
                        ? '地图状态由中台统一管理，只有中台「已发布」的地图能当园区基线'
                        : '设为园区基线（打开编辑弹窗确认保存）'
                    "
                    @click="useAsBaseline(map)"
                  >
                    设为基线
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!parkBoundMaps.length">
              <td colspan="6"><div class="empty-box">该园区还没有绑定任何地图</div></td>
            </tr>
          </tbody>
        </table>
      </div>
      <template #footer>
        <button class="btn" @click="mapsDialog = false"><AppIcon name="close" />关闭</button>
      </template>
    </el-dialog>

    <MapBaselineDialog
      v-model="baselineDialog"
      :map-id="baselineMapId"
    />
    <MapPolicyDialog v-model="policyDialog" :mode="policyMode" />
  </div>
</template>

<style scoped>
.bound-maps{
  margin-top:10px;
  padding:10px 12px;
  border:1px solid var(--line);
  border-radius:10px;
  background:#fbfcfd;
}
.bm-head{
  display:flex;
  align-items:center;
  gap:8px;
  flex-wrap:wrap;
  font-size:11.5px;
  color:var(--text);
}
.bm-hint{color:var(--muted);font-size:10.5px}
.bm-head .btn{margin-left:auto}
.bm-list{
  margin-top:8px;
  display:flex;
  flex-direction:column;
  gap:6px;
  max-height:190px;
  overflow:auto;
}
.bm-row{
  display:flex;
  align-items:center;
  gap:8px;
  padding:6px 8px;
  border:1px solid transparent;
  border-radius:8px;
}
.bm-row.on{background:#eef7ff;border-color:#cfe4f5}
.bm-name{display:flex;flex-direction:column;min-width:0;flex:1 1 150px}
.bm-name b{
  font-size:11.5px;
  color:var(--text);
  overflow:hidden;
  text-overflow:ellipsis;
  white-space:nowrap;
}
.bm-id{
  font-family:"SFMono-Regular",Consolas,"Liberation Mono",monospace;
  font-size:10px;
  color:var(--muted);
  overflow:hidden;
  text-overflow:ellipsis;
  white-space:nowrap;
}
.bm-meta{font-size:10.5px;color:var(--muted);white-space:nowrap}
.bm-row .btn{flex:none}
</style>
