<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { deviceApi, parkApi, zoneApi } from "@/api/ops";
import type { Device, Park, Zone } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import ProgressBar from "@/components/ProgressBar.vue";
import DeviceDrawer from "@/components/DeviceDrawer.vue";
import { DEVICE_MODELS, DEVICE_STATUSES } from "@/utils/constants";
import { downloadCsv, isLocReady, locStatusMeta, timeOnly } from "@/utils/format";

const route = useRoute();
const devices = ref<Device[]>([]);
const parks = ref<Park[]>([]);
const zones = ref<Zone[]>([]);
const stats = ref<{ total: number; online: number; counts: Record<string, number> }>({
  total: 0,
  online: 0,
  counts: {}
});
const search = ref("");
const parkFilter = ref<number | "">("");
const statusFilter = ref("全部");
const drawerOpen = ref(false);
const activeDevice = ref<Device | null>(null);
const deviceDialog = ref(false);
const editingDevice = ref<Device | null>(null);
const batchDialog = ref(false);
const batchCommand = ref("开始巡检");
/** 绑定园区弹窗 */
const bindDialog = ref(false);
const bindTarget = ref<Device | null>(null);
const bindForm = reactive({
  parkId: null as number | null,
  zoneId: null as number | null
});

const deviceForm = reactive({
  deviceId: "",
  model: "X30-Pro",
  parkId: null as number | null,
  zoneId: null as number | null,
  status: "待绑定",
  battery: 100,
  temperature: 32,
  firmware: "v2.6.1",
  deviceIp: "",
  videoUrl: "",
  videoType: "none"
});

/** 本体工作模式（1315 mode / MachineStatus）→ 中文；未知值原样展示 */
const ROBOT_MODE_LABELS: Record<string, string> = {
  remote: "遥控",
  auto: "自动",
  manual: "手动",
  idle: "待机",
  standby: "待机",
  mapping: "建图",
  charging: "充电",
  estop: "急停"
};

function robotModeLabel(mode: string) {
  return ROBOT_MODE_LABELS[mode.toLowerCase()] ?? mode;
}

/** 视频协议选项，none 表示未接入 */
const VIDEO_TYPES = [
  { value: "none", label: "未接入" },
  { value: "hls", label: "HLS（.m3u8）" },
  { value: "flv", label: "HTTP-FLV（.flv）" },
  { value: "mp4", label: "MP4 / WebM" },
  { value: "webrtc", label: "WebRTC（需信令网关）" },
  { value: "rtsp", label: "RTSP（经客户端回传，可播放）" }
];

const statusTabs = [
  { key: "全部", label: "全部设备", color: "#0284c7" },
  { key: "在线", label: "在线设备", color: "#10b981" },
  { key: "执行任务", label: "执行任务", color: "#3b82f6" },
  { key: "充电中", label: "充电中", color: "#f59e0b" },
  { key: "待绑定", label: "待绑定", color: "#a855f7" },
  { key: "离线", label: "离线", color: "#94a3b8" },
  { key: "故障", label: "故障", color: "#ef4444" }
];

const filteredDevices = computed(() => {
  const keyword = search.value.trim().toLowerCase();
  return devices.value.filter(device => {
    const matchesKeyword =
      !keyword ||
      `${device.deviceId}${device.model}${device.parkName}${device.zoneName}${device.currentTaskName}`
        .toLowerCase()
        .includes(keyword);
    const matchesPark = !parkFilter.value || device.parkId === parkFilter.value;
    const matchesStatus =
      statusFilter.value === "全部" ||
      (statusFilter.value === "在线"
        ? ["执行任务", "在线待命"].includes(device.status)
        : device.status === statusFilter.value);
    return matchesKeyword && matchesPark && matchesStatus;
  });
});

const zonesOfPark = computed(() =>
  zones.value.filter(zone => zone.parkId === deviceForm.parkId)
);

