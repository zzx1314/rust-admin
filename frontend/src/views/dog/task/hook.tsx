import { computed, nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import { ElMessageBox } from "element-plus";
import type { FieldValues } from "plus-pro-components";
import {
  dogTaskSave,
  dogTaskPage,
  dogTaskUpdate,
  dogTaskDelete,
  dogTaskStart,
  dogTaskGetTaskStatus
} from "@/api/dogTask";
import { CREATE_TIME_FIELDS, type DogSearchField } from "../components/search";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";
import { useRouter } from "vue-router";

export function useDogTask() {
  const router = useRouter();

  const rules = reactive<FormRules>({
    taskName: [{ required: true, message: "名称必填", trigger: "blur" }],
    deviceId: [{ required: true, message: "设备信息必填", trigger: "blur" }],
    routeId: [{ required: true, message: "航线必填", trigger: "blur" }]
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
    { label: "任务名称", prop: "taskName", minWidth: 150 },
    { label: "设备ID", prop: "deviceIdString", width: 180 },
    { label: "航线", prop: "routeName", width: 100 },
    { label: "任务描述", prop: "taskDescription", minWidth: 180 },
    {
      label: "任务状态",
      prop: "taskStatus",
      cellRenderer: ({ row, props }) => (
        <el-tag
          size={props.size}
          type={
            row.taskStatus === "已完成"
              ? "success"
              : row.taskStatus === "失败"
                ? "danger"
                : "warning"
          }
        >
          {row.taskStatus}
        </el-tag>
      ),
      width: 100
    },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", minWidth: 180, slot: "operation" }
  ];

  const queryFields: DogSearchField[] = [
    { label: "任务名称", prop: "taskName" },
    { label: "任务状态", prop: "taskStatus" }
  ];
  const moreQueryFields: DogSearchField[] = [
    { label: "设备ID", prop: "deviceIdString" },
    { label: "航线名称", prop: "routeName" },
    ...CREATE_TIME_FIELDS
  ];

  const queryForm = ref({
    taskName: "",
    taskStatus: "",
    deviceIdString: "",
    routeName: "",
    deviceId: null,
    userId: null,
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
  const dialogStatusVisible = ref(false);
  const dialogStatusData = ref({ successIds: [], failIds: [], errorInfoMap: {} });
  const title = ref("");
  const addForm = ref({ id: null });

  const buttonClass = computed(() => [
    "!h-[20px]",
    "reset-margin",
    "!text-gray-500",
    "dark:!text-white",
    "dark:hover:!text-primary"
  ]);

  function handleUpdate(row, formEl) {
    addForm.value = JSON.parse(JSON.stringify(row));
    openDia("修改任务", formEl);
  }

  function handleDelete(row) {
    dogTaskDelete(row.id).then(res => {
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

  const handleSubmit = (_values: FieldValues) => {
    if (addForm.value.id) {
      dogTaskUpdate(addForm.value.id, addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("修改成功！", { type: "success" });
          cancel();
        } else {
          message("修改失败！", { type: "error" });
        }
      });
    } else {
      dogTaskSave(addForm.value).then(res => {
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
    if (query.endTime) query.endTime += " 23:59:59";
    const { data } = await dogTaskPage(query);
    dataList.value = data.records;
    pagination.total = data.total;
    setTimeout(() => {
      loading.value = false;
    }, 500);
  }

  function cancel() {
    addForm.value = { id: null };
    queryForm.value = {
      taskName: "",
      taskStatus: "",
      deviceIdString: "",
      routeName: "",
      deviceId: null,
      userId: null,
      beginTime: "",
      endTime: ""
    };
    dialogFormVisible.value = false;
    dialogStatusVisible.value = false;
    onSearch();
  }

  function openDia(param, formEl) {
    dialogFormVisible.value = true;
    title.value = param;
    if (formEl) nextTick(() => formEl.formInstance?.clearValidate());
  }

  // 流程设计 - navigate to flow design page
  function flowRoute(row) {
    const workflowUuid = row.workflowUuid || `${Date.now()}_${row.id}`;
    router.push({
      name: "dronesFlowDesign",
      query: {
        taskId: row.id,
        deviceId: row.deviceId,
        taskType: "taskDesign",
        name: row.taskName,
        routeId: row.routeId,
        workflowId: workflowUuid
      }
    });
  }

  // 执行状态 - navigate to task status view or show dialog
  function flowRouteStatus(row) {
    dogTaskGetTaskStatus(row.id).then(res => {
      if (res.code === SUCCESS && res.data) {
        dialogStatusData.value = res.data;
        dialogStatusVisible.value = true;
        title.value = `${row.taskName} - 执行状态`;
      } else {
        message(res.msg || "暂无执行状态", { type: "info" });
      }
    });
  }

  // 启动流程
  function startTask(row) {
    if (row.taskStatus === "进行中") {
      message("任务正在进行中，请勿重复启动", { type: "warning" });
      return;
    }
    ElMessageBox.confirm(
      `确定要<strong>启动</strong><strong style='color:var(--el-color-primary)'>${row.taskName}</strong>任务吗?`,
      "系统提示",
      {
        confirmButtonText: "确定",
        cancelButtonText: "取消",
        type: "warning",
        dangerouslyUseHTMLString: true,
        draggable: true
      }
    )
      .then(() => {
        dogTaskStart(row.id).then(res => {
          if (res.code === SUCCESS) {
            message("任务启动成功", { type: "success" });
            onSearch();
          } else {
            message(res.msg || "启动失败", { type: "error" });
          }
        });
      })
      .catch(() => {
        message("已取消", { type: "info" });
      });
  }

  // 数据看板
  function datadashBoard(row) {
    window.open(
      `/#/datadashboard?type=datadash&taskId=${row.id}&deviceId=${row.deviceIdString || ""}&deviceIdNum=${row.deviceId}`,
      "_blank"
    );
  }

  onMounted(() => onSearch());

  return {
    queryForm,
    dataList,
    loading,
    dialogFormVisible,
    dialogStatusVisible,
    dialogStatusData,
    title,
    pagination,
    addForm,
    rules,
    columns,
    queryFields,
    moreQueryFields,
    buttonClass,
    onSearch,
    handleUpdate,
    handleDelete,
    handleSizeChange,
    handleCurrentChange,
    handleSelectionChange,
    handleSubmit,
    handleSubmitError,
    cancel,
    openDia,
    flowRoute,
    flowRouteStatus,
    startTask,
    datadashBoard
  };
}
