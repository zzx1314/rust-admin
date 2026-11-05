import { nextTick, onMounted, reactive, ref } from "vue";
import type { FormRules } from "element-plus";
import type { FieldValues } from "plus-pro-components";
import {
  dogServicesSave,
  dogServicesGetAll,
  dogServicesUpdate,
  dogServicesDelete
} from "@/api/dogServices";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogServices() {
  const rules = reactive<FormRules>({});
  const columns: TableColumnList = [
    {
      type: "selection",
      width: 55,
      align: "left",
      fixed: "left",
      label: "勾选列"
    },
    { label: "序号", type: "index", fixed: "left", width: 70 },
    { label: "类型", prop: "type", minWidth: 120 },
    { label: "参数", prop: "params", minWidth: 300 },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", width: 180, slot: "operation" }
  ];
  const dataList = ref([]);
  const loading = ref(true);
  const dialogFormVisible = ref(false);
  const title = ref("");
  const addForm = ref({ id: null });

  function handleUpdate(row, formEl) {
    addForm.value = JSON.parse(JSON.stringify(row));
    openDia("修改服务", formEl);
  }
  function handleDelete(row) {
    dogServicesDelete(row.id).then(res => {
      if (res.code === SUCCESS) {
        message("删除成功！", { type: "success" });
        onSearch();
      } else {
        message(res.msg, { type: "error" });
      }
    });
  }
  const handleSubmitError = (err: any) => console.log(err);
  const handleSubmit = (_values: FieldValues) => {
    if (addForm.value.id) {
      dogServicesUpdate(addForm.value.id, addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("修改成功！", { type: "success" });
          cancel();
        } else {
          message("修改失败！", { type: "error" });
        }
      });
    } else {
      dogServicesSave(addForm.value).then(res => {
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
    const { data } = await dogServicesGetAll();
    dataList.value = data;
    setTimeout(() => {
      loading.value = false;
    }, 500);
  }
  function cancel() {
    addForm.value = { id: null };
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
    dataList,
    loading,
    dialogFormVisible,
    title,
    addForm,
    rules,
    columns,
    handleUpdate,
    handleDelete,
    handleSubmit,
    handleSubmitError,
    cancel,
    openDia,
    onSearch
  };
}
