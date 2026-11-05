import { computed, nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import type { FieldValues } from "plus-pro-components";
import {
  dogMapSave,
  dogMapPage,
  dogMapUpdate,
  dogMapDelete
} from "@/api/dogMap";
import { dogParkGetAll } from "@/api/dogPark";
import { CREATE_TIME_FIELDS, type DogSearchField } from "../components/search";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogMap() {
  const rules = reactive<FormRules>({
    mapName: [{ required: true, message: "地图名称必填", trigger: "blur" }],
    parkId: [{ required: true, message: "所属园区ID必填", trigger: "blur" }]
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
    { label: "地图名称", prop: "mapName", minWidth: 150 },
    { label: "地图类型", prop: "mapType", width: 120 },
    { label: "所属园区ID", prop: "parkId", width: 120 },
    { label: "所属用户ID", prop: "userId", width: 120 },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", width: 180, slot: "operation" }
  ];

  const parkOptions = ref<Array<{ label: string; value: number }>>([]);
  const queryFields = computed<DogSearchField[]>(() => [
    { label: "地图名称", prop: "mapName" },
    {
      label: "所属园区",
      prop: "parkId",
      type: "select",
      options: parkOptions.value
    }
  ]);
  const moreQueryFields: DogSearchField[] = [...CREATE_TIME_FIELDS];
  const queryForm = ref({
    mapName: "",
    parkId: "",
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
    openDia("修改地图", formEl);
  }
  function handleDelete(row) {
    dogMapDelete(row.id).then(res => {
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
      dogMapUpdate(addForm.value.id, addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("修改成功！", { type: "success" });
          cancel();
        } else {
          message(res.msg, { type: "error" });
        }
      });
    } else {
      dogMapSave(addForm.value).then(res => {
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
    const { data } = await dogMapPage({ ...page, ...queryForm.value });
    dataList.value = data.records;
    pagination.total = data.total;
    setTimeout(() => {
      loading.value = false;
    }, 500);
  }

  function cancel() {
    addForm.value = { id: null };
    queryForm.value = {
      mapName: "",
      parkId: "",
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

  async function loadParkOptions() {
    const { data } = await dogParkGetAll();
    parkOptions.value = (data ?? []).map((park: any) => ({
      label: park.parkName,
      value: park.id
    }));
  }

  onMounted(() => {
    loadParkOptions();
    onSearch();
  });

  return {
    queryForm,
    queryFields,
    moreQueryFields,
    dataList,
    loading,
    dialogFormVisible,
    title,
    pagination,
    addForm,
    rules,
    columns,
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
