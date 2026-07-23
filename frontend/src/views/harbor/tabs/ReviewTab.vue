<script setup lang="ts">
import { ref, reactive, onMounted, computed } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import { PureTableBar } from "@/components/RePureTableBar";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import { ElMessage, ElMessageBox } from "element-plus";
import {
  getAppReviews,
  approveAppReview,
  rejectAppReview,
  deleteAppReview,
  type AppReview
} from "@/api/appReview";

import Refresh from "~icons/ep/refresh";
import Search from "~icons/ep/search";
import Check from "~icons/ep/circle-check";
import Close from "~icons/ep/circle-close";
import Delete from "~icons/ep/delete";

const loading = ref(false);
const reviews = ref<AppReview[]>([]);
const searchRepo = ref("");
const searchStatus = ref("");

const statusOptions = [
  { label: "全部", value: "" },
  { label: "待审核", value: "pending" },
  { label: "已通过", value: "approved" },
  { label: "已拒绝", value: "rejected" }
];

const pagination = reactive<PaginationProps>({
  total: 0,
  pageSize: 10,
  currentPage: 1,
  background: true
});

const commentDialogVisible = ref(false);
const commentForm = ref({ comment: "" });
const currentReview = ref<AppReview | null>(null);
const currentAction = ref<"approve" | "reject">("approve");

const columns = [
  { label: "源项目", prop: "srcProject", width: 150 },
  { label: "目标项目", prop: "destProject", width: 150 },
  { label: "仓库", prop: "repositoryName", minWidth: 200 },
  { label: "Tag", prop: "tag", width: 150 },
  { label: "摘要", prop: "digest", width: 200, showOverflowTooltip: true },
  { label: "状态", prop: "status", slot: "status" },
  { label: "审核意见", prop: "reviewerComment", minWidth: 200 },
  { label: "创建时间", prop: "createTime", minWidth: 200 },
  { label: "操作", prop: "actions", slot: "actions", width: 250 }
];

const statusTag = (status: string) => {
  switch (status) {
    case "pending":
      return { type: "warning" as const, label: "待审核" };
    case "approved":
      return { type: "success" as const, label: "已通过" };
    case "rejected":
      return { type: "danger" as const, label: "已拒绝" };
    default:
      return { type: "info" as const, label: status };
  }
};

