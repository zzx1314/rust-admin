<script setup lang="ts">
import { computed } from "vue";

/**
 * 状态徽标
 *
 * - 文案始终可见，不依赖颜色单独传达含义（颜色仅作辅助）；
 * - `live` 仅用于「会变化且用户正在等待变化」的单个状态，避免满屏 aria-live；
 * - 支持中英文业务状态，未知状态回退为中性徽标而不是隐藏。
 */
type Tone = "online" | "offline" | "busy" | "fault" | "pending" | "neutral";

interface StatusMeta {
  label: string;
  tone: Tone;
}

const STATUS_MAP: Record<string, StatusMeta> = {
  online: { label: "在线", tone: "online" },
  offline: { label: "离线", tone: "offline" },
  charging: { label: "充电中", tone: "busy" },
  busy: { label: "执行中", tone: "busy" },
  running: { label: "进行中", tone: "busy" },
  fault: { label: "故障", tone: "fault" },
  error: { label: "失败", tone: "fault" },
  failed: { label: "失败", tone: "fault" },
  success: { label: "成功", tone: "online" },
  completed: { label: "已完成", tone: "online" },
  done: { label: "已完成", tone: "online" },
  pending: { label: "待下发", tone: "pending" },
  sent: { label: "已下发", tone: "pending" },
  queued: { label: "排队中", tone: "pending" },
  在线: { label: "在线", tone: "online" },
  离线: { label: "离线", tone: "offline" },
  成功: { label: "成功", tone: "online" },
  已完成: { label: "已完成", tone: "online" },
  进行中: { label: "进行中", tone: "busy" },
  执行中: { label: "执行中", tone: "busy" },
  失败: { label: "失败", tone: "fault" },
  异常: { label: "异常", tone: "fault" },
  待执行: { label: "待执行", tone: "pending" },
  待下发: { label: "待下发", tone: "pending" },
  已下发: { label: "已下发", tone: "pending" },
  已取消: { label: "已取消", tone: "offline" },
  已停止: { label: "已停止", tone: "offline" },
  未绑定: { label: "未绑定", tone: "neutral" }
};

const props = withDefaults(
  defineProps<{
    /** 业务状态原始值（英文枚举或中文文案） */
    status?: string | null;
    /** 覆盖展示文案 */
    label?: string;
    /** 自定义色调，用于业务特有状态 */
    tone?: Tone;
    size?: "small" | "default";
    /** 该状态会异步变化时打开（同一屏最多一处） */
    live?: boolean;
  }>(),
  { status: "", label: "", size: "default", live: false, tone: undefined }
);

const meta = computed<StatusMeta>(() => {
  const key = (props.status ?? "").trim();
  const mapped = STATUS_MAP[key] || STATUS_MAP[key.toLowerCase()];
  if (mapped) return props.tone ? { ...mapped, tone: props.tone } : mapped;
  return { label: key || "未知", tone: props.tone ?? "neutral" };
});

const text = computed(() => props.label || meta.value.label);
const isLivePulse = computed(
  () =>
    props.live && (meta.value.tone === "online" || meta.value.tone === "busy")
);
</script>

<template>
  <span
    class="dog-badge"
    :class="[
      `dog-badge--${meta.tone}`,
      { 'dog-badge--small': size === 'small' }
    ]"
    :role="live ? 'status' : undefined"
    :aria-live="live ? 'polite' : undefined"
    :aria-atomic="live ? 'true' : undefined"
  >
    <span
      class="dog-badge__dot"
      :class="{ 'dog-badge__dot--live': isLivePulse }"
      aria-hidden="true"
    />
    <span class="dog-badge__text">{{ text }}</span>
  </span>
</template>

<style scoped lang="scss">
@keyframes dog-badge-pulse {
  0%,
  100% {
    opacity: 1;
    transform: scale(1);
  }

  50% {
    opacity: 0.45;
    transform: scale(0.82);
  }
}

.dog-badge {
  display: inline-flex;
  gap: 6px;
  align-items: center;
  max-width: 100%;
  padding: 2px 10px;
  font-size: 13px;
  font-weight: 500;
  line-height: 20px;
  color: var(--dog-badge-fg);
  white-space: nowrap;
  background: var(--dog-badge-bg);
  border: 1px solid var(--dog-badge-border);
  border-radius: var(--dog-radius-pill);
}

.dog-badge--small {
  gap: 4px;
  padding: 0 8px;
  font-size: 12px;
  line-height: 18px;
}

.dog-badge__dot {
  flex: none;
  width: 6px;
  height: 6px;
  background: currentcolor;
  border-radius: 50%;
}

/* 仅"实时"徽标呼吸，且 reduced-motion 下自动静止 */
.dog-badge__dot--live {
  animation: dog-badge-pulse 2s var(--dog-ease) infinite;
}

.dog-badge__text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dog-badge--online {
  --dog-badge-fg: var(--dog-status-online);
  --dog-badge-bg: var(--dog-status-online-bg);
  --dog-badge-border: transparent;
}

.dog-badge--offline {
  --dog-badge-fg: var(--dog-status-offline);
  --dog-badge-bg: var(--dog-status-offline-bg);
  --dog-badge-border: transparent;
}

.dog-badge--busy {
  --dog-badge-fg: var(--dog-status-busy);
  --dog-badge-bg: var(--dog-status-busy-bg);
  --dog-badge-border: transparent;
}

.dog-badge--fault {
  --dog-badge-fg: var(--dog-status-fault);
  --dog-badge-bg: var(--dog-status-fault-bg);
  --dog-badge-border: transparent;
}

.dog-badge--pending {
  --dog-badge-fg: var(--dog-status-pending);
  --dog-badge-bg: var(--dog-status-pending-bg);
  --dog-badge-border: transparent;
}

.dog-badge--neutral {
  --dog-badge-fg: var(--dog-status-neutral);
  --dog-badge-bg: var(--dog-status-neutral-bg);
  --dog-badge-border: transparent;
}
</style>
