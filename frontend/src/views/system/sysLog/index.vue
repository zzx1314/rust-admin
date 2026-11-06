<script setup lang="ts">
import { ref } from "vue";
import { FormInstance } from "element-plus";
import { usePSysLogrecord } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import Delete from "~icons/ep/delete";
import { PureTableBar } from "@/components/RePureTableBar";
import { useLogForm } from "./form";
import PureTable from "@pureadmin/table";
import { PlusDialogForm } from "plus-pro-components";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";

defineOptions({
  name: "PSysLogrecord"
});

const addFormRef = ref<FormInstance>();
const { columnsForm } = useLogForm();

const {
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
  handleSizeChange,
  handleCurrentChange,
  handleSelectionChange,
  handleSubmitError,
  handleSubmit,
  cancel
} = usePSysLogrecord();
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
    <PureTableBar title="日志列表" :columns="columns" @refresh="onSearch">
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
        />
      </template>
    </PureTableBar>

    <PlusDialogForm
      ref="addFormRef"
      v-model:visible="dialogFormVisible"
      v-model="addForm"
      :dialog="{ title: title }"
      :form="{
        columns: columnsForm,
        rules,
        labelWidth: '100px'
      }"
      @cancel="cancel"
      @confirm-error="handleSubmitError"
      @confirm="handleSubmit"
    />
  </div>
</template>

<style scoped lang="scss">
:deep(.el-dropdown-menu__item i) {
  margin: 0;
}

:deep(.el-link) {
  padding-left: 10px;
}
</style>
