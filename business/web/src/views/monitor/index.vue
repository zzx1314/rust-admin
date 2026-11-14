<script setup lang="ts">
/**
 * 实时视频（数据采集背包 · 单路 + AI）
 *
 * 页面职责：
 * - 6 路源单选取一路播放（狗前置 RTSP / 四路 USB / 红外）；
 * - AI 开关：五路可用（红外不支持），NPU 在板端单路独占；
 * - **画框方式**二选一：浏览器叠加（干净流 + SVG）或背包烧录（播 preview 流）；
 * - 巡检/演示时看「谁在识别、跑多快、检出了什么」。
 *
 * 数据来源：`/api/backpack/status`（业务平台聚合背包 Agent）+ SSE `/api/backpack/events`。
 * 视频：浏览器直连板端 mediamtx（WHEP），地址由 status 的 `webrtc.base` 给出。
 * 设计与链路说明见 `business/docs/backpack-video-integration.md`。
 */
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { ElMessage } from "element-plus";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import VideoStage from "@/components/backpack/VideoStage.vue";
import SourceRail from "@/components/backpack/SourceRail.vue";
import AiPanel from "@/components/backpack/AiPanel.vue";
import {
  backpackApi,
  clientFor,
  loadDirectStatus,
  type BackpackBox,
  type BackpackDetectEvent,
  type BackpackSource,
  type BackpackStatus
} from "@/api/backpack";
import type { WhepPhase } from "@/lib/whep";
import { isPrivateOrigin } from "@/utils/runtime";

const status = ref<BackpackStatus | null>(null);
const loading = ref(true);
const refreshing = ref(false);
const loadError = ref("");
const actionError = ref("");
const notes = ref<string[]>([]);
const busy = ref(false);
const updatedAt = ref("");

const activeKey = ref("");
const overlay = ref<"browser" | "osd">("browser");
const form = reactive({
  task: "",
  labels: [] as string[],
  conf: 0.35,
  fps: 6
});

const boxes = ref<BackpackBox[]>([]);
const frame = reactive({ w: 1280, h: 720 });
const counts = ref<{ name: string; zh: string; color: string; value: number }[]>([]);
const feed = ref<{ ts: number; text: string; color: string }[]>([]);
const lastDetectAt = ref(0);
const phase = ref<{ phase: WhepPhase; message: string }>({ phase: "connecting", message: "" });

/** 本地时:分:秒（`toISOString` 是 UTC，现场看时间会对不上） */
function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString("zh-CN", { hour12: false });
}

let eventSource: EventSource | null = null;
/** 当前事件流地址：接法（代理/直连）不变时不重复建连 */
let eventUrl = "";
let pollTimer: ReturnType<typeof setInterval> | null = null;

/* ---------------- 派生状态 ---------------- */

/** 代理/直连两种接法由这一层吸收，页面其余逻辑不区分 */
const client = computed(() => clientFor(status.value));
const sources = computed<BackpackSource[]>(() => status.value?.sources ?? []);
const activeSource = computed<BackpackSource | null>(
  () => sources.value.find(item => item.key === activeKey.value) ?? null
);
const detectStatus = computed(() => status.value?.detect.status ?? null);
const aiRunning = computed(() => !!detectStatus.value?.running);
const aiCam = computed(() => detectStatus.value?.cams?.[0] ?? "");
const tasks = computed(() => status.value?.detect.tasks?.tasks ?? []);

/** 画框方式为「背包烧录」且板端真的在出图时，才切到 preview 流 */
const osdActive = computed(
  () => aiRunning.value && overlay.value === "osd" && !!status.value?.osd.ready
);
const previewStream = computed(() => status.value?.osd.preview_stream || "preview");
const playStream = computed(() => {
  if (osdActive.value) return previewStream.value;
  return activeSource.value?.stream ?? "";
});

