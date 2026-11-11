<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import type { Device, TeleopAxes } from "@/api/types";
import { deviceApi } from "@/api/ops";
import AppIcon from "./AppIcon.vue";
import StatusBadge from "./StatusBadge.vue";
import VideoPlayer from "./VideoPlayer.vue";
import TeleopJoystick from "./TeleopJoystick.vue";
import { formatDuration, timeOnly } from "@/utils/format";

const props = defineProps<{ device: Device | null; open: boolean }>();
const emit = defineEmits<{
  close: [];
  command: [command: string];
  /** 接管态或设备数据变化，通知父级刷新列表 */
  changed: [];
}>();

/** 面板页签：监控 / 遥控 */
const tab = ref<"monitor" | "teleop">("monitor");
/** 速度档位 0.1 ~ 1.0 */
const speed = ref(0.5);
/** 接管请求进行中，防重复点击 */
const busy = ref(false);
/** 最近一次遥控回执，用于显示「指令已送达」 */
const lastAck = ref("");

const online = computed(() => !!props.device && props.device.status !== "离线");
const holding = computed(() => !!props.device?.teleopHolder);
/** 已接管且在线才允许遥控 */
const canControl = computed(() => holding.value && online.value);

const statusText = computed(() => {
  if (!props.device) return "";
  if (props.device.status === "执行任务") return "正在执行巡检任务";
  if (props.device.status === "充电中") return "正在充电座回充";
  if (props.device.status === "故障") return "设备故障，等待运维处理";
  if (props.device.status === "离线") return "设备离线，最后心跳已超时";
  return "在线待命，等待任务下发";
});

/** 本体动作按钮：码值 + 文案 + 是否危险 */
const actions = [
  { code: "stand_up", label: "站立", icon: "trend-up", danger: false },
  { code: "lie_down", label: "卧倒", icon: "trend-down", danger: true },
  { code: "light_on", label: "开灯", icon: "scan", danger: false },
  { code: "light_off", label: "关灯", icon: "shield", danger: false },
  { code: "reset_pose", label: "位姿复位", icon: "undo", danger: false }
] as const;

const speedPresets = [
  { value: 0.2, label: "慢" },
  { value: 0.5, label: "中" },
  { value: 1, label: "快" }
];

const events = computed(() => {
  const device = props.device;
  if (!device) return [];
  return [
    {
      time: timeOnly(device.lastHeartbeat),
      text: "状态心跳上报正常",
      color: "#10b981"
    },
    {
      time: device.status === "执行任务" ? "09:02" : "08:58",
      text:
        device.currentTaskName && device.currentTaskName !== "—"
          ? `开始执行「${device.currentTaskName}」`
          : "进入待命状态",
      color: "#0284c7"
    },
    {
      time: "08:40",
      text: "完成自检，传感器状态正常",
      color: "#94a3b8"
    }
  ];
});

// ── 接管保活 ────────────────────────────────────────────
// 服务端接管 120s 超时，这里 45s 续一次，页面关掉自然失效
let keepalive: number | null = null;

function stopKeepalive() {
  if (keepalive !== null) {
    window.clearInterval(keepalive);
    keepalive = null;
  }
}

function startKeepalive() {
  stopKeepalive();
  if (!props.device) return;
  const id = props.device.id;
  keepalive = window.setInterval(() => {
    if (!holding.value) return;
    deviceApi.ping(id).catch(() => stopKeepalive());
  }, 45_000);
}

watch(
  () => [props.open, props.device?.id, props.device?.teleopHolder],
  () => {
    if (props.open && holding.value) startKeepalive();
    else stopKeepalive();
  }
);

onBeforeUnmount(stopKeepalive);

// ── 操作 ────────────────────────────────────────────────
async function takeOver() {
  if (!props.device || busy.value) return;
  busy.value = true;
  try {
    const res = await deviceApi.takeOver(props.device.id);
    ElMessage.success(
      res.wsDelivered ? "已接管，可以遥控了" : "已接管，但设备未连接（指令无法下发）"
    );
    emit("changed");
  } catch (error) {
    ElMessage.error((error as Error).message || "接管失败");
  } finally {
    busy.value = false;
  }
}

