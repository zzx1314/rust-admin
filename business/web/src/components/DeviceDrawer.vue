<script setup lang="ts">
import { computed } from "vue";
import type { Device } from "@/api/types";
import AppIcon from "./AppIcon.vue";
import StatusBadge from "./StatusBadge.vue";
import { formatDuration, timeOnly } from "@/utils/format";

const props = defineProps<{ device: Device | null; open: boolean }>();
const emit = defineEmits<{
  close: [];
  command: [command: string];
}>();

const statusText = computed(() => {
  if (!props.device) return "";
  if (props.device.status === "执行任务") return "正在执行巡检任务";
  if (props.device.status === "充电中") return "正在充电座回充";
  if (props.device.status === "故障") return "设备故障，等待运维处理";
  if (props.device.status === "离线") return "设备离线，最后心跳已超时";
  return "在线待命，等待任务下发";
});

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
</script>

<template>
  <div v-if="open" class="drawer-overlay" @click="emit('close')" />
  <aside v-if="open && device" class="drawer">
    <div class="drawer-head">
      <div>
        <div class="drawer-title">
          <AppIcon name="dog" />{{ device.deviceId }}
          <span class="tag blue">{{ device.model }}</span>
        </div>
        <div class="muted" style="font-size: 11px; margin-top: 5px">
          {{ device.parkName }} · {{ device.zoneName }}
        </div>
      </div>
      <button class="btn ghost icon" title="关闭" @click="emit('close')">
        <AppIcon name="close" />
      </button>
    </div>

    <div class="drawer-body">
      <div class="video-box">
        <div class="live-tag"><i />LIVE</div>
        <div class="cam">
          <AppIcon name="video" />
          <p>实时画面 · 摄像头 01</p>
        </div>
        <div class="cam-ts mono">{{ timeOnly(device.lastHeartbeat) }} · 1080P / 25fps</div>
      </div>

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
          <span><AppIcon name="clock" />今日运行时长</span><b>{{ formatDuration(device.runtimeMinutes * 60) }}</b>
        </div>
        <div class="metric">
          <span><AppIcon name="pin" />累计里程</span><b>{{ device.odometer.toFixed(1) }}<small> km</small></b>
        </div>
      </div>

      <div class="card-title" style="margin-bottom: 10px"><AppIcon name="list" />设备档案</div>
      <div class="kv"><span>设备编号</span><b class="mono">{{ device.deviceId }}</b></div>
      <div class="kv"><span>设备型号</span><b>{{ device.model }}</b></div>
      <div class="kv"><span>所属园区</span><b>{{ device.parkName }}</b></div>
      <div class="kv"><span>当前区域</span><b>{{ device.zoneName }}</b></div>
      <div class="kv"><span>实时位置</span><b class="mono">{{ device.position }}</b></div>
      <div class="kv"><span>当前任务</span><b>{{ device.currentTaskName }}</b></div>
      <div class="kv"><span>信号强度</span><b>{{ device.signal > 0 ? `良好（${device.signal}/4）` : "无信号" }}</b></div>
      <div class="kv"><span>固件版本</span><b class="mono">{{ device.firmware }}</b></div>
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
    </div>

    <div class="drawer-foot">
      <button class="btn primary" @click="emit('command', '开始巡检')">
        <AppIcon name="play" />开始巡检
      </button>
      <button class="btn" @click="emit('command', '暂停任务')">
        <AppIcon name="pause" />暂停
      </button>
      <button class="btn" @click="emit('command', '召回充电')">
        <AppIcon name="charge" />召回充电
      </button>
      <button class="btn" @click="emit('command', '远程接管')">
        <AppIcon name="video" />远程接管
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
</style>