const aiStat = computed(() => {
  const status0 = detectStatus.value;
  const cam = aiCam.value;
  if (!status0 || !cam) return null;
  return status0.stats?.[cam] ?? null;
});

const aiBadge = computed(() => {
  if (!aiRunning.value) return "";
  const fps = aiStat.value?.fps ?? 0;
  return `AI ${fps ? fps.toFixed(0) : "—"}fps · ${boxes.value.length} 框`;
});

const stageHint = computed(() => {
  if (!status.value) return loadError.value;
  if (!status.value.agent.reachable) {
    const reason = status.value.agent.error || `连不上背包 Agent（${status.value.agent.base}）`;
    // 云端页面直连内网地址会被 Chrome 的 Local Network Access 拦掉，
    // 这种失败和「板子没开」长得一样，必须直接告诉现场该换哪个入口。
    if (!isPrivateOrigin()) {
      return `${reason}。当前页面由公网地址打开，Chrome 不允许公网页面访问内网地址，请改用现场入口 http://<背包IP>:8080 打开控制台`;
    }
    return reason;
  }
  if (!status.value.webrtc.base) {
    return "后端未给出浏览器可访问的背包地址：请配置 ROBODOG_BACKPACK_WEBRTC_URL";
  }
  if (!activeSource.value) return "没有可用的视频源";
  if (!activeSource.value.available) return activeSource.value.hint;
  if (phase.value.phase === "retry") return phase.value.message || "视频流重连中…";
  return "";
});

const detailText = computed(() => {
  if (!activeSource.value) return "";
  if (!aiRunning.value) return "干净流 · H264 720p30 · 无 AI";
  if (osdActive.value) return `背包烧录 · ${previewStream.value} 流自带中文标签`;
  if (overlay.value === "osd") return "背包烧录未就绪 · 已回退浏览器叠加";
  return "浏览器叠加 · 平台按识别坐标画框（延时最低）";
});

const aiDisabledReason = computed(() => {
  if (!status.value?.agent.reachable) return "背包不可达，暂不能开启 AI";
  if (!activeSource.value) return "没有可用的视频源";
  if (!activeSource.value.available) return activeSource.value.hint;
  if (!activeSource.value.ai) return `${activeSource.value.label}通道不支持 AI 识别，请切到可见光相机`;
  if (!tasks.value.length) return "板端未发现可用模型包（rknn/models/）";
  return "";
});

const overlayNote = computed(() => {
  if (!aiRunning.value || overlay.value !== "osd" || osdActive.value) return "";
  const reason = status.value?.osd.error || "板端画框未就绪";
  return `已回退浏览器叠加：${reason}`;
});

const relayText = computed(() => {
  const relay = status.value?.relay;
  if (!relay) return status.value?.relay_error || "未接入";
  if (!relay.enabled) return "未配置源";
  return relay.running ? "推流中" : `已配置 · 未推流（重启 ${relay.restarts} 次）`;
});

/* ---------------- 拉取状态 ---------------- */

function pickSource(list: BackpackSource[], preferred: string): string {
  const current = list.find(item => item.key === preferred && item.available);
  if (current) return current.key;
  const defaults = [status.value?.ai.default_cam, "dogcam", "cam0"];
  for (const key of defaults) {
    const found = list.find(item => item.key === key && item.available);
    if (found) return found.key;
  }
  return list.find(item => item.available)?.key ?? list[0]?.key ?? "";
}

function applyFormFrom(status0: BackpackStatus) {
  const list = status0.detect.tasks?.tasks ?? [];
  const running = status0.detect.status;
  const taskId = running?.task || form.task || list.find(item => item.ok)?.task || list[0]?.task || "";
  form.task = taskId;
  form.conf = running?.conf ?? list.find(item => item.task === taskId)?.conf ?? 0.35;
  form.fps = running?.fps ?? 6;
  const labels = running?.labels;
  form.labels = labels?.length ? [...labels] : labelsOfTask(taskId).map(label => label.name);
}

