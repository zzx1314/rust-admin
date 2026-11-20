import { nextTick, onMounted, reactive, ref } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import type { FormRules } from "element-plus";
import type { FieldValues } from "plus-pro-components";
import {
  dogMediaSave,
  dogMediaPage,
  dogMediaUpdate,
  dogMediaDelete,
  dogMediaDownload,
  dogMediaExport,
  dogMediaImage
} from "@/api/dogMedia";
import { CREATE_TIME_FIELDS, type DogSearchField } from "../components/search";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

/** 媒体来源：设备「同步媒体」回传 vs 手工新增 */
export const MEDIA_SOURCE_LABEL: Record<string, string> = {
  "ai-patrol": "AI巡检",
  manual: "手工上传"
};

export function useDogMedia() {
  const rules = reactive<FormRules>({
    mediaName: [{ required: true, message: "名称必填", trigger: "blur" }]
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
    { label: "媒体名称", prop: "mediaName", minWidth: 180 },
    { label: "媒体类型", prop: "mediaType", width: 90 },
    { label: "设备编号", prop: "deviceCode", width: 130 },
    { label: "巡检计划", prop: "planId", minWidth: 130 },
    {
      label: "航点",
      prop: "pointIndex",
      width: 80,
      formatter: row =>
        row.pointIndex === null || row.pointIndex === undefined
          ? "-"
          : row.pointIndex
    },
    {
      label: "来源",
      prop: "source",
      width: 110,
      formatter: row => MEDIA_SOURCE_LABEL[row.source] ?? row.source ?? "-"
    },
    { label: "采集时间", prop: "capturedAt", width: 170 },
    { label: "文件路径", prop: "mediaPath", minWidth: 220 },
    { label: "文件大小", prop: "mediaSize", width: 110 },
    { label: "备注", prop: "remarks", minWidth: 150 },
    { label: "创建时间", prop: "createTime", width: 170 },
    { label: "操作", fixed: "right", width: 220, slot: "operation" }
  ];

  const queryFields: DogSearchField[] = [
    { label: "媒体名称", prop: "mediaName" },
    { label: "媒体类型", prop: "mediaType" },
    { label: "设备编号", prop: "deviceCode" }
  ];
  const moreQueryFields: DogSearchField[] = [
    { label: "巡检计划", prop: "planId" },
    {
      label: "来源",
      prop: "source",
      type: "select",
      options: Object.entries(MEDIA_SOURCE_LABEL).map(([value, label]) => ({
        label,
        value
      }))
    },
    ...CREATE_TIME_FIELDS
  ];
  const queryForm = ref({
    mediaName: "",
    mediaType: "",
    deviceCode: "",
    planId: "",
    source: "",
    beginTime: "",
    endTime: ""
  });

  // 预览：单张走 /api/dogMedia/{id}/image（JSON + base64 data URL），
  // 列表里看一眼图糊没糊 / 框偏没偏；手工登记的历史记录没有落盘文件，接口 404。
  const previewVisible = ref(false);
  const previewLoading = ref(false);
  const previewImage = ref("");
  const previewTitle = ref("");
  async function handlePreview(row) {
    previewVisible.value = true;
    previewLoading.value = true;
    previewImage.value = "";
    previewTitle.value = `${row.mediaName}（${MEDIA_SOURCE_LABEL[row.source] ?? "手工上传"}）`;
    const { code, data, msg } = await dogMediaImage(row.id);
    if (code === SUCCESS && data?.image) {
      previewImage.value = data.image;
    } else {
      message(msg || "这张记录没有可预览的图片文件（可直接下载或走批量导出）", {
        type: "warning"
      });
      previewVisible.value = false;
    }
    previewLoading.value = false;
  }
  function closePreview() {
    previewVisible.value = false;
    previewImage.value = "";
  }

  // 这些图是训练数据：除了单张预览，还提供下载（单张 / 批量 zip）。
  // 单张走 /api/dogMedia/{id}/file；批量走 /api/dogMedia/export（勾选优先，
  // 否则按当前筛选条件），服务端打包成 zip（原图 + manifest.json）。
  const selectedIds = ref<number[]>([]);
  const exporting = ref(false);
  async function handleDownload(row) {
    try {
      await dogMediaDownload(row.id);
    } catch (error) {
      message(
        `下载失败：${(error as any)?.message ?? "该记录可能没有落盘文件"}`,
        { type: "error" }
      );
    }
  }
  async function handleExport() {
    if (exporting.value) return;
    exporting.value = true;
    try {
      const params =
        selectedIds.value.length > 0
          ? { ids: selectedIds.value.join(",") }
          : {
              mediaName: queryForm.value.mediaName,
              mediaType: queryForm.value.mediaType,
              deviceCode: queryForm.value.deviceCode,
              planId: queryForm.value.planId,
              source: queryForm.value.source,
              beginTime: queryForm.value.beginTime,
              endTime: queryForm.value.endTime
            };
      await dogMediaExport(params);
      message(
        selectedIds.value.length > 0
          ? `已导出选中的 ${selectedIds.value.length} 条`
          : "已按当前筛选条件导出（单次最多 200 张 / 100MB）",
        { type: "success" }
      );
    } catch (error) {
      message(`导出失败：${(error as any)?.message ?? "没有符合条件的文件"}`, {
        type: "error"
      });
    } finally {
      exporting.value = false;
    }
  }
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
    openDia("修改媒体", formEl);
  }
  function handleDelete(row) {
    dogMediaDelete(row.id).then(res => {
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
  function handleSelectionChange(rows) {
    selectedIds.value = (rows ?? []).map((row: any) => Number(row.id));
  }
  const handleSubmitError = (err: any) => {
    console.log(err);
  };
  const handleSubmit = (_values: FieldValues) => {
    if (addForm.value.id) {
      dogMediaUpdate(addForm.value.id, addForm.value).then(res => {
        if (res.code === SUCCESS) {
          message("修改成功！", { type: "success" });
          cancel();
        } else {
          message("修改失败！", { type: "error" });
        }
      });
    } else {
      dogMediaSave(addForm.value).then(res => {
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
    const { data } = await dogMediaPage({
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
      mediaName: "",
      mediaType: "",
      deviceCode: "",
      planId: "",
      source: "",
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
    openDia,
    selectedIds,
    exporting,
    handleDownload,
    handleExport,
    previewVisible,
    previewLoading,
    previewImage,
    previewTitle,
    handlePreview,
    closePreview
  };
}
