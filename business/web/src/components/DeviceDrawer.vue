<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import type { Device, TeleopAxes } from "@/api/types";
import { deviceApi } from "@/api/ops";
import AppIcon from "./AppIcon.vue";
import StatusBadge from "./StatusBadge.vue";
import VideoPlayer from "./VideoPlayer.vue";
import TeleopJoystick from "./TeleopJoystick.vue";
import {
  canGait,
  ctrlSourceMeta,
  formatDuration,
  isLocReady,
  locStatusMeta,
  motionStatusLabel,
  timeOnly
} from "@/utils/format";

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

/** 本体健康门禁：与客户端/服务端保持一致。
 *
 * 官方 App 在 sdk_node 未就绪时显示“设备初始化中”，在 ctrl_source 被外部
 * 抢占时暂停遥控，在急停未解除时不允许运动；这里同样给出可读原因并禁用操作。
 *
 * 注意：**不包含 ctrl_source**。控制权被抢占单列在 `controlIssue`，因为它可以被
 * 「强制接管」越过（遥控器关了但本体还在报 ctrl_source=3 就是这种情况）。
 */
const healthIssue = computed(() => {
  const device = props.device;
  if (!device) return "";
  if (!device.bodyOnline) return "本体 1315 状态超时（body_online=false）";
  if (device.sdkNodeAvailable === false) return "本体 SDK 节点未就绪（sdk_node=false）";
  if (device.estopSoftware === true) return "本体软件急停已触发";
  return "";
});
/** 控制源与运动状态：判断「是否真的完全接管」的两个硬指标 */
const ctrlMeta = computed(() => ctrlSourceMeta(props.device?.ctrlSource));
const ctrlByOther = computed(() => props.device?.ctrlSource === 3);
/** 平台强制接管中（跳过了 ctrl_source 门禁） */
const forceHeld = computed(() => !!props.device?.teleopForce);
/** 控制权在别人手里且没有强制接管时才拦遥控 */
const controlIssue = computed(() =>
  ctrlByOther.value && !forceHeld.value ? "控制权被其他遥控器/外部程序抢占" : ""
);
/** 已接管、在线、本体健康且控制权可用才允许遥控 */
const canControl = computed(
  () => holding.value && online.value && !healthIssue.value && !controlIssue.value
);
/** 本体离线 / SDK 节点未就绪时不建议接管；急停允许先接管再解除 */
const takeoverAllowed = computed(
  () =>
    online.value &&
    props.device?.sdkNodeAvailable !== false &&
    props.device?.estopSoftware !== true &&
    !!props.device?.bodyOnline
);

const gaitReady = computed(() => canGait(props.device?.motionStatus));
/** 接管了但本体/别的客户端还在拦着，摇杆按下去不会走 */
const takeoverBlocked = computed(
  () =>
    holding.value && (!!healthIssue.value || !!controlIssue.value || !gaitReady.value)
);

/** 重连本体链路：只在空闲时允许（任务/建图/接管中重连会丢运行态，服务端也会拒） */
const canReconnect = computed(
  () =>
    online.value &&
    !holding.value &&
    props.device?.status !== "执行任务" &&
    props.device?.runMode !== "sim"
);

/** 恢复类动作不要求接管、也不受急停/控制权门禁限制（与后端一致） */
const RECOVERY_ACTIONS = ["clear_emergency_stop", "avoid_on", "avoid_off"];
function actionDisabled(code: string) {
  if (RECOVERY_ACTIONS.includes(code)) return !online.value;
  return !canControl.value;
}

/** 定位状态：100 才算可以接任务，其余需要重定位或人工介入 */
const locMeta = computed(() => locStatusMeta(props.device?.locStatus));
const locReady = computed(() => isLocReady(props.device?.locStatus));

const statusText = computed(() => {
  if (!props.device) return "";
  if (props.device.status === "执行任务") return "正在执行巡检任务";
  if (props.device.status === "充电中") return "正在充电座回充";
  if (props.device.status === "故障") return "设备故障，等待运维处理";
  if (props.device.status === "离线") return "设备离线，最后心跳已超时";
  return "在线待命，等待任务下发";
});

/** 本体动作按钮：码值 + 文案 + 是否危险
 *
 * 只列协议 1302 里真实存在的动作（码值在设备客户端按文档映射）。
 * 「位姿复位」在 1302 枚举里没有对应值，已下线。
 */