function labelsOfTask(taskId: string) {
  const task = (status.value?.detect.tasks?.tasks ?? []).find(item => item.task === taskId);
  return (task?.labels ?? []).filter(label => !label.ignored);
}

async function refreshStatus(quiet = false) {
  if (!quiet) refreshing.value = true;
  try {
    const shell = await backpackApi.status();
    // 直连模式（平台在公网、背包在现场内网）：聚合由浏览器自己向板端发起
    const data = shell.mode === "direct" ? await loadDirectStatus(shell) : shell;
    const previous = status.value;
    status.value = data;
    loadError.value = "";
    updatedAt.value = new Date().toLocaleTimeString("zh-CN", { hour12: false });

    if (!activeKey.value || !data.sources.some(item => item.key === activeKey.value)) {
      activeKey.value = pickSource(data.sources, activeKey.value);
    }
    // 只在首次 / 场景变化时回填表单，避免覆盖用户正在调整的取值
    if (!previous || previous.detect.status?.task !== data.detect.status?.task) {
      applyFormFrom(data);
    }
  } catch (error) {
    loadError.value = error instanceof Error ? error.message : "读取背包状态失败";
  } finally {
    loading.value = false;
    refreshing.value = false;
  }
}

/* ---------------- 交互：切源 / AI 启停 ---------------- */

async function selectSource(key: string) {
  if (key === activeKey.value) return;
  activeKey.value = key;
  boxes.value = [];
  counts.value = [];
  const source = sources.value.find(item => item.key === key);
  if (!source || !aiRunning.value) return;
  if (!source.ai) {
    await stopAi("已切到红外通道，AI 识别自动停止（红外不参与可见光模型）");
    return;
  }
  // 单路约束：换源 = 把板端 NPU 的目标路换过去，而不是再开一路
  const result = await client.value.config({ cams: [source.key] });
  if (result?.ok === false) {
    actionError.value = result.error || "切换 AI 目标路失败";
  }
  await refreshStatus(true);
}

/** 表单取值归一化：数字输入框被清空时 v-model 会给出空串，直接发给板端会 400 */
function normalizeForm() {
  form.conf = Math.min(0.95, Math.max(0.05, Number(form.conf) || 0.25));
  form.fps = Math.min(12, Math.max(1, Math.round(Number(form.fps) || 6)));
}

async function startAi() {
  normalizeForm();
  const source = activeSource.value;
  if (!source || aiDisabledReason.value) {
    actionError.value = aiDisabledReason.value || "当前不可开启 AI";
    return;
  }
  busy.value = true;
  actionError.value = "";
  notes.value = [];
  try {
    const result = await client.value.start({
      task: form.task,
      labels: form.labels.length ? form.labels : null,
      conf: form.conf,
      cams: [source.key],
      fps: form.fps
    });
    if (!result?.ok) {
      actionError.value = result?.error || "启动识别失败";
      return;
    }
    await refreshStatus(true);
    if (overlay.value === "osd" && !status.value?.osd.ready) {
      // 保留用户的选择（板端可能只是慢一步就绪），期间自动用浏览器叠加兜底
      notes.value = [`板端画框未就绪，暂以浏览器叠加显示：${status.value?.osd.error || "未知原因"}`];
    }
    ElMessage.success("AI 识别已开启");
  } catch (error) {
    actionError.value = error instanceof Error ? error.message : "启动识别失败";
  } finally {
    busy.value = false;
  }
}

async function stopAi(reason?: string) {
  busy.value = true;
  actionError.value = "";
  try {
    await client.value.stop();
    if (reason) ElMessage.info(reason);
  } catch (error) {
    actionError.value = error instanceof Error ? error.message : "停止识别失败";
  } finally {
    busy.value = false;
    await refreshStatus(true);
    boxes.value = [];
    counts.value = [];
    feed.value = [];
    notes.value = [];
  }
}

