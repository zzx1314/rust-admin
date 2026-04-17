<script setup lang="ts">
import { ref, watch, h } from "vue";
import { useDict } from "./utils/hook";
import { message } from "@/utils/message";
import { useDark } from "@pureadmin/utils";
import { ElMessageBox } from "element-plus";
import { ReText } from "@/components/ReText";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import "@imengyu/vue3-context-menu/lib/vue3-context-menu.css";
import ContextMenu from "@imengyu/vue3-context-menu";

import Delete from "~icons/ep/delete";
import EditPen from "~icons/ep/edit-pen";
import AddFill from "~icons/ri/add-circle-line";

interface Tree {
  id: number;
  name: string;
  code: string;
  highlight?: boolean;
  children?: Tree[];
}

const emit = defineEmits(["tree-select"]);

const treeRef = ref();
const searchValue = ref("");
const highlightMap = ref({});
const defaultProps = {
  children: "children",
  label: "name"
};

const { isDark } = useDark();

const { treeLoading, treeData, openDictDialog, getDictTreeData } = useDict();

const filterNode = (value: string, data: Tree) => {
  if (!value) return true;
  return `${data.name}（${data.code}）`.includes(value);
};

function nodeClick(value) {
  const nodeId = value.$treeNodeId;
  highlightMap.value[nodeId] = highlightMap.value[nodeId]?.highlight
    ? Object.assign({ id: nodeId }, highlightMap.value[nodeId], {
        highlight: false
      })
    : Object.assign({ id: nodeId }, highlightMap.value[nodeId], {
        highlight: true
      });
  Object.values(highlightMap.value).forEach((v: Tree) => {
    if (v.id !== nodeId) {
      v.highlight = false;
    }
  });
  emit(
    "tree-select",
    highlightMap.value[nodeId]?.highlight
      ? Object.assign({ ...value, selected: true })
      : Object.assign({ ...value, selected: false })
  );
}

// https://docs.imengyu.top/vue3-context-menu-docs/
function onContextMenu(e: MouseEvent, { name, data }) {
  e.preventDefault();
  ContextMenu.showContextMenu({
    x: e.x,
    y: e.y,
    theme: isDark.value ? "dark" : "default",
    items: [
      {
        label: "修改",
        icon: h(useRenderIcon(EditPen)),
        onClick: () => {
          openDictDialog("修改", data);
        }
      },
      {
        label: "删除",
        icon: h(useRenderIcon(Delete)),
        // disabled: true,
        onClick: () => {
          ElMessageBox.confirm(
            `确定要删除 <strong style='color:var(--el-color-primary)'>${
              name
            }</strong> 字典吗?`,
            "系统提示",
            {
              confirmButtonText: "确定",
              cancelButtonText: "取消",
              type: "warning",
              dangerouslyUseHTMLString: true,
              draggable: true
            }
          ).then(() => {
            // 实际开发中请调用删除字典接口
            getDictTreeData();
            message("已成功删除", {
              type: "success"
            });
          });
        }
      }
    ]
  });
}

watch(searchValue, val => {
  treeRef.value!.filter(val);
});
</script>

<template>
  <div
    v-loading="treeLoading"
    class="h-full bg-bg_color overflow-hidden relative"
    :style="{ minHeight: `calc(100vh - 141px)` }"
  >
    <div class="flex items-center h-[34px]">
      <el-input
        v-model="searchValue"
        class="mx-2"
        size="small"
        placeholder="请输入字典名称"
        clearable
      >
        <template #suffix>
          <el-icon class="el-input__icon">
            <IconifyIconOffline
              v-show="searchValue.length === 0"
              icon="ri/search-line"
            />
          </el-icon>
        </template>
      </el-input>
    </div>
    <el-divider />
    <el-scrollbar height="calc(90vh - 150px)">
      <el-tree
        ref="treeRef"
        :data="treeData"
        node-key="id"
        size="small"
        :props="defaultProps"
        default-expand-all
        :expand-on-click-node="false"
        :filter-node-method="filterNode"
        @node-click="nodeClick"
      >
        <template #default="{ node, data }">
          <ReText
            :class="[
              'w-full!',
              'p-1!',
              'mt-2!',
              'mr-2!',
              'rounded',
              'select-none',
              'hover:text-primary',
              searchValue.trim().length > 0 &&
                `${node.label}（${data.code}）`.includes(searchValue) &&
                'text-red-500!',
              highlightMap[node.id]?.highlight ? 'dark:text-primary!' : ''
            ]"
            :style="{
              color: highlightMap[node.id]?.highlight
                ? 'var(--el-color-primary)'
                : '',
              background: highlightMap[node.id]?.highlight
                ? 'var(--el-color-primary-light-7)'
                : 'transparent'
            }"
            @contextmenu="onContextMenu($event, { name: node.label, data })"
          >
            {{ `${node.label}（${data.code}）` }}
          </ReText>
        </template>
      </el-tree>
    </el-scrollbar>
    <el-button
      class="w-[90%] absolute bottom-[22px] left-[50%] -translate-x-[50%]"
      type="primary"
      :icon="useRenderIcon(AddFill)"
      @click="openDictDialog()"
    >
      新增字典
    </el-button>
  </div>
</template>

<style>
.mx-context-menu {
  padding: 6px;
  border-radius: 4px;
}

.mx-context-menu-item {
  cursor: pointer;
}
</style>

<style lang="scss" scoped>
:deep(.el-divider) {
  margin: 0;
}

:deep(.el-tree) {
  --el-tree-node-hover-bg-color: transparent;
}

:deep(.el-tree-node) {
  margin-top: 4px;
  margin-left: -10px;
}
</style>