const zonesOfBindPark = computed(() =>
  zones.value.filter(zone => zone.parkId === bindForm.parkId)
);

function countOf(key: string) {
  if (key === "全部") return stats.value.total;
  if (key === "在线") return stats.value.online;
  return stats.value.counts[key] ?? 0;
}

async function load() {
  const [deviceList, statsData, parkList, zoneList] = await Promise.all([
    deviceApi.list(),
    deviceApi.stats(),
    parkApi.list(),
    zoneApi.list()
  ]);
  devices.value = deviceList;
  stats.value = statsData;
  parks.value = parkList;
  zones.value = zoneList;
}

function openDrawer(device: Device) {
  activeDevice.value = device;
  drawerOpen.value = true;
}

async function closeDrawer() {
  drawerOpen.value = false;
}

/** 接管/释放后刷新当前设备与列表，保持抽屉数据同步 */
async function refreshActive() {
  await load();
  const id = activeDevice.value?.id;
  activeDevice.value = id ? (devices.value.find(d => d.id === id) ?? null) : null;
}

async function sendCommand(command: string) {
  if (!activeDevice.value) return;
  await deviceApi.command(activeDevice.value.id, command);
  ElMessage.success(`已向 ${activeDevice.value.deviceId} 下发指令：${command}`);
  await load();
  activeDevice.value = devices.value.find(d => d.id === activeDevice.value?.id) ?? null;
}

/** 重新定位：机器狗不在图上 / 定位丢失时的恢复入口 */
async function relocalize(device: Device) {
  try {
    await ElMessageBox.confirm(
      `对 ${device.deviceId} 下发重新定位（地图：${device.activeMapId || "园区地图基线"}）。\n` +
        "设备没被搬动时约 10~30 秒收敛；若已被搬到地图外，请先远程接管移回原位。",
      "重新定位",
      { type: "warning", confirmButtonText: "下发", cancelButtonText: "取消" }
    );
  } catch {
    return;
  }
  try {
    const res = await deviceApi.locate(device.id, { mapId: device.activeMapId });
    ElMessage.success(
      res.wsDelivered ? "已下发重新定位，等待定位收敛" : "设备未连接，指令未送达"
    );
  } catch {
    // 拦截器已弹出可读错误（例如「设备未在线」「未指定 mapId」）
  }
  await load();
  activeDevice.value = devices.value.find(d => d.id === device.id) ?? null;
}

function openCreateDevice() {
  editingDevice.value = null;
  Object.assign(deviceForm, {
    deviceId: `RD-${String(25 + devices.value.length).padStart(3, "0")}`,
    model: "X30-Pro",
    // 新设备默认不绑定园区，由人工在设备管理里绑定
    parkId: null,
    zoneId: null,
    status: "待绑定",
    battery: 100,
    temperature: 32,
    firmware: "v2.6.1",
    deviceIp: "",
    videoUrl: "",
    videoType: "none"
  });
  deviceDialog.value = true;
}

function openEditDevice(device: Device) {
  editingDevice.value = device;
  Object.assign(deviceForm, {
    deviceId: device.deviceId,
    model: device.model ?? "X30-Pro",
    parkId: device.parkId,
    zoneId: device.zoneId,
    status: device.status,
    battery: device.battery,
    temperature: device.temperature,
    firmware: device.firmware ?? "v2.6.1",
    deviceIp: device.deviceIp ?? "",
    videoUrl: device.videoUrl ?? "",
    videoType: device.videoType ?? "none"
  });
  deviceDialog.value = true;
}

async function saveDevice() {
  if (!deviceForm.deviceId.trim()) {
    ElMessage.warning("请填写设备编号");
    return;
  }
  const payload = { ...deviceForm };
  if (editingDevice.value) {
    await deviceApi.update(editingDevice.value.id, payload);
    ElMessage.success("设备信息已更新");
  } else {
    await deviceApi.create(payload);
    ElMessage.success(`设备 ${deviceForm.deviceId} 已添加`);
  }
  deviceDialog.value = false;
  await load();
}

