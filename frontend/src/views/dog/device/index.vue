<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import {
  dogDeviceAssign,
  dogDeviceDelete,
  dogDevicePage,
  dogDeviceSave,
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
        <el-table-column label="操作" fixed="right" width="240">
          <template #default="{ row }">
            <el-button link type="primary" @click="openAssign([row])">
              分配用户
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
