<script setup lang="ts">
/**
 * 机器狗前置摄像头画面（与「实时视频」页的前置通道同源同路）。
 *
 * 设备管理 → 控制抽屉、计划管理 → 实时监控都渲染它，因此三处看到的永远是
 * 同一块画面：板端 `dogcam`（本体前置 RTSP 归一化）→ 浏览器直连 mediamtx（WHEP）
 * 或平台中继（板端推到平台 mediamtx，同样是 WHEP）。判定规则与 `/monitor` 一致，
 * 见 `useFrontCamera`。
 *
 * 部署没接背包时不做任何兜底（设备档案里的 `video_url` 已不再播放），
 * 直接给一张「怎么配背包」的说明卡 —— 不显示一块永远黑屏的播放器。
 */
import { computed } from "vue";
import AppIcon from "./AppIcon.vue";
import VideoPlayer from "./VideoPlayer.vue";
import VideoStage from "./backpack/VideoStage.vue";
import { FRONT_STREAM, useFrontCamera } from "@/composables/useFrontCamera";

withDefaults(
  defineProps<{
    /** 直连模式下画面左上角的标题 */
    title?: string;
  }>(),
  { title: "机器狗前置" }
);

const emit = defineEmits<{ (event: "meta", size: { w: number; h: number }): void }>();

const { path, whepBase, hint, detail } = useFrontCamera();

const emptyText = computed(() => {
  if (hint.value) return hint.value;
  return "本部署未接入数据采集背包，看不到机器狗前置摄像头（与「实时视频」页同一路）。请在业务服务配置 ROBODOG_BACKPACK_URL / ROBODOG_BACKPACK_WEBRTC_URL 后重试。";
});
</script>

<template>
  <div class="front-cam">
    <!-- 直连板端：与「实时视频」的现场直连完全同一条通路 -->
    <VideoStage
      v-if="path === 'direct'"
      :whep-base="whepBase"
      :stream="FRONT_STREAM"
      :title="title"
      :ai-active="false"
      :ai-badge="''"
      :show-boxes="false"
      :boxes="[]"
      :frame-w="1280"
      :frame-h="720"
      :hint="hint"
      :detail="detail"
      @meta="size => emit('meta', size)"
    />

    <!-- 跨网段 / 公网入口：板端 relay 推到平台，浏览器同源播放 -->
    <VideoPlayer v-else-if="path === 'relay'" :stream="FRONT_STREAM" @meta="size => emit('meta', size)" />

    <!-- 未接背包：给配置说明，不放一块永远黑屏的播放器 -->
    <div v-else class="front-cam-empty">
      <AppIcon name="video" :size="26" />
      <p>{{ emptyText }}</p>
    </div>
  </div>
</template>

<style scoped>
.front-cam {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.front-cam-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  aspect-ratio: 16 / 9;
  padding: 16px;
  border-radius: 10px;
  border: 1px solid var(--line, #1e293b);
  background: #0b1220;
  color: #94a3b8;
  text-align: center;
}

.front-cam-empty p {
  margin: 0;
  max-width: 80%;
  font-size: 12px;
  line-height: 1.7;
}
</style>
