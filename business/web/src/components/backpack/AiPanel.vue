<script setup lang="ts">
/**
 * AI 识别面板：场景 / 类别 / 阈值 / 帧率 / **画框方式**。
 *
 * 画框方式是本模块的核心开关（对应需求「浏览器勾选或背包内出图自带」）：
 * - `browser`：播干净流，平台前端用 SSE 坐标叠 SVG。延时最低；
 * - `osd`：播板端 `preview` 流，框已经烧进视频像素，录屏/抓拍/转发都自带。
 *
 * 面板不自己发请求，只把用户操作抛给页面（`start` / `stop` / `apply`），
 * 这样「谁在跑、切哪一路」只有页面一处状态，不会两边打架。
 */
import { computed } from "vue";
import type { BackpackDetectStatus, BackpackTask } from "@/api/backpack";
import AppIcon from "@/components/AppIcon.vue";

const enabled = defineModel<boolean>("enabled", { required: true });
const overlay = defineModel<"browser" | "osd">("overlay", { required: true });
const task = defineModel<string>("task", { required: true });
const labels = defineModel<string[]>("labels", { required: true });
const conf = defineModel<number>("conf", { required: true });
const fps = defineModel<number>("fps", { required: true });

const props = defineProps<{
  tasks: BackpackTask[];
  detect: BackpackDetectStatus | null;
  /** 不可用原因（红外 / 无可用源），空串表示可用 */
  disabledReason: string;
  /** 板端画框是否真的在出图（running + 已写出帧） */
  osdReady: boolean;
  busy: boolean;
  /** 现场提示（如自动回退浏览器叠加） */
  notes: string[];
  /** 启动失败等错误文案 */
  error: string;
}>();

const emit = defineEmits<{
  (event: "start"): void;
  (event: "stop"): void;
  (event: "apply"): void;
}>();

const currentTask = computed(() => props.tasks.find(item => item.task === task.value) ?? null);
const labelsOfTask = computed(() => currentTask.value?.labels.filter(label => !label.ignored) ?? []);
const running = computed(() => !!props.detect?.running);
const aiDisabled = computed(() => props.busy || !!props.disabledReason);

const stat = computed(() => {
  const cams = props.detect?.cams ?? [];
  return cams.length ? props.detect?.stats?.[cams[0]] ?? null : null;
});

const stateText = computed(() => {
  if (!props.detect) return "未开启";
  if (props.detect.state === "starting") return "模型加载中…";
  if (!props.detect.running) return "未开启";
  return `识别中 · ${props.detect.task}`;
});

const overlayHint = computed(() => {
  if (!enabled.value) return "开启 AI 后可选择框的呈现方式";
  if (overlay.value === "browser") return "干净流 + 平台画框：延时最低，切源即生效";
  if (props.osdReady) return "画框已烧进视频：抓拍、录屏、转发到第三方都自带框";
  return "板端画框尚未就绪，当前仍以浏览器叠加显示";
});

function toggleEnabled() {
  if (aiDisabled.value) return;
  enabled.value = !enabled.value;
}

function toggleLabel(name: string) {
  const next = labels.value.includes(name)
    ? labels.value.filter(item => item !== name)
    : [...labels.value, name];
  labels.value = next;
}

function selectAllLabels() {
  labels.value = labelsOfTask.value.map(label => label.name);
}

function clearLabels() {
  labels.value = [];
}

function clampConf(value: number) {
  if (!Number.isFinite(value)) return 0.25;
  return Math.min(0.95, Math.max(0.05, Math.round(value * 100) / 100));
}

function clampFps(value: number) {
  if (!Number.isFinite(value)) return 6;
  return Math.min(12, Math.max(1, Math.round(value)));
}

/** 运行中改阈值/类别/帧率走热更新，不重启模型 */
function applyIfRunning() {
  if (running.value) emit("apply");
}
</script>

