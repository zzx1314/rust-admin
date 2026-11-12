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
import AddFill from "~icons/ri/add-circle-line";
import More from "~icons/ep/more-filled";
import UploadFileIcon from "~icons/ep/upload-filled";
import { PureTableBar } from "@/components/RePureTableBar";
import PureTable from "@pureadmin/table";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";
import DogTableActions from "@/components/DogUI/DogTableActions.vue";
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
  newVersionMap
} = useDogMap();

const statusFormOptions = MAP_STATUS_OPTIONS.filter(
  item => item.value === "草稿" || item.value === "部分标定"
);

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

const handlePublish = row => {
  ElMessageBox.confirm(
    `发布后该版本状态将变为已发布，并记录发布时间。是否继续？`,
    "发布地图基线",
    { type: "warning", confirmButtonText: "发布", cancelButtonText: "取消" }
  )
    .then(() => publishMap(row))
    .catch(() => {});
};

const handleNewVersion = row => {
  ElMessageBox.confirm(
    `将基于 ${row.mapVersion || row.mapName} 派生下一个版本并立即发布。是否继续？`,
    "发布新版本",
    { type: "warning", confirmButtonText: "发布", cancelButtonText: "取消" }
  )
    .then(() => newVersionMap(row))
    .catch(() => {});
};

/** 操作列「更多」菜单分发：发布 / 新版本。 */
const handleRowCommand = (row, command: string) => {
  if (command === "publish") {
    handlePublish(row);
  } else if (command === "newVersion") {
    handleNewVersion(row);
  }
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
          <!-- 操作列：只留「修改 / 删除」+ 一个「更多」菜单（发布 / 新版本） -->
          <template #operation="{ row }">
            <DogTableActions
              :row="row"
              :size="size"
              @edit="handleUpdate(row, addFormRef)"
              @delete="handleDelete(row)"
            >
              <template #extra="{ row: current, size: currentSize }">
                <el-dropdown
                  trigger="click"
                  @command="
                    (command: string) => handleRowCommand(current, command)
                  "
                >
                  <el-button
                    class="reset-margin"
                    link
                    type="primary"
                    :size="currentSize"
                    :icon="useRenderIcon(More)"
                  />
                  <template #dropdown>
                    <el-dropdown-menu>
                      <el-dropdown-item
                        v-if="
                          current.mapStatus === '草稿' ||
                          current.mapStatus === '部分标定'
                        "
                        command="publish"
                        >发布</el-dropdown-item
                      >
                      <el-dropdown-item
                        v-if="current.mapStatus !== '草稿'"
                        command="newVersion"
                        >新版本</el-dropdown-item
                      >
                    </el-dropdown-menu>
                  </template>
                </el-dropdown>
              </template>
            </DogTableActions>
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
        <el-form-item label="园区编码" prop="parkCode" style="width: 100%">
          <el-input
            v-model="addForm.parkCode"
            placeholder="业务平台园区编码，如 PK-001（机器人回传时自动填写）"
          />
        </el-form-item>
        <el-form-item label="园区名称" prop="parkName" style="width: 100%">
          <el-input
            v-model="addForm.parkName"
            placeholder="业务平台园区名称（冗余展示）"
          />
        </el-form-item>
        <el-form-item label="设备编号" prop="deviceCode" style="width: 100%">
          <el-input
            v-model="addForm.deviceCode"
            placeholder="上报该地图的设备编号，如 RD-201（机器人回传时自动填写）"
          />
        </el-form-item>
        <el-form-item label="基线版本" prop="mapVersion" style="width: 100%">
          <el-input v-model="addForm.mapVersion" placeholder="例如：v1.0" />
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
  </div>
</template>
