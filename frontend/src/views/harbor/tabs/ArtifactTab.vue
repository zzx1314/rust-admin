<script setup lang="ts">
import { ref, reactive, watch, onMounted, computed } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import { PureTableBar } from "@/components/RePureTableBar";
import { ElMessage } from "element-plus";
import { listArtifacts, type HarborArtifact } from "@/api/harbor";
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
  { label: "命令", prop: "commands", slot: "commands", width: 100 }
];

const formatSize = (bytes?: number) => {
  if (!bytes) return "-";
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  if (bytes < 1024 * 1024 * 1024)
    return (bytes / (1024 * 1024)).toFixed(1) + " MB";
  return (bytes / (1024 * 1024 * 1024)).toFixed(2) + " GB";
};

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
          <template #commands="{ row }">
            <el-tooltip
              content="查看镜像命令"
              placement="top"
              :show-after="300"
            >
              <el-button
                size="small"
                circle
                type="primary"
                plain
                @click="openCommandDialog(row)"
              >
                <IconifyIconOffline icon="ep:terminal" width="14" height="14" />
              </el-button>
            </el-tooltip>
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
  </div>
</template>