const fetchReviews = async () => {
  loading.value = true;
  try {
    const res = await getAppReviews({
      repositoryName: searchRepo.value || undefined,
      status: searchStatus.value || undefined,
      current: pagination.currentPage,
      size: pagination.pageSize
    });
    if (res.code === 10200 && res.data) {
      reviews.value = res.data.records || [];
      pagination.total = res.data.total;
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取审核列表失败");
  } finally {
    loading.value = false;
  }
};

const handleSizeChange = (val: number) => {
  pagination.pageSize = val;
  fetchReviews();
};

const handleCurrentChange = (val: number) => {
  pagination.currentPage = val;
  fetchReviews();
};

const onSearch = () => {
  pagination.currentPage = 1;
  fetchReviews();
};

const onReset = () => {
  searchRepo.value = "";
  searchStatus.value = "";
  pagination.currentPage = 1;
  fetchReviews();
};

const openCommentDialog = (row: AppReview, action: "approve" | "reject") => {
  currentReview.value = row;
  currentAction.value = action;
  commentForm.value.comment = "";
  commentDialogVisible.value = true;
};

const submitAction = async () => {
  if (!currentReview.value) return;
  const id = currentReview.value.id;
  const data = { reviewerComment: commentForm.value.comment };
  try {
    if (currentAction.value === "approve") {
      const res = await approveAppReview(id, data);
      if (res.code === 10200) {
        ElMessage.success("审核通过并已触发复制");
      } else {
        ElMessage.error(res.msg || "审核失败");
      }
    } else {
      const res = await rejectAppReview(id, data);
      if (res.code === 10200) {
        ElMessage.success("已拒绝");
      } else {
        ElMessage.error(res.msg || "操作失败");
      }
    }
    commentDialogVisible.value = false;
    fetchReviews();
  } catch (err: any) {
    ElMessage.error(err.message || "操作失败");
  }
};

const handleDelete = (row: AppReview) => {
  ElMessageBox.confirm("确认删除该审核记录？", "提示", {
    confirmButtonText: "确认",
    cancelButtonText: "取消",
    type: "warning"
  })
    .then(async () => {
      try {
        const res = await deleteAppReview(row.id);
        if (res.code === 10200) {
          ElMessage.success("删除成功");
          fetchReviews();
        } else {
          ElMessage.error(res.msg || "删除失败");
        }
      } catch (err: any) {
        ElMessage.error(err.message || "删除失败");
      }
    })
    .catch(() => {});
};

onMounted(fetchReviews);
</script>

<template>
  <div>
    <div class="bg-bg_color w-[99/100] pl-8 pt-4">
      <el-form :inline="true" class="demo-form-inline">
        <el-form-item label="仓库名称">
          <el-input
            v-model="searchRepo"
            placeholder="搜索仓库名称"
            clearable
            class="w-50!"
          />
        </el-form-item>
        <el-form-item label="状态">
          <el-select
            v-model="searchStatus"
            placeholder="请选择"
            clearable
            style="width: 120px"
          >
            <el-option
              v-for="opt in statusOptions"
              :key="opt.value"
              :label="opt.label"
              :value="opt.value"
            />
          </el-select>
        </el-form-item>
        <el-form-item>
          <el-button
            type="primary"
            :icon="useRenderIcon(Search)"
            @click="onSearch"
          >
            搜索
          </el-button>
          <el-button :icon="useRenderIcon(Refresh)" @click="onReset"
            >重置</el-button
          >
        </el-form-item>
      </el-form>
    </div>

    <PureTableBar title="应用审核" :columns="columns" @refresh="fetchReviews">
      <template v-slot="{ size, dynamicColumns }">
        <pure-table
          border
          adaptive
          :adaptiveConfig="{ offsetBottom: 108 }"
          align-whole="center"

          :loading="loading"
          :size="size"
          :data="reviews"
          :columns="dynamicColumns"
          :pagination="pagination"
          :paginationSmall="size === 'small' ? true : false"
          :header-cell-style="{
            background: 'var(--el-table-row-hover-bg-color)',
            color: 'var(--el-text-color-primary)'
          }"
          @page-size-change="handleSizeChange"
          @page-current-change="handleCurrentChange"
        >
          <template #status="{ row }">
            <el-tag :type="statusTag(row.status).type as any" size="small">
              {{ statusTag(row.status).label }}
            </el-tag>
          </template>
          <template #actions="{ row }">
            <el-button
              v-if="row.status === 'pending'"
              link
              type="success"
              :size="size"
              :icon="useRenderIcon(Check)"
              @click="openCommentDialog(row, 'approve')"
            >
              通过
            </el-button>
            <el-button
              v-if="row.status === 'pending'"
              link
              type="danger"
              :size="size"
              :icon="useRenderIcon(Close)"
              @click="openCommentDialog(row, 'reject')"
            >
              拒绝
            </el-button>
            <el-button
              link
              type="danger"
              :size="size"
              :icon="useRenderIcon(Delete)"
              @click="handleDelete(row)"
            >
              删除
            </el-button>
          </template>
        </pure-table>
      </template>
    </PureTableBar>

    <el-dialog
      v-model="commentDialogVisible"
      :title="currentAction === 'approve' ? '审核通过' : '审核拒绝'"
      width="500px"
    >
      <el-form label-width="80px">
        <el-form-item label="审核意见">
          <el-input
            v-model="commentForm.comment"
            type="textarea"
            :rows="4"
            placeholder="请输入审核意见"
          />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="commentDialogVisible = false">取消</el-button>
        <el-button type="primary" @click="submitAction">确认</el-button>
      </template>
    </el-dialog>
  </div>
</template>