<template>
  <div class="card ai-panel">
    <div class="card-head">
      <div class="card-title">
        <AppIcon name="scan" :size="15" />AI 识别
      </div>
      <button
        type="button"
        class="ai-switch"
        role="switch"
        :aria-checked="enabled"
        :aria-label="enabled ? '关闭 AI 识别' : '开启 AI 识别'"
        :disabled="aiDisabled"
        :class="{ on: enabled }"
        @click="toggleEnabled"
      >
        <span class="ai-switch-knob" />
      </button>
    </div>

    <div class="card-body">
      <p v-if="disabledReason" class="ai-note warn">
        <AppIcon name="info" :size="13" />{{ disabledReason }}
      </p>

      <div class="field">
        <label for="ai-task">识别场景</label>
        <select
          id="ai-task"
          v-model="task"
          class="select"
          :disabled="aiDisabled || running"
          @change="applyIfRunning"
        >
          <option v-for="item in tasks" :key="item.task" :value="item.task">
            {{ item.name }}（{{ item.nc }} 类）{{ item.ok ? "" : " · 校验未通过" }}
          </option>
        </select>
        <p v-if="currentTask?.desc" class="ai-desc">{{ currentTask.desc }}</p>
        <p v-if="currentTask && !currentTask.ok" class="ai-note warn">
          <AppIcon name="warn" :size="13" />{{ currentTask.issues.join("；") }}
        </p>
      </div>

      <div class="ai-row">
        <div class="field">
          <label for="ai-conf">置信度阈值</label>
          <div class="ai-inline">
            <input
              id="ai-conf"
              v-model.number="conf"
              type="range"
              min="0.05"
              max="0.95"
              step="0.05"
              :disabled="aiDisabled"
              @change="conf = clampConf(conf); applyIfRunning()"
            />
            <span class="ai-num">{{ conf.toFixed(2) }}</span>
          </div>
        </div>
        <div class="field ai-fps">
          <label for="ai-fps">推理帧率</label>
          <input
            id="ai-fps"
            v-model.number="fps"
            class="input"
            type="number"
            min="1"
            max="12"
            :disabled="aiDisabled"
            @change="fps = clampFps(fps); applyIfRunning()"
          />
        </div>
      </div>

      <div class="field">
        <label class="ai-label-head">
          识别类别
          <span class="muted">（勾选后才在画面中显示）</span>
          <span class="ai-label-actions">
            <button type="button" :disabled="aiDisabled" @click="selectAllLabels">全选</button>
            <button type="button" :disabled="aiDisabled" @click="clearLabels">清空</button>
          </span>
        </label>
        <div class="ai-labels">
          <label
            v-for="label in labelsOfTask"
            :key="label.name"
            class="ai-label"
            :class="{ on: labels.includes(label.name) }"
          >
            <input
              type="checkbox"
              :checked="labels.includes(label.name)"
              :disabled="aiDisabled"
              @change="toggleLabel(label.name); applyIfRunning()"
            />
            <i :style="{ background: label.color || '#8c9bb4' }" />
            {{ label.zh }}
          </label>
        </div>
      </div>

      <div class="field">
        <label>框的呈现方式</label>
        <div class="ai-overlay" role="radiogroup" aria-label="框的呈现方式">
          <button
            type="button"
            role="radio"
            class="ai-overlay-item"
            :class="{ on: overlay === 'browser' }"
            :aria-checked="overlay === 'browser'"
            :disabled="aiDisabled"
            @click="overlay = 'browser'"
          >
            <span class="ai-overlay-title"><AppIcon name="layers" :size="13" />浏览器叠加</span>
            <span class="ai-overlay-desc">播干净流，平台按识别坐标画框；延时最低</span>
          </button>
          <button
            type="button"
            role="radio"
            class="ai-overlay-item"
            :class="{ on: overlay === 'osd' }"
            :aria-checked="overlay === 'osd'"
            :disabled="aiDisabled"
            @click="overlay = 'osd'"
          >
            <span class="ai-overlay-title"><AppIcon name="video" :size="13" />背包烧录</span>
            <span class="ai-overlay-desc">框烧进视频画面，抓拍/录屏自带标注</span>
          </button>
        </div>
        <p class="ai-desc" :class="{ warn: enabled && overlay === 'osd' && !osdReady }">
          {{ overlayHint }}
        </p>
      </div>

      <div class="btns ai-actions">
        <button
          type="button"
          class="btn primary"
          :disabled="aiDisabled || running || !task"
          @click="emit('start')"
        >
          <AppIcon name="play" :size="13" />{{ busy && !running ? "启动中…" : "开始识别" }}
        </button>
        <button type="button" class="btn" :disabled="busy || !running" @click="emit('stop')">
          <AppIcon name="pause" :size="13" />停止识别
        </button>
      </div>

      <div class="ai-status">
        <div class="ai-status-head">
          <span class="ai-state" :class="{ on: running }">{{ stateText }}</span>
          <span v-if="running && detect" class="tag teal">
            {{ detect.cams[0] === "dogcam" ? "机器狗前置" : detect.cams[0] }}
          </span>
        </div>
        <p v-if="running" class="ai-desc">
          运行中调整阈值 / 类别 / 帧率会立即热更新到板端，不用重启模型。
        </p>
        <dl v-if="running && detect" class="ai-metrics">
          <div>
            <dt>推理耗时</dt>
            <dd>{{ stat ? `${Math.round(stat.last_ms)} ms` : "—" }}</dd>
          </div>
          <div>
            <dt>实测帧率</dt>
            <dd>{{ stat ? `${stat.fps} fps` : "—" }}</dd>
          </div>
          <div>
            <dt>累计检出</dt>
            <dd>{{ stat ? stat.hits : 0 }}</dd>
          </div>
        </dl>
        <p v-for="note in notes" :key="note" class="ai-note warn">
          <AppIcon name="warn" :size="13" />{{ note }}
        </p>
        <p v-if="error" class="ai-note bad">
          <AppIcon name="warn" :size="13" />{{ error }}
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.ai-panel :deep(.card-body) {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.ai-switch {
  position: relative;
  width: 42px;
  height: 23px;
  border-radius: 999px;
  border: 1px solid var(--line);
  background: #eef2f7;
  cursor: pointer;
  padding: 0;
  transition: background 0.18s, border-color 0.18s;
}

.ai-switch:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.ai-switch:focus-visible {
  outline: 2px solid var(--brand);
  outline-offset: 2px;
}

.ai-switch.on {
  background: linear-gradient(135deg, #0284c7, #06b6d4);
  border-color: transparent;
}

.ai-switch-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 17px;
  height: 17px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 1px 3px rgba(15, 23, 42, 0.28);
  transition: transform 0.18s cubic-bezier(0.22, 0.61, 0.36, 1);
}

.ai-switch.on .ai-switch-knob {
  transform: translateX(19px);
}

.ai-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 92px;
  gap: 12px;
}

