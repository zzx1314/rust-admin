<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { ElMessage } from "element-plus";

const props = defineProps<{
  visible: boolean;
  projectName: string;
  repoName: string;
  registryUrl: string;
}>();

const emit = defineEmits<{
  (e: "update:visible", val: boolean): void;
}>();

const dialogVisible = ref(props.visible);

watch(
  () => props.visible,
  val => {
    dialogVisible.value = val;
  }
);

watch(dialogVisible, val => {
  emit("update:visible", val);
});

const shortRepoName = computed(() => {
  return props.repoName.split("/").pop() || props.repoName;
});

const registryHost = computed(() => {
  return props.registryUrl || window.location.hostname;
});

const projectPath = computed(() => {
  return `${registryHost.value}/${props.projectName}`;
});

interface PushCommandItem {
  label: string;
  command: string;
  description: string;
  icon: string;
  section: string;
}

const commands = computed<PushCommandItem[]>(() => [
  // Docker commands
  {
    label: "Docker Tag",
    command: `docker tag SOURCE_IMAGE[:TAG] ${projectPath.value}/REPOSITORY[:TAG]`,
    description: "给本地镜像打标签",
    icon: "docker",
    section: "Docker"
  },
  {
    label: "Docker Push",
    command: `docker push ${projectPath.value}/REPOSITORY[:TAG]`,
    description: "推送镜像到仓库",
    icon: "docker",
    section: "Docker"
  },
  // Podman commands
  {
    label: "Podman Push",
    command: `podman push IMAGE_ID ${projectPath.value}/REPOSITORY[:TAG]`,
    description: "使用 Podman 推送镜像",
    icon: "podman",
    section: "Podman"
  },
  // Example with actual repo and tag
  {
    label: "推送示例",
    command: `docker push ${projectPath.value}/${shortRepoName.value}:latest`,
    description: "推送当前仓库 latest 标签示例",
    icon: "docker",
    section: "示例"
  }
]);

const copyText = async (text: string, label: string) => {
  try {
    await navigator.clipboard.writeText(text);
    ElMessage.success({
      message: `${label} 命令已复制到剪贴板`,
      duration: 2000
    });
  } catch {
    const textarea = document.createElement("textarea");
    textarea.value = text;
    textarea.style.position = "fixed";
    textarea.style.opacity = "0";
    document.body.appendChild(textarea);
    textarea.select();
    document.execCommand("copy");
    document.body.removeChild(textarea);
    ElMessage.success({
      message: `${label} 命令已复制到剪贴板`,
      duration: 2000
    });
  }
};

const copiedIndex = ref<string>("");

const handleCopy = (text: string, label: string, key: string) => {
  copyText(text, label);
  copiedIndex.value = key;
  setTimeout(() => {
    copiedIndex.value = "";
  }, 2000);
};

// Group commands by section
const sectionKeys = computed(() => {
  const keys: string[] = [];
  commands.value.forEach(c => {
    if (!keys.includes(c.section)) keys.push(c.section);
  });
  return keys;
});

const getSectionCommands = (section: string) => {
  return commands.value.filter(c => c.section === section);
};
</script>

<template>
  <el-dialog
    :model-value="dialogVisible"
    title="推送命令"
    width="680px"
    :close-on-click-modal="false"
    destroy-on-close
    class="push-command-dialog"
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="command-dialog-body">
      <!-- Project path header -->
      <div class="project-path-header">
        <div class="path-label">项目路径</div>
        <div class="path-value">
          <span class="path-text">{{ projectPath }}</span>
          <el-tooltip content="复制项目路径" placement="top">
            <el-button
              text
              size="small"
              class="copy-path-btn"
              @click="handleCopy(projectPath, '项目路径', 'path')"
            >
              <IconifyIconOffline
                :icon="copiedIndex === 'path' ? 'ep:check' : 'ep:copy-document'"
                :class="{ 'copied-icon': copiedIndex === 'path' }"
                width="16"
                height="16"
              />
            </el-button>
          </el-tooltip>
        </div>
      </div>

      <!-- Section: Steps hint -->
      <div class="steps-hint">
        <div class="step">
          <span class="step-num">1</span>
          <span
            >为本地镜像打标签：<code
              >docker tag SOURCE_IMAGE[:TAG]
              {{ projectPath }}/REPOSITORY[:TAG]</code
            ></span
          >
        </div>
        <div class="step">
          <span class="step-num">2</span>
          <span
            >登录到镜像仓库：<code>docker login {{ registryHost }}</code></span
          >
        </div>
        <div class="step">
          <span class="step-num">3</span>
          <span
            >推送镜像：<code
              >docker push {{ projectPath }}/REPOSITORY[:TAG]</code
            ></span
          >
        </div>
      </div>

      <!-- Command sections -->
      <div v-for="section in sectionKeys" :key="section" class="command-group">
        <div class="command-group-title">
          <el-icon
            ><IconifyIconOffline icon="ep:upload" width="16" height="16"
          /></el-icon>
          <span>{{ section }}</span>
          <el-tag
            v-if="section === '示例'"
            size="small"
            type="success"
            effect="plain"
          >
            可直接使用
          </el-tag>
        </div>

        <div
          v-for="(cmd, cIdx) in getSectionCommands(section)"
          :key="`${section}-${cIdx}`"
          class="command-item"
        >
          <div class="command-info">
            <span class="command-icon" :class="`icon-${cmd.icon}`">
              <IconifyIconOffline icon="ep:terminal" width="18" height="18" />
            </span>
            <div class="command-meta">
              <span class="command-label">{{ cmd.label }}</span>
              <span class="command-desc">{{ cmd.description }}</span>
            </div>
          </div>
          <div class="command-code-row">
            <code class="command-code">{{ cmd.command }}</code>
            <el-button
              :type="
                copiedIndex === `${section}-${cIdx}` ? 'success' : 'primary'
              "
              size="small"
              class="copy-btn"
              :class="{ 'is-copied': copiedIndex === `${section}-${cIdx}` }"
              @click="handleCopy(cmd.command, cmd.label, `${section}-${cIdx}`)"
            >
              <template v-if="copiedIndex === `${section}-${cIdx}`">
                <IconifyIconOffline
                  icon="ep:check"
                  width="14"
                  height="14"
                  class="mr-1"
                />
                已复制
              </template>
              <template v-else>
                <IconifyIconOffline
                  icon="ep:copy-document"
                  width="14"
                  height="14"
                  class="mr-1"
                />
                复制
              </template>
            </el-button>
          </div>
        </div>
      </div>

      <div class="dialog-footer-info">
        <el-alert
          title="提示：REPOSITORY[:TAG] 为占位符，请替换为实际的仓库名称和标签。例如：my-image:v1.0"
          type="info"
          :closable="false"
          show-icon
          size="small"
        />
      </div>
    </div>
  </el-dialog>
