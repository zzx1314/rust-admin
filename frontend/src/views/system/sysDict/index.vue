<script setup lang="ts">
import { ref } from "vue";
import tree from "./tree.vue";
import { useDict } from "./utils/hook";
import { isAllEmpty } from "@pureadmin/utils";
import { PureTableBar } from "@/components/RePureTableBar";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";

import AddFill from "~icons/ri/add-circle-line";
import DogTableActions from "@/components/DogUI/DogTableActions.vue";

defineOptions({
  name: "SystemDict"
});

const treeRef = ref();
const tableRef = ref();

const {
  dictId,
  loading,
  columns,
  dataList,
  pagination,
  deviceDetection,
  onSearch,
  openDialog,
  onTreeSelect,
  handleDelete,
  handleSizeChange,
  handleCurrentChange
} = useDict();
</script>

<template>
  <div :class="['flex', 'justify-between', deviceDetection() && 'flex-wrap']">
    <tree
      ref="treeRef"
      :class="['mr-2', deviceDetection() ? 'w-full' : 'min-w-[200px]']"
      @tree-select="onTreeSelect"
    />
    <div :class="[deviceDetection() ? ['w-full'] : 'w-[calc(100%-200px)]']">
      <PureTableBar
        class="mt-0!"
        title="字典管理（左侧字典树可通过右键单击进行修改和删除）"
        :columns="columns"
        @refresh="onSearch"
      >
        <template #buttons>
          <el-button
            type="primary"
            :disabled="isAllEmpty(dictId)"
            :icon="useRenderIcon(AddFill)"
            @click="openDialog()"
          >
            新增字典详情
          </el-button>
        </template>
        <template v-slot="{ size, dynamicColumns }">
          <pure-table
            ref="tableRef"
            row-key="id"
            adaptive
            :adaptiveConfig="{ offsetBottom: 108 }"
            align-whole="center"
            table-layout="auto"
            :loading="loading"
            :size="size"
            :data="dataList"
            :columns="dynamicColumns"
            :pagination="pagination"
            :paginationSmall="size === 'small' ? true : false"
            :header-cell-style="{
              background: 'var(--el-fill-color-light)',
              color: 'var(--el-text-color-primary)'
            }"
            @page-size-change="handleSizeChange"
            @page-current-change="handleCurrentChange"
          >
            <template #operation="{ row }">
              <DogTableActions
                :row="row"
                :size="size"
                edit-label="修改"
                delete-label="删除"
                :confirm-title="`是否确定删除字典标签为${row.label}的这条数据`"
                @edit="openDialog('修改', row)"
                @delete="handleDelete(row)"
              />
            </template>
          </pure-table>
        </template>
      </PureTableBar>
    </div>
  </div>
</template>

<style scoped lang="scss">
:deep(.el-dropdown-menu__item i) {
  margin: 0;
}

:deep(.el-button:focus-visible) {
  outline: none;
}

.main-content {
  margin: 24px 24px 0 !important;
}
</style>
