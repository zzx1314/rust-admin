<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import {
  dogDeviceAssign,
  dogDeviceCommand,
  dogDeviceDelete,
  dogDeviceDispatchTask,
  dogDevicePage,
  dogDeviceRuntime,
  dogDeviceSave,
  dogDeviceSyncMaps,
  dogDeviceUnassign,
  dogDeviceUpdate,
  dogDeviceUsers,
  type DogDevice,
  type DogDeviceUser
} from "@/api/dogDevice";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";
import DogTableActions from "@/components/DogUI/DogTableActions.vue";
import type { DogSearchField } from "@/views/dog/components/search";

defineOptions({ name: "DogDevice" });

const DEVICE_STATUS = [
  "离线",
  "在线待命",
  "执行任务",
  "待绑定",
  "故障",
  "维护中"
];

const queryForm = reactive({
  deviceId: "",
  deviceName: "",
  status: "",
  parkCode: ""
});
const queryFields: DogSearchField[] = [
  { label: "设备编号", prop: "deviceId" },
  { label: "设备名称", prop: "deviceName" },
  {
    label: "设备状态",
    prop: "status",
    type: "select",
    options: DEVICE_STATUS.map(item => ({ label: item, value: item }))
  },
  { label: "园区编码", prop: "parkCode" }
];

const loading = ref(false);
const dataList = ref<DogDevice[]>([]);
const total = ref(0);
const current = ref(1);
const pageSize = ref(10);
const selection = ref<DogDevice[]>([]);
const selectedIds = computed(() => selection.value.map(item => item.id));

const users = ref<DogDeviceUser[]>([]);
const syncingId = ref<number | null>(null);
const sendingId = ref<number | null>(null);
const dispatchingId = ref<number | null>(null);

/** 允许从中台发起的指令，与后端白名单、业务平台 `BUSINESS_COMMANDS` 对齐 */
const DEVICE_COMMANDS = [
  "开始巡检",
  "暂停任务",
  "继续任务",
  "召回充电",
  "远程接管",
  "释放接管"
];

/**
 * 幂等键：一次用户操作一个值，失败重试时复用同一个值，
 * 中台与业务平台都不会把同一条指令下发给设备两次。
 */
function newRequestId(deviceId: string) {
  const random =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
  return `ui-${deviceId}-${random}`;
}

const dispatchVisible = ref(false);
const dispatchDevice = ref<DogDevice | null>(null);
const dispatchForm = reactive({
  taskName: "",
  routeId: undefined as number | undefined,
  planStart: "",
  planEnd: "",
  description: ""
});

const runtimeVisible = ref(false);
const runtimeLoading = ref(false);
const runtimeDevice = ref<DogDevice | null>(null);
const runtimeData = ref<any>(null);

const formVisible = ref(false);
const formTitle = ref("新增设备");
const form = reactive({
  id: null as number | null,
  deviceId: "",
  deviceName: "",
  model: "",
  status: "离线",
  firmware: "",
  deviceIp: "",
  parkCode: "",
  parkName: "",
  remarks: ""
});

const assignVisible = ref(false);
const assignUsers = ref<number[]>([]);
const assignMode = ref<"append" | "replace">("append");
const assignIds = ref<number[]>([]);

function resetQuery() {
  Object.assign(queryForm, {
    deviceId: "",
    deviceName: "",
    status: "",
    parkCode: ""
  });
  current.value = 1;
  void load();
}

async function load() {
  loading.value = true;
  try {
    const res = await dogDevicePage({
      current: current.value,
      size: pageSize.value,
      ...queryForm
    });
    if (res.code === SUCCESS) {
      dataList.value = res.data?.records ?? [];
      total.value = res.data?.total ?? 0;
    } else {
      message(res.msg || "查询失败", { type: "error" });
    }
  } finally {
    loading.value = false;
  }
}

async function loadUsers() {
  const res = await dogDeviceUsers();
  if (res.code === SUCCESS) {
    users.value = res.data ?? [];
  }
}

function openCreate() {
  formTitle.value = "新增设备";
  Object.assign(form, {
    id: null,
    deviceId: "",
    deviceName: "",
    model: "",
    status: "离线",
    firmware: "",
    deviceIp: "",
    parkCode: "",
    parkName: "",
    remarks: ""
  });
  formVisible.value = true;
}

