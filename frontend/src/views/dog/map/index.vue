<script setup lang="ts">
import { ref } from "vue";
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
import { MAP_TYPE_OPTIONS } from "../components/search";
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
  openDia
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
              <div class="el-upload__tip">仅支持 .zip 地图包</div>
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
