<script setup lang="ts">
import { ref, reactive, onMounted } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import { PureTableBar } from "@/components/RePureTableBar";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import { ElMessage } from "element-plus";
import {
  listProjects,
  listRepositories,
  type HarborProject,
  type HarborRepository
} from "@/api/harbor";

import Refresh from "~icons/ep/refresh";

const loading = ref(false);
const projects = ref<HarborProject[]>([]);
const repositories = ref<HarborRepository[]>([]);
const selectedProject = ref("");
const repoPagination = reactive<PaginationProps>({
  total: 0,
  pageSize: 10,
  currentPage: 1,
  background: true
});

const columns = [
  { label: "仓库名称", prop: "name" },
  { label: "描述", prop: "description" },
  { label: "Artifact 数", prop: "artifact_count" },
  { label: "拉取次数", prop: "pull_count" },
  { label: "创建时间", prop: "creation_time" },
  { label: "更新时间", prop: "update_time" }
];

const fetchProjects = async () => {
  try {
    const res = await listProjects({ page_size: 100 });
    if (res.code === 10200 && res.data) {
      projects.value = res.data.records || [];
      if (projects.value.length > 0 && !selectedProject.value) {
        selectedProject.value = projects.value[0].name;
        fetchRepositories();
      }
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取项目失败");
  }
};

const fetchRepositories = async () => {
  if (!selectedProject.value) return;
  loading.value = true;
  try {
    const res = await listRepositories(selectedProject.value, {
      page: repoPagination.currentPage,
      page_size: repoPagination.pageSize
    });
    if (res.code === 10200 && res.data) {
      repositories.value = res.data.records || [];
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

const onProjectChange = () => {
  repoPagination.currentPage = 1;
  fetchRepositories();
};

onMounted(fetchProjects);
</script>

<template>
  <div>
    <div class="bg-bg_color w-[99/100] pl-8 pt-4 pb-4">
      <el-form :inline="true" class="demo-form-inline">
        <el-form-item label="所属项目">
          <el-select
            v-model="selectedProject"
            placeholder="请选择项目"
            style="width: 200px"
            @change="onProjectChange"
          >
            <el-option
              v-for="p in projects"
              :key="p.project_id"
              :label="p.name"
              :value="p.name"
            />
          </el-select>
        </el-form-item>
        <el-form-item>
          <el-button :icon="useRenderIcon(Refresh)" @click="fetchRepositories">
            刷新
          </el-button>
        </el-form-item>
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
        />
      </template>
    </PureTableBar>
  </div>
</template>
