<script setup lang="ts">
import { ref } from "vue";
import type { FormInstance } from "element-plus";
import { useDogDevice } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import EditPen from "~icons/ep/edit-pen";
import Delete from "~icons/ep/delete";
import AddFill from "~icons/ri/add-circle-line";
import UserAddLine from "~icons/ri/user-add-line";
import LinkUnlink from "~icons/ri/link-unlink";
import { PureTableBar } from "@/components/RePureTableBar";
import { useCollectorBusDevForm } from "./form";
import PureTable from "@pureadmin/table";
import { PlusDialogForm } from "plus-pro-components";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";

defineOptions({ name: "DogDevice" });

const addFormRef = ref<FormInstance>();
const { columnsForm } = useCollectorBusDevForm();
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
  canManageUsers,
  bindVisible,
  bindLoading,
  bindingsLoading,
  bindDevice,
  bindings,
  bindUserId,
  userOptions,
  onSearch,
  handleUpdate,
  handleDelete,
  handleBind,
  submitBind,
  removeBinding,
  handleUnbindSelf,
  handleSizeChange,
  handleCurrentChange,
  handleSelectionChange,
  handleSubmitError,
  handleSubmit,
  cancel,
  openDia
} = useDogDevice();
</script>
<template>
  <div class="dog-page">
    <DogSearchForm
      v-model="queryForm"
      :fields="queryFields"
      :more-fields="moreQueryFields"
      :loading="loading"
      @search="onSearch"
      @reset="cancel"
    />
    <PureTableBar title="设备列表" :columns="columns" @refresh="onSearch">
      <template #buttons>
        <span class="mr-3 text-[13px] text-gray-500 dark:text-gray-400">
          共
          <b class="dog-num">{{ pagination.total }}</b>
          台设备
        </span>
        <el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="openDia('新增设备', addFormRef)"
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
              v-if="canManageUsers"
              class="reset-margin"
              link
              type="primary"
              :size="size"
              :icon="useRenderIcon(UserAddLine)"
              @click="handleBind(row)"
              >绑定用户</el-button
            >
            <el-popconfirm
              v-else
              title="是否确认解绑当前账号?"
              @confirm="handleUnbindSelf(row)"
            >
              <template #reference>
                <el-button
                  class="reset-margin"
                  link
                  type="primary"
                  :size="size"
                  :icon="useRenderIcon(LinkUnlink)"
                  >解绑</el-button
                >
              </template>
            </el-popconfirm>
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
    <el-dialog v-model="bindVisible" title="绑定用户" width="480px">
      <el-form label-width="90px">
        <el-form-item label="设备编号">
          <span>{{ bindDevice.deviceId }}</span>
        </el-form-item>
        <el-form-item label="已绑定">
          <div v-if="bindingsLoading" class="text-gray-400">加载中…</div>
          <div v-else-if="!bindings.length" class="text-gray-400">
            暂未绑定用户
          </div>
          <div v-else class="flex w-full flex-col gap-2">
            <div
              v-for="item in bindings"
              :key="item.userId"
              class="flex w-full items-center justify-between"
            >
              <el-tag>{{ item.userName }}</el-tag>
              <el-button
                link
                type="danger"
                :icon="useRenderIcon(LinkUnlink)"
                @click="removeBinding(item)"
                >解绑</el-button
              >
            </div>
          </div>
        </el-form-item>
        <el-form-item label="绑定用户">
          <el-select
            v-model="bindUserId"
            filterable
            clearable
            placeholder="请选择用户"
            style="width: 100%"
          >
            <el-option
              v-for="item in userOptions"
              :key="item.value"
              :label="item.label"
              :value="item.value"
              :disabled="
                bindings.some(binding => binding.userId === item.value)
              "
            />
          </el-select>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="bindVisible = false">关闭</el-button>
        <el-button type="primary" :loading="bindLoading" @click="submitBind"
          >绑定</el-button
        >
      </template>
    </el-dialog>
  </div>
</template>