function onToggleAi(next: boolean) {
  if (busy.value) return;
  if (next) void startAi();
  else void stopAi();
}

async function applyConfig() {
  if (!aiRunning.value) return;
  normalizeForm();
  const result = await client.value.config({
    task: form.task,
    labels: form.labels.length ? form.labels : null,
    conf: form.conf,
    fps: form.fps
  });
  if (result?.ok === false) {
    actionError.value = result.error || "应用设置失败";
  } else {
    actionError.value = "";
    await refreshStatus(true);
  }
}

// 切到「背包烧录」时立刻确认一次画框链路：等 8s 轮询会让用户以为没生效
watch(overlay, value => {
  if (value === "osd" && aiRunning.value) void refreshStatus(true);
});

// 接法变化（代理 ⇄ 直连）意味着事件流地址变了，必须重连，否则框永远不刷新
watch(
  () => client.value.eventsUrl,
  () => connectEvents()
);

watch(
  () => form.task,
  task => {
    if (!task) return;
    form.labels = labelsOfTask(task).map(label => label.name);
  }
);

/* ---------------- SSE：识别框与检出事件 ---------------- */

function zhOf(label: string, fallback?: string) {
  const task = tasks.value.find(item => item.task === detectStatus.value?.task);
  return task?.labels.find(item => item.name === label)?.zh ?? fallback ?? label;
}

function colorOf(label: string, fallback?: string) {
  const task = tasks.value.find(item => item.task === detectStatus.value?.task);
  return task?.labels.find(item => item.name === label)?.color ?? fallback ?? "#8c9bb4";
}

function handleDetect(event: BackpackDetectEvent) {
  // 单路：只认当前 AI 目标路的坐标，切源瞬间的旧帧不能画到新画面上
  if (aiCam.value && event.cam !== aiCam.value) return;
  frame.w = event.frame_w || frame.w;
  frame.h = event.frame_h || frame.h;
  lastDetectAt.value = Date.now();
  // 两种画框方式都保留坐标：浏览器叠加用它画 SVG，背包烧录用它算「检出 N 个」。
  // 是否真的画出来由 VideoStage 的 showBoxes 决定，这里不做二次判断。
  boxes.value = event.boxes ?? [];
  counts.value = Object.entries(event.counts ?? {}).map(([name, value]) => ({
    name,
    zh: zhOf(name),
    color: colorOf(name),
    value
  }));
  if (event.boxes?.length) {
    const texts = Array.from(new Set(event.boxes.map(box => box.text)));
    feed.value = [
      { ts: event.ts * 1000, text: texts.join("、"), color: event.boxes[0]?.color || "#0284c7" },
      ...feed.value
    ].slice(0, 8);
  }
}

function onPhase(value: { phase: WhepPhase; message: string }) {
  phase.value = value;
}

function connectEvents() {
  const url = client.value.eventsUrl;
  if (eventSource && url === eventUrl) return;
  eventUrl = url;
  eventSource?.close();
  eventSource = new EventSource(url);
  eventSource.onmessage = raw => {
    let message: { type?: string } & Record<string, unknown>;
    try {
      message = JSON.parse(raw.data);
    } catch {
      return;
    }
    switch (message.type) {
      case "detect":
        handleDetect(message as unknown as BackpackDetectEvent);
        break;
      case "detect_clear":
        boxes.value = [];
        counts.value = [];
        void refreshStatus(true);
        break;
      case "detect_state":
        void refreshStatus(true);
        break;
      case "detect_error":
        notes.value = [...notes.value, String(message.msg ?? "识别异常")];
        void refreshStatus(true);
        break;
      default:
        // telemetry / button / lingsi：本页不用，留给后续的遥测与告警接入
        break;
    }
  };
}

/* ---------------- 生命周期 ---------------- */