function openEdit(row: DogDevice) {
  formTitle.value = "修改设备";
  Object.assign(form, {
    id: row.id,
    deviceId: row.deviceId,
    deviceName: row.deviceName ?? "",
    model: row.model ?? "",
    status: row.status,
    firmware: row.firmware ?? "",
    deviceIp: row.deviceIp ?? "",
    parkCode: row.parkCode ?? "",
    parkName: row.parkName ?? "",
    remarks: row.remarks ?? ""
  });
  formVisible.value = true;
}

async function submitForm() {
  if (!form.deviceId.trim()) {
    message("设备编号必填", { type: "error" });
    return;
  }
  const payload = {
    deviceId: form.deviceId.trim(),
    deviceName: form.deviceName || null,
    model: form.model || null,
    status: form.status || null,
    firmware: form.firmware || null,
    deviceIp: form.deviceIp || null,
    parkCode: form.parkCode || null,
    parkName: form.parkName || null,
    remarks: form.remarks || null
  };
  const res = form.id
    ? await dogDeviceUpdate(form.id, payload)
    : await dogDeviceSave(payload);
  if (res.code === SUCCESS) {
    message(form.id ? "修改成功" : "新增成功", { type: "success" });
    formVisible.value = false;
    void load();
  } else {
    message(res.msg || "保存失败", { type: "error" });
  }
}

async function removeDevice(row: DogDevice) {
  await ElMessageBox.confirm(
    `确定删除设备「${row.deviceName || row.deviceId}」吗？`,
    "删除设备",
    { type: "warning" }
  );
  const res = await dogDeviceDelete(row.id);
  if (res.code === SUCCESS) {
    message("删除成功", { type: "success" });
    void load();
  } else {
    message(res.msg || "删除失败", { type: "error" });
  }
}

function openAssign(rows: DogDevice[]) {
  if (!rows.length) {
    message("请先勾选设备", { type: "warning" });
    return;
  }
  assignIds.value = rows.map(row => row.id);
  assignUsers.value = [];
  assignMode.value = "append";
  assignVisible.value = true;
}

async function submitAssign() {
  if (!assignUsers.value.length) {
    message("请选择要分配的用户", { type: "warning" });
    return;
  }
  const res = await dogDeviceAssign(
    assignIds.value,
    assignUsers.value,
    assignMode.value
  );
  if (res.code === SUCCESS) {
    message(`已更新 ${assignIds.value.length} 台设备的用户分配`, {
      type: "success"
    });
    assignVisible.value = false;
    void load();
  } else {
    message(res.msg || "分配失败", { type: "error" });
  }
}

async function submitUnassign() {
  if (!assignUsers.value.length) {
    message("请选择要解除分配的用户", { type: "warning" });
    return;
  }
  const res = await dogDeviceUnassign(assignIds.value, assignUsers.value);
  if (res.code === SUCCESS) {
    message("已解除分配", { type: "success" });
    assignVisible.value = false;
    void load();
  } else {
    message(res.msg || "操作失败", { type: "error" });
  }
}

async function syncMaps(row: DogDevice) {
  try {
    await ElMessageBox.confirm(
      `将通知设备「${row.deviceName || row.deviceId}」用 4012 获取地图列表，` +
        `并把缺失地图同步到中台。是否继续？`,
      "同步设备地图",
      {
        type: "warning",
        confirmButtonText: "开始同步",
        cancelButtonText: "取消"
      }
    );
  } catch {
    return;
  }
  syncingId.value = row.id;
  try {
    const res = await dogDeviceSyncMaps(row.id);
    if (res.code === SUCCESS) {
      message("同步指令已下发，设备会上报同步结果", { type: "success" });
    } else {
      message(res.msg || "下发失败", { type: "error" });
    }
  } finally {
    syncingId.value = null;
  }
}

async function sendCommand(row: DogDevice, command: string) {
  try {
    await ElMessageBox.confirm(
      `将向设备「${row.deviceName || row.deviceId}」下发指令：${command}。是否继续？`,
      "下发指令",
      {
        type: "warning",
        confirmButtonText: "下发",
        cancelButtonText: "取消"
      }
    );
  } catch {
    return;
  }
  sendingId.value = row.id;
  try {
    const res = await dogDeviceCommand(
      row.id,
      command,
      newRequestId(row.deviceId)
    );
    if (res.code === SUCCESS) {
      message(`指令「${command}」已下发`, { type: "success" });
      void load();
    } else {
      message(res.msg || "下发失败", { type: "error" });
    }
  } finally {
    sendingId.value = null;
  }
}

