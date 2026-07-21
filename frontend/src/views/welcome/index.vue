<script setup lang="ts">
import { ref, onMounted, reactive } from "vue";
import ReCol from "@/components/ReCol";
import { useDark } from "./utils";
import { ReNormalCountTo } from "@/components/ReCountTo";
import { getHarborStatistics, type HarborStatistics, type RepoStat } from "@/api/harbor";
import { ElMessage } from "element-plus";

import StoreLine from "~icons/ri/store-2-line";
import Box from "~icons/ep/box";
import Download from "~icons/ep/download";
import Fire from "~icons/ri/fire-line";

defineOptions({
  name: "Welcome"
});

const { isDark } = useDark();
const loading = ref(true);
const stats = ref<HarborStatistics | null>(null);

const summaryCards = ref([
  {
    icon: StoreLine,
    bgColor: "#effaff",
    color: "#41b6ff",
    name: "总应用数",
    value: 0,
    suffix: "个"
  },
  {
    icon: Box,
    bgColor: "#fff5f4",
    color: "#e85f33",
    name: "总镜像数",
    value: 0,
    suffix: "个"
  },
  {
    icon: Download,
    bgColor: "#eff8f4",
    color: "#26ce83",
    name: "总拉取量",
    value: 0,
    suffix: "次"
  },
  {
    icon: Fire,
    bgColor: "#f6f4fe",
    color: "#7846e5",
    name: "活跃项目",
    value: 0,
    suffix: "个"
  }
]);

const fetchStatistics = async () => {
  loading.value = true;
  try {
    const res = await getHarborStatistics();
    if (res.code === 10200 && res.data) {
      stats.value = res.data;
      summaryCards.value[0].value = res.data.total_projects;
      summaryCards.value[1].value = res.data.total_repositories;
      summaryCards.value[2].value = res.data.total_pull_count;
      summaryCards.value[3].value = res.data.total_projects;
    }
  } catch (err: any) {
    ElMessage.error(err.message || "获取统计数据失败");
  } finally {
    loading.value = false;
  }
};

const publicPercent = ref(0);

onMounted(async () => {
  await fetchStatistics();
  if (stats.value) {
    const total = stats.value.public_project_count + stats.value.private_project_count;
    publicPercent.value = total > 0
      ? Math.round((stats.value.public_project_count / total) * 100)
      : 0;
  }
});
</script>

