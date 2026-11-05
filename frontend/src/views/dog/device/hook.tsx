import { computed, nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import type { FieldValues } from "plus-pro-components";
import {
  dogDeviceSave,
  dogDevicePage,
  dogDeviceUpdate,
  dogDeviceDelete
} from "@/api/dogDevice";
import {
  dogGatewayAssign,
  dogGatewayAssignees,
  dogGatewayUnassign
} from "@/api/dogGateway";
import { dogUserDeviceGetAll } from "@/api/dogUserDevice";
import { userPage } from "@/api/system";
import { isPlatformAdmin } from "@/utils/permission";
import { useUserStoreHook } from "@/store/modules/user";
import {
  CREATE_TIME_FIELDS,
  DEVICE_STATUS_OPTIONS,
  type DogSearchField
} from "../components/search";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogDevice() {
  const rules = reactive<FormRules>({
    deviceId: [{ required: true, message: "设备ID必填", trigger: "blur" }]
  });
  /** 设备主键 → 已绑定用户展示名。 */
  const boundUserNames = ref<Record<number, string[]>>({});
  const columns: TableColumnList = [
    {
      type: "selection",
      width: 55,
      align: "left",
      fixed: "left",
      label: "勾选列"
    },
    { label: "序号", type: "index", fixed: "left", width: 70 },
    { label: "设备ID", prop: "deviceId", minWidth: 180 },
    { label: "设备类型", prop: "deviceType", width: 100 },
    {
      label: "设备状态",
      prop: "status",
      width: 100,
      cellRenderer: ({ row, props }) => (
        <el-tag
          size={props.size}
          type={row.status === "online" ? "success" : "warning"}
        >
          {row.status === "online" ? "在线" : row.status}
        </el-tag>
      )
    },
    { label: "IP地址", prop: "deviceIp", width: 150 },
    {
      label: "绑定用户",
      prop: "boundUsers",
      minWidth: 160,
      cellRenderer: ({ row, props }) => {
        const names = boundUserNames.value[row.id] ?? [];
        if (!names.length) {
          return (
            <el-tag size={props.size} type="info">
              未绑定
            </el-tag>
          );
        }
        return (
          <div class="flex flex-wrap justify-center gap-1">
            {names.map(name => (
              <el-tag key={name} size={props.size}>
                {name}
              </el-tag>
            ))}
          </div>
        );
      }
    },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "通信时间", prop: "commTime", width: 180 },
    { label: "操作", fixed: "right", width: 260, slot: "operation" }
  ];

  const queryFields: DogSearchField[] = [
    { label: "设备ID", prop: "deviceId" },
    { label: "设备类型", prop: "deviceType" },
    {
      label: "状态",
      prop: "status",
      type: "select",
      options: DEVICE_STATUS_OPTIONS
    }
  ];
  const moreQueryFields: DogSearchField[] = [...CREATE_TIME_FIELDS];
  const queryForm = ref({
    deviceId: "",
    deviceType: "",
    status: "",
    beginTime: "",
    endTime: ""
  });
  const dataList = ref([]);
  const loading = ref(true);
  const pagination = reactive<PaginationProps>({
    total: 0,
    pageSize: 10,
    currentPage: 1,
    background: true
  });
  const dialogFormVisible = ref(false);
  const title = ref("");
  const addForm = ref({ id: null });

  /** 管理员可在任意设备上维护绑定用户。 */
  const canManageUsers = computed(() => isPlatformAdmin());
  const bindVisible = ref(false);
  const bindLoading = ref(false);
  const bindingsLoading = ref(false);
  const bindDevice = ref<{ deviceId: string }>({ deviceId: "" });
  const bindings = ref<Array<{ userId: number; userName: string }>>([]);
  const bindUserId = ref<number | null>(null);
  const userOptions = ref<Array<{ label: string; value: number }>>([]);
  const currentUsername = computed(
    () => useUserStoreHook().username || "当前账号"
  );

  const buttonClass = computed(() => [
    "!h-[20px]",
    "reset-margin",
    "!text-gray-500",
    "dark:!text-white",
    "dark:hover:!text-primary"
  ]);

  function handleUpdate(row, formEl) {
    addForm.value = JSON.parse(JSON.stringify(row));
    openDia("修改设备", formEl);
  }
  function handleDelete(row) {
    dogDeviceDelete(row.id).then(res => {
      if (res.code === SUCCESS) {
        message("删除成功！", { type: "success" });
        onSearch();
      } else {
        message(res.msg, { type: "error" });
      }
    });
  }
  function handleSizeChange(val: number) {
    pagination.pageSize = val;
    onSearch();
  }
  function handleCurrentChange(val: number) {
    pagination.currentPage = val;
    onSearch();
  }
  function handleSelectionChange(_val) {}
  const handleSubmitError = (err: any) => console.log(err);

  async function loadUserOptions() {
    if (userOptions.value.length) return;
    const { data } = await userPage({ current: 1, size: 1000 });
    userOptions.value = (data?.records ?? []).map((user: any) => ({
      label: user.realName
        ? `${user.realName}（${user.username}）`
        : user.username,
      value: user.id
    }));
  }

  function userNameOf(userId: number) {
    const option = userOptions.value.find(item => item.value === userId);
    return option?.label ?? `用户 ${userId}`;
  }

  /** 拉取当前可见的全部绑定关系，用于表格「绑定用户」列。 */
  async function loadBoundUsers() {
    try {
      const { data } = await dogUserDeviceGetAll();
      if (canManageUsers.value) await loadUserOptions();
      const names: Record<number, string[]> = {};
      (data ?? []).forEach((item: any) => {
        const label = canManageUsers.value
          ? userNameOf(item.userId)
          : currentUsername.value;
        const list = names[item.deviceId] ?? [];
        if (!list.includes(label)) list.push(label);
        names[item.deviceId] = list;
      });
      boundUserNames.value = names;
    } catch {
      boundUserNames.value = {};
    }
  }

  /** 单个设备的绑定明细（对话框内使用）。 */
  async function loadBindings(deviceId: string) {
    bindingsLoading.value = true;
    try {
      const { data } = await dogGatewayAssignees(deviceId);
      await loadUserOptions();
      bindings.value = (data ?? []).map((item: any) => ({
        userId: item.userId,
        userName: userNameOf(item.userId)
      }));
    } catch {
      bindings.value = [];
    } finally {
      bindingsLoading.value = false;
    }
  }

  function handleBind(row) {
    if (!canManageUsers.value) return;
    bindDevice.value = { deviceId: row.deviceId };
    bindUserId.value = null;
    bindings.value = [];
    bindVisible.value = true;
    loadBindings(row.deviceId);
  }

  async function refreshBindings() {
    await Promise.all([
      loadBindings(bindDevice.value.deviceId),
      loadBoundUsers()
    ]);
  }

  async function submitBind() {
    if (!bindUserId.value) {
      message("请选择要绑定的用户", { type: "warning" });
      return;
    }
    bindLoading.value = true;
    try {
      const res = await dogGatewayAssign({
        deviceId: bindDevice.value.deviceId,
        userId: bindUserId.value
      });
      if (res.code === SUCCESS) {
        message("绑定成功！", { type: "success" });
        bindUserId.value = null;
        await refreshBindings();
      } else {
        message(res.msg, { type: "error" });
      }
    } finally {
      bindLoading.value = false;
    }
  }

  async function removeBinding(binding: { userId: number }) {
    const res = await dogGatewayUnassign(
      bindDevice.value.deviceId,
      binding.userId
    );
    if (res.code === SUCCESS) {
      message("解绑成功！", { type: "success" });
      await refreshBindings();
    } else {
      message(res.msg, { type: "error" });
    }
  }

  /** 普通用户在设备列表里解绑自己（设备列表只返回已绑定设备）。 */
  function handleUnbindSelf(row) {
    dogGatewayUnassign(row.deviceId).then(res => {
      if (res.code === SUCCESS) {
        message("解绑成功！", { type: "success" });
        onSearch();
      } else {
        message(res.msg, { type: "error" });
      }
    });
  }

  const handleSubmit = (_values: FieldValues) => {
    if (addForm.value.id) {
      dogDeviceUpdate(addForm.value.id, addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("修改成功！", { type: "success" });
          cancel();
        } else {
          message("修改失败！", { type: "error" });
        }
      });
    } else {
      dogDeviceSave(addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("保存成功！", { type: "success" });
          cancel();
        } else {
          message(res.msg, { type: "error" });
        }
      });
    }
  };

  async function onSearch() {
    loading.value = true;
    const page = { size: pagination.pageSize, current: pagination.currentPage };
    const query = { ...page, ...queryForm.value };
    const { data } = await dogDevicePage(query);
    dataList.value = data.records;
    pagination.total = data.total;
    loadBoundUsers();
    setTimeout(() => {
      loading.value = false;
    }, 500);
  }

  function cancel() {
    addForm.value = { id: null };
    queryForm.value = {
      deviceId: "",
      deviceType: "",
      status: "",
      beginTime: "",
      endTime: ""
    };
    dialogFormVisible.value = false;
    onSearch();
  }
  function openDia(param, formEl) {
    dialogFormVisible.value = true;
    title.value = param;
    if (formEl) nextTick(() => formEl.formInstance?.clearValidate());
  }

  onMounted(() => onSearch());

  return {
    queryForm,
    dataList,
    loading,
    dialogFormVisible,
    title,
    pagination,
    addForm,
    rules,
    columns,
    buttonClass,
    queryFields,
    moreQueryFields,
    canManageUsers,
    boundUserNames,
    bindVisible,
    bindLoading,
    bindingsLoading,
    bindDevice,
    bindings,
    bindUserId,
    userOptions,
    onSearch,
    handleUpdate,
    handleDelete,
    handleBind,
    submitBind,
    removeBinding,
    handleUnbindSelf,
    handleSizeChange,
    handleCurrentChange,
    handleSelectionChange,
    handleSubmit,
    handleSubmitError,
    cancel,
    openDia
  };
}
