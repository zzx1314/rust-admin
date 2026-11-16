<script setup lang="ts">
/**
 * 航点动作「AI 识别」的场景 / 类别 / 阈值选择器。
 *
 * 与「实时视频 → AI 识别」面板共用同一份模型目录：场景 = 板端模型包（`task`），
 * 类别 = 该模型输出的标签（英文 `name` 存库，中文 `zh` 展示）。选出来的
 * `scene` + `labels` 会原样写进航点动作，服务端 `ai_patrol` 直接拿它们开识别窗口，
 * 不再依赖中文关键词匹配。
 *
 * 目录读不到（比如规划时不在园区内网）时组件只报错，不阻断规划：
 * 父组件会让用户手填场景关键词，后端仍有关键词兜底。
 */
import { computed, watch } from "vue";
import type { BackpackTask } from "@/api/backpack";
import type { LiveAiSelection } from "@/composables/useAiCatalog";
import AppIcon from "@/components/AppIcon.vue";

const scene = defineModel<string>("scene", { required: true });
const labels = defineModel<string[]>("labels", { required: true });
const conf = defineModel<number>("conf", { required: true });

const props = defineProps<{
  tasks: BackpackTask[];
  loading: boolean;
  error: string;
  /** 「实时视频」当前正在跑的识别：同一场景时直接套用它勾选的类别 */
  live?: LiveAiSelection | null;
}>();

const emit = defineEmits<{ (event: "retry"): void }>();

const current = computed(() => props.tasks.find(item => item.task === scene.value) ?? null);
const labelList = computed(
  () => current.value?.labels.filter(label => !label.ignored) ?? []
);
/** 当前选中的场景就是实时视频在跑的那个：类别/阈值默认沿用现场那一路 */
const liveMatch = computed(
  () => !!props.live && props.live.task === scene.value && props.live.labels.length > 0
);

/** 记住已经套用过的场景：同一场景重渲染时不要覆盖用户手动改过的类别 */
let applied = "";

/**
 * 场景与目录的联动：
 * - 目录到位但还没选场景（或选的场景在板端已不存在）→ 优先落到「实时视频」正在识别的
 *   那个模型包，否则第一个可用模型包，与实时视频页开识别时的默认值一致；
 * - 场景变化 → 类别默认全选 + 阈值取模型包自带的 `conf`；
 * - 选中的就是实时视频在跑的场景 → 类别 / 阈值直接沿用现场勾选的那一套。
 */
watch(
  [() => props.tasks, () => scene.value, () => props.live],
  ([list, value, live]) => {
    if (!list.length) return;
    if (!value || !list.some(item => item.task === value)) {
      const preferred =
        (live && list.find(item => item.task === live.task)) ??
        list.find(item => item.ok) ??
        list[0];
      if (preferred) scene.value = preferred.task;
      return;
    }
    if (value === applied) return;
    applied = value;
    const task = list.find(item => item.task === value);
    if (!task) return;
    if (live && live.task === value && live.labels.length) {
      labels.value = [...live.labels];
      conf.value = clampConf(live.conf);
      return;
    }
    labels.value = task.labels.filter(label => !label.ignored).map(label => label.name);
    if (task.conf >= 0.05 && task.conf <= 0.95) {
      conf.value = Math.round(task.conf * 100) / 100;
    }
  },
  { immediate: true }
);

function toggleLabel(name: string) {
  labels.value = labels.value.includes(name)
    ? labels.value.filter(item => item !== name)
    : [...labels.value, name];
}

function selectAllLabels() {
  labels.value = labelList.value.map(label => label.name);
}

function clearLabels() {
  labels.value = [];
}

function clampConf(value: number) {
  if (!Number.isFinite(value)) return 0.35;
  return Math.min(0.95, Math.max(0.05, Math.round(value * 100) / 100));
}
</script>