const actions = [
  { code: "stand_up", label: "站立", icon: "trend-up", danger: false },
  { code: "lie_down", label: "卧倒", icon: "trend-down", danger: true },
  // 1302 cmd=1：现场 X30-Pro 没有物理急停按钮，`estop.software=true` 时只能靠它解锁
  { code: "clear_emergency_stop", label: "解除急停", icon: "shield", danger: false },
  { code: "light_on", label: "开灯", icon: "scan", danger: false },
  { code: "light_off", label: "关灯", icon: "shield", danger: false },
  // 1302 模式切换：摇杆走不动时先确认本体在「通用模式」（运动模式）
  { code: "generic_mode", label: "通用模式", icon: "activity", danger: false },
  { code: "navigation_mode", label: "导航模式", icon: "route", danger: false },
  // 1302 cmd=110/111：感知降级（相机无图/检测延迟）时规划器会报「终点附近有障碍物，等待中」
  { code: "avoid_off", label: "避障关闭", icon: "warn", danger: true },
  { code: "avoid_on", label: "避障开启", icon: "check", danger: false }
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
  // 遥控器还占着控制权时，普通接管会被服务端 400 拒掉；这里引导运维二次确认后强制接管
  const force = ctrlByOther.value;
  if (force) {
    try {
      await ElMessageBox.confirm(
        "本体当前仍报告控制权在遥控器/外部程序上（ctrl_source=3）。" +
          "如果遥控器已经断电或按下了「释放控制」键，可以强制接管：平台只跳过这一条门禁，" +
          "急停/离线/SDK 节点/姿态等其余健康检查照旧。遥控器还在使用中请勿继续，否则两边会互相抢控制。",
        "强制接管",
        {
          type: "warning",
          confirmButtonText: "确认遥控器已关闭，强制接管",
          cancelButtonText: "取消"
        }
      );
    } catch {
      return;
    }
  }
  busy.value = true;
  try {
    const res = await deviceApi.takeOver(props.device.id, force);
    if (res.forced) {
      ElMessage.warning(
        res.wsDelivered
          ? "已强制接管（本体仍报 ctrl_source=3）：摇杆无效说明遥控器还在控制，请确认已断电"
          : "已强制接管，但设备未连接（指令无法下发）"
      );
    } else {
      ElMessage.success(
        res.wsDelivered ? "已接管，可以遥控了" : "已接管，但设备未连接（指令无法下发）"
      );
    }
    emit("changed");
  } catch (error) {
    ElMessage.error((error as Error).message || "接管失败");
  } finally {
    busy.value = false;
  }
}

