<script setup lang="ts">
import { MdPreview } from "md-editor-v3";
import "md-editor-v3/lib/style.css";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { emitter } from "@/utils/mitt";
import { useUserStoreHook } from "@/store/modules/user";
import { useAppStoreHook } from "@/store/modules/app";
import adminManual from "@/views/manual/admin.md?raw";
import developerManual from "@/views/manual/developer.md?raw";

import ManualIcon from "~icons/ri/book-open-line";
import CloseIcon from "~icons/ep/close";

defineOptions({
  name: "LayManual"
});

const { t } = useI18n();
const visible = ref(false);
const activeName = ref("developer");

/**
 * 管理员角色 code（登录接口 permissions 字段即角色 code）。
 * 平台默认管理员角色 sysadm 的 code 为 "110"；如需扩展请在此追加。
 */
const ADMIN_ROLE_CODES = ["110", "admin", "administrator", "role_admin"];

// 是否为平台管理员（决定是否展示「管理员使用手册」标签页）
const isAdmin = computed(() => {
  const permissions = useUserStoreHook().permissions ?? [];
  return permissions.some(code => ADMIN_ROLE_CODES.includes(code));
});

// 普通用户：只看开发者手册；管理员：管理员 + 开发者手册
const tabs = computed(() => {
  const list = [
    { name: "developer", label: "开发者使用手册", content: developerManual }
  ];
  if (isAdmin.value) {
    list.unshift({
      name: "admin",
      label: "管理员使用手册",
      content: adminManual
    });
  }
  return list;
});

const dialogWidth = computed(() => {
  return useAppStoreHook().device === "mobile" ? "92%" : "760px";
});

function openManual() {
  // 打开时默认选中开发者手册；管理员也可手动切到管理员手册
  activeName.value = isAdmin.value ? "admin" : "developer";
  visible.value = true;
}

onMounted(() => {
  emitter.on("openManual", openManual);
});

onBeforeUnmount(() => {
  emitter.off("openManual");
});
</script>

<template>
  <el-dialog
    v-model="visible"
    :width="dialogWidth"
    align-center
    :close-on-click-modal="false"
    class="lay-manual-dialog"
  >
    <template #header>
      <div class="manual-header">
        <div class="manual-title-row">
          <div class="manual-title flex items-center gap-2">
            <IconifyIconOffline :icon="ManualIcon" width="20" height="20" />
            <span class="dark:text-white">{{ t("buttons.pureManual") }}</span>
          </div>
          <el-button text @click="visible = false">
            <IconifyIconOffline :icon="CloseIcon" width="16" height="16" />
          </el-button>
        </div>
        <el-tabs v-model="activeName" class="manual-tabs">
          <el-tab-pane
            v-for="item in tabs"
            :key="item.name"
            :label="item.label"
            :name="item.name"
          />
        </el-tabs>
      </div>
    </template>

    <div class="manual-body">
      <template v-for="item in tabs" :key="item.name">
        <div v-show="activeName === item.name" class="manual-content">
          <MdPreview
            :model-value="item.content"
            preview-theme="github"
            code-theme="github"
            code-fold-render="hover"
            class="manual-preview"
          />
        </div>
      </template>
      <el-empty
        v-if="!tabs.length"
        description="暂无手册内容"
        :image-size="60"
      />
    </div>
  </el-dialog>
</template>

<style lang="scss" scoped>
:deep(.manual-body) {
  min-height: 300px;
  max-height: 62vh;
  padding-right: 4px;
  overflow: auto;
}

.manual-header {
  .manual-title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .manual-title {
    font-size: 15px;
    font-weight: 600;
  }

  .manual-tabs {
    margin-bottom: -2px;

    :deep(.el-tabs__nav-wrap::after) {
      background-color: var(--pure-border-color);
    }
  }
}
</style>
