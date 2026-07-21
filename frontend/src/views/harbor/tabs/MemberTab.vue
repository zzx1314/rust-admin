<script setup lang="ts">
import { ref, onMounted } from "vue";
import { PureTableBar } from "@/components/RePureTableBar";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import { ElMessage, ElMessageBox } from "element-plus";
import {
  listProjects,
  listMembers,
  addMember,
  removeMember,
  type HarborProject,
  type HarborMember
} from "@/api/harbor";

import Search from "~icons/ep/search";
import Refresh from "~icons/ep/refresh";
import AddFill from "~icons/ri/add-circle-line";
import Delete from "~icons/ep/delete";

const loading = ref(false);
const projects = ref<HarborProject[]>([]);
const members = ref<HarborMember[]>([]);
const selectedProject = ref("");
const searchName = ref("");
const dialogVisible = ref(false);
const addForm = ref({
  entity_name: "",
  role_id: 2
});

const roleOptions = [
  { label: "项目管理员", value: 1 },
  { label: "开发者", value: 2 },
  { label: "访客", value: 3 },
  { label: "维护者", value: 4 }
];

const columns = [
  { label: "成员名称", prop: "entity_name" },
  { label: "角色", prop: "role_name" },
  { label: "实体类型", prop: "entity_type" },
  { operation: "操作", width: 120 }
];

const fetchProjects = async () => {
  try {
    const res = await listProjects({ page_size: 100 });
    if (res.code === 10200) {
      projects.value = res.data || [];
      if (projects.value.length > 0 && !selectedProject.value) {
        selectedProject.value = projects.value[0].name;
        fetchMembers();
      }
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取项目失败");
  }
};

const fetchMembers = async () => {
  if (!selectedProject.value) return;
  loading.value = true;
  try {
    const res = await listMembers(selectedProject.value, { page_size: 100 });
    if (res.code === 10200) {
      let data = res.data || [];
      if (searchName.value) {
        data = data.filter(item =>
          item.entity_name
            .toLowerCase()
            .includes(searchName.value.toLowerCase())
        );
      }
      members.value = data;
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取成员失败");
  } finally {
    loading.value = false;
  }
};

const onSearch = () => fetchMembers();
const onReset = () => {
  searchName.value = "";
  fetchMembers();
};

const onProjectChange = () => {
  searchName.value = "";
  fetchMembers();
};

const handleAdd = async () => {
  try {
    const res = await addMember(selectedProject.value, {
      role_id: addForm.value.role_id,
      member_user: { username: addForm.value.entity_name }
    });
    if (res.code === 10200) {
      ElMessage.success("添加成功");
      dialogVisible.value = false;
      addForm.value = { entity_name: "", role_id: 2 };
      fetchMembers();
    } else {
      ElMessage.error(res.msg || "添加失败");
    }
  } catch (err: any) {
    ElMessage.error(err.message || "添加失败");
  }
};

const handleDelete = (row: HarborMember) => {
  ElMessageBox.confirm(`确认移除成员 ${row.entity_name} 吗？`, "提示", {
    confirmButtonText: "确认",
    cancelButtonText: "取消",
    type: "warning"
  })
    .then(async () => {
      const res = await removeMember(selectedProject.value, row.id);
      if (res.code === 10200) {
        ElMessage.success("移除成功");
        fetchMembers();
      } else {
        ElMessage.error(res.msg || "移除失败");
      }
    })
    .catch(() => {});
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
        <el-form-item label="成员名称">
          <el-input
            v-model="searchName"
            placeholder="请输入成员名称"
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
          <el-button
            type="primary"
            :icon="useRenderIcon(AddFill)"
            @click="dialogVisible = true"
          >
            添加成员
          </el-button>
        </el-form-item>
      </el-form>
    </div>

    <PureTableBar title="项目成员" :columns="columns" @refresh="fetchMembers">
      <template v-slot="{ size, dynamicColumns }">
        <pure-table
          border
          adaptive
          align-whole="center"
          table-layout="auto"
          :loading="loading"
          :size="size"
          :data="members"
          :columns="dynamicColumns"
          :header-cell-style="{
            background: 'var(--el-table-row-hover-bg-color)',
            color: 'var(--el-text-color-primary)'
          }"
        >
          <template #operation="{ row }">
            <el-button
              link
              type="danger"
              :size="size"
              :icon="useRenderIcon(Delete)"
              @click="handleDelete(row)"
            >
              移除
            </el-button>
          </template>
        </pure-table>
      </template>
    </PureTableBar>

    <el-dialog v-model="dialogVisible" title="添加成员" width="450px">
      <el-form label-width="120px">
        <el-form-item label="用户名">
          <el-input v-model="addForm.entity_name" placeholder="请输入用户名" />
        </el-form-item>
        <el-form-item label="角色">
          <el-select v-model="addForm.role_id" placeholder="请选择角色">
            <el-option
              v-for="role in roleOptions"
              :key="role.value"
              :label="role.label"
              :value="role.value"
            />
          </el-select>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dialogVisible = false">取消</el-button>
        <el-button type="primary" @click="handleAdd">确认</el-button>
      </template>
    </el-dialog>
  </div>
</template>
