// vue
import { computed, nextTick, onMounted, reactive, ref } from "vue";
// types
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import type { FieldValues } from "plus-pro-components";
// api
import {
  chargingDeviceSave,
  chargingDevicePage,
  chargingDeviceUpdate,
  chargingDeviceDelete
} from "@/api/chargingDevice";
// utils
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useChargingDevice() {
  /** =================== 静态配置 =================== */
  const rules = reactive<FormRules>({
    name: [{ required: true, message: "名称必填", trigger: "blur" }]
  });
  const columns: TableColumnList = [
    {
      type: "selection",
      width: 55,
      align: "left",
      fixed: "left",
      label: "勾选列"
    },
    {
      label: "序号",
      type: "index",
      fixed: "left",
      width: 70
    },
    {
      label: "设备ID",
      prop: "deviceId",
      minWidth: 150
    },
    {
      label: "设备类型",
      prop: "deviceType",
      width: 100
    },
    {
      label: "设备状态",
      prop: "status",
      width: 100
    },
    {
      label: "IP地址",
      prop: "deviceIp",
      width: 180
    },
    {
      label: "设备型号",
      prop: "model",
      width: 100
    },
    {
      label: "创建时间",
      prop: "createTime",
      width: 180
    },
    {
      label: "最后通信时间",
      prop: "commTime",
      width: 180
    },
    {
      label: "操作",
      fixed: "right",
      width: 180,
      slot: "operation"
    }
  ];
  /** =================== State =================== */
  // 查询
  const queryForm = ref({
    name: "",
    beginTime: "",
    endTime: ""
  });
  const moreCondition = ref(false);
  // 表格
  const dataList = ref([]);
  const loading = ref(true);
  // 分页
  const pagination = reactive<PaginationProps>({
    total: 0,
    pageSize: 10,
    currentPage: 1,
    background: true
  });
  // 弹窗 & 表单
  const dialogFormVisible = ref(false);
  const title = ref("");
  const addForm = ref({
    id: null
  });
  /** =================== Computed =================== */
  const buttonClass = computed(() => {
    return [
      "!h-[20px]",
      "reset-margin",
      "!text-gray-500",
      "dark:!text-white",
      "dark:hover:!text-primary"
    ];
  });
  /** =================== 表单行为 =================== */
  function handleUpdate(row, formEl) {
    console.log(row);
    const data = JSON.stringify(row);
    addForm.value = JSON.parse(data);
    openDia("修改配置", formEl);
  }

  function handleDelete(row) {
    console.log(row);
    chargingDeviceDelete(row.id).then(res => {
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

  function handleSelectionChange(val) {
    console.log("handleSelectionChange", val);
  }

  const handleSubmitError = (err: any) => {
    console.log(err, "err");
  };

  const handleSubmit = (values: FieldValues) => {
    console.log(values, "Submit");
    if (addForm.value.id) {
      // 修改
      console.log("修改");
      chargingDeviceUpdate(addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("修改成功！", { type: "success" });
          cancel();
        } else {
          message("修改失败！", { type: "error" });
        }
      });
    } else {
      // 新增
      console.log("新增");
      chargingDeviceSave(addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("保存成功！", { type: "success" });
          cancel();
        } else {
          message(res.msg, { type: "error" });
        }
      });
    }
  };
  /** =================== 搜索 & 重置 =================== */
  async function onSearch() {
    loading.value = true;
    console.log("查询信息");
    const page = {
      size: pagination.pageSize,
      current: pagination.currentPage
    };
    const query = {
      ...page,
      ...queryForm.value
    };
    if (query.endTime) {
      query.endTime = query.endTime + " 23:59:59";
    }
    const { data } = await chargingDevicePage(query);
    dataList.value = data.records;
    pagination.total = data.total;
    setTimeout(() => {
      loading.value = false;
    }, 500);
  }

  const resetForm = formEl => {
    if (!formEl) return;
    nextTick(() => {
      formEl.formInstance.clearValidate();
      console.log("resetForm");
    });
  };

  const restartForm = formEl => {
    if (!formEl) return;
    formEl.resetFields();
    cancel();
  };
  /** =================== 弹窗行为 =================== */
  // 取消
  function cancel() {
    addForm.value = {
      id: null
    };
    queryForm.value.name = "";
    queryForm.value.beginTime = "";
    queryForm.value.endTime = "";
    dialogFormVisible.value = false;
    onSearch();
  }
  // 打开弹框
  function openDia(param, formEl) {
    dialogFormVisible.value = true;
    title.value = param;
    resetForm(formEl);
  }
  /** =================== 生命周期 =================== */
  onMounted(() => {
    onSearch();
  });

  return {
    // state
    queryForm,
    dataList,
    loading,
    dialogFormVisible,
    title,
    pagination,
    addForm,
    moreCondition,
    // config
    rules,
    columns,
    buttonClass,
    // actions
    onSearch,
    resetForm,
    handleUpdate,
    handleDelete,
    handleSizeChange,
    handleCurrentChange,
    handleSelectionChange,
    handleSubmit,
    handleSubmitError,
    cancel,
    restartForm,
    openDia
  };
}
