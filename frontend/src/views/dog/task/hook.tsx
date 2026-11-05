import { nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import type { FieldValues } from "plus-pro-components";
import {
  dogTaskSave,
  dogTaskPage,
  dogTaskUpdate,
  dogTaskDelete
} from "@/api/dogTask";
import { CREATE_TIME_FIELDS, type DogSearchField } from "../components/search";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogTask() {
  const rules = reactive<FormRules>({
    taskName: [{ required: true, message: "名称必填", trigger: "blur" }]
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
    { label: "任务描述", prop: "taskDescription", minWidth: 200 },
    { label: "任务状态", prop: "taskStatus", width: 100 },
    { label: "工作流ID", prop: "workflowId", width: 100 },
    { label: "设备ID", prop: "deviceId", width: 100 },
    { label: "路线ID", prop: "routeId", width: 100 },
    { label: "所属用户ID", prop: "userId", width: 120 },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", width: 180, slot: "operation" }
  ];

  const queryFields: DogSearchField[] = [
    { label: "任务名称", prop: "taskName" },
    { label: "任务状态", prop: "taskStatus" }
  ];
  const moreQueryFields: DogSearchField[] = [
    { label: "设备ID", prop: "deviceId", type: "number" },
    { label: "用户ID", prop: "userId", type: "number" },
    ...CREATE_TIME_FIELDS
  ];
  const queryForm = ref({
    taskName: "",
    taskStatus: "",
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
  const title = ref("");
  const addForm = ref({ id: null });

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
    const { data } = await dogTaskPage({ ...page, ...queryForm.value });
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
      deviceId: null,
      userId: null,
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
    queryFields,
    moreQueryFields,
    onSearch,
    handleUpdate,
    handleDelete,
    handleSizeChange,
    handleCurrentChange,
    handleSelectionChange,
    handleSubmit,
    handleSubmitError,
    cancel,
    openDia
  };
}
