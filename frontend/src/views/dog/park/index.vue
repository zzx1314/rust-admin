<script setup lang="ts">
import { ref } from "vue";
import type { FormInstance } from "element-plus";
import { useDogPark } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import AddFill from "~icons/ri/add-circle-line";
import DogTableActions from "@/components/DogUI/DogTableActions.vue";
import { PureTableBar } from "@/components/RePureTableBar";
import { useDogParkForm } from "./form";
import PureTable from "@pureadmin/table";
import { PlusDialogForm } from "plus-pro-components";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";

defineOptions({ name: "DogPark" });

const addFormRef = ref<FormInstance>();
const { columnsForm } = useDogParkForm();
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
  baselineDialogVisible,
  baselineTitle,
  baselineList,
  baselineLoading,
  openBaselines
} = useDogPark();
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
    <el-alert
      class="mb-4"
      type="info"
      :closable="false"
      show-icon
      title="地图基线约束"
    >
      <template #default>
        园区建档时登记坐标系，地图每次重标定生成不可变版本；路线创建时冻结版本引用。
        未绑定已发布基线的园区仅作资产台账，<b>禁止编排巡检路线</b>。
      </template>
    </el-alert>
    <PureTableBar title="园区列表" :columns="columns" @refresh="onSearch">
      <template #buttons>
        <el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="openDia('新增园区', addFormRef)"
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
            <DogTableActions
              :row="row"
              :size="size"
              @edit="handleUpdate(row, addFormRef)"
              @delete="handleDelete(row)"
            >
              <template #extra="{ size: s }">
                <el-button
                  class="reset-margin"
                  link
                  type="primary"
                  :size="s"
                  @click="openBaselines(row)"
                  >基线</el-button
                >
              </template>
            </DogTableActions>
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

    <el-dialog
      v-model="baselineDialogVisible"
      :title="baselineTitle"
      width="820px"
    >
      <el-alert
        type="info"
        :closable="false"
        title="地图版本不可变：发布新版不会自动改写历史路线坐标，旧版本保留至引用清零。"
      />
      <el-table
        v-loading="baselineLoading"
        :data="baselineList"
        border
        class="mt-3"
      >
        <el-table-column prop="mapName" label="地图名称" min-width="140" />
        <el-table-column prop="mapVersion" label="版本" width="90" />
        <el-table-column prop="mapStatus" label="状态" width="100">
          <template #default="{ row }">
            <el-tag
              :type="
                row.mapStatus === '已发布'
                  ? 'success'
                  : row.mapStatus === '部分标定'
                    ? 'warning'
                    : 'info'
              "
            >
              {{ row.mapStatus }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column prop="frameId" label="frame_id" min-width="130" />
        <el-table-column prop="floors" label="楼层" width="120" />
        <el-table-column prop="resolution" label="分辨率" width="110" />
        <el-table-column label="栅格尺寸" width="120">
          <template #default="{ row }">
            <span v-if="row.mapWidth && row.mapHeight">
              {{ row.mapWidth }}×{{ row.mapHeight }}
            </span>
            <span v-else>-</span>
          </template>
        </el-table-column>
        <el-table-column label="map_hash" min-width="160">
          <template #default="{ row }">
            <span
              v-if="row.mapHash"
              class="font-mono text-xs"
              :title="row.mapHash"
            >
              {{ String(row.mapHash).slice(0, 12) }}…
            </span>
            <span v-else class="text-red-400">缺失</span>
          </template>
        </el-table-column>
        <el-table-column prop="publishTime" label="发布时间" width="170" />
        <template #empty
          >该园区暂无地图版本，请先在地图管理中建图并发布基线</template
        >
      </el-table>
    </el-dialog>
  </div>
</template>