async function removeDevice(device: Device) {
  await ElMessageBox.confirm(`确定删除设备「${device.deviceId}」吗？`, "删除设备", {
    type: "warning"
  });
  await deviceApi.remove(device.id);
  ElMessage.success("设备已删除");
  await load();
}

async function refreshStatus() {
  // 接口已改成真探活（查活跃 WebSocket），不再伪造 last_heartbeat
  const count = await deviceApi.refresh();
  await load();
  ElMessage.success(`已探活：${count} 台在线，其余置为离线`);
}

async function batchDispatch() {
  const targets = devices.value.filter(device => device.status !== "离线");
  await Promise.all(targets.map(device => deviceApi.command(device.id, batchCommand.value)));
  batchDialog.value = false;
  await load();
  ElMessage.success(`已向 ${targets.length} 台在线设备下发指令：${batchCommand.value}`);
}

function exportDevices() {
  downloadCsv("robodog-devices.csv", [
    ["设备编号", "型号", "园区", "区域", "状态", "工作模式", "本体在线", "电量", "当前任务", "最后心跳"],
    ...filteredDevices.value.map(device => [
      device.deviceId,
      device.model,
      device.parkName ?? "未绑定",
      device.zoneName ?? "",
      device.status,
      device.robotMode ? robotModeLabel(device.robotMode) : "—",
      device.bodyOnline ? "是" : "否",
      device.bodyOnline ? device.battery : "—",
      device.currentTaskName,
      device.lastHeartbeat
    ])
  ]);
  ElMessage.success("设备台账已导出");
}