.ai-inline {
  display: flex;
  align-items: center;
  gap: 8px;
}

.ai-inline input[type="range"] {
  flex: 1;
  accent-color: var(--brand);
  min-width: 0;
}

.ai-num {
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  color: var(--text-2);
  background: var(--line-2);
  border-radius: 6px;
  padding: 2px 6px;
}

.ai-desc {
  margin: 6px 0 0;
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--muted);
}

.ai-desc.warn {
  color: var(--warn);
}

.ai-label-head {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.ai-label-head .muted {
  font-weight: 400;
  font-size: 11px;
}

.ai-label-actions {
  margin-left: auto;
  display: flex;
  gap: 8px;
}

.ai-label-actions button {
  border: 0;
  background: transparent;
  color: var(--brand);
  font-size: 11.5px;
  font-weight: 600;
  cursor: pointer;
  padding: 0;
}

.ai-label-actions button:disabled {
  color: var(--muted);
  cursor: not-allowed;
}

.ai-labels {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  max-height: 108px;
  overflow-y: auto;
  padding-right: 2px;
}

.ai-label {
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

.ai-label:hover {
  border-color: #b9d8ec;
}

.ai-label.on {
  border-color: #9ed3f2;
  background: var(--brand-soft);
  color: #075985;
}

.ai-label input {
  margin: 0;
  accent-color: var(--brand);
  cursor: pointer;
}

.ai-label i {
  width: 7px;
  height: 7px;
  border-radius: 2px;
  flex: none;
}

.ai-overlay {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
}

.ai-overlay-item {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 10px;
  text-align: left;
  border-radius: 10px;
  border: 1px solid var(--line);
  background: #fbfcfe;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s, box-shadow 0.15s;
}

.ai-overlay-item:hover:not(:disabled) {
  border-color: #b9d8ec;
}

.ai-overlay-item.on {
  border-color: var(--brand);
  background: linear-gradient(135deg, #eef9ff, #fbfeff);
  box-shadow: 0 0 0 3px rgba(2, 132, 199, 0.09);
}

.ai-overlay-item:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.ai-overlay-item:focus-visible {
  outline: 2px solid var(--brand);
  outline-offset: 2px;
}

.ai-overlay-title {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 12.5px;
  font-weight: 600;
  color: var(--text);
}

.ai-overlay-item.on .ai-overlay-title {
  color: #075985;
}

.ai-overlay-desc {
  font-size: 11px;
  line-height: 1.5;
  color: var(--muted);
}

.ai-actions {
  margin-top: 0;
}

.ai-actions .btn {
  flex: 1;
  justify-content: center;
}

.ai-status {
  margin-top: 2px;
  padding-top: 12px;
  border-top: 1px dashed var(--line);
}

.ai-status-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.ai-state {
  font-size: 12px;
  font-weight: 600;
  color: var(--muted);
}

.ai-state.on {
  color: #0d9488;
}

.ai-metrics {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 8px;
  margin: 10px 0 0;
}

.ai-metrics div {
  background: #f8fafc;
  border-radius: 8px;
  padding: 7px 9px;
}

.ai-metrics dt {
  font-size: 10.5px;
  color: var(--muted);
}

.ai-metrics dd {
  margin: 3px 0 0;
  font-size: 12.5px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  color: var(--text);
}

.ai-note {
  display: flex;
  align-items: flex-start;
  gap: 5px;
  margin: 8px 0 0;
  font-size: 11.5px;
  line-height: 1.6;
}

.ai-note.warn {
  color: var(--warn);
}

.ai-note.bad {
  color: var(--danger);
}
</style>
