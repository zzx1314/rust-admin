<script setup lang="ts">
import { ref, reactive, watch, onMounted, computed } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import { PureTableBar } from "@/components/RePureTableBar";
import { ElMessage, ElMessageBox } from "element-plus";
import {
  listArtifacts,
  deleteArtifact,
  type HarborArtifact
} from "@/api/harbor";
import {
  createAppReview,
  getAppReviews,
  type AppReview
} from "@/api/appReview";
import { useHarborStoreHook } from "@/store/modules/harbor";
import ImageCommandDialog from "../components/ImageCommandDialog.vue";

const props = defineProps<{
  projectName: string;
  repoName: string;
}>();

const loading = ref(false);
const artifacts = ref<HarborArtifact[]>([]);
const harborStore = useHarborStoreHook();
const registryUrl = computed(() => harborStore.registryUrl);
const commandDialogVisible = ref(false);
const selectedArtifact = ref<HarborArtifact | null>(null);

const reviewDialogVisible = ref(false);
const reviewForm = ref({
  srcProject: props.projectName,
  destProject: "production-project",
  repositoryName: props.repoName,
  tag: "",
  digest: "",
  artifactId: undefined as number | undefined,
  reviewerComment: ""
});

const pagination = reactive<PaginationProps>({
  total: 0,
  pageSize: 10,
  currentPage: 1,
  background: true
});

const columns = [
  { label: "标签", prop: "tags", slot: "tags" },
  { label: "大小", prop: "size", slot: "size" },
  { label: "推送时间", prop: "push_time", slot: "push_time" },
  { label: "拉取时间", prop: "pull_time", slot: "pull_time" },
  { label: "镜像摘要", prop: "digest" },
  { label: "审核状态", prop: "reviewStatus", slot: "reviewStatus", width: 120 },
  { label: "操作", prop: "actions", slot: "actions", width: 220 }
];

const formatSize = (bytes?: number) => {
  if (!bytes) return "-";
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  if (bytes < 1024 * 1024 * 1024)
    return (bytes / (1024 * 1024)).toFixed(1) + " MB";
  return (bytes / (1024 * 1024 * 1024)).toFixed(2) + " GB";
};
const statusTag = (status: AppReview["status"]) => {
  if (status === "approved")
    return { type: "success" as const, label: "审核通过" };
  if (status === "rejected")
    return { type: "danger" as const, label: "审核拒绝" };
  return { type: "warning" as const, label: "待审核" };
};

const getArtifactStatus = (artifact: HarborArtifact) => artifact.review_status;

