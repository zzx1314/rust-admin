import { nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import type { FieldValues } from "plus-pro-components";
import {
  dogWorkflowSave,
  dogWorkflowUpdate,
  dogWorkflowDelete
} from "@/api/dogWorkflow";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogWorkflow() {
  const rules = reactive<FormRules>({
    uuid: [{ required: true, message: "UUID必填", trigger: "blur" }]
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
    { label: "UUID", prop: "uuid", minWidth: 200 },
    { label: "任务ID", prop: "taskId", width: 80 },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", width: 180, slot: "operation" }
  ];
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
    openDia("修改工作流", formEl);
  }
  function handleDelete(row) {
    dogWorkflowDelete(row.id).then(res => {
      if (res.code === SUCCESS) {
        message("删除成功！", { type: "success" });
      } else {
        message(res.msg, { type: "error" });
      }
    });
  }
  const handleSubmitError = (err: any) => console.log(err);
  const handleSubmit = (_values: FieldValues) => {
    if (addForm.value.id) {
      dogWorkflowUpdate(addForm.value.id, addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("修改成功！", { type: "success" });
          cancel();
        } else {
          message("修改失败！", { type: "error" });
        }
      });
    } else {
      dogWorkflowSave(addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("保存成功！", { type: "success" });
          cancel();
        } else {
          message(res.msg, { type: "error" });
        }
      });
    }
  };
  function cancel() {
    addForm.value = { id: null };
    dialogFormVisible.value = false;
  }
  function openDia(param, formEl) {
    dialogFormVisible.value = true;
    title.value = param;
    if (formEl) nextTick(() => formEl.formInstance?.clearValidate());
  }
  onMounted(() => {
    loading.value = false;
  });
  return {
    dataList,
    loading,
    dialogFormVisible,
    title,
    pagination,
    addForm,
    rules,
    columns,
    handleUpdate,
    handleDelete,
    handleSubmit,
    handleSubmitError,
    cancel,
    openDia
  };
}