</template>

<style scoped lang="scss">
.push-command-dialog {
  :deep(.el-dialog__body) {
    padding-top: 0;
    padding-bottom: 16px;
  }
}

.command-dialog-body {
  max-height: 70vh;
  overflow-y: auto;

  &::-webkit-scrollbar {
    width: 4px;
  }

  &::-webkit-scrollbar-thumb {
    background: var(--el-border-color-darker);
    border-radius: 4px;
  }
}

.project-path-header {
  padding: 12px 16px;
  margin-bottom: 12px;
  background: linear-gradient(
    135deg,
    var(--el-color-warning-light-9),
    var(--el-color-danger-light-9)
  );
  border: 1px solid var(--el-border-color-light);
  border-radius: 8px;

  .path-label {
    margin-bottom: 4px;
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .path-value {
    display: flex;
    gap: 8px;
    align-items: center;

    .path-text {
      font-family: Menlo, Monaco, "Courier New", monospace;
      font-size: 14px;
      font-weight: 600;
      color: var(--el-color-danger);
      word-break: break-all;
    }

    .copy-path-btn {
      flex-shrink: 0;
    }
  }
}

.steps-hint {
  padding: 12px 16px;
  margin-bottom: 16px;
  background: var(--el-fill-color-light);
  border-radius: 8px;

  .step {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-bottom: 6px;
    font-size: 12px;
    color: var(--el-text-color-regular);

    &:last-child {
      margin-bottom: 0;
    }

    .step-num {
      display: inline-flex;
      flex-shrink: 0;
      align-items: center;
      justify-content: center;
      width: 20px;
      height: 20px;
      font-size: 11px;
      font-weight: 600;
      color: #fff;
      background: var(--el-color-primary);
      border-radius: 50%;
    }

    code {
      padding: 1px 6px;
      font-family: Menlo, Monaco, "Courier New", monospace;
      font-size: 11px;
      background: var(--el-fill-color);
      border-radius: 3px;
    }
  }
}

.command-group {
  margin-bottom: 20px;

  &:last-child {
    margin-bottom: 12px;
  }
}

.command-group-title {
  display: flex;
  gap: 6px;
  align-items: center;
  padding-bottom: 6px;
  margin-bottom: 10px;
  font-size: 13px;
  font-weight: 600;
  color: var(--el-text-color-primary);
  border-bottom: 1px solid var(--el-border-color-light);
}

.command-item {
  padding: 10px 12px;
  margin-bottom: 8px;
  background: var(--el-fill-color-blank);
  border: 1px solid var(--el-border-color-light);
  border-radius: 8px;
  transition: all 0.2s ease;

  &:hover {
    border-color: var(--el-color-primary-light-5);
    box-shadow: 0 2px 8px rgb(0 0 0 / 6%);
  }
}

.command-info {
  display: flex;
  gap: 8px;
  align-items: center;
  margin-bottom: 8px;

  .command-icon {
    display: flex;
    flex-shrink: 0;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    background: var(--el-fill-color);
    border-radius: 6px;

    &.icon-docker {
      background: #0db7ed20;
    }

    &.icon-podman {
      background: #892ca020;
    }
  }

  .command-meta {
    display: flex;
    flex-direction: column;

    .command-label {
      font-size: 13px;
      font-weight: 600;
      color: var(--el-text-color-primary);
    }

    .command-desc {
      font-size: 11px;
      color: var(--el-text-color-secondary);
    }
  }
}

.command-code-row {
  display: flex;
  gap: 8px;
  align-items: center;

  .command-code {
    flex: 1;
    padding: 6px 10px;
    overflow-x: auto;
    font-family: Menlo, Monaco, "Courier New", monospace;
    font-size: 12.5px;
    color: var(--el-text-color-regular);
    white-space: nowrap;
    cursor: text;
    user-select: all;
    background: var(--el-fill-color);
    border-radius: 4px;

    &::-webkit-scrollbar {
      height: 2px;
    }

    &::-webkit-scrollbar-thumb {
      background: var(--el-border-color-darker);
      border-radius: 2px;
    }

    &:hover {
      background: var(--el-fill-color-dark);
    }
  }

  .copy-btn {
    flex-shrink: 0;
    min-width: 72px;
    transition: all 0.2s ease;

    &.is-copied {
      border-color: var(--el-color-success);
    }
  }
}

.dialog-footer-info {
  margin-top: 4px;
}

.copied-icon {
  color: var(--el-color-success);
}

.mr-1 {
  margin-right: 4px;
}
</style>
