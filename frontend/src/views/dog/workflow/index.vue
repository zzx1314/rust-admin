<script setup lang="ts">
import { ref } from "vue";
import type { FormInstance } from "element-plus";
import { useDogWorkflow } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import EditPen from "~icons/ep/edit-pen";
import Delete from "~icons/ep/delete";
import AddFill from "~icons/ri/add-circle-line";
import { PureTableBar } from "@/components/RePureTableBar";
import { useCollectorWorkflowForm } from "./form";
import PureTable from "@pureadmin/table";
import { PlusDialogForm } from "plus-pro-components";
defineOptions({ name: "DogWorkflow" });
const addFormRef = ref<FormInstance>();
const { columnsForm } = useCollectorWorkflowForm();
const {
  dataList,
  loading,
  dialogFormVisible,
  title,
  pagination,
  addForm,
  rules,
  columns,
  handleUpdate,
  handleDelete,
  handleSubmit,
  handleSubmitError,
  cancel,
  openDia
} = useDogWorkflow();
</script>
<template>
  <div class="main">
    <PureTableBar title="工作流列表" :columns="columns" @refresh="cancel">
      <template #buttons
        ><el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="openDia('新增工作流', addFormRef)"
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
          :header-cell-style="{
            background: 'var(--el-table-row-hover-bg-color)',
            color: 'var(--el-text-color-primary)'
          }"
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
            <el-popconfirm title="是否确认删除?" @confirm="handleDelete(row)"
              ><template #reference
                ><el-button
                  class="reset-margin"
                  link
                  type="primary"
                  :size="size"
                  :icon="useRenderIcon(Delete)"
                  >删除</el-button
                ></template
              ></el-popconfirm
            >
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
