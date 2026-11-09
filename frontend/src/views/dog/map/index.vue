<script setup lang="ts">
import { ref } from "vue";
import { ElMessageBox } from "element-plus";
import {
  genFileId,
  type FormInstance,
  type UploadFile,
  type UploadInstance,
  type UploadProps,
  type UploadRawFile
} from "element-plus";
import { useDogMap } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import EditPen from "~icons/ep/edit-pen";
import Delete from "~icons/ep/delete";
import AddFill from "~icons/ri/add-circle-line";
import UploadFileIcon from "~icons/ep/upload-filled";
import { PureTableBar } from "@/components/RePureTableBar";
import PureTable from "@pureadmin/table";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";
import {
  MAP_STATUS_OPTIONS,
  MAP_STATUS_TAG_TYPE,
  MAP_TYPE_OPTIONS
} from "../components/search";
import { message } from "@/utils/message";

defineOptions({ name: "DogMap" });

const addFormRef = ref<FormInstance>();
const uploadRef = ref<UploadInstance>();
const {
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
  parkOptions,
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
  cancel,
  openDia,
  publishMap,
  newVersionMap,
  loadRefs
} = useDogMap();

const handleExceed: UploadProps["onExceed"] = files => {
  uploadRef.value?.clearFiles();
  const file = files[0] as UploadRawFile;
  file.uid = genFileId();
  uploadRef.value?.handleStart(file);
};

const beforeUpload = (uploadFile: UploadFile) => {
  if (uploadFile.name.toLowerCase().endsWith(".zip")) {
    uploadFileTemp.value = uploadFile;
  } else {
    uploadRef.value?.clearFiles();
    uploadFileTemp.value = null;
    message("请选择正确的安装包格式：.zip", { type: "error" });
  }
};

const refsVisible = ref(false);
const statusFormOptions = MAP_STATUS_OPTIONS.filter(
  item => item.value === "草稿" || item.value === "部分标定"
);
const refsTitle = ref("");
const refsMap = ref<Record<string, any>>({});
const refsData = ref<{ routes: any[]; taskCount: number }>({
  routes: [],
  taskCount: 0
});

const openRefs = async (row, mode: "detail" | "impact") => {
  refsMap.value = row || {};
  const data = await loadRefs(row);
  refsData.value = data || { routes: [], taskCount: 0 };
  refsTitle.value =
    mode === "detail"
      ? `地图基线详情 · ${row.mapVersion || row.mapName}`
      : `版本变更影响分析 · ${row.mapVersion || row.mapName}`;
  refsVisible.value = true;
};

const handlePublish = row => {
  ElMessageBox.confirm(
    `发布后将归档同园区旧版本，并把园区当前基线指向 ${row.mapVersion || row.mapName}。是否继续？`,
    "发布地图基线",
    { type: "warning", confirmButtonText: "发布", cancelButtonText: "取消" }
  )
    .then(() => publishMap(row))
    .catch(() => {});
};

const handleNewVersion = row => {
  ElMessageBox.confirm(
    `将基于 ${row.mapVersion || row.mapName} 派生下一个版本并立即发布。历史路线仍冻结在旧版本，需要人工确认迁移。是否继续？`,
    "发布新版本",
    { type: "warning", confirmButtonText: "发布", cancelButtonText: "取消" }
  )
    .then(() => newVersionMap(row))
    .catch(() => {});
};