async function release() {
  if (!props.device || busy.value) return;
  busy.value = true;
  try {
    await deviceApi.release(props.device.id);
    ElMessage.success("已释放接管");
    emit("changed");
  } catch (error) {
    ElMessage.error((error as Error).message || "释放失败");
  } finally {
    busy.value = false;
  }
}

async function sendAxes(axes: TeleopAxes) {
  if (!props.device || !canControl.value) return;
  try {
    const res = await deviceApi.teleop(props.device.id, axes);
    lastAck.value = res.stopped ? "已停止" : "遥控中";
  } catch (error) {
    lastAck.value = "下发失败";
    ElMessage.error((error as Error).message || "遥控下发失败");
  }
}

async function sendAction(code: string, label: string, danger: boolean) {
  if (!props.device) return;
  // 危险动作二次确认，防误触
  if (danger) {
    try {
      await ElMessageBox.confirm(`确定让 ${props.device.deviceId} 执行「${label}」？`, "确认动作", {
        type: "warning",
        confirmButtonText: "执行",
        cancelButtonText: "取消"
      });
    } catch {
      return;
    }
  }
  try {
    const res = await deviceApi.action(props.device.id, code);
    lastAck.value = `${res.label}已下发`;
    ElMessage.success(`${res.label} 已下发${res.wsDelivered ? "" : "（设备未连接）"}`);
  } catch (error) {
    ElMessage.error((error as Error).message || `${label} 下发失败`);
  }
}

/** 急停：不受接管态限制，任何状态都能停 */
async function emergencyStop() {
  if (!props.device) return;
  try {
    await deviceApi.action(props.device.id, "emergency_stop");
    ElMessage.warning("急停已下发");
    lastAck.value = "急停";
  } catch (error) {
    ElMessage.error((error as Error).message || "急停下发失败");
  }
}
</script>