function startPolling() {
  stopPolling();
  pollTimer = setInterval(() => {
    if (document.visibilityState === "hidden") return;
    void refreshStatus(true);
  }, 8000);
}

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

onMounted(async () => {
  await refreshStatus();
  connectEvents();
  startPolling();
});

onBeforeUnmount(() => {
  eventSource?.close();
  eventSource = null;
  stopPolling();
});

defineExpose({ refreshStatus });
</script>

<template>
  <div>
    <PageHeader
      title="实时视频"
      subtitle="数据采集背包 · 单路视频 + AI 识别（另可切换为背包烧录画框）"
    >
      <template #actions>
        <span class="conn-tag" :class="{ ok: status?.agent.reachable, bad: !!status && !status.agent.reachable }">
          <i />{{ status ? (status.agent.reachable ? "背包已连接" : "背包不可达") : "连接中" }}
        </span>
        <button type="button" class="btn" :disabled="refreshing" @click="refreshStatus()">
          <AppIcon name="refresh" :size="13" />{{ refreshing ? "刷新中…" : "刷新" }}
        </button>
      </template>
    </PageHeader>

    <!-- 未接入 / 不可达：直接给出配置方法，不展示空转的播放器 -->
    <div v-if="!loading && status && !status.configured" class="card empty-card">
      <AppIcon name="video" :size="26" />
      <h3>本部署尚未接入数据采集背包</h3>
      <p>
        在业务服务上设置 <code>ROBODOG_BACKPACK_URL</code>（板端 Agent 地址，例如
        <code>http://192.168.234.200:8000</code>），浏览器侧再加
        <code>ROBODOG_BACKPACK_WEBRTC_URL</code>（mediamtx，例如
        <code>http://192.168.234.200:8889</code>），然后重启服务。
      </p>
    </div>

    <div v-else class="monitor">
      <div class="monitor-main">
        <div class="monitor-rail">
          <SourceRail
            :sources="sources"
            :active="activeKey"
            :ai-running="aiRunning"
            :ai-source="aiCam"
            :disabled="busy || loading"
            @select="selectSource"
          />
          <span class="rail-updated">
            {{ updatedAt ? `更新于 ${updatedAt}` : "" }}
            <b v-if="refreshing">· 同步中</b>
          </span>
        </div>

        <p v-if="activeSource && !activeSource.available" class="rail-hint">
          <AppIcon name="info" :size="13" />{{ activeSource.hint }}
        </p>

        <VideoStage
          :whep-base="status?.webrtc.base ?? null"
          :stream="playStream"
          :title="activeSource?.label || '—'"
          :ai-active="aiRunning"
          :ai-badge="aiBadge"
          :show-boxes="aiRunning && !osdActive"
          :boxes="boxes"
          :frame-w="frame.w"
          :frame-h="frame.h"
          :hint="stageHint"
          :detail="detailText"
          @phase="onPhase"
        />

        <div class="facts">
          <div class="fact">
            <span class="fact-label">当前通道</span>
            <b>{{ activeSource?.label || "—" }}</b>
            <small>{{ activeSource?.kind === "infrared" ? "红外网络摄像头" : activeSource?.kind === "relay" ? "机器狗前置 RTSP" : "USB 摄像头" }}</small>
          </div>
          <div class="fact">
            <span class="fact-label">AI 识别</span>
            <b :class="{ on: aiRunning }">{{ aiRunning ? "运行中" : "未开启" }}</b>
            <small>{{ aiRunning ? `${aiStat?.fps ?? 0} fps · ${Math.round(aiStat?.last_ms ?? 0)} ms` : "NPU 空闲" }}</small>
          </div>
          <div class="fact">
            <span class="fact-label">画框方式</span>
            <b>{{ osdActive ? "背包烧录" : overlay === "osd" ? "浏览器叠加（回退）" : "浏览器叠加" }}</b>
            <small>{{ osdActive ? `${previewStream} 流` : "干净流 + SVG" }}</small>
          </div>
          <div class="fact">
            <span class="fact-label">检出</span>
            <b>{{ boxes.length ? `${boxes.length} 个目标` : "无目标" }}</b>
            <small>{{ lastDetectAt ? `最近 ${clock(lastDetectAt)}` : "等待识别结果" }}</small>
          </div>
        </div>

        <p v-if="overlayNote" class="rail-hint warn">
          <AppIcon name="warn" :size="13" />{{ overlayNote }}
        </p>
        <p v-if="loadError" class="rail-hint bad">
          <AppIcon name="warn" :size="13" />{{ loadError }}
        </p>
      </div>

      <aside class="monitor-side">
        <AiPanel
          :enabled="aiRunning"
          :tasks="tasks"
          :detect="detectStatus"
          :disabled-reason="aiDisabledReason"
          :osd-ready="!!status?.osd.ready"
          :busy="busy || loading"
          :notes="notes"
          :error="actionError"
          v-model:overlay="overlay"
          v-model:task="form.task"
          v-model:labels="form.labels"
          v-model:conf="form.conf"
          v-model:fps="form.fps"
          @update:enabled="onToggleAi"
          @start="startAi"
          @stop="stopAi"
          @apply="applyConfig"
        />

        <div class="card">
          <div class="card-head">
            <div class="card-title"><AppIcon name="activity" :size="15" />检出汇总</div>
            <span class="card-sub">{{ counts.length ? `${counts.length} 类` : "—" }}</span>
          </div>
          <div class="card-body">
            <div v-if="counts.length" class="counts">
              <span
                v-for="item in counts"
                :key="item.name"
                class="count-chip"
                :style="{ borderColor: item.color, color: item.color }"
              >
                {{ item.zh }}<b>{{ item.value }}</b>
              </span>
            </div>
            <p v-else class="side-empty">
              {{ aiRunning ? "当前画面没有通过阈值的目标" : "开启 AI 后显示实时检出数量" }}
            </p>

            <div v-if="feed.length" class="feed">
              <div v-for="item in feed" :key="`${item.ts}-${item.text}`" class="feed-row">
                <i :style="{ background: item.color }" />
                <span class="feed-time">{{ clock(item.ts) }}</span>
                <span class="feed-text">{{ item.text }}</span>
              </div>
            </div>

            <p class="side-note">
              类别名与「同类数量」由板端统一生成（例：火焰 3处），与背包烧录进视频的标签完全一致。
            </p>
          </div>
        </div>

        <div class="card">
          <div class="card-head">
            <div class="card-title"><AppIcon name="cpu" :size="15" />链路状态</div>
          </div>
          <div class="card-body">
            <dl class="link-list">
              <div>
                <dt>背包 Agent</dt>
                <dd :class="status?.agent.reachable ? 'ok' : 'bad'">
                  {{ status?.agent.reachable ? "可达" : "不可达" }}
                </dd>
              </div>
              <div>
                <dt>机器狗前置</dt>
                <dd :class="status?.relay?.running ? 'ok' : ''">{{ relayText }}</dd>
              </div>
              <div>
                <dt>画框链路</dt>
                <dd :class="status?.osd.ready ? 'ok' : ''">
                  {{
                    status?.osd.ready
                      ? `就绪（${previewStream}）`
                      : aiRunning
                        ? "未就绪（已回退浏览器叠加）"
                        : "待 AI 开启"
                  }}
                </dd>
              </div>
              <div>
                <dt>模型包</dt>
                <dd>{{ status?.detect.tasks?.ready ?? 0 }} / {{ status?.detect.tasks?.count ?? 0 }} 可用</dd>
              </div>
            </dl>
            <p v-if="status?.detect.tasks_error" class="side-note bad">{{ status.detect.tasks_error }}</p>
            <p v-if="status?.detect.status_error" class="side-note bad">{{ status.detect.status_error }}</p>
            <p v-if="status?.detect.tasks?.note" class="side-note">{{ status.detect.tasks.note }}</p>
          </div>
        </div>
      </aside>
    </div>
  </div>
