<script setup lang="ts">
import { ref } from "vue";
import type { FormInstance } from "element-plus";
import { useDogMedia } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import AddFill from "~icons/ri/add-circle-line";
import More from "~icons/ep/more-filled";
import { PureTableBar } from "@/components/RePureTableBar";
import { useCollectorMediaForm } from "./form";
import PureTable from "@pureadmin/table";
import { PlusDialogForm } from "plus-pro-components";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";
defineOptions({ name: "DogMedia" });
const addFormRef = ref<FormInstance>();
const { columnsForm } = useCollectorMediaForm();
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
  onSearch,
  handleUpdate,
  handleDelete,
  handleSizeChange,
  handleCurrentChange,
  handleSelectionChange,
  handleSubmitError,
  handleSubmit,
  cancel,
  openDia,
  selectedIds,
  exporting,
  handleDownload,
  handleExport,
  handleRowCommand,
  previewVisible,
  previewLoading,
  previewImage,
  previewTitle,
  handlePreview,
  closePreview
} = useDogMedia();
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
    <PureTableBar title="媒体列表" :columns="columns" @refresh="onSearch">
      <template #buttons>
        <el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="openDia('新增媒体', addFormRef)"
          >新增</el-button
        >
        <el-button
          type="success"
          plain
          :loading="exporting"
          @click="handleExport"
          >{{
            selectedIds.length
              ? `下载选中（${selectedIds.length}）`
              : "导出筛选结果"
          }}</el-button
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
          <!-- 操作列：只留「预览」+「更多」（下载原图 / 修改 / 删除） -->
          <template #operation="{ row }">
            <el-button
              class="reset-margin"
              link
              type="primary"
              :size="size"
              @click="handlePreview(row)"
              >预览</el-button
            >
            <el-dropdown
              trigger="click"
              @command="
                (command: string) => handleRowCommand(command, row, addFormRef)
              "
            >
              <el-button
                class="reset-margin"
                link
                type="primary"
                :size="size"
                :icon="useRenderIcon(More)"
              />
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item command="download"
                    >下载原图</el-dropdown-item
                  >
                  <el-dropdown-item command="edit" divided
                    >修改</el-dropdown-item
                  >
                  <el-dropdown-item command="delete">删除</el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
          </template>
        </pure-table>
      </template>
    </PureTableBar>
    <PlusDialogForm
      ref="addFormRef"
      v-model:visible="dialogFormVisible"
      v-model="addForm"
      :dialog="{ title: title }"
      :form="{ columns: columnsForm, rules, labelWidth: '100px' }"
      @cancel="cancel"
      @confirm-error="handleSubmitError"
      @confirm="handleSubmit"
    />

    <!-- 单张预览：内容来自 /api/dogMedia/{id}/image（base64 data URL） -->
    <el-dialog
      v-model="previewVisible"
      :title="previewTitle"
      width="760px"
      destroy-on-close
      @close="closePreview"
    >
      <div v-loading="previewLoading" class="media-preview">
        <img v-if="previewImage" :src="previewImage" alt="媒体预览" />
      </div>
      <template #footer>
        <el-button @click="closePreview">关闭</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<style scoped>
.media-preview {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 220px;
}

.media-preview img {
  max-width: 100%;
  max-height: 70vh;
  object-fit: contain;
}
</style>
