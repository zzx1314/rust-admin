<script setup lang="ts">
import { usePSysMessage } from "./hook";
import { PureTableBar } from "@/components/RePureTableBar";
import PureTable from "@pureadmin/table";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";
import DogTableActions from "@/components/DogUI/DogTableActions.vue";
import HandlerMessageForm from "@/views/system/sysMessage/HandlerMessageForm.vue";
import EditPen from "~icons/ep/edit-pen";
import { ref } from "vue";
import { useMessageStoreHook } from "@/store/modules/message";
import { hasAuth } from "@/router/utils";

defineOptions({
  name: "PSysMessage"
});

const messageId = ref(null);
const dialogFormVisible = ref(false);

function closeDia() {
  dialogFormVisible.value = false;
  messageId.value = null;
  useMessageStoreHook().handleMessage = true;
  onSearch();
}

const handlerItem = item => {
  console.log("Received item:", item);
  dialogFormVisible.value = true;
  messageId.value = item.id;
};

const {
  queryForm,
  queryFields,
  moreQueryFields,
  dataList,
  loading,
  pagination,
  columns,
  onSearch,
  handleDelete,
  handleSizeChange,
  handleCurrentChange,
  handleSelectionChange,
  cancel
} = usePSysMessage();
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
    <PureTableBar title="消息列表" :columns="columns" @refresh="onSearch">
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
              edit-auth="message_handler"
              edit-label="处置"
              delete-auth="message_delete"
              hide-edit
              @delete="handleDelete(row)"
            >
              <template #extra="{ row: r, size: s }">
                <el-button
                  v-if="hasAuth('message_handler')"
                  class="reset-margin"
                  link
                  type="primary"
                  :size="s"
                  :icon="useRenderIcon(EditPen)"
                  @click="handlerItem(r)"
                >
                  处置
                </el-button>
              </template>
            </DogTableActions>
          </template>
        </pure-table>
      </template>
    </PureTableBar>

    <handler-message-form
      :id="messageId"
      :dialogFormVisible="dialogFormVisible"
      title="消息处置"
      @update:dialogFormVisible="closeDia"
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
