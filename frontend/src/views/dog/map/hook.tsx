import { nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type {
  FormRules,
  UploadFile,
  UploadRequestOptions,
  UploadUserFile
} from "element-plus";
import {
  dogMapPage,
  dogMapUpload,
  dogMapUpdate,
  dogMapDelete
} from "@/api/dogMap";
import {
  CREATE_TIME_FIELDS,
  MAP_TYPE_OPTIONS,
  type DogSearchField
} from "../components/search";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

export function useDogMap() {
  const rules = reactive<FormRules>({
    mapName: [{ required: true, message: "地图名称必填", trigger: "blur" }],
    mapType: [{ required: true, message: "地图类型必填", trigger: "change" }],
    remarks: [{ required: true, message: "地图描述必填", trigger: "blur" }]
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
    {
      label: "地图类型",
      prop: "mapType",
      width: 110,
      cellRenderer: ({ row, props }) =>
        row.mapType ? (
          <el-tag
            size={props.size}
            type={row.mapType === "在线地图" ? "success" : "warning"}
          >
            {row.mapType}
          </el-tag>
        ) : (
          <span>-</span>
        )
    },
    { label: "文件名称", prop: "fileName", minWidth: 180 },
    { label: "文件格式", prop: "fileFormat", width: 100 },
    { label: "文件大小（字节）", prop: "fileSize", width: 150 },
    { label: "描述信息", prop: "remarks", minWidth: 180 },
    { label: "创建时间", prop: "createTime", width: 180 },
    { label: "操作", fixed: "right", width: 180, slot: "operation" }
  ];

  const queryFields: DogSearchField[] = [
    { label: "地图名称", prop: "mapName" },
    {
      label: "地图类型",
      prop: "mapType",
      type: "select",
      options: MAP_TYPE_OPTIONS
    },
    { label: "文件名称", prop: "fileName" }
  ];
  const moreQueryFields: DogSearchField[] = [...CREATE_TIME_FIELDS];

  const queryForm = ref({
    mapName: "",
    mapType: "",
    fileName: "",
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
  const addForm = ref<{
    id: number | null;
    mapName: string;
    mapType: string;
    remarks: string;
  }>({
    id: null,
    mapName: "",
    mapType: "离线地图",
    remarks: ""
  });
  /** 待上传的地图包（新增 / 修改时选择的新文件）。 */
  const fileList = ref<UploadUserFile[]>([]);
  const uploadFileTemp = ref<UploadFile>(null);

  function handleUpdate(row, formEl) {
    addForm.value = {
      id: row.id,
      mapName: row.mapName ?? "",
      mapType: row.mapType ?? "",
      remarks: row.remarks ?? ""
    };
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

  /** 上传地图包并保存 / 更新地图记录。 */
  function onUpload(options: UploadRequestOptions) {
    const formData = new FormData();
    formData.append("file", options.file);
    formData.append(
      "mapInfo",
      JSON.stringify({
        id: addForm.value.id ?? undefined,
        mapName: addForm.value.mapName,
        mapType: addForm.value.mapType || undefined,
        remarks: addForm.value.remarks || undefined
      })
    );
    return dogMapUpload(formData)
      .then(res => {
        if (res.code === SUCCESS) {
          message(addForm.value.id ? "修改成功！" : "保存成功！", {
            type: "success"
          });
          options.onSuccess?.(res);
          cancel();
        } else {
          message(res.msg, { type: "error" });
          options.onError?.(new Error(res.msg) as any);
        }
      })
      .catch(error => {
        options.onError?.(error);
      });
  }

  /** 未更换地图包时，仅更新名称 / 类型 / 描述。 */
  const handleSubmit = (_values?: unknown) => {
    const id = addForm.value.id;
    if (!id) return;
    dogMapUpdate(id, {
      mapName: addForm.value.mapName,
      mapType: addForm.value.mapType,
      remarks: addForm.value.remarks
    }).then(res => {
      if (res.code === SUCCESS) {
        message("修改成功！", { type: "success" });
        cancel();
      } else {
        message(res.msg, { type: "error" });
      }
    });
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
    addForm.value = {
      id: null,
      mapName: "",
      mapType: "离线地图",
      remarks: ""
    };
    queryForm.value = {
      mapName: "",
      mapType: "",
      fileName: "",
      beginTime: "",
      endTime: ""
    };
    fileList.value = [];
    uploadFileTemp.value = null;
    dialogFormVisible.value = false;
    onSearch();
  }
  function openDia(param, formEl) {
    fileList.value = [];
    uploadFileTemp.value = null;
    dialogFormVisible.value = true;
    title.value = param;
    if (formEl) nextTick(() => formEl.clearValidate?.());
  }

  onMounted(() => onSearch());

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
    fileList,
    uploadFileTemp,
    onSearch,
    handleUpdate,
    handleDelete,
    onUpload,
    handleSizeChange,
    handleCurrentChange,
    handleSelectionChange,
    handleSubmit,
    handleSubmitError,
    cancel,
    openDia
  };
}
