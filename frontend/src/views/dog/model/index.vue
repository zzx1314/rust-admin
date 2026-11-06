<script setup lang="ts">
import { ref } from "vue";
import type { FormInstance } from "element-plus";
import { useDogModel } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import AddFill from "~icons/ri/add-circle-line";
import DogTableActions from "@/components/DogUI/DogTableActions.vue";
import { PureTableBar } from "@/components/RePureTableBar";
import { useCollectorModelForm } from "./form";
import PureTable from "@pureadmin/table";
import { PlusDialogForm } from "plus-pro-components";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";
defineOptions({ name: "DogModel" });
const addFormRef = ref<FormInstance>();
const { columnsForm } = useCollectorModelForm();
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
  openDia
} = useDogModel();
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
    <PureTableBar title="模型列表" :columns="columns" @refresh="onSearch">
      <template #buttons
        ><el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="openDia('新增模型', addFormRef)"
          >新增</el-button
        ></template
      >
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
            <DogTableActions
              :row="row"
              :size="size"
              @edit="handleUpdate(row, addFormRef)"
              @delete="handleDelete(row)"
            />
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
  </div>
</template>
