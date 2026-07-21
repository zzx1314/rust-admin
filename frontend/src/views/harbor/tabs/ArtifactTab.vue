<script setup lang="ts">
import { ref, reactive, watch } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import { PureTableBar } from "@/components/RePureTableBar";
import { ElMessage } from "element-plus";
import {
  listArtifacts,
  type HarborArtifact
} from "@/api/harbor";

const props = defineProps<{
  projectName: string;
  repoName: string;
}>();

const loading = ref(false);
const artifacts = ref<HarborArtifact[]>([]);

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
  { label: "镜像摘要", prop: "digest" }
];

const formatSize = (bytes?: number) => {
  if (!bytes) return "-";
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + " MB";
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

const handleSizeChange = (val: number) => {
  pagination.pageSize = val;
  fetchArtifacts();
};

const handleCurrentChange = (val: number) => {
  pagination.currentPage = val;
  fetchArtifacts();
};

watch(() => [props.projectName, props.repoName], () => {
  pagination.currentPage = 1;
  fetchArtifacts();
}, { immediate: true });
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
          border
          adaptive
          align-whole="center"
          table-layout="auto"
          :key="`artifacts-${repoName}-${artifacts.length}-${pagination.currentPage}`"
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
            <div class="flex flex-wrap gap-1 items-center justify-center w-full">
              <el-tag
                v-for="tag in row.tags || []"
                :key="tag.name"
                size="small"
                type="success"
                effect="plain"
              >
                {{ tag.name }}
              </el-tag>
              <span v-if="!row.tags?.length" class="text-gray-400 text-xs">无标签</span>
            </div>
          </template>
          <template #size="{ row }">
            <span class="text-sm">{{ formatSize(row.size) }}</span>
          </template>
          <template #push_time="{ row }">
            <span>{{ row.push_time?.startsWith('0001-01-01') ? '-' : row.push_time }}</span>
          </template>
          <template #pull_time="{ row }">
            <span>{{ row.pull_time?.startsWith('0001-01-01') ? '-' : row.pull_time }}</span>
          </template>

        </pure-table>
      </template>
    </PureTableBar>
  </div>
</template>
