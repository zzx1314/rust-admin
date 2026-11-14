<script setup lang="ts">
/**
 * 视频源切换条：6 路单选（狗前置 / 左前 / 右前 / 右后 / 左后 / 红外）。
 *
 * 「不可用」与「不支持 AI」是两件事：
 * - `available=false`（狗前置未配置源）→ 置灰 + tooltip 写恢复方式；
 * - `ai=false`（红外）→ 仍可播放，只是 AI 面板会被父组件禁掉。
 */
import type { BackpackSource } from "@/api/backpack";
import AppIcon from "@/components/AppIcon.vue";

defineProps<{
  sources: BackpackSource[];
  active: string;
  /** AI 识别运行中（在对应 chip 上打一个实时点） */
  aiRunning: boolean;
  /** AI 运行在哪个源上（cams[0]） */
  aiSource: string;
  disabled: boolean;
}>();

const emit = defineEmits<{ (event: "select", key: string): void }>();

function iconOf(source: BackpackSource): string {
  if (source.kind === "infrared") return "thermo";
  if (source.kind === "relay") return "dog";
  return "camera";
}
</script>

<template>
  <div class="source-rail" role="group" aria-label="视频源">
    <button
      v-for="source in sources"
      :key="source.key"
      type="button"
      class="src"
      :class="{ on: source.key === active, muted: !source.available }"
      :aria-pressed="source.key === active"
      :disabled="disabled || !source.available"
      :title="source.hint || source.label"
      @click="emit('select', source.key)"
    >
      <AppIcon :name="iconOf(source)" :size="14" />
      <span class="src-label">{{ source.label }}</span>
      <span v-if="!source.ai" class="src-flag">无 AI</span>
      <span
        v-else-if="aiRunning && aiSource === source.key"
        class="src-dot"
        aria-hidden="true"
      />
    </button>
  </div>
</template>

<style scoped>
.source-rail {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.src {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 34px;
  padding: 0 12px;
  border-radius: 10px;
  border: 1px solid var(--line);
  background: var(--surface);
  color: var(--text-2);
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
  transition: border-color 0.15s, color 0.15s, background 0.15s, box-shadow 0.15s;
}

.src:hover:not(:disabled) {
  border-color: #b9d8ec;
  color: var(--text);
  background: #f8fbff;
}

.src:focus-visible {
  outline: 2px solid var(--brand);
  outline-offset: 2px;
}

.src.on {
  border-color: var(--brand);
  color: var(--brand);
  background: linear-gradient(135deg, #e9f6ff, #f6fcff);
  box-shadow: 0 0 0 3px rgba(2, 132, 199, 0.1);
}

.src:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.src-flag {
  font-size: 10px;
  font-weight: 600;
  color: var(--muted);
  background: var(--line-2);
  border-radius: 5px;
  padding: 1px 5px;
}

.src.on .src-flag {
  color: #64748b;
  background: #e2eef9;
}

.src-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #10b981;
  box-shadow: 0 0 0 0 rgba(16, 185, 129, 0.55);
  animation: src-pulse 1.8s infinite;
}

@keyframes src-pulse {
  70% {
    box-shadow: 0 0 0 6px rgba(16, 185, 129, 0);
  }
  100% {
    box-shadow: 0 0 0 0 rgba(16, 185, 129, 0);
  }
}

@media (prefers-reduced-motion: reduce) {
  .src-dot {
    animation: none;
  }
}
</style>
