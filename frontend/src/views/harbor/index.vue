<script setup lang="ts">
import { ref, computed } from "vue";
import { useUserStoreHook } from "@/store/modules/user";
import type { TabsPaneContext } from "element-plus";
import ProjectTab from "./tabs/ProjectTab.vue";
import RepositoryTab from "./tabs/RepositoryTab.vue";
import MemberTab from "./tabs/MemberTab.vue";
import ArtifactTab from "./tabs/ArtifactTab.vue";
import ReviewTab from "./tabs/ReviewTab.vue";

const userStore = useUserStoreHook();
const isAdmin = computed(() => {
  const permissions = userStore.permissions ?? [];
  console.log("User permissions:", permissions);
  return (
    permissions.some(permission => {
      const normalized = permission.trim().toLowerCase();
      return normalized === "110";
    }) || ["sysadmin"].includes(userStore.username?.toLowerCase() ?? "")
  );
});
const activeTab = ref<"projects" | "repos" | "members" | "reviews">("projects");
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

const handleSelectRepo = (payload: { project: string; repo: string }) => {
  drillProject.value = payload.project;
  drillRepo.value = payload.repo;
};

const onTabClick = (tab: TabsPaneContext) => {
  if (tab.paneName === "reviews" && !isAdmin.value) {
    activeTab.value = "projects";
    return;
  }
  activeTab.value = tab.paneName as
    | "projects"
    | "repos"
    | "members"
    | "reviews";
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
          <ProjectTab
            v-if="activeTab === 'projects'"
            @select-project="handleSelectProject"
          />
        </el-tab-pane>
        <el-tab-pane label="应用仓库" name="repos">
          <!-- Switch between repository list and artifact detail in the same tab -->
          <RepositoryTab
            v-if="activeTab === 'repos' && !drillRepo"
            :project-name="drillProject"
            @select-repo="handleSelectRepo"
          />
          <ArtifactTab
            v-if="activeTab === 'repos' && drillRepo"
            :project-name="drillProject"
            :repo-name="drillRepo"
          />
        </el-tab-pane>
        <el-tab-pane label="项目成员" name="members">
          <MemberTab v-if="activeTab === 'members'" />
        </el-tab-pane>
        <el-tab-pane v-if="isAdmin" label="应用审核" name="reviews">
          <ReviewTab v-if="activeTab === 'reviews'" />
        </el-tab-pane>
      </el-tabs>
    </div>
  </div>
</template>

<style scoped lang="scss">
.harbor-page {
  padding: 0;
}

.harbor-breadcrumb {
  padding: 8px 12px;
  margin-bottom: 12px;
  background: var(--el-bg-color);
  border-radius: 6px;
}

.breadcrumb-link {
  font-size: 13px;
  color: var(--el-text-color-secondary);
  cursor: pointer;
  transition: color 0.2s;

  &:hover {
    color: var(--el-color-primary);
  }

  &.is-active {
    font-weight: 600;
    color: var(--el-text-color-primary);
    cursor: default;
  }
}

.harbor-tabs-wrapper {
  padding: 0 12px;
  background: var(--el-bg-color);
  border-radius: 6px;
}
</style>
