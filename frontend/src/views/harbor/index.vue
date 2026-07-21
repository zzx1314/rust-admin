<script setup lang="ts">
import { ref, computed } from "vue";
import type { TabsPaneContext } from "element-plus";
import ProjectTab from "./tabs/ProjectTab.vue";
import RepositoryTab from "./tabs/RepositoryTab.vue";
import MemberTab from "./tabs/MemberTab.vue";
import ArtifactTab from "./tabs/ArtifactTab.vue";

const activeTab = ref<"projects" | "repos" | "members">("projects");
const drillProject = ref("");
const drillRepo = ref("");

const showBreadcrumb = computed(
  () => !!drillProject.value || !!drillRepo.value
);

const breadcrumbItems = computed(() => {
  const items = [
    {
      label: "应用管理",
      action: () => {
        activeTab.value = "projects";
        drillProject.value = "";
        drillRepo.value = "";
      }
    }
  ];
  if (drillProject.value) {
    items.push({
      label: drillProject.value,
      action: () => {
        activeTab.value = "repos";
        drillRepo.value = "";
      }
    });
  }
  if (drillRepo.value) {
    items.push({
      label: drillRepo.value.split("/").pop() || drillRepo.value,
      action: () => {}
    });
  }
  return items;
});

const handleSelectProject = (name: string) => {
  drillProject.value = name;
  drillRepo.value = "";
  activeTab.value = "repos";
};

const handleSelectRepo = (name: string) => {
  drillRepo.value = name;
};

const onTabClick = (tab: TabsPaneContext) => {
  activeTab.value = tab.paneName as "projects" | "repos" | "members";
  if (tab.paneName !== "repos") {
    drillProject.value = "";
  }
  drillRepo.value = "";
};
</script>

<template>
  <div class="harbor-page">
    <!-- Breadcrumb -->
    <el-breadcrumb
      v-if="showBreadcrumb"
      class="harbor-breadcrumb"
      separator="/"
    >
      <el-breadcrumb-item v-for="(item, idx) in breadcrumbItems" :key="idx">
        <span
          class="breadcrumb-link"
          :class="{ 'is-active': idx === breadcrumbItems.length - 1 }"
          @click="item.action"
        >
          {{ item.label }}
        </span>
      </el-breadcrumb-item>
    </el-breadcrumb>

    <!-- Tabs -->
    <div class="harbor-tabs-wrapper">
      <el-tabs v-model="activeTab" @tab-click="onTabClick">
        <el-tab-pane label="项目概要" name="projects">
          <ProjectTab @select-project="handleSelectProject" />
        </el-tab-pane>
        <el-tab-pane label="镜像仓库" name="repos">
          <!-- Switch between repository list and artifact detail in the same tab -->
          <RepositoryTab
            v-if="!drillRepo"
            :project-name="drillProject"
            @select-repo="handleSelectRepo"
          />
          <ArtifactTab
            v-else
            :project-name="drillProject"
            :repo-name="drillRepo"
          />
        </el-tab-pane>
        <el-tab-pane label="项目成员" name="members">
          <MemberTab />
        </el-tab-pane>
      </el-tabs>
    </div>
  </div>
</template>

<style scoped lang="scss">
.harbor-page {
  padding: 16px 20px;
}

.harbor-breadcrumb {
  margin-bottom: 12px;
  padding: 8px 12px;
  background: var(--el-bg-color);
  border-radius: 6px;
}

.breadcrumb-link {
  cursor: pointer;
  color: var(--el-text-color-secondary);
  font-size: 13px;
  transition: color 0.2s;

  &:hover {
    color: var(--el-color-primary);
  }

  &.is-active {
    color: var(--el-text-color-primary);
    font-weight: 600;
    cursor: default;
  }
}

.harbor-tabs-wrapper {
  background: var(--el-bg-color);
  border-radius: 6px;
  padding: 8px 16px;
}
</style>
