<script setup lang="ts">
import { ref, reactive, onMounted } from "vue";
import type { PaginationProps } from "@pureadmin/table";
import { PureTableBar } from "@/components/RePureTableBar";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import { ElMessage, ElMessageBox } from "element-plus";
import {
  listProjects,
  createProject,
  deleteProject,
  getProjectSummary,
  type HarborProject,
  type ProjectSummary
} from "@/api/harbor";

import Search from "~icons/ep/search";
import Refresh from "~icons/ep/refresh";
import AddFill from "~icons/ri/add-circle-line";
import Delete from "~icons/ep/delete";
import View from "~icons/ep/view";

const loading = ref(false);
const projectList = ref<HarborProject[]>([]);
const searchName = ref("");
const dialogVisible = ref(false);
const addForm = ref({ project_name: "", is_public: false });
const summaryVisible = ref(false);
const currentSummary = ref<ProjectSummary | null>(null);
const currentProjectName = ref("");

const pagination = reactive<PaginationProps>({
  total: 0,
  pageSize: 10,
  currentPage: 1,
  background: true
});

const columns: TableColumnList = [
  { label: "项目名称", prop: "name" },
  { label: "所有者", prop: "owner_name" },
  { label: "仓库数", prop: "repo_count" },
  { label: "公开", prop: "is_public" },
  { label: "创建时间", prop: "creation_time" },
  { label: "操作", fixed: "right", width: 200, slot: "operation" }
];

const fetchProjects = async () => {
  loading.value = true;
  try {
    const res = await listProjects({
      name: searchName.value || undefined,
      page: pagination.currentPage,
      page_size: pagination.pageSize
    });
    if (res.code === 10200 && res.data) {
      projectList.value =
        res.data.records?.map(item => ({
          ...item,
          is_public: item.metadata?.public === "true"
        })) || [];
      pagination.total = res.data.total;
    }
  } finally {
    loading.value = false;
  }
};

const handleSizeChange = (val: number) => {
  pagination.pageSize = val;
  fetchProjects();
};

const handleCurrentChange = (val: number) => {
  pagination.currentPage = val;
  fetchProjects();
};

const onSearch = () => {
  pagination.currentPage = 1;
  fetchProjects();
};
const onReset = () => {
  searchName.value = "";
  pagination.currentPage = 1;
  fetchProjects();
};

const handleCreate = async () => {
  try {
    const res = await createProject({
      project_name: addForm.value.project_name,
      metadata: { public: addForm.value.is_public ? "true" : "false" }
    });
    if (res.code === 10200) {
      ElMessage.success("创建成功");
      dialogVisible.value = false;
      addForm.value = { project_name: "", is_public: false };
      fetchProjects();
    } else {
      ElMessage.error(res.msg || "创建失败");
    }
  } catch (err: any) {
    ElMessage.error(err.message || "创建失败");
  }
};

const handleDelete = (row: HarborProject) => {
  ElMessageBox.confirm(`确认删除项目 ${row.name} 吗？`, "提示", {
    confirmButtonText: "确认",
    cancelButtonText: "取消",
    type: "warning"
  })
    .then(async () => {
      const res = await deleteProject(row.name);
      if (res.code === 10200) {
        ElMessage.success("删除成功");
        fetchProjects();
      } else {
        ElMessage.error(res.msg || "删除失败");
      }
    })
    .catch(() => {});
};

const viewSummary = async (row: HarborProject) => {
  currentProjectName.value = row.name;
  try {
    const res = await getProjectSummary(row.name);
    if (res.code === 10200) {
      currentSummary.value = res.data || null;
      summaryVisible.value = true;
    } else {
      ElMessage.error(res.msg || "获取概要失败");
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取概要失败");
  }
};

onMounted(fetchProjects);
</script>

<template>
  <div>
    <div class="bg-bg_color w-[99/100] pl-8 pt-4 pb-4">
      <el-form :inline="true" class="demo-form-inline">
        <el-form-item label="项目名称">
          <el-input
            v-model="searchName"
            placeholder="请输入项目名称"
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

    <PureTableBar title="项目概要" :columns="columns" @refresh="fetchProjects">
      <template #buttons>
        <el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="dialogVisible = true"
        >
          新增项目
        </el-button>
      </template>
      <template v-slot="{ size, dynamicColumns }">
        <pure-table
          border
          adaptive
          align-whole="center"
          table-layout="auto"
          :loading="loading"
          :size="size"
          :data="projectList"
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
          <template #is_public="{ row }">
            <el-tag :type="row.is_public ? 'success' : 'info'">
              {{ row.is_public ? "公开" : "私有" }}
            </el-tag>
          </template>
          <template #operation="{ row }">
            <el-button
              link
              type="primary"
              :size="size"
              :icon="useRenderIcon(View)"
              @click="viewSummary(row)"
            >
              概要
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

    <el-dialog v-model="dialogVisible" title="新增项目" width="450px">
      <el-form label-width="120px">
        <el-form-item label="项目名称">
          <el-input
            v-model="addForm.project_name"
            placeholder="请输入项目名称"
          />
        </el-form-item>
        <el-form-item label="是否公开">
          <el-switch v-model="addForm.is_public" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dialogVisible = false">取消</el-button>
        <el-button type="primary" @click="handleCreate">确认</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="summaryVisible" title="项目概要" width="500px">
      <div v-if="currentSummary">
        <p><strong>项目：</strong>{{ currentProjectName }}</p>
        <p><strong>仓库数：</strong>{{ currentSummary.repo_count }}</p>
        <p>
          <strong>管理员：</strong>{{ currentSummary.project_admin_count || 0 }}
        </p>
        <p>
          <strong>维护者：</strong>{{ currentSummary.maintainer_count || 0 }}
        </p>
        <p>
          <strong>开发者：</strong>{{ currentSummary.developer_count || 0 }}
        </p>
        <p><strong>访客：</strong>{{ currentSummary.guest_count || 0 }}</p>
      </div>
    </el-dialog>
  </div>
</template>