function openDispatch(row: DogDevice) {
  dispatchDevice.value = row;
  dispatchForm.taskName = `${row.deviceName || row.deviceId} 巡检`;
  dispatchForm.routeId = undefined;
  dispatchForm.planStart = "";
  dispatchForm.planEnd = "";
  dispatchForm.description = "";
  dispatchVisible.value = true;
}

async function submitDispatch() {
  const device = dispatchDevice.value;
  if (!device) return;
  if (!dispatchForm.taskName.trim()) {
    message("任务名称必填", { type: "error" });
    return;
  }
  if (!dispatchForm.routeId) {
    message("请填写业务平台的路线 ID", { type: "error" });
    return;
  }
  dispatchingId.value = device.id;
  try {
    const res = await dogDeviceDispatchTask(device.id, {
      taskName: dispatchForm.taskName.trim(),
      routeId: dispatchForm.routeId,
      planStart: dispatchForm.planStart || null,
      planEnd: dispatchForm.planEnd || null,
      description: dispatchForm.description || null,
      requestId: newRequestId(device.deviceId)
    });
    if (res.code === SUCCESS) {
      message("任务已下发，设备上线后自动执行", { type: "success" });
      dispatchVisible.value = false;
      void load();
    } else {
      message(res.msg || "下发失败", { type: "error" });
    }
  } finally {
    dispatchingId.value = null;
  }
}

async function openRuntime(row: DogDevice) {
  runtimeDevice.value = row;
  runtimeData.value = null;
  runtimeVisible.value = true;
  runtimeLoading.value = true;
  try {
    const res = await dogDeviceRuntime(row.id);
    if (res.code === SUCCESS) {
      runtimeData.value = res.data ?? null;
    } else {
      message(res.msg || "读取运行态失败", { type: "error" });
    }
  } finally {
    runtimeLoading.value = false;
  }
}

function handleSelectionChange(rows: DogDevice[]) {
  selection.value = rows;
}

function onPageChange(page: number) {
  current.value = page;
  void load();
}

function onSizeChange(size: number) {
  pageSize.value = size;
  current.value = 1;
  void load();
}

onMounted(() => {
  void load();
  void loadUsers();
});
</script>

