<script setup lang="ts">
import { ref, reactive, onMounted, watch } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import { PureTableBar } from "@/components/RePureTableBar";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import { ElMessage } from "element-plus";
import {
  listProjects,
  listRepositories,
  getHarborInfo,
  type HarborProject,
  type HarborRepository
} from "@/api/harbor";
import PushCommandDialog from "../components/PushCommandDialog.vue";

import Search from "~icons/ep/search";
import Refresh from "~icons/ep/refresh";

const props = defineProps<{
  projectName: string;
}>();

const emit = defineEmits<{
  (e: "selectRepo", name: string): void;
}>();

const loading = ref(false);
const repositories = ref<HarborRepository[]>([]);
const searchName = ref("");
const registryUrl = ref("");

// Push dialog state
const pushDialogVisible = ref(false);
const pushRepoName = ref("");

// Project selector state (for direct tab access)
const projects = ref<HarborProject[]>([]);
const selectedProject = ref("");

const repoPagination = reactive<PaginationProps>({
  total: 0,
  pageSize: 10,
  currentPage: 1,
  background: true
});

const columns = [
  { label: "仓库名称", prop: "name", slot: "name" },
  { label: "描述", prop: "description" },
  { label: "Artifact 数", prop: "artifact_count" },
  { label: "拉取次数", prop: "pull_count" },
  { label: "创建时间", prop: "creation_time" },
  { label: "更新时间", prop: "update_time" },
  { label: "操作", prop: "actions", slot: "actions", width: 100 }
];

const fetchProjects = async () => {
  try {
    const res = await listProjects({ page_size: 100 });
    if (res.code === 10200 && res.data) {
      projects.value = res.data.records || [];
      if (
        projects.value.length > 0 &&
        !selectedProject.value &&
        !props.projectName
      ) {
        selectedProject.value = projects.value[0].name;
        fetchRepositories();
      }
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取项目列表失败");
  }
};

const fetchRepositories = async () => {
  const project = props.projectName || selectedProject.value;
  if (!project) return;
  loading.value = true;
  try {
    const res = await listRepositories(project, {
      page: repoPagination.currentPage,
      page_size: repoPagination.pageSize
    });
    if (res.code === 10200 && res.data) {
      let records = res.data.records || [];
      if (searchName.value) {
        records = records.filter(item =>
          item.name.toLowerCase().includes(searchName.value.toLowerCase())
        );
      }
      repositories.value = records;
      repoPagination.total = res.data.total;
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取镜像仓库失败");
  } finally {
    loading.value = false;
  }
};

const handleSizeChange = (val: number) => {
  repoPagination.pageSize = val;
  fetchRepositories();
};

const handleCurrentChange = (val: number) => {
  repoPagination.currentPage = val;
  fetchRepositories();
};

const onSearch = () => {
  repoPagination.currentPage = 1;
  fetchRepositories();
};

const onReset = () => {
  searchName.value = "";
  repoPagination.currentPage = 1;
  fetchRepositories();
};

// When projectName prop changes (drill-down), switch to that project
watch(
  () => props.projectName,
  val => {
    if (val) {
      selectedProject.value = val;
      repoPagination.currentPage = 1;
      fetchRepositories();
    }
  },
  { immediate: true }
);

// When select changes in direct mode, fetch repos
watch(selectedProject, val => {
  if (val && !props.projectName) {
    repoPagination.currentPage = 1;
    fetchRepositories();
  }
});

const fetchHarborInfo = async () => {
  try {
    const res = await getHarborInfo();
    if (res.code === 10200 && res.data) {
      registryUrl.value = res.data.registry_url;
    }
  } catch {
    // Use default hostname fallback
  }
};

const openPushDialog = (repoName: string) => {
  const project = props.projectName || selectedProject.value;
  if (!project) return;
  pushRepoName.value = repoName;
  pushDialogVisible.value = true;
};

onMounted(() => {
  fetchHarborInfo();
  if (!props.projectName) {
    fetchProjects();
  }
});
</script>

<template>
  <div>
    <div class="bg-bg_color w-[99/100] pl-8 pt-4 pb-4">
      <el-form :inline="true" class="demo-form-inline">
        <template v-if="!props.projectName">
          <!-- Direct tab mode: project selector -->
          <el-form-item label="所属项目">
            <el-select
              v-model="selectedProject"
              placeholder="请选择项目"
              style="width: 200px"
            >
              <el-option
                v-for="p in projects"
                :key="p.project_id"
                :label="p.name"
                :value="p.name"
              />
            </el-select>
          </el-form-item>
          <el-form-item label="仓库名称">
            <el-input
              v-model="searchName"
              placeholder="搜索仓库名称"
              clearable
              class="w-50!"
            />
          </el-form-item>
          <el-form-item>
            <el-button
              type="primary"
              :icon="useRenderIcon(Search)"
              @click="onSearch"
            >
              搜索
            </el-button>
            <el-button :icon="useRenderIcon(Refresh)" @click="onReset">
              重置
            </el-button>
          </el-form-item>
        </template>
        <template v-else>
          <!-- Drill-down mode: breadcrumb already provides navigation -->
        </template>
      </el-form>
    </div>

    <PureTableBar
      title="镜像仓库"
      :columns="columns"
      @refresh="fetchRepositories"
    >
      <template v-slot="{ size, dynamicColumns }">
        <pure-table
          border
          adaptive
          align-whole="center"
          table-layout="auto"
          :loading="loading"
          :size="size"
          :data="repositories"
          :columns="dynamicColumns"
          :pagination="repoPagination"
          :paginationSmall="size === 'small' ? true : false"
          :header-cell-style="{
            background: 'var(--el-table-row-hover-bg-color)',
            color: 'var(--el-text-color-primary)'
          }"
          @page-size-change="handleSizeChange"
          @page-current-change="handleCurrentChange"
        >
          <template #name="{ row }">
            <span class="repo-name-link" @click="emit('selectRepo', row.name)">
              <IconifyIconOffline
                icon="ep:box"
                width="14"
                height="14"
                class="mr-1"
              />
              {{ row.name.split("/").pop() }}
            </span>
          </template>
          <template #actions="{ row }">
            <el-tooltip content="推送命令" placement="top" :show-after="300">
              <el-button
                size="small"
                circle
                type="warning"
                plain
                @click="openPushDialog(row.name)"
              >
                <IconifyIconOffline icon="ep:upload" width="14" height="14" />
              </el-button>
            </el-tooltip>
          </template>
        </pure-table>
      </template>
    </PureTableBar>

    <!-- Push Command Dialog -->
    <PushCommandDialog
      v-model:visible="pushDialogVisible"
      :project-name="props.projectName || selectedProject"
      :repo-name="pushRepoName"
      :registry-url="registryUrl"
    />
  </div>
</template>

<style scoped>
.repo-name-link {
  cursor: pointer;
  color: var(--el-color-primary);
  display: inline-flex;
  align-items: center;
  transition: color 0.2s;
}
.repo-name-link:hover {
  color: var(--el-color-primary-dark-2);
  text-decoration: underline;
}
</style>