<template>
  <div v-if="open" class="drawer-overlay" @click="emit('close')" />
  <aside v-if="open && device" class="drawer">
    <div class="drawer-head">
      <div>
        <div class="drawer-title">
          <AppIcon name="dog" />{{ device.deviceId }}
          <span class="tag blue">{{ device.model }}</span>
          <span v-if="holding" class="tag amber"><i class="dot" />已接管</span>
        </div>
        <div class="muted" style="font-size: 11px; margin-top: 5px">
          {{ device.parkName || "未绑定园区" }}{{ device.zoneName ? ` · ${device.zoneName}` : "" }}
        </div>
      </div>
      <button class="btn ghost icon" title="关闭" @click="emit('close')">
        <AppIcon name="close" />
      </button>
    </div>

    <div class="drawer-tabs">
      <button class="drawer-tab" :class="{ on: tab === 'monitor' }" @click="tab = 'monitor'">
        <AppIcon name="video" />实时监控
      </button>
      <button class="drawer-tab" :class="{ on: tab === 'teleop' }" @click="tab = 'teleop'">
        <AppIcon name="board" />手动遥控
      </button>
    </div>

    <div class="drawer-body">
      <!-- ── 实时监控 ─────────────────────────────── -->
      <template v-if="tab === 'monitor'">
        <VideoPlayer :url="device.videoUrl" :type="device.videoType" :online="online" />

        <div class="drawer-section-head">
          <div class="card-title"><AppIcon name="activity" />运行指标</div>
          <StatusBadge :status="device.status" />
        </div>
        <div class="metric-grid">
          <div class="metric">
            <span><AppIcon name="battery" />剩余电量</span><b>{{ device.battery }}<small> %</small></b>
          </div>
          <div class="metric">
            <span><AppIcon name="cpu" />机身温度</span>
            <b :style="{ color: device.temperature > 50 ? '#dc2626' : 'inherit' }">
              {{ device.temperature.toFixed(1) }}<small> ℃</small>
            </b>
          </div>
          <div class="metric">
            <span><AppIcon name="clock" />今日运行时长</span>
            <b>{{ formatDuration(device.runtimeMinutes * 60) }}</b>
          </div>
          <div class="metric">
            <span><AppIcon name="pin" />累计里程</span>
            <b>{{ device.odometer.toFixed(1) }}<small> km</small></b>
          </div>
        </div>

        <div class="card-title" style="margin-bottom: 10px"><AppIcon name="list" />设备档案</div>
        <div class="kv"><span>设备编号</span><b class="mono">{{ device.deviceId }}</b></div>
        <div class="kv"><span>设备型号</span><b>{{ device.model }}</b></div>
        <div class="kv"><span>所属园区</span><b>{{ device.parkName || "未绑定" }}</b></div>
        <div class="kv"><span>当前区域</span><b>{{ device.zoneName || "—" }}</b></div>
        <div class="kv"><span>实时位置</span><b class="mono">{{ device.position }}</b></div>
        <div class="kv"><span>当前任务</span><b>{{ device.currentTaskName }}</b></div>
        <div class="kv"><span>信号强度</span><b>{{ device.signal > 0 ? `良好（${device.signal}/4）` : "无信号" }}</b></div>
        <div class="kv"><span>固件版本</span><b class="mono">{{ device.firmware }}</b></div>
        <div class="kv"><span>视频源</span><b class="mono">{{ device.videoUrl || "未配置" }}</b></div>
        <div class="kv"><span>运行状态</span><b>{{ statusText }}</b></div>

        <div class="card-title" style="margin: 18px 0 10px"><AppIcon name="clock" />最近事件</div>
        <div class="timeline">
          <div v-for="event in events" :key="event.text + event.time" class="tl-item">
            <div
              class="tl-dot"
              :style="{
                background: `${event.color}1a`,
                borderColor: `${event.color}55`,
                color: event.color
              }"
            >
              <AppIcon name="check" />
            </div>
            <div class="tl-body">
              <b>{{ event.text }}</b>
              <time class="mono">{{ event.time }}</time>
            </div>
          </div>
        </div>
      </template>

      <!-- ── 手动遥控 ─────────────────────────────── -->
      <template v-else>
        <div class="take-bar" :class="{ on: holding }">
          <div class="take-info">
            <b>{{ holding ? "已接管控制权" : "未接管" }}</b>
            <span v-if="holding" class="muted">
              遥控与动作指令已解锁，闲置 2 分钟自动释放
            </span>
            <span v-else class="muted">
              必须先接管才能遥控，避免多人同时操作
            </span>
          </div>
          <button v-if="!holding" class="btn primary sm" :disabled="!online || busy" @click="takeOver">
            <AppIcon name="shield" />接管
          </button>
          <button v-else class="btn sm" :disabled="busy" @click="release">
            <AppIcon name="undo" />释放
          </button>
        </div>

        <div class="teleop-panel" :class="{ locked: !canControl }">
          <TeleopJoystick :disabled="!canControl" :speed="speed" @control="sendAxes" />

          <div class="speed-box">
            <span class="speed-label">速度档位</span>
            <div class="speed-presets">
              <button
                v-for="preset in speedPresets"
                :key="preset.value"
                class="speed-btn"
                :class="{ on: speed === preset.value }"
                @click="speed = preset.value"
              >
                {{ preset.label }}
              </button>
            </div>
            <small class="muted">{{ Math.round(speed * 100) }}% 上限</small>
          </div>
        </div>

        <div class="action-grid">
          <button
            v-for="action in actions"
            :key="action.code"
            class="action-btn"
            :disabled="!canControl"
            @click="sendAction(action.code, action.label, action.danger)"
          >
            <AppIcon :name="action.icon" />{{ action.label }}
          </button>
        </div>

        <button class="stop-btn" :disabled="!online" @click="emergencyStop">
          <AppIcon name="warn" />急停
          <small>任何状态下可用</small>
        </button>

        <p v-if="lastAck" class="ack-line">
          <AppIcon name="check-circle" />{{ lastAck }}
        </p>

        <div class="card-title" style="margin: 16px 0 8px"><AppIcon name="info" />操作须知</div>
        <ul class="teleop-notes">
          <li>摇杆推住即连续下发，松手立即停车，无需手动停止。</li>
          <li>网络中断时设备侧 1 秒看门狗会自动停车。</li>
          <li>电池低于 15% 时本体可能拒绝大速度指令。</li>
          <li>急停不释放接管，恢复后需手动点「站立」。</li>
        </ul>
      </template>
    </div>

    <div class="drawer-foot">
      <button class="btn primary" :disabled="!online" @click="emit('command', '开始巡检')">
        <AppIcon name="play" />开始巡检
      </button>
      <button class="btn" :disabled="!online" @click="emit('command', '暂停任务')">
        <AppIcon name="pause" />暂停
      </button>
      <button class="btn" :disabled="!online" @click="emit('command', '召回充电')">
        <AppIcon name="charge" />召回充电
      </button>
      <button class="btn" @click="tab = 'teleop'">
        <AppIcon name="board" />遥控
      </button>
    </div>
  </aside>