<template>
  <div class="main dog-device-page">
    <DogSearchForm
      v-model="queryForm"
      :fields="queryFields"
      :loading="loading"
      @search="load"
      @reset="resetQuery"
    />

    <el-card shadow="never" class="dog-card">
      <div class="toolbar">
        <div>
          <el-button type="primary" @click="openCreate">新增设备</el-button>
          <el-button
            type="success"
            plain
            :disabled="!selectedIds.length"
            @click="openAssign(selection)"
          >
            批量分配用户
          </el-button>
        </div>
        <div class="hint">
          设备由业务平台同步到中台；分配后，用户用同一个中台账号登录业务平台即可看到自己管辖的设备。
        </div>
      </div>

      <el-table
        v-loading="loading"
        :data="dataList"
        border
        stripe
        row-key="id"
        @selection-change="handleSelectionChange"
      >
        <el-table-column type="selection" width="48" />
        <el-table-column label="设备编号" prop="deviceId" min-width="150">
          <template #default="{ row }">
            <span class="mono">{{ row.deviceId }}</span>
          </template>
        </el-table-column>
        <el-table-column label="设备名称" prop="deviceName" min-width="140">
          <template #default="{ row }">
            {{ row.deviceName || "-" }}
          </template>
        </el-table-column>
        <el-table-column label="型号" prop="model" width="120" />
        <el-table-column label="状态" prop="status" width="110">
          <template #default="{ row }">
            <el-tag size="small">{{ row.status }}</el-tag>
          </template>
        </el-table-column>
        <el-table-column label="归属园区" min-width="160">
          <template #default="{ row }">
            <span v-if="row.parkName || row.parkCode">
              {{ row.parkName || row.parkCode }}
            </span>
            <span v-else class="text-muted">未关联</span>
          </template>
        </el-table-column>
        <el-table-column label="分配用户" min-width="200">
          <template #default="{ row }">
            <div v-if="row.users?.length" class="user-tags">
              <el-tag
                v-for="user in row.users"
                :key="user.id"
                size="small"
                type="info"
              >
                {{ user.realName || user.username }}
              </el-tag>
            </div>
            <span v-else class="text-muted">未分配</span>
          </template>
        </el-table-column>
        <el-table-column label="设备 IP" prop="deviceIp" width="140" />
        <el-table-column label="固件" prop="firmware" width="120" />
        <el-table-column label="创建时间" prop="createTime" width="180" />
        <el-table-column label="操作" fixed="right" width="420">
          <template #default="{ row }">
            <el-button link type="primary" @click="openAssign([row])">
              分配用户
            </el-button>
            <el-dropdown
              trigger="click"
              @command="(command: string) => sendCommand(row, command)"
            >
              <el-button link type="warning" :loading="sendingId === row.id">
                下发指令
              </el-button>
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item
                    v-for="command in DEVICE_COMMANDS"
                    :key="command"
                    :command="command"
                  >
                    {{ command }}
                  </el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
            <el-button link type="primary" @click="openDispatch(row)">
              下发任务
            </el-button>
            <el-button link type="info" @click="openRuntime(row)">
              运行态
            </el-button>
            <el-button
              link
              type="success"
              :loading="syncingId === row.id"
              @click="syncMaps(row)"
            >
              同步地图
            </el-button>
            <DogTableActions
              :row="row"
              @edit="openEdit(row)"
              @delete="removeDevice(row)"
            />
          </template>
        </el-table-column>
      </el-table>

      <div class="pager">
        <el-pagination
          background
          layout="total, sizes, prev, pager, next, jumper"
          :total="total"
          :current-page="current"
          :page-size="pageSize"
          :page-sizes="[10, 20, 50, 100]"
          @current-change="onPageChange"
          @size-change="onSizeChange"
        />
      </div>
    </el-card>

    <el-dialog v-model="formVisible" :title="formTitle" width="620px">
      <el-form label-width="100px">
        <el-form-item label="设备编号" required>
          <el-input
            v-model="form.deviceId"
            :disabled="form.id !== null"
            placeholder="与业务平台 deviceId 一致"
          />
        </el-form-item>
        <el-form-item label="设备名称">
          <el-input v-model="form.deviceName" />
        </el-form-item>
        <el-form-item label="型号">
          <el-input v-model="form.model" />
        </el-form-item>
        <el-form-item label="设备状态">
          <el-select v-model="form.status" style="width: 100%">
            <el-option
              v-for="status in DEVICE_STATUS"
              :key="status"
              :label="status"
              :value="status"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="设备 IP">
          <el-input v-model="form.deviceIp" />
        </el-form-item>
        <el-form-item label="固件版本">
          <el-input v-model="form.firmware" />
        </el-form-item>
        <el-form-item label="园区编码">
          <el-input
            v-model="form.parkCode"
            placeholder="业务库 parks.park_code"
          />
        </el-form-item>
        <el-form-item label="园区名称">
          <el-input v-model="form.parkName" />
        </el-form-item>
        <el-form-item label="备注">
          <el-input v-model="form.remarks" type="textarea" :rows="3" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="formVisible = false">取消</el-button>
        <el-button type="primary" @click="submitForm">保存</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="assignVisible" title="分配设备用户" width="560px">
      <div class="assign-summary">
        已选择 <b>{{ assignIds.length }}</b> 台设备
      </div>
      <el-form label-width="90px">
        <el-form-item label="分配方式">
          <el-radio-group v-model="assignMode">
            <el-radio value="append">追加（保留已有分配）</el-radio>
            <el-radio value="replace">覆盖（清空后重新分配）</el-radio>
          </el-radio-group>
        </el-form-item>
        <el-form-item label="选择用户">
          <el-select
            v-model="assignUsers"
            multiple
            filterable
            style="width: 100%"
            placeholder="选择中台账号"
          >
            <el-option
              v-for="user in users"
              :key="user.id"
              :label="`${user.realName || user.username}（${user.username}）`"
              :value="user.id"
            />
          </el-select>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="assignVisible = false">取消</el-button>
        <el-button type="danger" plain @click="submitUnassign">
          解除分配
        </el-button>
        <el-button type="primary" @click="submitAssign">确认分配</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="dispatchVisible" title="下发巡检任务" width="560px">
      <el-alert
        type="info"
        :closable="false"
        show-icon
        title="路线（含航点 / 航段 / 动作）由业务平台巡检路线维护，这里填写其路线 ID"
      />
      <el-form label-width="100px" style="margin-top: 16px">
        <el-form-item label="设备">
          <span>
            {{ dispatchDevice?.deviceName || dispatchDevice?.deviceId }}
            <span class="text-muted">（{{ dispatchDevice?.deviceId }}）</span>
          </span>
        </el-form-item>
        <el-form-item label="任务名称" required>
          <el-input
            v-model="dispatchForm.taskName"
            placeholder="例如：A 区夜间巡检"
          />
        </el-form-item>
        <el-form-item label="路线 ID" required>
          <el-input-number
            v-model="dispatchForm.routeId"
            :min="1"
            :controls="false"
            style="width: 160px"
          />
        </el-form-item>
        <el-form-item label="计划开始">
          <el-date-picker
            v-model="dispatchForm.planStart"
            type="datetime"
            value-format="YYYY-MM-DD HH:mm:ss"
            placeholder="留空表示立即"
            style="width: 100%"
          />
        </el-form-item>
        <el-form-item label="计划结束">
          <el-date-picker
            v-model="dispatchForm.planEnd"
            type="datetime"
            value-format="YYYY-MM-DD HH:mm:ss"
            placeholder="可选"
            style="width: 100%"
          />
        </el-form-item>
        <el-form-item label="备注">
          <el-input
            v-model="dispatchForm.description"
            type="textarea"
            :rows="2"
          />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dispatchVisible = false">取消</el-button>
        <el-button
          type="primary"
          :loading="dispatchingId === dispatchDevice?.id"
          @click="submitDispatch"
        >
          下发任务
        </el-button>
      </template>
    </el-dialog>

    <el-drawer
      v-model="runtimeVisible"
      size="480px"
      :title="`设备运行态 · ${runtimeDevice?.deviceName || runtimeDevice?.deviceId || ''}`"
    >
      <div v-loading="runtimeLoading">
        <el-descriptions v-if="runtimeData" :column="1" border>
          <el-descriptions-item label="在线">
            <el-tag
              :type="runtimeData.online ? 'success' : 'info'"
              size="small"
            >
              {{ runtimeData.online ? "已连接" : "未连接" }}
            </el-tag>
          </el-descriptions-item>
          <el-descriptions-item label="设备状态">
            {{ runtimeData.status }}
          </el-descriptions-item>
          <el-descriptions-item label="电量">
            {{ runtimeData.battery }}%
          </el-descriptions-item>
          <el-descriptions-item label="信号">
            {{ runtimeData.signal }}
          </el-descriptions-item>
          <el-descriptions-item label="园区">
            {{ runtimeData.parkName || "未绑定" }}
          </el-descriptions-item>
          <el-descriptions-item label="最近心跳">
            {{ runtimeData.lastHeartbeat || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="当前任务">
            {{ runtimeData.currentTaskName || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="任务进度">
            <template v-if="runtimeData.currentTask">
              {{ runtimeData.currentTask.taskName }} ·
              {{ runtimeData.currentTask.taskStatus }} ·
              {{ runtimeData.currentTask.progress }}%
            </template>
            <span v-else class="text-muted">无任务</span>
          </el-descriptions-item>
          <el-descriptions-item label="接管人">
            {{ runtimeData.teleopHolder || "-" }}
          </el-descriptions-item>
        </el-descriptions>
        <el-empty v-else-if="!runtimeLoading" description="暂无运行数据" />
      </div>
    </el-drawer>
  </div>
</template>

<style scoped lang="scss">
.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 14px;
  flex-wrap: wrap;
}

.hint {
  color: var(--el-text-color-secondary);
  font-size: 12px;
}

.mono {
  font-family: var(--el-font-family-mono, monospace);
}

.text-muted {
  color: var(--el-text-color-placeholder);
}

.user-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.pager {
  display: flex;
  justify-content: flex-end;
  margin-top: 16px;
}

.assign-summary {
  margin-bottom: 12px;
  color: var(--el-text-color-regular);
}
</style>
