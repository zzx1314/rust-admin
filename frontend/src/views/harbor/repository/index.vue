<script setup lang="ts">
import { ref, onMounted } from "vue";
import { PureTableBar } from "@/components/RePureTableBar";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import { ElMessage } from "element-plus";
import {
  listProjects,
  listRepositories,
  type HarborProject,
  type HarborRepository
} from "@/api/harbor";

import Search from "~icons/ep/search";
import Refresh from "~icons/ep/refresh";

const loading = ref(false);
const projects = ref<HarborProject[]>([]);
const repositories = ref<HarborRepository[]>([]);
const selectedProject = ref("");
const searchName = ref("");

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
    const res = await listProjects({ page_size: 1000 });
    if (res.code === 10200) {
      projects.value = res.data || [];
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
      page_size: 1000
    });
    if (res.code === 10200) {
      repositories.value = res.data || [];
      if (searchName.value) {
        repositories.value = repositories.value.filter(item =>
          item.name.toLowerCase().includes(searchName.value.toLowerCase())
        );
      }
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取镜像仓库失败");
  } finally {
    loading.value = false;
  }
};

const onSearch = () => fetchRepositories();
const onReset = () => {
  searchName.value = "";
  fetchRepositories();
};

const onProjectChange = () => {
  searchName.value = "";
  fetchRepositories();
};

onMounted(fetchProjects);
</script>

<template>
  <div class="main">
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
        <el-form-item label="仓库名称">
          <el-input
            v-model="searchName"
            placeholder="请输入仓库名称"
            clearable
            class="!w-[200px]"
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
          :header-cell-style="{
            background: 'var(--el-table-row-hover-bg-color)',
            color: 'var(--el-text-color-primary)'
          }"
        />
      </template>
    </PureTableBar>
  </div>
</template>

<style scoped lang="scss">
.main {
  padding: 20px;
}
</style>