const fetchArtifacts = async () => {
  if (!props.projectName || !props.repoName) return;
  loading.value = true;
  try {
    const res = await listArtifacts(props.projectName, props.repoName, {
      page: pagination.currentPage,
      page_size: pagination.pageSize
    });
    if (res.code === 10200 && res.data) {
      artifacts.value = res.data.records || [];
      pagination.total = res.data.total;
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取制品列表失败");
  } finally {
    loading.value = false;
  }
};

const openCommandDialog = (artifact: HarborArtifact) => {
  selectedArtifact.value = artifact;
  commandDialogVisible.value = true;
};

const handleDelete = (artifact: HarborArtifact) => {
  if (!props.projectName || !props.repoName || !artifact.digest) return;
  ElMessageBox.confirm("确认删除该 Artifact 吗？", "提示", {
    confirmButtonText: "确认",
    cancelButtonText: "取消",
    type: "warning"
  })
    .then(async () => {
      try {
        const shortName = props.repoName.split("/").pop() || props.repoName;
        const res = await deleteArtifact(
          props.projectName,
          shortName,
          artifact.digest as string
        );
        if (res.code === 10200) {
          ElMessage.success("删除成功");
          fetchArtifacts();
        } else {
          ElMessage.error(res.msg || "删除失败");
        }
      } catch (err: any) {
        ElMessage.error(err.message || "删除失败");
      }
    })
    .catch(() => {});
};

const openReviewDialog = (artifact: HarborArtifact) => {
  const firstTag = artifact.tags?.[0]?.name || "";
  reviewForm.value = {
    srcProject: props.projectName,
    destProject: "production-project",
    repositoryName: props.repoName,
    tag: firstTag,
    digest: artifact.digest || "",
    artifactId: artifact.id,
    reviewerComment: ""
  };
  selectedArtifact.value = artifact;
  reviewDialogVisible.value = true;
};

const submitReview = async () => {
  if (!reviewForm.value.tag) {
    ElMessage.warning("请选择或填写 Tag");
    return;
  }
  try {
    const res = await createAppReview({
      srcProject: reviewForm.value.srcProject,
      destProject: reviewForm.value.destProject,
      repositoryName: reviewForm.value.repositoryName,
      tag: reviewForm.value.tag,
      digest: reviewForm.value.digest || undefined,
      artifactId: reviewForm.value.artifactId,
      reviewerComment: reviewForm.value.reviewerComment || undefined
    });
    if (res.code === 10200) {
      ElMessage.success("已创建审核记录");
      reviewDialogVisible.value = false;
    } else {
      ElMessage.error(res.msg || "创建审核记录失败");
    }
  } catch (err: any) {
    ElMessage.error(err.message || "创建审核记录失败");
  }
};

const handleSizeChange = (val: number) => {
  pagination.pageSize = val;
  fetchArtifacts();
};

const handleCurrentChange = (val: number) => {
  pagination.currentPage = val;
  fetchArtifacts();
};

watch(
  () => [props.projectName, props.repoName],
  () => {
    pagination.currentPage = 1;
    fetchArtifacts();
  },
  { immediate: true }
);

onMounted(() => {
  harborStore.fetchRegistryUrl();
});
</script>

<template>
  <div>
    <PureTableBar
      title="Artifacts"
      :columns="columns"
      @refresh="fetchArtifacts"
    >
      <template v-slot="{ size, dynamicColumns }">
        <pure-table
          :key="`artifacts-${repoName}-${artifacts.length}-${pagination.currentPage}`"
          border
          adaptive
          align-whole="center"
          table-layout="auto"
          :loading="loading"
          :size="size"
          :data="artifacts"
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
          <template #tags="{ row }">
            <div
              class="flex flex-wrap gap-1 items-center justify-center w-full"
            >
              <el-tag
                v-for="tag in row.tags || []"
                :key="tag.name"
                size="small"
                type="success"
                effect="plain"
              >
                {{ tag.name }}
              </el-tag>
              <span v-if="!row.tags?.length" class="text-gray-400 text-xs"
                >无标签</span
              >
            </div>
          </template>
          <template #size="{ row }">
            <span class="text-sm">{{ formatSize(row.size) }}</span>
          </template>
          <template #push_time="{ row }">
            <span>{{
              row.push_time?.startsWith("0001-01-01") ? "-" : row.push_time
            }}</span>
          </template>
          <template #pull_time="{ row }">
            <span>{{
              row.pull_time?.startsWith("0001-01-01") ? "-" : row.pull_time
            }}</span>
          </template>
          <template #reviewStatus="{ row }">
            <el-tag
              :type="statusTag(getArtifactStatus(row) || 'pending').type"
              size="small"
            >
              {{
                getArtifactStatus(row)
                  ? statusTag(getArtifactStatus(row)!).label
                  : "未提交"
              }}
            </el-tag>
          </template>
          <template #actions="{ row }">
            <el-button
              link
              type="primary"
              :size="size"
              @click="openCommandDialog(row)"
            >
              镜像命令
            </el-button>
            <el-button
              link
              type="warning"
              :size="size"
              :disabled="
                getArtifactStatus(row) === 'pending' ||
                getArtifactStatus(row) === 'approved'
              "
              @click="openReviewDialog(row)"
            >
              {{ getArtifactStatus(row) === "rejected" ? "重新审核" : "审核" }}
            </el-button>
            <el-button
              link
              type="danger"
              :size="size"
              @click="handleDelete(row)"
            >
              删除
            </el-button>
          </template>
        </pure-table>
      </template>
    </PureTableBar>

    <!-- Image Command Dialog -->
    <ImageCommandDialog
      v-model:visible="commandDialogVisible"
      :artifact="selectedArtifact"
      :project-name="props.projectName"
      :repo-name="props.repoName"
      :registry-url="registryUrl"
    />

    <!-- Create Review Dialog -->
    <el-dialog v-model="reviewDialogVisible" title="发起应用审核" width="500px">
      <el-form label-width="100px">
        <el-form-item label="源项目">
          <el-input v-model="reviewForm.srcProject" disabled />
        </el-form-item>
        <el-form-item label="目标项目">
          <el-input
            v-model="reviewForm.destProject"
            placeholder="production-project"
          />
        </el-form-item>
        <el-form-item label="仓库">
          <el-input v-model="reviewForm.repositoryName" disabled />
        </el-form-item>
        <el-form-item label="Tag">
          <el-select
            v-if="selectedArtifact?.tags?.length"
            v-model="reviewForm.tag"
            placeholder="选择 Tag"
            style="width: 100%"
          >
            <el-option
              v-for="t in selectedArtifact.tags"
              :key="t.name"
              :label="t.name"
              :value="t.name"
            />
          </el-select>
          <el-input v-else v-model="reviewForm.tag" placeholder="请输入 Tag" />
        </el-form-item>
        <el-form-item label="摘要">
          <el-input v-model="reviewForm.digest" disabled />
        </el-form-item>
        <el-form-item label="备注">
          <el-input
            v-model="reviewForm.reviewerComment"
            type="textarea"
            :rows="2"
            placeholder="请输入备注"
          />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="reviewDialogVisible = false">取消</el-button>
        <el-button type="primary" @click="submitReview">确认</el-button>
      </template>
    </el-dialog>
  </div>
</template>