</template>

<style scoped>
.conn-tag {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 32px;
  padding: 0 11px;
  border-radius: 8px;
  border: 1px solid var(--line);
  background: #fff;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-2);
}

.conn-tag i {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--muted);
}

.conn-tag.ok i {
  background: var(--ok);
}

.conn-tag.bad i {
  background: var(--danger);
}

.monitor {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 384px;
  gap: 18px;
  align-items: start;
}

.monitor-main {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}

.monitor-rail {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}

.rail-updated {
  margin-left: auto;
  font-size: 11.5px;
  color: var(--muted);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.rail-updated b {
  color: var(--brand);
  font-weight: 600;
}

.rail-hint {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  margin: 0;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-2);
  background: #f8fafc;
  border: 1px dashed var(--line);
  border-radius: 9px;
  padding: 8px 11px;
}

.rail-hint.warn {
  color: #b45309;
  background: #fffbeb;
  border-color: #fde68a;
}

.rail-hint.bad {
  color: var(--danger);
  background: #fef2f2;
  border-color: #fecaca;
}

.facts {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 10px;
}

.fact {
  background: var(--surface);
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.fact-label {
  font-size: 11px;
  color: var(--muted);
  letter-spacing: 0.3px;
}

.fact b {
  font-size: 13.5px;
  font-weight: 700;
  color: var(--text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.fact b.on {
  color: #0d9488;
}

.fact small {
  font-size: 11px;
  color: var(--muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.monitor-side {
  display: flex;
  flex-direction: column;
  gap: 14px;
  min-width: 0;
}

.counts {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.count-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 3px 9px;
  border-radius: 7px;
  border: 1px solid currentColor;
  background: #fff;
  font-size: 11.5px;
  font-weight: 600;
}

.count-chip b {
  font-variant-numeric: tabular-nums;
}

.feed {
  margin-top: 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 168px;
  overflow-y: auto;
}

.feed-row {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 11.5px;
  color: var(--text-2);
  padding-bottom: 6px;
  border-bottom: 1px dashed var(--line-2);
}

.feed-row:last-child {
  border-bottom: 0;
  padding-bottom: 0;
}

.feed-row i {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex: none;
}

.feed-time {
  font-variant-numeric: tabular-nums;
  color: var(--muted);
  flex: none;
}

.feed-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.side-empty {
  margin: 0;
  font-size: 11.5px;
  color: var(--muted);
  line-height: 1.6;
}

.side-note {
  margin: 12px 0 0;
  font-size: 11px;
  line-height: 1.65;
  color: var(--muted);
}

.side-note.bad {
  color: var(--danger);
}

.link-list {
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.link-list div {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  font-size: 12px;
}

.link-list dt {
  color: var(--muted);
  flex: none;
}

.link-list dd {
  margin: 0;
  font-weight: 600;
  color: var(--text-2);
  text-align: right;
}

.link-list dd.ok {
  color: var(--ok);
}

.link-list dd.bad {
  color: var(--danger);
}

.empty-card {
  padding: 42px 28px;
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
  gap: 10px;
  color: var(--text-2);
}

.empty-card h3 {
  margin: 0;
  font-size: 15px;
  color: var(--text);
}

.empty-card p {
  margin: 0;
  max-width: 620px;
  font-size: 12.5px;
  line-height: 1.8;
}

.empty-card code {
  background: #f1f5f9;
  border-radius: 4px;
  padding: 1px 5px;
  font-size: 11.5px;
}

@media (max-width: 1280px) {
  .monitor {
    grid-template-columns: minmax(0, 1fr) 340px;
  }
}

@media (max-width: 1080px) {
  .monitor {
    grid-template-columns: minmax(0, 1fr);
  }

  .facts {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
</style>
