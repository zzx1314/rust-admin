<script setup lang="ts">
import { ref } from "vue";
import type { FormInstance } from "element-plus";
import { useDogTask } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import EditPen from "~icons/ep/edit-pen";
import Delete from "~icons/ep/delete";
import AddFill from "~icons/ri/add-circle-line";
import More from "~icons/ep/more-filled";
import DropperLine from "~icons/ri/dropper-line";
import RiRestartFill from "~icons/ri/restart-fill";
import DropperStatus from "~icons/ri/text-wrap";
import RiBarChartBoxLine from "~icons/ri/bar-chart-box-line";
import { PureTableBar } from "@/components/RePureTableBar";
import { useCollectorTaskForm } from "./form";
import PureTable from "@pureadmin/table";
import { PlusDialogForm } from "plus-pro-components";
import DogSearchForm from "../components/DogSearchForm.vue";

defineOptions({ name: "DogTask" });

const addFormRef = ref<FormInstance>();
const { columnsForm, columnsQueryForm } = useCollectorTaskForm();

const {
  queryForm,
  dataList,
  loading,
  dialogFormVisible,
  dialogStatusVisible,
  dialogStatusData,
  title,
  pagination,
  addForm,
  rules,
  columns,
  queryFields,
  moreQueryFields,
  buttonClass,
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
  flowRoute,
  flowRouteStatus,
  startTask,
  datadashBoard
} = useDogTask();

function formatStatusData(data: any) {
  const successIds = data.successIds || [];
  const failIds = data.failIds || [];
  const errorInfoMap = data.errorInfoMap || {};
  return `成功: ${successIds.length} 个 | 失败: ${failIds.length} 个`;
}
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
    <PureTableBar title="任务列表" :columns="columns" @refresh="onSearch">
      <template #buttons>
        <el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="openDia('新增任务', addFormRef)"
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
            >
              修改
            </el-button>
            <el-popconfirm title="是否确认删除?" @confirm="handleDelete(row)">
              <template #reference>
                <el-button
                  class="reset-margin"
                  link
                  type="primary"
                  :size="size"
                  :icon="useRenderIcon(Delete)"
                >
                  删除
                </el-button>
              </template>
            </el-popconfirm>
            <el-dropdown class="ml-2" trigger="click" :hide-on-click="false">
              <el-button
                class="ml-4 mt-0.5"
                link
                type="primary"
                :size="size"
                :icon="useRenderIcon(More)"
              />
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item>
                    <el-button
                      :class="buttonClass"
                      link
                      type="primary"
                      :size="size"
                      :icon="useRenderIcon(DropperLine)"
                      @click="flowRoute(row)"
                    >
                      流程设计
                    </el-button>
                  </el-dropdown-item>
                  <el-dropdown-item>
                    <el-button
                      :class="['status-btn', buttonClass]"
                      link
                      type="primary"
                      :size="size"
                      :icon="useRenderIcon(RiRestartFill)"
                      @click="startTask(row)"
                    >
                      启动流程
                    </el-button>
                  </el-dropdown-item>
                  <el-dropdown-item>
                    <el-button
                      :class="buttonClass"
                      link
                      type="primary"
                      :size="size"
                      :icon="useRenderIcon(DropperStatus)"
                      @click="flowRouteStatus(row)"
                    >
                      执行状态
                    </el-button>
                  </el-dropdown-item>
                  <el-dropdown-item>
                    <el-button
                      :class="buttonClass"
                      link
                      type="primary"
                      :size="size"
                      :icon="useRenderIcon(RiBarChartBoxLine)"
                      @click="datadashBoard(row)"
                    >
                      数据看板
                    </el-button>
                  </el-dropdown-item>
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
      :form="{
        columns: columnsForm,
        rules,
        labelWidth: '100px'
      }"
      @cancel="cancel"
      @confirm-error="handleSubmitError"
      @confirm="handleSubmit"
    />

    <el-dialog
      v-model="dialogStatusVisible"
      :title="title"
      width="600px"
      @close="cancel"
    >
      <div v-if="dialogStatusData">
        <el-alert
          :title="`成功: ${(dialogStatusData.successIds || []).length} 个`"
          type="success"
          show-icon
          :closable="false"
          style="margin-bottom: 12px"
        />
        <el-alert
          :title="`失败: ${(dialogStatusData.failIds || []).length} 个`"
          type="error"
          show-icon
          :closable="false"
          style="margin-bottom: 16px"
        />
        <div v-if="Object.keys(dialogStatusData.errorInfoMap || {}).length > 0">
          <h4 style="margin-bottom: 8px; color: #f56c6c">错误详情</h4>
          <div v-for="(msg, key) in dialogStatusData.errorInfoMap" :key="key">
            <el-descriptions :column="1" size="small" border style="margin-bottom: 8px">
              <el-descriptions-item label="操作ID">{{ key }}</el-descriptions-item>
              <el-descriptions-item label="错误信息">{{ msg }}</el-descriptions-item>
            </el-descriptions>
          </div>
        </div>
        <div
          v-if="
            (dialogStatusData.successIds || []).length === 0 &&
            (dialogStatusData.failIds || []).length === 0
          "
        >
          <el-empty description="暂无执行结果" />
        </div>
      </div>
    </el-dialog>
  </div>
</template>

<style scoped lang="scss">
.my-card ::v-deep(.el-card__body) {
  padding: 10px 10px 0;
}

:deep(.el-dropdown-menu__item i) {
  margin: 0;
}

:deep(.el-link) {
  padding-left: 10px;
}

:deep(.el-form-item) {
  margin-bottom: 12px;
}

.status-btn :deep(svg) {
  color: #67c23a;
}
</style>