function runtimeText(minutes: number) {
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

watch(
  () => route.query.q,
  value => {
    if (typeof value === "string") search.value = value;
  },
  { immediate: true }
);

watch(
  () => deviceForm.parkId,
  () => {
    if (deviceForm.parkId === null) {
      deviceForm.zoneId = null;
      if (deviceForm.status === "在线待命") deviceForm.status = "待绑定";
      return;
    }
    if (!zonesOfPark.value.some(zone => zone.id === deviceForm.zoneId)) {
      deviceForm.zoneId = zonesOfPark.value[0]?.id ?? null;
    }
    if (deviceForm.status === "待绑定") deviceForm.status = "在线待命";
  }
);

watch(
  () => bindForm.parkId,
  () => {
    if (!zonesOfBindPark.value.some(zone => zone.id === bindForm.zoneId)) {
      bindForm.zoneId = null;
    }
  }
);

/** 手动绑定园区 */
function openBind(device: Device) {
  bindTarget.value = device;
  bindForm.parkId = device.parkId ?? parks.value[0]?.id ?? null;
  bindForm.zoneId = device.zoneId ?? null;
  bindDialog.value = true;
}

async function submitBind() {
  if (!bindTarget.value || !bindForm.parkId) {
    ElMessage.warning("请选择园区");
    return;
  }
  const parkId = bindForm.parkId;
  await deviceApi.bind(bindTarget.value.id, parkId, bindForm.zoneId);
  bindDialog.value = false;
  await load();
  ElMessage.success(`设备 ${bindTarget.value.deviceId} 已绑定园区`);
}

async function unbind(device: Device) {
  await ElMessageBox.confirm(
    `解除后设备「${device.deviceId}」将进入待绑定状态，且无法下发任务。是否继续？`,
    "解除园区绑定",
    { type: "warning" }
  );
  await deviceApi.unbind(device.id);
  bindDialog.value = false;
  await load();
  ElMessage.success("已解除园区绑定");
}

onMounted(load);
</script>

<template>
  <div>
    <PageHeader title="设备管理" subtitle="设备档案、实时状态监控与远程控制">
      <template #actions>
        <button class="btn" @click="batchDialog = true">
          <AppIcon name="send" />批量下发
        </button>
        <button class="btn primary" @click="openCreateDevice">
          <AppIcon name="plus" />添加设备
        </button>
      </template>
    </PageHeader>

    <div class="stat-strip">
      <div
        v-for="tab in statusTabs"
        :key="tab.key"
        class="stat-tab"
        :class="{ on: statusFilter === tab.key }"
        @click="statusFilter = tab.key"
      >
        <div class="st-label"><i :style="{ background: tab.color }" />{{ tab.label }}</div>
        <div class="st-val">{{ countOf(tab.key) }} <small>台</small></div>
      </div>
    </div>

    <div class="toolbar">
      <div class="search">
        <AppIcon name="search" />
        <input v-model="search" class="input" placeholder="搜索设备编号 / 型号 / 区域" />
      </div>
      <select v-model="parkFilter" class="select" style="width: 160px">
        <option value="">全部园区</option>
        <option v-for="park in parks" :key="park.id" :value="park.id">
          {{ park.parkName }}
        </option>
      </select>
      <select v-model="statusFilter" class="select" style="width: 140px">
        <option v-for="status in ['全部', '在线', ...DEVICE_STATUSES]" :key="status">
          {{ status }}
        </option>
      </select>
      <div class="sep" />
      <button class="btn" @click="refreshStatus">
        <AppIcon name="refresh" />刷新状态
      </button>
      <button class="btn" @click="exportDevices">
        <AppIcon name="download" />导出
      </button>
    </div>

    <div class="card">
      <div class="card-head">
        <div class="card-title">
          <AppIcon name="dog" />设备列表
          <span class="card-sub">
            共 {{ stats.total }} 台设备 · 当前展示 {{ filteredDevices.length }} 台
          </span>
        </div>
        <span class="live-chip"><i />心跳实时同步</span>
      </div>
      <div class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>设备编号 / 型号</th>
              <th>所属园区 / 区域</th>
              <th>运行状态</th>
              <th>电量</th>
              <th>当前任务</th>
              <th>工作模式</th>
              <th>最后心跳</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="device in filteredDevices"
              :key="device.id"
              class="cursor-pointer"
              @click="openDrawer(device)"
            >
              <td>
                <div class="row-flex">
                  <span class="row-icon"><AppIcon name="dog" /></span>
                  <div>
                    <div class="cell-main mono">{{ device.deviceId }}</div>
                    <div class="cell-sub">{{ device.model }}</div>
                  </div>
                </div>
              </td>
              <td>
                {{ device.parkName || "未绑定" }}
                <div class="cell-sub">{{ device.zoneName || (device.parkName ? "未指定区域" : "待手动绑定园区") }}</div>
              </td>
              <td>
                <StatusBadge :status="device.status" />
                <div
                  v-if="device.status !== '离线' && device.ctrlSource === 3"
                  class="cell-sub"
                  style="margin-top: 4px"
                  title="1315 ctrl_source=3：控制权不在平台（遥控器 / 第三方 App / 本体内部残留任务），平台摇杆与任务下发会失效"
                >
                  <span class="badge danger"><i />控制权被占用</span>
                </div>
                <div
                  v-if="device.status !== '离线' && device.bodyOnline"
                  class="cell-sub"
                  style="display: flex; align-items: center; gap: 6px; margin-top: 4px"
                  :title="`地图：${device.activeMapId || '未载图'}`"
                >
                  <span class="badge" :class="locStatusMeta(device.locStatus).tone">
                    <i />{{ locStatusMeta(device.locStatus).label }}
                  </span>
                </div>
                <div
                  v-if="device.status !== '离线' && !device.bodyOnline"
                  class="cell-sub"
                  :title="
                    device.runMode === 'sim'
                      ? '客户端未连本体（模拟模式），电量等数值不是本体上报'
                      : '本体链路已断，电量等数值已停止更新'
                  "
                >
                  {{ device.runMode === "sim" ? "模拟模式" : "本体离线" }}
                </div>
              </td>
              <td style="min-width: 130px">
                <template v-if="device.bodyOnline">
                  <ProgressBar :value="device.battery" />
                  <div v-if="device.charging" class="cell-sub" style="color: #0d9488">充电中</div>
                </template>
                <span v-else class="mono muted" title="本体未上报，不展示模拟值">—</span>
              </td>
              <td><div class="cell-main" style="font-weight: 500">{{ device.currentTaskName }}</div></td>
              <td>
                <span
                  v-if="device.robotMode"
                  class="cell-main"
                  :title="`本体 1315 mode = ${device.robotMode}`"
                >
                  {{ robotModeLabel(device.robotMode) }}
                </span>
                <span v-else class="mono muted" title="本体未上报工作模式">—</span>
              </td>
              <td class="mono muted">{{ timeOnly(device.lastHeartbeat) }}</td>
              <td>
                <div class="actions" @click.stop>
                  <button class="btn sm" @click="openDrawer(device)">
                    <AppIcon name="play" />控制
                  </button>
                  <button
                    v-if="device.bodyOnline && !isLocReady(device.locStatus)"
                    class="btn sm"
                    style="color: #b45309"
                    title="机器人不在地图上 / 定位丢失时使用"
                    @click="relocalize(device)"
                  >
                    <AppIcon name="waypoint" />重新定位
                  </button>
                  <button
                    v-if="!device.parkId"
                    class="btn sm"
                    style="color: #7c3aed"
                    @click="openBind(device)"
                  >
                    <AppIcon name="park" />绑定园区
                  </button>
                  <button class="btn sm ghost icon" @click="openEditDevice(device)">
                    <AppIcon name="edit" />
                  </button>
                  <button
                    class="btn sm ghost icon"
                    style="color: #dc2626"
                    @click="removeDevice(device)"
                  >
                    <AppIcon name="trash" />
                  </button>
                </div>
              </td>
            </tr>
            <tr v-if="!filteredDevices.length">
              <td colspan="8"><div class="empty-box">没有符合条件的设备</div></td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="tfoot">
        <span>显示 {{ filteredDevices.length }} 条记录</span>
        <span class="muted">运行时长示例：{{ runtimeText(266) }}</span>
      </div>
    </div>

    <DeviceDrawer
      :device="activeDevice"
      :open="drawerOpen"
      @close="closeDrawer"
      @command="sendCommand"
      @changed="refreshActive"
    />

    <el-dialog
      v-model="deviceDialog"
      :title="editingDevice ? '编辑设备' : '添加设备'"
      width="620px"
    >
      <div class="form-grid">
        <div class="field">
          <label>设备编号 <em>*</em></label>
          <input v-model="deviceForm.deviceId" class="input" placeholder="RD-XXX" />
        </div>
        <div class="field">
          <label>设备型号</label>
          <select v-model="deviceForm.model" class="select">
            <option v-for="model in DEVICE_MODELS" :key="model">{{ model }}</option>
          </select>
        </div>
        <div class="field">
          <label>所属园区</label>
          <select v-model.number="deviceForm.parkId" class="select">
            <option :value="null">未绑定（稍后手动绑定）</option>
            <option v-for="park in parks" :key="park.id" :value="park.id">
              {{ park.parkName }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>所属区域</label>
          <select
            v-model.number="deviceForm.zoneId"
            class="select"
            :disabled="!deviceForm.parkId"
          >
            <option :value="null">未指定</option>
            <option v-for="zone in zonesOfPark" :key="zone.id" :value="zone.id">
              {{ zone.zoneName }}
            </option>
          </select>
        </div>
        <div class="field">
          <label>初始状态</label>
          <select v-model="deviceForm.status" class="select">
            <option v-for="status in DEVICE_STATUSES" :key="status">{{ status }}</option>
          </select>
        </div>
        <div class="field">
          <label>固件版本</label>
          <input v-model="deviceForm.firmware" class="input" />
        </div>
        <div class="field">
          <label>电量（%，模拟/初始值）</label>
          <input v-model.number="deviceForm.battery" class="input" type="number" min="0" max="100" />
        </div>
        <div class="field">
          <label>机身温度（℃）</label>
          <input v-model.number="deviceForm.temperature" class="input" type="number" step="0.1" />
        </div>
        <div class="field">
          <label>设备 IP</label>
          <input v-model="deviceForm.deviceIp" class="input" placeholder="选填" />
        </div>
        <div class="field">
          <label>视频协议</label>
          <select v-model="deviceForm.videoType" class="select">
            <option v-for="item in VIDEO_TYPES" :key="item.value" :value="item.value">
              {{ item.label }}
            </option>
          </select>
        </div>
        <div class="field" style="grid-column: 1 / -1">
          <label>视频地址</label>
          <input
            v-model="deviceForm.videoUrl"
            class="input mono"
            placeholder="rtsp://user:密码@192.168.234.1:8554/test 或 http://.../live.m3u8"
          />
          <small class="muted" style="font-size: 11px">
            留空则设备详情显示「未配置」。选「未接入」时会按地址后缀自动识别协议；
            RTSP 由设备客户端拉流转 FLV 回传（浏览器不直连）。
          </small>
        </div>
      </div>
      <template #footer>
        <button class="btn" @click="deviceDialog = false">取消</button>
        <button class="btn primary" @click="saveDevice">
          <AppIcon name="check" />保存
        </button>
      </template>
    </el-dialog>

    <el-dialog v-model="bindDialog" title="绑定园区" width="460px">
      <p class="muted" style="font-size: 12px; line-height: 1.7">
        设备编号：<b class="mono">{{ bindTarget?.deviceId }}</b>。绑定园区后设备才能接收任务，
        建图回传也会带上该园区，便于业务平台按园区从中台检索地图。
      </p>
      <div class="field">
        <label>目标园区 <em>*</em></label>
        <select v-model.number="bindForm.parkId" class="select">
          <option :value="null">请选择园区</option>
          <option v-for="park in parks" :key="park.id" :value="park.id">
            {{ park.parkName }}
          </option>
        </select>
      </div>
      <div class="field">
        <label>所属区域</label>
        <select
          v-model.number="bindForm.zoneId"
          class="select"
          :disabled="!bindForm.parkId"
        >
          <option :value="null">不指定区域</option>
          <option v-for="zone in zonesOfBindPark" :key="zone.id" :value="zone.id">
            {{ zone.zoneName }}
          </option>
        </select>
      </div>
      <template #footer>
        <button
          v-if="bindTarget?.parkId"
          class="btn"
          style="color: #dc2626"
          @click="unbind(bindTarget)"
        >
          解除绑定
        </button>
        <button class="btn" @click="bindDialog = false">取消</button>
        <button class="btn primary" @click="submitBind">
          <AppIcon name="check" />确认绑定
        </button>
      </template>
    </el-dialog>

    <el-dialog v-model="batchDialog" title="批量指令下发" width="440px">
      <div class="field">
        <label>指令类型</label>
        <select v-model="batchCommand" class="select">
          <option>开始巡检</option>
          <option>暂停任务</option>
          <option>召回充电</option>
          <option>远程接管</option>
        </select>
      </div>
      <p class="muted" style="font-size: 12px; line-height: 1.7">
        将向所有非离线设备（{{ devices.filter(d => d.status !== "离线").length }} 台）下发该指令。
      </p>
      <template #footer>
        <button class="btn" @click="batchDialog = false">取消</button>
        <button class="btn primary" @click="batchDispatch">
          <AppIcon name="send" />确认下发
        </button>
      </template>
    </el-dialog>
  </div>
</template>