</template>

<style scoped>
.drawer-title {
  font-size: 15px;
  font-weight: 700;
  display: flex;
  align-items: center;
  gap: 9px;
}

.drawer-section-head {
  margin-top: 16px;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: #f59e0b;
  display: inline-block;
  margin-right: 4px;
}

.drawer-tabs {
  display: flex;
  gap: 4px;
  padding: 0 16px;
  border-bottom: 1px solid var(--line);
}

.drawer-tab {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 9px 12px;
  font-size: 12px;
  font-weight: 600;
  color: var(--muted);
  background: none;
  border: none;
  border-bottom: 2px solid transparent;
  cursor: pointer;
}

.drawer-tab.on {
  color: #2563eb;
  border-bottom-color: #2563eb;
}

.take-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  border-radius: 9px;
  border: 1px solid var(--line);
  background: var(--panel, #f8fafc);
  margin-bottom: 14px;
}

.take-bar.on {
  border-color: #f59e0b;
  background: #fffbeb;
}

.take-info {
  display: flex;
  flex-direction: column;
  gap: 3px;
  font-size: 12px;
}

.take-info b {
  font-size: 13px;
}

.teleop-panel {
  display: flex;
  align-items: center;
  justify-content: space-around;
  gap: 12px;
  padding: 14px 6px;
  border-radius: 10px;
  border: 1px dashed var(--line);
}

.teleop-panel.locked {
  opacity: 0.85;
}

.speed-box {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 7px;
}

.speed-label {
  font-size: 11px;
  color: var(--muted);
}

.speed-presets {
  display: flex;
  gap: 5px;
}

.speed-btn {
  width: 34px;
  height: 28px;
  border-radius: 7px;
  border: 1px solid var(--line);
  background: var(--card);
  color: var(--text);
  font-size: 12px;
  cursor: pointer;
}

.speed-btn.on {
  border-color: #2563eb;
  color: #2563eb;
  background: #eff6ff;
}

.action-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 8px;
  margin-top: 14px;
}

.action-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: 38px;
  border-radius: 8px;
  border: 1px solid var(--line);
  background: var(--card);
  color: var(--text);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}

.action-btn:hover:not(:disabled) {
  border-color: #2563eb;
  color: #2563eb;
}

.action-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.stop-btn {
  width: 100%;
  margin-top: 10px;
  height: 46px;
  border-radius: 9px;
  border: 1px solid #dc2626;
  background: #fef2f2;
  color: #dc2626;
  font-size: 14px;
  font-weight: 700;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  cursor: pointer;
}

.stop-btn small {
  font-size: 10px;
  font-weight: 400;
  opacity: 0.75;
}

.stop-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.ack-line {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 11px;
  color: #10b981;
  margin: 8px 0 0;
}

.teleop-notes {
  margin: 0;
  padding-left: 16px;
  font-size: 11px;
  line-height: 1.9;
  color: var(--muted);
}
</style>
