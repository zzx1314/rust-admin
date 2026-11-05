import { computed, nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import { ElMessageBox, type FormInstance, type FormRules } from "element-plus";
import {
  dogRouteSave,
  dogRoutePage,
  dogRouteUpdate,
  dogRouteDelete
} from "@/api/dogRoute";
import { dogMapGetAll } from "@/api/dogMap";
import { CREATE_TIME_FIELDS, type DogSearchField } from "../components/search";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogRoute() {
  const rules = reactive<FormRules>({
    routeName: [{ required: true, message: "名称必填", trigger: "blur" }],
    routeType: [{ required: true, message: "路线类型必填", trigger: "change" }]
  });
  const switchLoadMap = ref({});
  const mapOptions = ref<Array<{ label: string; value: number }>>([]);

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
    {
      label: "路线类型",
      prop: "routeType",
      width: 100,
      cellRenderer: ({ row, props }) => (
        <el-tag
          size={props.size}
          type={row.routeType === "在线地图" ? "success" : "warning"}
        >
          {row.routeType}
        </el-tag>
      )
    },
    {
      label: "路线状态",
      prop: "routeStatus",
      width: 100,
      cellRenderer: scope => (
        <el-switch
          size={scope.props.size === "small" ? "small" : "default"}
          loading={switchLoadMap.value[scope.index]?.loading}
          v-model={scope.row.routeStatus}
          active-value={"启用"}
          inactive-value={"停用"}
          active-text="启用"
          inactive-text="停用"
          inline-prompt
          onChange={() => onChange(scope as any)}
        />
      )
    },
    { label: "路线描述", prop: "routeDescription", minWidth: 120 },
    { label: "路线数据", prop: "routeData", minWidth: 150 },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", width: 220, slot: "operation" }
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
  const addForm = ref({
    id: null,
    routeName: "",
    routeType: "",
    routeStatus: "停用",
    routeDescription: "",
    modelId: null,
    routeData: ""
  });

  const buttonClass = computed(() => {
    return [
      "!h-[20px]",
      "reset-margin",
      "!text-gray-500",
      "dark:!text-white",
      "dark:hover:!text-primary"
    ];
  });

  function onChange({ row, index }) {
    ElMessageBox.confirm(
      `确认要<strong>${
        row.routeStatus
      }</strong><strong style='color:var(--el-color-primary)'>${
        row.routeName
      }</strong>路线吗?`,
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
        switchLoadMap.value[index] = Object.assign(
          {},
          switchLoadMap.value[index],
          { loading: true }
        );
        const updateParam = {
          routeStatus: row.routeStatus
        };
        dogRouteUpdate(row.id, updateParam).then(res => {
          switchLoadMap.value[index] = Object.assign(
            {},
            switchLoadMap.value[index],
            { loading: false }
          );
          if (res.code === SUCCESS) {
            message("已成功修改路线状态", { type: "success" });
          } else {
            message(res.msg, { type: "error" });
            row.routeStatus = row.routeStatus === "启用" ? "停用" : "启用";
          }
        });
      })
      .catch(() => {
        row.routeStatus = row.routeStatus === "启用" ? "停用" : "启用";
      });
  }

  function handleUpdate(row, formEl) {
    if (row.routeStatus === "启用") {
      message("请先停用该路线！", { type: "warning" });
      return;
    }
    addForm.value = JSON.parse(JSON.stringify(row));
    openDia("修改路线", formEl);
  }

  function handleDelete(row) {
    if (row.routeStatus === "启用") {
      message("请先停用该路线！", { type: "warning" });
      return;
    }
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

  const handleSubmit = async (formEl: FormInstance | undefined) => {
    if (!formEl) return;
    await formEl.validate(valid => {
      if (!valid) return;
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
    });
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
    addForm.value = {
      id: null,
      routeName: "",
      routeType: "",
      routeStatus: "停用",
      routeDescription: "",
      modelId: null,
      routeData: ""
    };
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

  function loadMapOptions() {
    dogMapGetAll().then(res => {
      if (res.code === SUCCESS && res.data) {
        mapOptions.value = res.data.map(m => ({
          label: m.mapName || `地图${m.id}`,
          value: m.id
        }));
      }
    });
  }

  onMounted(() => {
    onSearch();
    loadMapOptions();
  });

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
    switchLoadMap,
    mapOptions,
    buttonClass,
    onSearch,
    handleUpdate,
    handleDelete,
    handleSizeChange,
    handleCurrentChange,
    handleSelectionChange,
    handleSubmit,
    cancel,
    openDia
  };
}