<template>
  <div class="ai-action">
    <p v-if="loading" class="ai-action-note">
      <AppIcon name="refresh" :size="13" />正在从背包读取 AI 模型目录（与「实时视频」同一套）…
    </p>

    <p v-else-if="error" class="ai-action-note bad">
      <AppIcon name="warn" :size="13" />读不到 AI 模型目录：{{ error }}
      <button type="button" class="ai-action-retry" @click="emit('retry')">重新读取</button>
    </p>

    <p v-else class="ai-action-note ok">
      <AppIcon name="scan" :size="13" />模型目录来自「实时视频 → AI 识别」，类别与现场识别完全一致。
    </p>

    <p v-if="liveMatch" class="ai-action-note ok">
      <AppIcon name="activity" :size="13" />已沿用「实时视频」当前识别中的场景与类别（{{ labels.length }} 类）。
    </p>

    <template v-if="tasks.length">
      <div class="field">
        <label for="route-ai-scene">识别场景（模型包）</label>
        <select id="route-ai-scene" v-model="scene" class="select">
          <option v-for="item in tasks" :key="item.task" :value="item.task">
            {{ item.name }}（{{ item.nc }} 类）{{ item.ok ? "" : " · 校验未通过" }}
          </option>
        </select>
        <p v-if="current?.desc" class="ai-action-desc">{{ current.desc }}</p>
        <p v-if="current && !current.ok" class="ai-action-note bad">
          <AppIcon name="warn" :size="13" />{{ current.issues.join("；") }}
        </p>
      </div>

      <div class="field">
        <label class="ai-action-head">
          识别类别
          <span class="muted">（不勾 = 该场景全部类别）</span>
          <span class="ai-action-links">
            <button type="button" @click="selectAllLabels">全选</button>
            <button type="button" @click="clearLabels">清空</button>
          </span>
        </label>
        <div class="ai-action-labels">
          <label
            v-for="label in labelList"
            :key="label.name"
            class="ai-action-label"
            :class="{ on: labels.includes(label.name) }"
          >
            <input
              type="checkbox"
              :checked="labels.includes(label.name)"
              @change="toggleLabel(label.name)"
            />
            <i :style="{ background: label.color || '#8c9bb4' }" />
            {{ label.zh }}
          </label>
        </div>
      </div>

      <div class="field">
        <label for="route-ai-conf">置信度阈值</label>
        <div class="ai-action-inline">
          <input
            id="route-ai-conf"
            v-model.number="conf"
            type="range"
            min="0.05"
            max="0.95"
            step="0.05"
            @change="conf = clampConf(conf)"
          />
          <span class="ai-action-num">{{ conf.toFixed(2) }}</span>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.ai-action {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.ai-action-note {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 5px;
  margin: 0;
  font-size: 11.5px;
  line-height: 1.6;
  color: var(--muted);
}

.ai-action-note.ok {
  color: #0c6f9c;
}

.ai-action-note.bad {
  color: var(--danger);
}

.ai-action-retry {
  border: 0;
  background: transparent;
  color: var(--brand);
  font-size: 11.5px;
  font-weight: 600;
  cursor: pointer;
  padding: 0;
}

.ai-action-desc {
  margin: 6px 0 0;
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--muted);
}

.ai-action-head {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.ai-action-head .muted {
  font-weight: 400;
  font-size: 11px;
}

.ai-action-links {
  margin-left: auto;
  display: flex;
  gap: 8px;
}

.ai-action-links button {
  border: 0;
  background: transparent;
  color: var(--brand);
  font-size: 11.5px;
  font-weight: 600;
  cursor: pointer;
  padding: 0;
}

.ai-action-labels {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  max-height: 118px;
  overflow-y: auto;
  padding-right: 2px;
}

.ai-action-label {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 3px 8px;
  border-radius: 7px;
  border: 1px solid var(--line);
  background: #fbfcfe;
  font-size: 11.5px;
  color: var(--text-2);
  cursor: pointer;
  user-select: none;
  transition: border-color 0.15s, background 0.15s, color 0.15s;
}

.ai-action-label:hover {
  border-color: #b9d8ec;
}

.ai-action-label.on {
  border-color: #9ed3f2;
  background: var(--brand-soft);
  color: #075985;
}

.ai-action-label input {
  margin: 0;
  accent-color: var(--brand);
  cursor: pointer;
}

.ai-action-label i {
  width: 7px;
  height: 7px;
  border-radius: 2px;
  flex: none;
}

.ai-action-inline {
  display: flex;
  align-items: center;
  gap: 8px;
}

.ai-action-inline input[type="range"] {
  flex: 1;
  accent-color: var(--brand);
  min-width: 0;
}

.ai-action-num {
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  color: var(--text-2);
  background: var(--line-2);
  border-radius: 6px;
  padding: 2px 6px;
}
</style>
