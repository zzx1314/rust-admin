import { nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import type { FieldValues } from "plus-pro-components";
import {
  dogRouteSave,
  dogRoutePage,
  dogRouteUpdate,
  dogRouteDelete
} from "@/api/dogRoute";
import { CREATE_TIME_FIELDS, type DogSearchField } from "../components/search";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogRoute() {
  const rules = reactive<FormRules>({
    routeName: [{ required: true, message: "名称必填", trigger: "blur" }],
    modelId: [{ required: true, message: "模型ID必填", trigger: "blur" }]
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
    { label: "路线名称", prop: "routeName", minWidth: 150 },
    { label: "路线类型", prop: "routeType", width: 100 },
    { label: "路线状态", prop: "routeStatus", width: 100 },
    { label: "模型ID", prop: "modelId", width: 80 },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", width: 180, slot: "operation" }
  ];
  const queryFields: DogSearchField[] = [
    { label: "路线名称", prop: "routeName" },
    { label: "路线类型", prop: "routeType" },
    { label: "状态", prop: "routeStatus" }
  ];
  const moreQueryFields: DogSearchField[] = [...CREATE_TIME_FIELDS];
  const queryForm = ref({
    routeName: "",
    routeType: "",
    routeStatus: "",
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
    openDia("修改路线", formEl);
  }
  function handleDelete(row) {
    dogRouteDelete(row.id).then(res => {
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
      dogRouteUpdate(addForm.value.id, addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("修改成功！", { type: "success" });
          cancel();
        } else {
          message("修改失败！", { type: "error" });
        }
      });
    } else {
      dogRouteSave(addForm.value).then(res => {
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
    const { data } = await dogRoutePage({
      size: pagination.pageSize,
      current: pagination.currentPage,
      ...queryForm.value
    });
    dataList.value = data.records;
    pagination.total = data.total;
    setTimeout(() => {
      loading.value = false;
    }, 500);
  }
  function cancel() {
    addForm.value = { id: null };
    queryForm.value = {
      routeName: "",
      routeType: "",
      routeStatus: "",
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