<template>
  <div v-loading="loading">
    <el-row :gutter="24" justify="space-around">
      <!-- 概览卡片 -->
      <template v-for="(item, index) in summaryCards" :key="index">
        <re-col
          v-motion
          class="mb-4.5"
          :value="6"
          :md="12"
          :sm="12"
          :xs="24"
          :initial="{ opacity: 0, y: 100 }"
          :enter="{
            opacity: 1,
            y: 0,
            transition: { delay: 80 * (index + 1) }
          }"
        >
          <el-card class="stat-card" shadow="never">
            <div class="flex justify-between">
              <span class="text-md font-medium">{{ item.name }}</span>
              <div
                class="w-10 h-10 flex justify-center items-center rounded-lg"
                :style="{ backgroundColor: isDark ? 'transparent' : item.bgColor }"
              >
                <IconifyIconOffline
                  :icon="item.icon"
                  :color="item.color"
                  width="22"
                  height="22"
                />
              </div>
            </div>
            <div class="mt-3">
              <ReNormalCountTo
                :duration="1800 + index * 200"
                :fontSize="'2em'"
                :startVal="0"
                :endVal="item.value"
              />
              <p class="text-xs text-gray-400 mt-1">{{ item.suffix }}</p>
            </div>
          </el-card>
        </re-col>
      </template>

      <!-- 热门排行 + 最新项目 -->
      <re-col
        v-motion
        class="mb-4.5"
        :value="12"
        :xs="24"
        :initial="{ opacity: 0, y: 100 }"
        :enter="{ opacity: 1, y: 0, transition: { delay: 400 } }"
      >
        <el-card class="rank-card" shadow="never">
          <div class="flex justify-between items-center mb-4">
            <span class="text-md font-medium flex items-center gap-2">
              <IconifyIconOffline
                icon="ri:hotspot-line"
                color="#e85f33"
                width="20"
                height="20"
              />
              热门下载 TOP 5
            </span>
          </div>
          <div v-if="stats?.top_repositories?.length" class="rank-list">
            <div
              v-for="(repo, idx) in stats.top_repositories"
              :key="idx"
              class="rank-item flex items-center justify-between py-3 px-2 rounded-lg transition-colors hover:bg-gray-50 dark:hover:bg-gray-800"
            >
              <div class="flex items-center gap-3 min-w-0 flex-1">
                <span
                  class="rank-num w-6 h-6 flex items-center justify-center rounded-full text-xs font-bold"
                  :class="{
                    'bg-orange-500 text-white': idx === 0,
                    'bg-orange-400 text-white': idx === 1,
                    'bg-orange-300 text-white': idx === 2,
                    'bg-gray-200 text-gray-500 dark:bg-gray-700': idx >= 3
                  }"
                >
                  {{ idx + 1 }}
                </span>
                <div class="min-w-0 flex-1">
                  <p class="text-sm font-medium truncate">{{ repo.project_name }}/{{ repo.name.split('/').pop() }}</p>
                  <p class="text-xs text-gray-400 truncate">{{ repo.project_name }}</p>
                </div>
              </div>
              <div class="text-right flex-shrink-0 ml-3">
                <p class="text-sm font-semibold text-orange-500">{{ repo.pull_count.toLocaleString() }}</p>
                <p class="text-xs text-gray-400">拉取</p>
              </div>
              <div class="text-right flex-shrink-0 ml-4">
                <p class="text-sm font-semibold">{{ repo.artifact_count }}</p>
                <p class="text-xs text-gray-400">制品</p>
              </div>
            </div>
          </div>
          <el-empty v-else description="暂无数据" :image-size="60" />
        </el-card>
      </re-col>

      <!-- 最新项目 -->
      <re-col
        v-motion
        class="mb-4.5"
        :value="12"
        :xs="24"
        :initial="{ opacity: 0, y: 100 }"
        :enter="{ opacity: 1, y: 0, transition: { delay: 480 } }"
      >
        <el-card class="recent-card" shadow="never">
          <div class="flex justify-between items-center mb-4">
            <span class="text-md font-medium flex items-center gap-2">
              <IconifyIconOffline
                icon="ep:clock"
                color="#41b6ff"
                width="20"
                height="20"
              />
              最新上架
            </span>
          </div>
          <div v-if="stats?.recent_projects?.length" class="recent-list">
            <div
              v-for="(project, idx) in stats.recent_projects"
              :key="idx"
              class="recent-item flex items-center justify-between py-3 px-2 rounded-lg transition-colors hover:bg-gray-50 dark:hover:bg-gray-800 border-b border-gray-100 dark:border-gray-700 last:border-0"
            >
              <div class="flex items-center gap-3 min-w-0 flex-1">
                <div
                  class="w-8 h-8 flex items-center justify-center rounded-lg"
                  :style="{ backgroundColor: isDark ? '#374151' : '#f0f9ff' }"
                >
                  <IconifyIconOffline
                    icon="ri:store-2-line"
                    color="#41b6ff"
                    width="16"
                    height="16"
                  />
                </div>
                <div class="min-w-0">
                  <p class="text-sm font-medium truncate">{{ project.name }}</p>
                  <p class="text-xs text-gray-400">
                    {{ project.repo_count || 0 }} 个仓库
                    <template v-if="project.metadata?.public === 'true'">
                      <el-tag size="small" type="success" class="ml-1">公开</el-tag>
                    </template>
                    <template v-else>
                      <el-tag size="small" type="info" class="ml-1">私有</el-tag>
                    </template>
                  </p>
                </div>
              </div>
              <p class="text-xs text-gray-400 flex-shrink-0 ml-3">{{ project.creation_time }}</p>
            </div>
          </div>
          <el-empty v-else description="暂无数据" :image-size="60" />
        </el-card>
      </re-col>

      <!-- 项目分布 -->
      <re-col
        v-motion
        class="mb-4.5"
        :value="12"
        :xs="24"
        :initial="{ opacity: 0, y: 100 }"
        :enter="{ opacity: 1, y: 0, transition: { delay: 560 } }"
      >
        <el-card class="dist-card equal-height-card" shadow="never">
          <div class="flex justify-between items-center mb-4">
            <span class="text-md font-medium">项目分布</span>
          </div>
          <div class="flex items-center justify-center py-4">
            <div class="text-center">
              <div class="flex items-center justify-center gap-8 flex-wrap">
                <div class="text-center">
                  <div class="w-16 h-16 rounded-full bg-green-100 dark:bg-green-900/30 flex items-center justify-center mx-auto">
                    <span class="text-2xl font-bold text-green-500">{{ stats?.public_project_count || 0 }}</span>
                  </div>
                  <p class="text-xs text-gray-500 mt-2">
                    <span class="inline-block w-2 h-2 rounded-full bg-green-500 mr-1"></span>
                    公开项目
                  </p>
                </div>
                <div class="text-center">
                  <div class="w-16 h-16 rounded-full bg-blue-100 dark:bg-blue-900/30 flex items-center justify-center mx-auto">
                    <span class="text-2xl font-bold text-blue-500">{{ stats?.private_project_count || 0 }}</span>
                  </div>
                  <p class="text-xs text-gray-500 mt-2">
                    <span class="inline-block w-2 h-2 rounded-full bg-blue-500 mr-1"></span>
                    私有项目
                  </p>
                </div>
              </div>
              <el-progress
                :percentage="publicPercent"
                :stroke-width="12"
                striped
                striped-flow
                class="mt-6 max-w-xs mx-auto"
                color="#26ce83"
              >
                <span class="text-xs">公开占比 {{ publicPercent }}%</span>
              </el-progress>
            </div>
          </div>
        </el-card>
      </re-col>

      <!-- 汇总信息 -->
      <re-col
        v-motion
        class="mb-4.5"
        :value="12"
        :xs="24"
        :initial="{ opacity: 0, y: 100 }"
        :enter="{ opacity: 1, y: 0, transition: { delay: 640 } }"
      >
        <el-card class="summary-card equal-height-card" shadow="never">
          <div class="flex justify-between items-center mb-4">
            <span class="text-md font-medium">数据汇总</span>
          </div>
          <div class="grid grid-cols-2 gap-4 py-2">
            <div class="stat-item text-center p-3 rounded-lg bg-gray-50 dark:bg-gray-800">
              <p class="text-2xl font-bold text-primary">{{ (stats?.total_artifacts ?? 0).toLocaleString() }}</p>
              <p class="text-xs text-gray-400 mt-1">制品总数</p>
            </div>
            <div class="stat-item text-center p-3 rounded-lg bg-gray-50 dark:bg-gray-800">
              <p class="text-2xl font-bold text-orange-500">{{ (stats?.total_pull_count ?? 0).toLocaleString() }}</p>
              <p class="text-xs text-gray-400 mt-1">总拉取次数</p>
            </div>
            <div class="stat-item text-center p-3 rounded-lg bg-gray-50 dark:bg-gray-800">
              <p class="text-2xl font-bold text-green-500">{{ stats?.total_projects ?? 0 }}</p>
              <p class="text-xs text-gray-400 mt-1">项目总数</p>
            </div>
            <div class="stat-item text-center p-3 rounded-lg bg-gray-50 dark:bg-gray-800">
              <p class="text-2xl font-bold text-purple-500">{{ stats?.total_repositories ?? 0 }}</p>
              <p class="text-xs text-gray-400 mt-1">镜像总数</p>
            </div>
          </div>
        </el-card>
      </re-col>


    </el-row>
  </div>
</template>

<style lang="scss" scoped>
:deep(.el-card) {
  --el-card-border-color: none;
}

.stat-card {
  :deep(.el-card__body) {
    padding: 20px;
  }
}

.rank-item,
.recent-item {
  &:hover {
    .rank-num {
      transform: scale(1.1);
    }
  }
}

.rank-num {
  transition: transform 0.2s ease;
}

.equal-height-card {
  height: 100%;

  :deep(.el-card__body) {
    height: 100%;
    display: flex;
    flex-direction: column;
    justify-content: center;
  }
}
</style>
