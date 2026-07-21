<script setup lang="ts">
import { ref } from "vue";
import ProjectTab from "./tabs/ProjectTab.vue";
import RepositoryTab from "./tabs/RepositoryTab.vue";
import MemberTab from "./tabs/MemberTab.vue";

const activeTab = ref("project");
</script>

<template>
  <div class="harbor-page">
    <el-tabs v-model="activeTab" type="border-card" class="harbor-tabs">
      <el-tab-pane label="项目概要" name="project" lazy>
        <ProjectTab />
      </el-tab-pane>
      <el-tab-pane label="镜像仓库" name="repository" lazy>
        <RepositoryTab />
      </el-tab-pane>
      <el-tab-pane label="项目成员" name="member" lazy>
        <MemberTab />
      </el-tab-pane>
    </el-tabs>
  </div>
</template>

<style scoped lang="scss">
.harbor-page {
  padding: 20px;
}

.harbor-tabs {
  background: var(--el-bg-color);
}
</style>
