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
import { dogGatewayAssign } from "@/api/dogGateway";
import { userPage } from "@/api/system";
import { isPlatformAdmin } from "@/utils/permission";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogDevice() {
  const rules = reactive<FormRules>({
    deviceId: [{ required: true, message: "设备ID必填", trigger: "blur" }]
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
    { label: "设备型号", prop: "model", width: 100 },
    { label: "速度", prop: "speed", width: 80 },
    { label: "高度", prop: "height", width: 80 },
    { label: "电量", prop: "battery", width: 80 },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "通信时间", prop: "commTime", width: 180 },
    { label: "操作", fixed: "right", width: 260, slot: "operation" }
  ];

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

  const assignVisible = ref(false);
  const assignLoading = ref(false);
  const assignForm = ref<{ deviceId: string; userId: number | null }>({
    deviceId: "",
    userId: null
  });
  const userOptions = ref<Array<{ label: string; value: number }>>([]);
  const canAssign = computed(() => isPlatformAdmin());

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

  function handleAssign(row) {
    assignForm.value = { deviceId: row.deviceId, userId: null };
    assignVisible.value = true;
    loadUserOptions();
  }

  function submitAssign() {
    if (!assignForm.value.userId) {
      message("请选择要分配的用户", { type: "warning" });
      return;
    }
    assignLoading.value = true;
    dogGatewayAssign({
      deviceId: assignForm.value.deviceId,
      userId: assignForm.value.userId
    })
      .then(res => {
        if (res.code === SUCCESS) {
          message("分配成功！", { type: "success" });
          assignVisible.value = false;
          onSearch();
        } else {
          message(res.msg, { type: "error" });
        }
      })
      .finally(() => {
        assignLoading.value = false;
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
    if (query.endTime) query.endTime = query.endTime + " 23:59:59";
    const { data } = await dogDevicePage(query);
    dataList.value = data.records;
    pagination.total = data.total;
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
    assignVisible,
    assignLoading,
    assignForm,
    userOptions,
    canAssign,
    onSearch,
    handleUpdate,
    handleDelete,
    handleAssign,
    submitAssign,
    handleSizeChange,
    handleCurrentChange,
    handleSelectionChange,
    handleSubmit,
    handleSubmitError,
    cancel,
    openDia
  };
}