const submitForm = async (formEl?: FormInstance) => {
  if (!formEl) return;
  await formEl.validate(valid => {
    if (!valid) return;
    // 新增必须上传地图包；修改时选了新包则重新上传，否则只更新基础信息
    if (uploadFileTemp.value) {
      uploadRef.value?.submit();
      return;
    }
    if (!addForm.value.id) {
      message("请上传地图包文件", { type: "error" });
      return;
    }
    handleSubmit();
  });
};
</script>
<template>
  <div class="main">
    <DogSearchForm
      v-model="queryForm"
      :fields="queryFields"
      :more-fields="moreQueryFields"
      :loading="loading"
      @search="onSearch"
      @reset="cancel"
    />
    <PureTableBar title="地图列表" :columns="columns" @refresh="onSearch">
      <template #buttons>
        <el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="openDia('新增地图', addFormRef)"
          >新增</el-button
        >
      </template>
      <template v-slot="{ size, checkList, dynamicColumns }">
        <pure-table
          border
          adaptive
          align-whole="center"
          showOverflowTooltip
          table-layout="auto"
          :loading="loading"
          :size="size"
          :data="dataList"
          :columns="dynamicColumns"
          :checkList="checkList"
          :pagination="pagination"
          :paginationSmall="size === 'small'"
          :header-cell-style="{
            background: 'var(--el-table-row-hover-bg-color)',
            color: 'var(--el-text-color-primary)'
          }"
          @selection-change="handleSelectionChange"
          @page-size-change="handleSizeChange"
          @page-current-change="handleCurrentChange"
        >
          <template #operation="{ row }">
            <el-button
              v-if="row.mapStatus === '草稿' || row.mapStatus === '部分标定'"
              class="reset-margin"
              link
              type="primary"
              :size="size"
              @click="handlePublish(row)"
              >发布</el-button
            >
            <el-button
              v-if="row.mapStatus !== '草稿'"
              class="reset-margin"
              link
              type="primary"
              :size="size"
              @click="handleNewVersion(row)"
              >新版本</el-button
            >
            <el-button
              class="reset-margin"
              link
              type="primary"
              :size="size"
              @click="openRefs(row, 'detail')"
              >基线详情</el-button
            >
            <el-button
              class="reset-margin"
              link
              type="primary"
              :size="size"
              :icon="useRenderIcon(EditPen)"
              @click="handleUpdate(row, addFormRef)"
              >修改</el-button
            >
            <el-popconfirm title="是否确认删除?" @confirm="handleDelete(row)">
              <template #reference>
                <el-button
                  class="reset-margin"
                  link
                  type="primary"
                  :size="size"
                  :icon="useRenderIcon(Delete)"
                  >删除</el-button
                >
              </template>
            </el-popconfirm>
          </template>
        </pure-table>
      </template>
    </PureTableBar>
    <el-dialog
      v-model="dialogFormVisible"
      :title="title"
      width="800px"
      @close="cancel"
    >
      <el-form
        ref="addFormRef"
        :model="addForm"
        :inline="true"
        :rules="rules"
        label-width="150px"
      >
        <el-form-item label="所属园区" prop="parkId" style="width: 100%">
          <el-select
            v-model="addForm.parkId"
            placeholder="请选择所属园区"
            filterable
            style="width: 100%"
          >
            <el-option
              v-for="item in parkOptions"
              :key="item.value"
              :label="item.label"
              :value="item.value"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="地图名称" prop="mapName" style="width: 100%">
          <el-input v-model="addForm.mapName" placeholder="请输入地图名称" />
        </el-form-item>
        <el-form-item label="地图类型" prop="mapType" style="width: 100%">
          <el-select v-model="addForm.mapType" placeholder="请选择地图类型">
            <el-option
              v-for="item in MAP_TYPE_OPTIONS"
              :key="item.value"
              :label="item.label"
              :value="item.value"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="基线版本" prop="mapVersion" style="width: 100%">
          <el-input v-model="addForm.mapVersion" placeholder="例如：v1.0" />
        </el-form-item>
        <el-form-item
          label="坐标系 frame_id"
          prop="frameId"
          style="width: 100%"
        >
          <el-input
            v-model="addForm.frameId"
            placeholder="例如：zj_map / 局部米制"
          />
        </el-form-item>
        <el-form-item label="CRS" prop="crs" style="width: 100%">
          <el-input
            v-model="addForm.crs"
            placeholder="例如：GCJ-02 锚点 + 局部 ENU"
          />
        </el-form-item>
        <el-form-item
          label="原点锚点（包解析）"
          prop="mapOrigin"
          style="width: 100%"
        >
          <el-input
            v-model="addForm.mapOrigin"
            placeholder="例如：31.2014, 121.6023"
          />
        </el-form-item>
        <el-form-item
          label="分辨率（包解析）"
          prop="resolution"
          style="width: 100%"
        >
          <el-input
            v-model="addForm.resolution"
            placeholder="例如：0.05 m/px"
          />
        </el-form-item>
        <el-form-item label="覆盖楼层" prop="floors" style="width: 100%">
          <el-input v-model="addForm.floors" placeholder="例如：1F / 2F / B1" />
        </el-form-item>
        <el-form-item label="语义图层" style="width: 100%">
          <el-input-number
            v-model="addForm.roadCount"
            :min="0"
            placeholder="道路数"
            style="width: 180px"
          />
          <el-input-number
            v-model="addForm.nogoCount"
            :min="0"
            placeholder="禁行区数"
            style="width: 180px; margin-left: 12px"
          />
        </el-form-item>
        <el-form-item label="基线状态" prop="mapStatus" style="width: 100%">
          <el-select v-model="addForm.mapStatus" placeholder="请选择基线状态">
            <el-option
              v-for="item in statusFormOptions"
              :key="item.value"
              :label="item.label"
              :value="item.value"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="地图描述" prop="remarks" style="width: 100%">
          <el-input
            v-model="addForm.remarks"
            type="textarea"
            placeholder="请输入地图描述"
          />
        </el-form-item>
        <el-form-item label="上传文件" prop="file">
          <el-upload
            ref="uploadRef"
            v-model:file-list="fileList"
            :http-request="onUpload"
            :auto-upload="false"
            :limit="1"
            :on-exceed="handleExceed"
            :on-change="beforeUpload"
            style="width: 190px"
          >
            <template #trigger>
              <el-button
                :icon="useRenderIcon(UploadFileIcon)"
                type="primary"
                style="margin-right: 10px"
                >选取文件</el-button
              >
            </template>
            <template #tip>
              <div class="el-upload__tip">
                仅支持 .zip 地图包（ROS model.pgm +
                model.yaml）。上传后由后端解析 resolution / origin /
                栅格尺寸，并计算 SHA-256 写入 map_hash。
              </div>
            </template>
          </el-upload>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="cancel">取消</el-button>
        <el-button type="primary" @click="submitForm(addFormRef)"
          >确认</el-button
        >
      </template>
    </el-dialog>

    <el-dialog v-model="refsVisible" :title="refsTitle" width="760px">
      <el-descriptions :column="2" border>
        <el-descriptions-item label="基线版本">
          {{ refsMap.mapVersion || "-" }} · {{ refsMap.mapStatus || "-" }}
        </el-descriptions-item>
        <el-descriptions-item label="frame_id">
          {{ refsMap.frameId || "-" }}
        </el-descriptions-item>
        <el-descriptions-item label="分辨率 / 原点">
          {{ refsMap.resolution || "-" }} · {{ refsMap.mapOrigin || "-" }}
        </el-descriptions-item>
        <el-descriptions-item label="栅格尺寸">
          <span v-if="refsMap.mapWidth && refsMap.mapHeight">
            {{ refsMap.mapWidth }}×{{ refsMap.mapHeight }}
          </span>
          <span v-else>-</span>
        </el-descriptions-item>
        <el-descriptions-item label="map_hash" :span="2">
          <span class="font-mono text-xs">{{ refsMap.mapHash || "缺失" }}</span>
        </el-descriptions-item>
        <el-descriptions-item label="引用路线">
          {{ refsData.routes?.length || 0 }} 条
        </el-descriptions-item>
        <el-descriptions-item label="关联任务">
          {{ refsData.taskCount || 0 }} 个
        </el-descriptions-item>
      </el-descriptions>
      <el-alert
        class="mt-3"
        type="info"
        :closable="false"
        title="版本不可变：发布新版本不会自动迁移历史路线，需人工确认坐标迁移策略。"
      />
      <el-table :data="refsData.routes" class="mt-3" border>
        <el-table-column prop="id" label="路线编号" width="120" />
        <el-table-column prop="routeName" label="路线名称" min-width="180" />
        <el-table-column prop="routeStatus" label="状态" width="100" />
        <el-table-column prop="waypointCount" label="航点数" width="90" />
        <el-table-column label="处理状态" width="140">
          <template #default>
            <el-tag type="warning">待人工确认迁移</el-tag>
          </template>
        </el-table-column>
        <template #empty>该版本暂无路线引用，可直接替换或归档</template>
      </el-table>
    </el-dialog>
  </div>
</template>
