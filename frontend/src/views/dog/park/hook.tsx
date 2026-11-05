import { nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import type { FieldValues } from "plus-pro-components";
import {
  dogParkSave,
  dogParkPage,
  dogParkUpdate,
  dogParkDelete
} from "@/api/dogPark";
import { CREATE_TIME_FIELDS, type DogSearchField } from "../components/search";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogPark() {
  const rules = reactive<FormRules>({
    parkName: [{ required: true, message: "园区名称必填", trigger: "blur" }]
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
    { label: "园区名称", prop: "parkName", minWidth: 150 },
    { label: "园区描述", prop: "parkDescription", minWidth: 200 },
    { label: "园区地址", prop: "parkAddress", minWidth: 200 },
    {
      label: "所属用户",
      prop: "userName",
      minWidth: 140,
      formatter: row => row.userName || `用户 ${row.userId}`
    },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", width: 180, slot: "operation" }
  ];

  const queryFields: DogSearchField[] = [
    { label: "园区名称", prop: "parkName" }
  ];
  const moreQueryFields: DogSearchField[] = [...CREATE_TIME_FIELDS];
  const queryForm = ref({
    parkName: "",
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
    openDia("修改园区", formEl);
  }
  function handleDelete(row) {
    dogParkDelete(row.id).then(res => {
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
      dogParkUpdate(addForm.value.id, addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("修改成功！", { type: "success" });
          cancel();
        } else {
          message(res.msg, { type: "error" });
        }
      });
    } else {
      dogParkSave(addForm.value).then(res => {
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
    const { data } = await dogParkPage({ ...page, ...queryForm.value });
    dataList.value = data.records;
    pagination.total = data.total;
    setTimeout(() => {
      loading.value = false;
    }, 500);
  }

  function cancel() {
    addForm.value = { id: null };
    queryForm.value = { parkName: "", beginTime: "", endTime: "" };
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
