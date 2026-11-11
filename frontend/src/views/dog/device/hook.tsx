import { computed, nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import { ElMessageBox } from "element-plus";
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
import type { DogSearchField } from "../components/search";

/** 设备状态取值，与业务平台 `devices.status` 对齐。 */
export const DEVICE_STATUS = [
  "离线",
  "在线待命",
  "执行任务",
  "待绑定",
  "故障",
  "维护中"
];

/** 状态徽标配色，与地图 / 模型页的 el-tag 用法保持一致。 */
const DEVICE_STATUS_TAG_TYPE: Record<
  string,
  "success" | "warning" | "info" | "danger" | "primary"
> = {
  在线待命: "success",
  执行任务: "primary",
  待绑定: "warning",
  维护中: "warning",
  故障: "danger",
  离线: "info"
};

/** 允许从中台发起的指令，与后端白名单、业务平台 `BUSINESS_COMMANDS` 对齐 */
export const DEVICE_COMMANDS = [
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

/** 表单默认值（新增 / 重置共用）。 */
function emptyForm() {
  return {
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
  };
}

export function useDogDevice() {
  const rules = reactive<FormRules>({
    deviceId: [{ required: true, message: "设备编号必填", trigger: "blur" }]
  });

  const columns: TableColumnList = [
    {
      type: "selection",
      width: 55,
      align: "left",
      fixed: "left",
      label: "勾选列"
    },
    { label: "序号", type: "index", fixed: "left", width: 70 },
    {
      label: "设备编号",
      prop: "deviceId",
      minWidth: 160,
      cellRenderer: ({ row }) => <span class="font-mono">{row.deviceId}</span>
    },
    {
      label: "设备名称",
      prop: "deviceName",
      minWidth: 140,
      cellRenderer: ({ row }) => row.deviceName || "-"
    },
    { label: "型号", prop: "model", width: 120 },
    {
      label: "状态",
      prop: "status",
      width: 110,
      cellRenderer: ({ row, props }) => (
        <el-tag size={props.size} type={DEVICE_STATUS_TAG_TYPE[row.status]}>
          {row.status}
        </el-tag>
      )
    },
    {
      label: "归属园区",
      prop: "parkName",
      minWidth: 160,
      cellRenderer: ({ row }) =>
        row.parkName || row.parkCode ? (
          <span>{row.parkName || row.parkCode}</span>
        ) : (
          <span class="text-xs text-gray-400">未关联</span>
        )
    },
    {
      label: "分配用户",
      prop: "users",
      minWidth: 200,
      cellRenderer: ({ row, props }) =>
        row.users?.length ? (
          <div class="flex flex-wrap gap-1">
            {row.users.map((user: DogDeviceUser) => (
              <el-tag key={user.id} size={props.size} type="info">
                {user.realName || user.username}
              </el-tag>
            ))}
          </div>
        ) : (
          <span class="text-xs text-gray-400">未分配</span>
        )
    },
    { label: "设备 IP", prop: "deviceIp", width: 140 },
    { label: "固件", prop: "firmware", width: 120 },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", width: 470, slot: "operation" }
  ];

  const queryForm = ref({
    deviceId: "",
    deviceName: "",
    status: "",
    parkCode: "",
    userId: undefined as number | undefined
  });
  const queryFields = computed<DogSearchField[]>(() => [
    { label: "设备编号", prop: "deviceId" },
    { label: "设备名称", prop: "deviceName" },
    {
      label: "设备状态",
      prop: "status",
      type: "select",
      options: DEVICE_STATUS.map(item => ({ label: item, value: item }))
    },
    { label: "园区编码", prop: "parkCode" }
  ]);
  /** 更多条件：只看分配给某个中台用户的设备（后端 `user_id`）。 */
  const moreQueryFields = computed<DogSearchField[]>(() => [
    {
      label: "分配用户",
      prop: "userId",
      type: "select",
      options: users.value.map(user => ({
        label: user.realName || user.username,
        value: user.id
      }))
    }
  ]);

  const dataList = ref<DogDevice[]>([]);
  const loading = ref(true);
  const pagination = reactive<PaginationProps>({
    total: 0,
    pageSize: 10,
    currentPage: 1,
    background: true
  });
  const selection = ref<DogDevice[]>([]);
  const selectedIds = computed(() => selection.value.map(item => item.id));

  const users = ref<DogDeviceUser[]>([]);
  const syncingId = ref<number | null>(null);
  const sendingId = ref<number | null>(null);
  const dispatchingId = ref<number | null>(null);

  const dialogFormVisible = ref(false);
  const title = ref("新增设备");
  const addForm = ref(emptyForm());

  const assignVisible = ref(false);
  const assignUsers = ref<number[]>([]);
  const assignMode = ref<"append" | "replace">("append");
  const assignIds = ref<number[]>([]);

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
  const runtimeData = ref<Record<string, any> | null>(null);

  async function onSearch() {
    loading.value = true;
    const page = { size: pagination.pageSize, current: pagination.currentPage };
    const { data } = await dogDevicePage({ ...page, ...queryForm.value });
    dataList.value = data.records ?? [];
    pagination.total = data.total ?? 0;
    setTimeout(() => {
      loading.value = false;
    }, 500);
  }

  async function loadUsers() {
    const res = await dogDeviceUsers();
    if (res.code === SUCCESS) {
      users.value = res.data ?? [];
    }
  }

  function handleSizeChange(val: number) {
    pagination.pageSize = val;
    onSearch();
  }

  function handleCurrentChange(val: number) {
    pagination.currentPage = val;
    onSearch();
  }

  function handleSelectionChange(rows: DogDevice[]) {
    selection.value = rows;
  }

  function openDia(param: string, row?: DogDevice) {
    title.value = param;
    addForm.value = row
      ? {
          id: row.id,
          deviceId: row.deviceId,
          deviceName: row.deviceName ?? "",
          model: row.model ?? "",
          status: row.status || "离线",
          firmware: row.firmware ?? "",
          deviceIp: row.deviceIp ?? "",
          parkCode: row.parkCode ?? "",
          parkName: row.parkName ?? "",
          remarks: row.remarks ?? ""
        }
      : emptyForm();
    dialogFormVisible.value = true;
  }

  function handleUpdate(row: DogDevice, formEl?: any) {
    openDia("修改设备", row);
    if (formEl) nextTick(() => formEl.clearValidate?.());
  }

  async function handleSubmit() {
    const form = addForm.value;
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
      message(form.id ? "修改成功！" : "保存成功！", { type: "success" });
      cancel();
    } else {
      message(res.msg || "保存失败", { type: "error" });
    }
  }

  function handleDelete(row: DogDevice) {
    dogDeviceDelete(row.id).then(res => {
      if (res.code === SUCCESS) {
        message("删除成功！", { type: "success" });
        onSearch();
      } else {
        message(res.msg, { type: "error" });
      }
    });
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
      onSearch();
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
      onSearch();
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
        onSearch();
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
        onSearch();
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

  /** 重置查询条件并回到第一页（查询栏「重置」与弹窗取消共用）。 */
  function cancel() {
    addForm.value = emptyForm();
    queryForm.value = {
      deviceId: "",
      deviceName: "",
      status: "",
      parkCode: "",
      userId: undefined
    };
    dialogFormVisible.value = false;
    pagination.currentPage = 1;
    onSearch();
  }

  onMounted(() => {
    onSearch();
    loadUsers();
  });

  return {
    rules,
    columns,
    queryForm,
    queryFields,
    moreQueryFields,
    dataList,
    loading,
    pagination,
    selection,
    selectedIds,
    users,
    syncingId,
    sendingId,
    dispatchingId,
    dialogFormVisible,
    title,
    addForm,
    assignVisible,
    assignUsers,
    assignMode,
    assignIds,
    dispatchVisible,
    dispatchDevice,
    dispatchForm,
    runtimeVisible,
    runtimeLoading,
    runtimeDevice,
    runtimeData,
    onSearch,
    handleSizeChange,
    handleCurrentChange,
    handleSelectionChange,
    openDia,
    handleUpdate,
    handleSubmit,
    handleDelete,
    openAssign,
    submitAssign,
    submitUnassign,
    syncMaps,
    sendCommand,
    openDispatch,
    submitDispatch,
    openRuntime,
    cancel
  };
}
