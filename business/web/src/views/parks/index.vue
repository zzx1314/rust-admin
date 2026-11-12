<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useRouter } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { parkApi, mapApi, zoneApi } from "@/api/ops";
import type { BackendMap, MapVersion, Park, Zone } from "@/api/types";
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
const backendMaps = ref<BackendMap[]>([]);
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

function mapsOfPark(parkId: number | null | undefined) {
  if (!parkId) return [];
  return maps.value.filter(item => item.parkId === parkId && item.status !== "已归档");
}

const activeMapOptions = computed(() => {
  const parkId = editingPark.value?.id;
  return mapsOfPark(parkId).filter(item => item.status !== "已归档");
});

const selectedMap = computed(() => mapOf(parkForm.mapVersionId));

const isBackendSelection = computed(() =>
  parkForm.mapVersionId.startsWith("backend:")
);

const availableBackendMaps = computed(() => {
  const parkId = editingPark.value?.id;
  const parkCode = editingPark.value?.parkCode ?? parkForm.parkCode;
  const imported = new Set(
    (parkId ? mapsOfPark(parkId) : [])
      .map(item => item.sourceId)
      .filter((value): value is string => Boolean(value))
  );
  return backendMaps.value
    .filter(item => {
      if (imported.has(String(item.id))) return false;
      // 中台地图若已带上园区编码，只展示本园区的，避免导错
      return !item.parkCode || !parkCode || item.parkCode === parkCode;
    })
    .sort((a, b) => {
      const matchA = a.parkCode && a.parkCode === parkCode ? 0 : 1;
      const matchB = b.parkCode && b.parkCode === parkCode ? 0 : 1;
      return matchA - matchB;
    });
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
  try {
    backendMaps.value = await mapApi.backendMaps();
  } catch {
    backendMaps.value = [];
  }
  if (!zoneForm.parkId && parkList.length) zoneForm.parkId = parkList[0].id;
}

function openBaseline(mapId: string | null | undefined) {
  baselineMapId.value = mapId ?? null;
  baselineDialog.value = true;
}

function onBaselineUpdated(map: MapVersion) {
  baselineMapId.value = map.id;
  void load();
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
  const backendMapId = parkForm.mapVersionId.startsWith("backend:")
    ? Number(parkForm.mapVersionId.slice("backend:".length))
    : null;
  const payload = {
    ...parkForm,
    mapVersionId: backendMapId ? "" : parkForm.mapVersionId,
    area: Number(parkForm.area)
  };
  let targetPark: Park;
  if (editingPark.value) {
    targetPark = await parkApi.update(editingPark.value.id, payload);
    ElMessage.success("园区信息已更新");
  } else {
    targetPark = await parkApi.create(payload);
    ElMessage.success(`园区「${parkForm.parkName}」已创建`);
  }
  if (backendMapId) {
    const imported = await mapApi.importBackend(backendMapId, targetPark.id, true);
    ElMessage.success(`已从 backend 导入地图「${imported.id}」并绑定为园区基线`);
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
              <th>地图基线</th>
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
            <option v-for="map in activeMapOptions" :key="map.id" :value="map.id">
              {{ map.id }} · {{ map.status }} · {{ map.frameId }}
            </option>
            <optgroup
              v-if="availableBackendMaps.length"
              label="backend 地图库 · ROS / 激光雷达地图"
            >
              <option
                v-for="map in availableBackendMaps"
                :key="map.id"
                :value="`backend:${map.id}`"
              >
                {{ map.mapName }} · {{ map.mapStatus || "草稿" }}
                {{ map.ownerUsername ? `· 归属 ${map.ownerUsername}` : "" }}
                {{ map.parkName ? `· 园区 ${map.parkName}` : "· 未关联园区" }}
                {{ map.fileName ? `· ${map.fileName}` : "" }}
                {{ map.fileSize ? `· ${(map.fileSize / 1024).toFixed(0)} KB` : "" }}
              </option>
            </optgroup>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>坐标系框架 frame_id</label>
          <input
            class="input mono"
            :value="
              selectedMap?.frameId ??
              (isBackendSelection ? '导入后由 backend 地图服务提供' : '—')
            "
            readonly
            placeholder="由地图服务分配，前端只读"
          />
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <div class="rt-hint" style="margin: 0">
            建档时必须确定坐标系（frame_id / 原点 / 分辨率 / CRS / 楼层），并登记地图基线状态。只有「已发布」的地图版本才允许创建巡检路线；建图完成后再发布不可变版本。<br />
            选择「backend 地图库」中的 ROS 地图时，会在保存园区时导入到本业务库，并自动绑定为当前基线。
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

    <MapBaselineDialog
      v-model="baselineDialog"
      :map-id="baselineMapId"
      @updated="onBaselineUpdated"
      @data-source="openPolicy('lineage')"
    />
    <MapPolicyDialog v-model="policyDialog" :mode="policyMode" />
  </div>
</template>
