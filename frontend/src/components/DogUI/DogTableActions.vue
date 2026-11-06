<script setup lang="ts">
/**
 * DogTableActions - 表格行操作按钮组件
 *
 * 统一封装 edit（修改）/ delete（删除）操作按钮模式。
 * 支持权限校验（hasAuth）、isEdit 字段校验、el-popconfirm 删除确认。
 * 特殊操作可通过 #extra 插槽扩展（如下拉更多操作）。
 */
import { hasAuth } from "@/router/utils";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import EditPen from "~icons/ep/edit-pen";
import Delete from "~icons/ep/delete";

interface Props {
  /** 行数据 */
  row: any;
  /** 表格当前 size（从 pure-table 传入） */
  size?: "default" | "small" | "large";
  /** 编辑按钮的权限标识，不传或空则不校验权限（始终显示） */
  editAuth?: string;
  /** 删除按钮的权限标识，不传或空则不校验权限（始终显示） */
  deleteAuth?: string;
  /** 编辑按钮文字 */
  editLabel?: string;
  /** 删除按钮文字 */
  deleteLabel?: string;
  /** 删除确认弹窗提示文字 */
  confirmTitle?: string;
  /** 是否依赖行内 `isEdit` 字段控制按钮显示（如用户管理） */
  checkIsEdit?: boolean;
  /** 是否隐藏编辑按钮 */
  hideEdit?: boolean;
  /** 是否隐藏删除按钮 */
  hideDelete?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  size: "default",
  editAuth: "",
  deleteAuth: "",
  editLabel: "修改",
  deleteLabel: "删除",
  confirmTitle: "是否确认删除?",
  checkIsEdit: false,
  hideEdit: false,
  hideDelete: false
});

const emit = defineEmits<{
  edit: [row: any];
  delete: [row: any];
}>();

function canShow(auth: string, checkIsEdit: boolean, isEdit: boolean): boolean {
  if (auth && !hasAuth(auth)) return false;
  if (checkIsEdit && !isEdit) return false;
  return true;
}
</script>

<template>
  <!-- 编辑按钮 -->
  <el-button
    v-if="!hideEdit && canShow(editAuth, checkIsEdit, row.isEdit)"
    class="reset-margin"
    link
    type="primary"
    :size="size"
    :icon="useRenderIcon(EditPen)"
    @click="emit('edit', row)"
  >
    {{ editLabel }}
  </el-button>

  <!-- 删除按钮 -->
  <el-popconfirm
    v-if="!hideDelete && canShow(deleteAuth, checkIsEdit, row.isEdit)"
    :title="confirmTitle"
    @confirm="emit('delete', row)"
  >
    <template #reference>
      <el-button
        class="reset-margin"
        link
        type="primary"
        :size="size"
        :icon="useRenderIcon(Delete)"
      >
        {{ deleteLabel }}
      </el-button>
    </template>
  </el-popconfirm>

  <!-- 额外操作插槽（如下拉菜单等） -->
  <slot name="extra" :row="row" :size="size" />
</template>
