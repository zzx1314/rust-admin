<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { ElMessage } from "element-plus";
import type { HarborArtifact } from "@/api/harbor";

const props = defineProps<{
  visible: boolean;
  artifact: HarborArtifact | null;
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

// Extract short repo name from "project/repo"
const shortRepoName = computed(() => {
  return props.repoName.split("/").pop() || props.repoName;
});

// Full image path for display
const imagePath = computed(() => {
  const registry = props.registryUrl || window.location.hostname;
  return `${registry}/${props.projectName}/${shortRepoName.value}`;
});

// First tag name
const firstTag = computed(() => {
  if (props.artifact?.tags && props.artifact.tags.length > 0) {
    return props.artifact.tags[0].name;
  }
  return "";
});

const hasTags = computed(() => {
  return (
    props.artifact?.tags && props.artifact.tags.length > 0 && firstTag.value
  );
});

interface CommandItem {
  label: string;
  command: string;
  description: string;
  icon: string;
}

const commandGroups = computed(
  (): { title: string; commands: CommandItem[] }[] => {
    if (!props.artifact) return [];

    const registry = props.registryUrl || window.location.hostname;
    const imageRepo = `${registry}/${props.projectName}/${shortRepoName.value}`;
    const digest = props.artifact.digest || "";

    const groups: { title: string; commands: CommandItem[] }[] = [];

    // --- Pull by digest section ---
    const digestCommands: CommandItem[] = [
      {
        label: "Docker",
        command: `docker pull ${imageRepo}@${digest}`,
        description: "Docker CLI",
        icon: "docker"
      },
      {
        label: "Podman",
        command: `podman pull ${imageRepo}@${digest}`,
        description: "Podman CLI",
        icon: "podman"
      },
      {
        label: "nerdctl",
        command: `nerdctl pull ${imageRepo}@${digest}`,
        description: "nerdctl CLI (containerd)",
        icon: "nerdctl"
      },
      {
        label: "ctr",
        command: `ctr image pull ${imageRepo}@${digest}`,
        description: "containerd CLI",
        icon: "containerd"
      },
      {
        label: "crictl",
        command: `crictl pull ${imageRepo}@${digest}`,
        description: "CRI-O CLI",
        icon: "crio"
      }
    ];

    groups.push({
      title: "通过 Digest 拉取",
      commands: digestCommands
    });

    // --- Pull by tag section ---
    if (hasTags.value) {
      const tag = firstTag.value;
      const tagCommands: CommandItem[] = [
        {
          label: "Docker",
          command: `docker pull ${imageRepo}:${tag}`,
          description: "Docker CLI",
          icon: "docker"
        },
        {
          label: "Podman",
          command: `podman pull ${imageRepo}:${tag}`,
          description: "Podman CLI",
          icon: "podman"
        },
        {
          label: "nerdctl",
          command: `nerdctl pull ${imageRepo}:${tag}`,
          description: "nerdctl CLI (containerd)",
          icon: "nerdctl"
        },
        {
          label: "ctr",
          command: `ctr image pull ${imageRepo}:${tag}`,
          description: "containerd CLI",
          icon: "containerd"
        },
        {
          label: "crictl",
          command: `crictl pull ${imageRepo}:${tag}`,
          description: "CRI-O CLI",
          icon: "crio"
        }
      ];

      groups.push({
        title: `通过 Tag 拉取 (${tag})`,
        commands: tagCommands
      });
    }

    return groups;
  }
);

const copyText = async (text: string, label: string) => {
  try {
    await navigator.clipboard.writeText(text);
    ElMessage.success({
      message: `${label} 命令已复制到剪贴板`,
      duration: 2000
    });
  } catch {
    // Fallback for older browsers
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
</script>

<template>
  <el-dialog
    :model-value="dialogVisible"
    title="镜像命令"
    width="680px"
    :close-on-click-modal="false"
    destroy-on-close
    class="image-command-dialog"
    @update:model-value="emit('update:visible', $event)"
  >
    <div class="command-dialog-body">
      <!-- Image path header -->
      <div class="image-path-header">
        <div class="path-label">镜像路径</div>
        <div class="path-value">
          <span class="path-text">{{ imagePath }}</span>
          <el-tooltip content="复制镜像路径" placement="top">
            <el-button
              text
              size="small"
              class="copy-path-btn"
              @click="handleCopy(imagePath, '镜像路径', 'path')"
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

      <!-- Digest -->
      <div v-if="props.artifact?.digest" class="digest-info">
        <div class="digest-label">摘要 (Digest)</div>
        <div class="digest-value">
          <code class="digest-text"
            >{{ props.artifact.digest.substring(0, 32) }}...</code
          >
          <el-tooltip content="复制完整摘要" placement="top">
            <el-button
              text
              size="small"
              class="copy-digest-btn"
              @click="handleCopy(props.artifact!.digest!, '摘要', 'digest')"
            >
              <IconifyIconOffline
                :icon="
                  copiedIndex === 'digest' ? 'ep:check' : 'ep:copy-document'
                "
                :class="{ 'copied-icon': copiedIndex === 'digest' }"
                width="14"
                height="14"
              />
            </el-button>
          </el-tooltip>
        </div>
      </div>

      <!-- Command sections -->
      <div
        v-for="(group, gIdx) in commandGroups"
        :key="gIdx"
        class="command-group"
      >
        <div class="command-group-title">
          <el-icon>
            <IconifyIconOffline icon="ep:terminal" width="16" height="16" />
          </el-icon>
          <span>{{ group.title }}</span>
        </div>

        <div
          v-for="(cmd, cIdx) in group.commands"
          :key="`${gIdx}-${cIdx}`"
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
              :type="copiedIndex === `${gIdx}-${cIdx}` ? 'success' : 'primary'"
              size="small"
              class="copy-btn"
              :class="{ 'is-copied': copiedIndex === `${gIdx}-${cIdx}` }"
              @click="handleCopy(cmd.command, cmd.label, `${gIdx}-${cIdx}`)"
            >
              <template v-if="copiedIndex === `${gIdx}-${cIdx}`">
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

      <!-- Tags info -->
      <div v-if="hasTags" class="tags-info">
        <span class="tags-label">可用标签:</span>
        <el-tag
          v-for="tag in props.artifact?.tags"
          :key="tag.name"
          size="small"
          type="info"
          effect="plain"
          class="tag-item"
        >
          {{ tag.name }}
        </el-tag>
      </div>

      <div class="dialog-footer-info">
        <el-alert
          title="提示：请确保已在本地登录到应用仓库，使用 `docker login` 或对应 CLI 的登录命令进行身份验证。"
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
.image-command-dialog {
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

.image-path-header {
  padding: 12px 16px;
  margin-bottom: 12px;
  background: linear-gradient(
    135deg,
    var(--el-color-primary-light-9),
    var(--el-color-info-light-9)
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
      color: var(--el-color-primary);
      word-break: break-all;
    }

    .copy-path-btn {
      flex-shrink: 0;
    }
  }
}

.digest-info {
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 8px 16px;
  margin-bottom: 16px;
  background: var(--el-fill-color-light);
  border-radius: 6px;

  .digest-label {
    font-size: 12px;
    color: var(--el-text-color-secondary);
    white-space: nowrap;
  }

  .digest-value {
    display: flex;
    flex: 1;
    gap: 6px;
    align-items: center;

    .digest-text {
      font-family: Menlo, Monaco, "Courier New", monospace;
      font-size: 12px;
      color: var(--el-text-color-regular);
      background: transparent;
    }

    .copy-digest-btn {
      flex-shrink: 0;
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

  .ml-2 {
    margin-left: 8px;
  }
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

.tags-info {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  padding: 10px 14px;
  margin-bottom: 12px;
  background: var(--el-fill-color-light);
  border-radius: 6px;

  .tags-label {
    font-size: 12px;
    color: var(--el-text-color-secondary);
    white-space: nowrap;
  }

  .tag-item {
    margin: 0;
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
