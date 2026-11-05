<script setup lang="ts">
import { ref } from "vue";
import type { FormInstance } from "element-plus";
import { useDogRoute } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import EditPen from "~icons/ep/edit-pen";
import Delete from "~icons/ep/delete";
import AddFill from "~icons/ri/add-circle-line";
import More from "~icons/ep/more-filled";
import DropperLine from "~icons/ri/dropper-line";
import { PureTableBar } from "@/components/RePureTableBar";
import PureTable from "@pureadmin/table";
import DogSearchForm from "../components/DogSearchForm.vue";

defineOptions({ name: "DogRoute" });

const addFormRef = ref<FormInstance>();
const {
  buttonClass,
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
  modelOptions,
  onSearch,
  handleUpdate,
  handleDelete,
  handleSizeChange,
  handleCurrentChange,
  handleSelectionChange,
  handleSubmit,
  cancel,
  openDia
} = useDogRoute();
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
    <PureTableBar title="路线列表" :columns="columns" @refresh="onSearch">
      <template #buttons>
        <el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="openDia('新增路线', addFormRef)"
        >
          新增
        </el-button>
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
            <el-dropdown class="ml-2">
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
                    >
                      设计路线
                    </el-button>
                  </el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
          </template>
        </pure-table>
      </template>
    </PureTableBar>

    <el-dialog
      v-model="dialogFormVisible"
      :title="title"
      width="600px"
      @close="cancel"
    >
      <el-form
        ref="addFormRef"
        :model="addForm"
        :rules="rules"
        label-width="100px"
      >
        <el-form-item label="路线名称" prop="routeName">
          <el-input v-model="addForm.routeName" placeholder="请输入路线名称" />
        </el-form-item>
        <el-form-item label="路线类型" prop="routeType">
          <el-select
            v-model="addForm.routeType"
            placeholder="请选择路线类型"
            style="width: 100%"
          >
            <el-option label="在线地图" value="在线地图" />
            <el-option label="离线地图" value="离线地图" />
          </el-select>
        </el-form-item>
        <el-form-item label="路线状态" prop="routeStatus">
          <el-select
            v-model="addForm.routeStatus"
            placeholder="请选择路线状态"
            style="width: 100%"
          >
            <el-option label="启用" value="启用" />
            <el-option label="停用" value="停用" />
          </el-select>
        </el-form-item>
        <el-form-item label="路线描述" prop="routeDescription">
          <el-input
            v-model="addForm.routeDescription"
            type="textarea"
            placeholder="请输入路线描述"
          />
        </el-form-item>
        <el-form-item label="关联模型" prop="modelId">
          <el-select
            v-model="addForm.modelId"
            placeholder="请选择模型"
            clearable
            style="width: 100%"
          >
            <el-option
              v-for="opt in modelOptions"
              :key="opt.value"
              :label="opt.label"
              :value="opt.value"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="路线数据" prop="routeData">
          <el-input
            v-model="addForm.routeData"
            type="textarea"
            :rows="3"
            placeholder="请输入路线数据（JSON格式）"
          />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="cancel">取消</el-button>
        <el-button type="primary" @click="handleSubmit(addFormRef)">
          确定
        </el-button>
      </template>
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
</style>
