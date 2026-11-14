<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { Device, MapRaster, MapVersion, Route, Task } from "@/api/types";
import { mapApi } from "@/api/ops";
import AppIcon from "./AppIcon.vue";
import VideoPlayer from "./VideoPlayer.vue";
import TaskLiveMap from "./TaskLiveMap.vue";
import { locStatusMeta, robotStageLabel, timeOnly } from "@/utils/format";

/**
 * 任务执行实时监控：左边本体视频、右边地图上的实时位置。
 *
 * 数据都由父级轮询（任务 5s 一次 + 设备心跳 10s 一次）驱动，这里只负责渲染，
 * 不做请求（除了按地图版本拉一次栅格底图，带进程内缓存）。
 */
const props = withDefaults(
  defineProps<{
    task?: Task | null;
    device?: Device | null;
    route?: Route | null;
    /** 任务所用地图版本（路线地图 → 本体载图 → 园区基线） */
    map?: MapVersion | null;
    /** 上次轮询时间，用于体现「实时」 */
    refreshedAt?: string;
  }>(),
  { task: null, device: null, route: null, map: null, refreshedAt: "" }
);

/** 栅格底图缓存：同一地图版本只拉一次，地图切换时换图 */
const rasterCache = new Map<string, MapRaster>();
const raster = ref<MapRaster | null>(null);
let rasterToken = 0;

async function loadRaster(mapId?: string | null) {
  const seq = ++rasterToken;
  raster.value = null;
  if (!mapId || !props.map?.filePath) return;
  const cached = rasterCache.get(mapId);
  if (cached) {
    raster.value = cached;
    return;
  }
  try {
    const data = await mapApi.raster(mapId);
    rasterCache.set(mapId, data);
    if (seq === rasterToken) raster.value = data;
  } catch {
    // 栅格拉不到不影响点位：地图会退回坐标网格
    if (seq === rasterToken) raster.value = null;
  }
}

watch(
  () => [props.map?.id, props.map?.filePath],
  () => void loadRaster(props.map?.id),
  { immediate: true }
);

const online = computed(() => Boolean(props.device) && props.device?.status !== "离线");
const locMeta = computed(() => locStatusMeta(props.device?.locStatus));

/** 视频不能播时可读原因（设备页/任务页提示保持一致） */
const videoHint = computed(() => {
  if (!props.device) return "任务未关联执行设备";
  if (!props.device.videoUrl) return "该设备未配置视频地址（设备档案里填写 RTSP / HLS 地址）";
  return "";
});

const positionText = computed(() => props.device?.position || "未上报");
const stageText = computed(() => robotStageLabel(props.task?.robotStage));

/** 本体定位 / 链路异常：执行中看不见图上点位时最常见的原因 */
const healthNote = computed(() => {
  if (!props.device) return "";
  if (props.device.status === "离线") return "设备离线：视频与位置都已停止更新";
  if (!props.device.bodyOnline) return "本体链路已断：位置与电量是最后一次上报的值";
  if (props.device.locStatus !== 100) {
    return `定位状态「${locMeta.value.label}」：地图上的点位可能不动或跳动`;
  }
  return "";
});
</script>

<template>
  <div class="tlmon">
    <div class="tlmon-media">
      <div class="tlmon-video">
        <VideoPlayer
          v-if="device"
          :url="device.videoUrl"
          :type="device.videoType"
          :online="online"
          :device-id="device.id"
        />
        <div v-else class="tlmon-empty">
          <AppIcon name="video" />
          <p>{{ videoHint }}</p>
        </div>
      </div>

      <div class="tlmon-map">
        <TaskLiveMap
          :device="device"
          :route="route"
          :map="map"
          :raster="raster"
          :progress="task?.progress ?? 0"
        />
      </div>
    </div>

    <div class="tlmon-strip">
      <span class="tlmon-chip" :class="{ warn: !device?.position }">
        <AppIcon name="pin" /><b class="mono">{{ positionText }}</b>
        <small v-if="device?.position">地图米</small>
      </span>
      <span class="tlmon-chip">
        <AppIcon name="battery" />{{ device?.bodyOnline ? `${device?.battery}%` : "—" }}
      </span>
      <span class="tlmon-chip" :class="{ warn: device && device.locStatus !== 100 }">
        <AppIcon name="waypoint" />{{ locMeta.label }}
      </span>
      <span class="tlmon-chip">
        <AppIcon name="activity" />{{ stageText }}
      </span>
      <span class="tlmon-chip">
        <AppIcon name="task" />进度 {{ task?.progress ?? 0 }}%
      </span>
      <span class="tlmon-chip" :title="`地图：${map?.id ?? '未绑定'} · 本体载图：${device?.activeMapId ?? '未上报'}`">
        <AppIcon name="layers" />{{ map?.mapName ?? "未绑定地图" }}
      </span>
      <span class="tlmon-chip muted-chip">
        <AppIcon name="clock" />心跳 {{ timeOnly(device?.lastHeartbeat) }}
        <template v-if="refreshedAt"> · 刷新 {{ refreshedAt }}</template>
      </span>
    </div>

    <p v-if="healthNote" class="tlmon-note">
      <AppIcon name="warn" />{{ healthNote }}
    </p>
    <p v-else-if="videoHint" class="tlmon-note">
      <AppIcon name="info" />{{ videoHint }}
    </p>
  </div>
</template>

<style scoped>
.tlmon {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.tlmon-media {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1.15fr);
  gap: 10px;
  align-items: start;
}

.tlmon-video {
  min-width: 0;
}

.tlmon-map {
  min-width: 0;
}

.tlmon-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  aspect-ratio: 16 / 9;
  border-radius: 10px;
  border: 1px dashed var(--line);
  background: #0b1220;
  color: #94a3b8;
  text-align: center;
  padding: 12px;
}

.tlmon-empty p {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.6;
  max-width: 82%;
}

.tlmon-strip {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.tlmon-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 3px 8px;
  border-radius: 999px;
  border: 1px solid var(--line);
  background: #f8fafc;
  font-size: 11px;
  font-weight: 600;
  color: var(--text);
  white-space: nowrap;
}

.tlmon-chip b {
  font-weight: 700;
}

.tlmon-chip small {
  font-weight: 400;
  color: var(--muted);
  font-size: 10px;
}

.tlmon-chip.warn {
  border-color: #fbbf24;
  background: #fffbeb;
  color: #b45309;
}

.tlmon-chip.muted-chip {
  color: var(--muted);
  font-weight: 400;
}

.tlmon-note {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  margin: 0;
  padding: 6px 9px;
  border-radius: 7px;
  border: 1px solid #fde68a;
  background: #fffbeb;
  color: #92400e;
  font-size: 11px;
  line-height: 1.55;
}

.tlmon-note .ic {
  width: 13px;
  height: 13px;
  flex: none;
  margin-top: 1px;
}

@media (max-width: 900px) {
  .tlmon-media {
    grid-template-columns: 1fr;
  }
}
</style>