/** 重连本体链路：设备客户端原地重建它与本体的 WS（不退出进程、平台连接保持） */
async function reconnectBody() {
  if (!props.device || busy.value) return;
  try {
    await ElMessageBox.confirm(
      "将重建设备客户端与本体的 WebSocket 连接（约 5~15 秒）。" +
        "客户端不会重启、平台连接保持，本体、任务与遥控器都不受影响；" +
        "期间设备会短暂显示「设备初始化中」，接管态会失效需要重新接管。" +
        "请确认当前没有正在执行的任务或建图。",
      "重连本体链路",
      { type: "warning", confirmButtonText: "重连", cancelButtonText: "取消" }
    );
  } catch {
    return;
  }
  busy.value = true;
  try {
    const res = await deviceApi.command(props.device.id, "本体重连");
    if (res.wsDelivered) {
      ElMessage.success("已下发重连指令：客户端正在原地重建到本体的连接");
    } else {
      ElMessage.warning("重连指令未送达（设备客户端不在线）");
    }
    emit("changed");
  } catch (error) {
    ElMessage.error((error as Error).message || "重连失败");
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

/** 动作下发后刷新设备：本体状态变化经心跳回传有延迟，先立即刷一次，再补几次 */
function scheduleDeviceRefresh() {
  emit("changed");
  for (const ms of [1200, 3000, 6000, 12000]) {
    window.setTimeout(() => emit("changed"), ms);
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
    // 服务端只保证「指令送进了设备连接」，本体是否受理要看设备侧回执
    // （受理失败会以 error 事件落到告警中心）。
    lastAck.value = `${res.label}已下发`;
    ElMessage.success(
      `${res.label} 已下发至设备${res.wsDelivered ? "，等待本体受理" : "（设备未连接）"}`
    );
    // 解除急停/站立等动作会改变 1315 状态；设备心跳有间隔，补几次刷新，
    // 否则抽屉会一直停在动作前的 estopSoftware=true，看起来像“没生效”。
    scheduleDeviceRefresh();
  } catch (error) {
    ElMessage.error((error as Error).message || `${label} 下发失败`);
  }
}

async function relocalize() {
  if (!props.device || busy.value) return;
  const device = props.device;
  try {
    await ElMessageBox.confirm(
      `对 ${device.deviceId} 下发重新定位（地图：${device.activeMapId || "园区基线"}）。\n` +
        "设备没被搬动时约 10~30 秒即可收敛；若已被搬到地图外，请先远程接管移回原位。",
      "重新定位",
      { type: "warning", confirmButtonText: "下发", cancelButtonText: "取消" }
    );
  } catch {
    return;
  }
  busy.value = true;
  try {
    const res = await deviceApi.locate(device.id, { mapId: device.activeMapId });
    ElMessage.success(
      res.wsDelivered ? "已下发重新定位，等待定位收敛" : "设备未连接，指令未送达"
    );
    emit("changed");
  } catch (error) {
    ElMessage.error((error as Error).message || "重新定位下发失败");
  } finally {
    busy.value = false;
  }
}

/** 急停：不受接管态限制，任何状态都能停 */
async function emergencyStop() {
  if (!props.device) return;
  try {
    await deviceApi.action(props.device.id, "emergency_stop");
    ElMessage.warning("急停已下发");
    lastAck.value = "急停";
    scheduleDeviceRefresh();
  } catch (error) {
    ElMessage.error((error as Error).message || "急停下发失败");
  }
}
</script>

<template>
  <div v-if="open" class="drawer-overlay" @click="emit('close')" />
  <aside v-if="open && device" class="drawer" :class="{ 'drawer-wide': tab === 'teleop' }">
    <div class="drawer-head">
      <div>
        <div class="drawer-title">
          <AppIcon name="dog" />{{ device.deviceId }}
          <span class="tag blue">{{ device.model }}</span>
          <span v-if="holding" class="tag amber"><i class="dot" />已接管</span>
          <span v-if="device.estopSoftware" class="tag orange">软件急停</span>
          <span v-else-if="device.sdkNodeAvailable === false" class="tag orange">SDK未就绪</span>
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
        <VideoPlayer
          :url="device.videoUrl"
          :type="device.videoType"
          :online="online"
          :device-id="device.id"
        />

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

        <div v-if="healthIssue || controlIssue" class="loc-alert">
          <AppIcon name="warn" />
          <span>
            本体健康检查未通过：<b>{{ healthIssue || controlIssue }}</b>，遥控与任务下发已暂停。
            请先等待 SDK 节点就绪 / 解除急停 / 断开外部遥控器。
          </span>
        </div>

        <div v-if="device.bodyOnline && !locReady" class="loc-alert">
          <AppIcon name="warn" />
          <span>
            定位状态：<b>{{ locMeta.label }}</b>，机器狗可能不在地图上（当前地图：
            {{ device.activeMapId || "未载图" }}），任务会被拒绝下发。请先远程接管把设备
            移回地图覆盖范围，或下发「重新定位」。
          </span>
          <button class="btn sm" :disabled="busy" @click="relocalize">
            <AppIcon name="waypoint" />重新定位
          </button>
        </div>

        <div class="card-title" style="margin-bottom: 10px"><AppIcon name="list" />设备档案</div>
        <div class="kv"><span>设备编号</span><b class="mono">{{ device.deviceId }}</b></div>
        <div class="kv"><span>设备型号</span><b>{{ device.model }}</b></div>
        <div class="kv"><span>所属园区</span><b>{{ device.parkName || "未绑定" }}</b></div>
        <div class="kv"><span>当前区域</span><b>{{ device.zoneName || "—" }}</b></div>
        <div class="kv"><span>实时位置</span><b class="mono">{{ device.position }}</b></div>
        <div class="kv"><span>当前任务</span><b>{{ device.currentTaskName }}</b></div>
        <div class="kv"><span>工作模式</span><b>{{ device.robotMode || "—" }}</b></div>
        <div class="kv">
          <span>定位状态</span>
          <b>
            <span class="badge" :class="locMeta.tone"><i />{{ locMeta.label }}</span>
            <span v-if="!locReady" class="muted" style="margin-left: 6px; font-weight: 400">
              需要重定位后任务才能起步
            </span>
          </b>
        </div>
        <div class="kv">
          <span>当前地图</span><b class="mono">{{ device.activeMapId || "未载图" }}</b>
        </div>
        <div class="kv">
          <span>控制源</span>
          <b>
            <span class="badge" :class="ctrlMeta.tone"><i />{{ ctrlMeta.label }}</span>
            <span
              v-if="holding && ctrlByOther && !forceHeld"
              class="muted"
              style="margin-left: 6px; font-weight: 400"
            >
              遥控器仍持权，平台摇杆不会生效
            </span>
            <span
              v-else-if="holding && forceHeld"
              class="muted"
              style="margin-left: 6px; font-weight: 400"
            >
              已强制接管（本体仍报占用，摇杆无效说明遥控器未断开）
            </span>
          </b>
        </div>
        <div class="kv">
          <span>运动状态</span>
          <b>
            {{ motionStatusLabel(device.motionStatus) }}
            <span v-if="!gaitReady" class="muted" style="margin-left: 6px; font-weight: 400">
              卧倒 / 锁定状态无法迈步
            </span>
          </b>
        </div>
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

      <!-- ── 手动遥控：左实时画面 / 右遥控，一屏操作 ── -->
      <template v-else>
        <div class="teleop-layout">
          <div class="teleop-visual">
            <VideoPlayer
              :url="device.videoUrl"
              :type="device.videoType"
              :online="online"
              :device-id="device.id"
            />
            <div class="visual-strip">
              <span class="chip"><AppIcon name="battery" />{{ device.battery }}%</span>
              <span class="chip"><AppIcon name="activity" />{{ device.robotMode || "—" }}</span>
              <span class="chip"><AppIcon name="pin" />{{ device.position }}</span>
              <span class="chip" :class="{ warn: !holding }">
                <AppIcon name="shield" />{{ holding ? "已接管" : "未接管" }}
              </span>
            </div>
            <p v-if="lastAck" class="ack-line">
              <AppIcon name="check-circle" />{{ lastAck }}
            </p>
            <ul class="teleop-notes">
              <li>摇杆推住即连续下发，松手立即停车，无需手动停止。</li>
              <li>网络中断时设备侧 1 秒看门狗会自动停车。</li>
              <li>电池低于 15% 时本体可能拒绝大速度指令。</li>
              <li>急停不释放接管，恢复后需手动点「站立」。</li>
            </ul>
          </div>

          <div class="teleop-controls">
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
              <button
                v-if="!holding"
                class="btn primary sm"
                :disabled="!takeoverAllowed || busy"
                @click="takeOver"
              >
                <AppIcon name="shield" />{{ ctrlByOther ? "强制接管" : "接管" }}
              </button>
              <button v-else class="btn sm" :disabled="busy" @click="release">
                <AppIcon name="undo" />释放
              </button>
            </div>

            <div class="takeover-diag" :class="{ bad: takeoverBlocked }">
              <div class="diag-row">
                <span>控制源</span>
                <b><span class="badge" :class="ctrlMeta.tone"><i />{{ ctrlMeta.label }}</span></b>
                <span>运动状态</span>
                <b>{{ motionStatusLabel(device.motionStatus) }}</b>
                <span>机器模式</span>
                <b>{{ device.robotMode || "—" }}</b>
              </div>
              <p v-if="healthIssue" class="diag-tip">
                <AppIcon name="warn" />
                本体健康检查未通过：{{ healthIssue }}。遥控/任务已暂停，请先处理本体状态。
              </p>
              <p v-else-if="forceHeld && ctrlByOther" class="diag-tip">
                <AppIcon name="warn" />
                已强制接管，但本体仍报控制权在遥控器上：平台已跳过这条门禁，摇杆照常下发。
                若摇杆无效，说明遥控器仍在控制，请关闭遥控器电源后重新点「接管」（或先「释放」）。
              </p>
              <p v-else-if="ctrlByOther" class="diag-tip">
                <AppIcon name="warn" />
                硬件遥控器（或其他 SDK）仍持有控制权，本体不会执行平台摇杆。请关闭遥控器电源 /
                按遥控器的「释放控制」键；若遥控器已断电而状态还没刷新，可直接点「强制接管」。
              </p>
              <p v-else-if="holding && !gaitReady" class="diag-tip">
                <AppIcon name="warn" />
                本体当前是「{{ motionStatusLabel(device.motionStatus) }}」，摇杆不会动。请先点
                「站立」，等运动状态变成站立中再遥控。
              </p>
              <p v-else-if="holding && (!ctrlByOther || forceHeld) && gaitReady" class="diag-tip ok">
                <AppIcon name="check-circle" />
                已完全接管：控制权在平台、本体可迈步，摇杆按 50Hz 持续下发。
              </p>
            </div>

            <div class="link-tools">
              <span class="muted">
                本体连不上、状态迟迟不刷新时可重建客户端到本体的连接；不会改动本体、任务或遥控器。
              </span>
              <button class="btn sm" :disabled="!canReconnect || busy" @click="reconnectBody">
                <AppIcon name="refresh" />重连本体链路
              </button>
            </div>

            <div class="teleop-panel" :class="{ locked: !canControl }">
              <TeleopJoystick
                :disabled="!canControl || takeoverBlocked"
                :speed="speed"
                @control="sendAxes"
              />

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
                :disabled="actionDisabled(action.code)"
                @click="sendAction(action.code, action.label, action.danger)"
              >
                <AppIcon :name="action.icon" />{{ action.label }}
              </button>
            </div>

            <button class="stop-btn" :disabled="!online" @click="emergencyStop">
              <AppIcon name="warn" />急停
              <small>任何状态下可用</small>
            </button>
          </div>
        </div>
      </template>
    </div>

    <div class="drawer-foot">
      <button
        class="btn primary"
        :disabled="!online || !!healthIssue"
        @click="emit('command', '开始巡检')"
      >
        <AppIcon name="play" />开始巡检
      </button>
      <button class="btn" :disabled="!online || !!healthIssue" @click="emit('command', '暂停任务')">
        <AppIcon name="pause" />暂停
      </button>
      <button class="btn" :disabled="!online || !!healthIssue" @click="emit('command', '召回充电')">
        <AppIcon name="charge" />召回充电
      </button>
      <button class="btn" @click="tab = 'teleop'">
        <AppIcon name="board" />遥控
      </button>
    </div>
  </aside>
</template>

<style scoped>
/* 遥控页要看画面，440px 太窄：接管后把抽屉拉宽做双栏 */
.drawer.drawer-wide {
  width: min(780px, 100%);
}

.teleop-layout {
  display: grid;
  grid-template-columns: minmax(0, 1.12fr) minmax(0, 1fr);
  gap: 16px;
  align-items: start;
}

.teleop-visual {
  display: flex;
  flex-direction: column;
  gap: 10px;
  position: sticky;
  top: 0;
}

.visual-strip {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  border-radius: 999px;
  border: 1px solid var(--line);
  background: var(--panel, #f8fafc);
  font-size: 11px;
  font-weight: 600;
  color: var(--text);
  white-space: nowrap;
}

.chip.warn {
  border-color: #f59e0b;
  background: #fffbeb;
  color: #b45309;
}

.teleop-controls {
  display: flex;
  flex-direction: column;
}

@media (max-width: 860px) {
  .teleop-layout {
    grid-template-columns: 1fr;
  }

  .teleop-visual {
    position: static;
  }
}

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

/* 完全接管诊断条：控制源 / 运动状态 / 机器模式一眼可见 */
.takeover-diag {
  padding: 10px 12px;
  border-radius: 9px;
  border: 1px solid var(--line);
  background: #f8fafc;
  margin-bottom: 12px;
  font-size: 11.5px;
}

.takeover-diag.bad {
  border-color: #fde3b0;
  background: #fffaf0;
}

/* 运维入口：重连本体链路（只重建客户端到本体的连接） */
.link-tools {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 8px 12px;
  margin-bottom: 12px;
  border-radius: 9px;
  border: 1px dashed var(--line);
  font-size: 11.5px;
  line-height: 1.5;
}

.link-tools .muted {
  flex: 1;
}

.diag-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px 10px;
  color: var(--muted);
}

.diag-row b {
  color: var(--text);
  font-weight: 600;
}

.diag-row span:not(:first-child) {
  margin-left: 6px;
}

.diag-tip {
  display: flex;
  align-items: flex-start;
  gap: 7px;
  margin: 8px 0 0;
  line-height: 1.6;
  color: #92400e;
}

.diag-tip .ic {
  width: 14px;
  height: 14px;
  flex: none;
  margin-top: 2px;
  color: #d97706;
}

.diag-tip.ok {
  color: #0f766e;
}

.diag-tip.ok .ic {
  color: #0d9488;
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
  grid-template-columns: repeat(2, 1fr);
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
