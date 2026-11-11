<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref } from "vue";
import AppIcon from "./AppIcon.vue";

/**
 * 四向摇杆 + 转向
 *
 * 交互要点：
 * - 按住拖拽，按 10Hz 下发当前轴值（业务侧 `POST /devices/{id}/teleop`）
 * - **松手立即发一帧全 0**，不依赖服务端看门狗，避免设备继续走
 * - `disabled` 时不可拖拽（未接管），但仍可看到当前档位
 */
const props = defineProps<{
  disabled: boolean;
  /** 速度档 0.1 ~ 1.0 */
  speed: number;
}>();

const emit = defineEmits<{
  control: [axes: { forward: number; strafe: number; turn: number; speed: number }];
}>();

const SURFACE = 132; // 摇杆底盘尺寸（px）
const KNOB = 44; // 摇杆头尺寸（px）
const MAX_OFFSET = (SURFACE - KNOB) / 2;

const padRef = ref<HTMLDivElement | null>(null);
const knob = reactive({ x: 0, y: 0 });
const dragging = ref(false);
let timer: number | null = null;
let turnTimer: number | null = null;

/** 归一化后的轴值，y 向上为正 */
const axes = computed(() => ({
  forward: -knob.y,
  strafe: knob.x
}));

const engaged = computed(
  () => Math.abs(knob.x) > 0.01 || Math.abs(knob.y) > 0.01
);

function emitAxes(turn = 0) {
  emit("control", {
    forward: Number(axes.value.forward.toFixed(3)),
    strafe: Number(axes.value.strafe.toFixed(3)),
    turn,
    speed: props.speed
  });
}

function startRepeat(turn: number) {
  stopRepeat();
  emitAxes(turn);
  // 转向按住时持续下发，和推杆保持同一节奏
  turnTimer = window.setInterval(() => emitAxes(turn), 100);
}

function stopRepeat() {
  if (turnTimer !== null) {
    window.clearInterval(turnTimer);
    turnTimer = null;
  }
}

function startDrag(event: PointerEvent) {
  if (props.disabled) return;
  dragging.value = true;
  (event.target as HTMLElement).setPointerCapture?.(event.pointerId);
  moveDrag(event);
  // 连续下发：设备端有 1s 看门狗，这里 100ms 一帧留足余量
  timer = window.setInterval(() => {
    if (engaged.value) emitAxes();
  }, 100);
}

function moveDrag(event: PointerEvent) {
  if (!dragging.value || !padRef.value) return;
  const rect = padRef.value.getBoundingClientRect();
  const centerX = rect.left + rect.width / 2;
  const centerY = rect.top + rect.height / 2;
  let dx = event.clientX - centerX;
  let dy = event.clientY - centerY;
  const len = Math.hypot(dx, dy);
  if (len > MAX_OFFSET) {
    dx = (dx / len) * MAX_OFFSET;
    dy = (dy / len) * MAX_OFFSET;
  }
  knob.x = dx / MAX_OFFSET;
  knob.y = dy / MAX_OFFSET;
  emitAxes();
}

function endDrag() {
  if (!dragging.value) return;
  dragging.value = false;
  knob.x = 0;
  knob.y = 0;
  if (timer !== null) {
    window.clearInterval(timer);
    timer = null;
  }
  // 松手立刻停，不等服务端超时
  emit("control", { forward: 0, strafe: 0, turn: 0, speed: props.speed });
}

function turn(direction: -1 | 1) {
  if (props.disabled) return;
  startRepeat(direction);
}

onBeforeUnmount(() => {
  if (timer !== null) window.clearInterval(timer);
  stopRepeat();
});
</script>

<template>
  <div class="joystick-wrap">
    <div
      ref="padRef"
      class="joystick"
      :class="{ disabled, active: dragging }"
      :style="{ width: `${SURFACE}px`, height: `${SURFACE}px` }"
      @pointerdown="startDrag"
      @pointermove="moveDrag"
      @pointerup="endDrag"
      @pointercancel="endDrag"
      @pointerleave="dragging && endDrag()"
    >
      <span class="ring" />
      <span class="cross-h" />
      <span class="cross-v" />
      <span
        class="knob"
        :style="{
          width: `${KNOB}px`,
          height: `${KNOB}px`,
          transform: `translate(${knob.x * MAX_OFFSET}px, ${knob.y * MAX_OFFSET}px)`
        }"
      >
        <AppIcon name="dog" />
      </span>
    </div>

    <div class="turn-bar">
      <button
        class="turn-btn"
        :disabled="disabled"
        title="左转（按住）"
        @pointerdown="turn(-1)"
        @pointerup="stopRepeat"
        @pointerleave="stopRepeat"
      >
        <AppIcon name="undo" />
      </button>
      <span class="turn-label">转向</span>
      <button
        class="turn-btn"
        :disabled="disabled"
        title="右转（按住）"
        @pointerdown="turn(1)"
        @pointerup="stopRepeat"
        @pointerleave="stopRepeat"
      >
        <AppIcon name="flag" />
      </button>
    </div>

    <p v-if="disabled" class="joystick-hint">未接管，摇杆已锁定</p>
  </div>
</template>

<style scoped>
.joystick-wrap {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}

.joystick {
  position: relative;
  border-radius: 50%;
  background: radial-gradient(circle at 50% 45%, #1e293b 0%, #0f172a 70%);
  border: 1px solid var(--line);
  display: grid;
  place-items: center;
  touch-action: none;
  cursor: grab;
  user-select: none;
}

.joystick.active {
  cursor: grabbing;
}

.joystick.disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.ring {
  position: absolute;
  inset: 14px;
  border-radius: 50%;
  border: 1px dashed rgb(148 163 184 / 35%);
}

.cross-h,
.cross-v {
  position: absolute;
  background: rgb(148 163 184 / 22%);
}

.cross-h {
  width: 60%;
  height: 1px;
}

.cross-v {
  height: 60%;
  width: 1px;
}

.knob {
  position: relative;
  border-radius: 50%;
  display: grid;
  place-items: center;
  color: #38bdf8;
  background: linear-gradient(160deg, #1e3a5f, #0c1a2b);
  border: 1px solid #2563eb;
  box-shadow: 0 4px 14px rgb(2 132 199 / 35%);
  transition: transform 0.06s linear;
}

.turn-bar {
  display: flex;
  align-items: center;
  gap: 10px;
}

.turn-btn {
  width: 38px;
  height: 30px;
  display: grid;
  place-items: center;
  border-radius: 7px;
  border: 1px solid var(--line);
  background: var(--card);
  color: var(--text);
  cursor: pointer;
}

.turn-btn:hover:not(:disabled) {
  border-color: #2563eb;
  color: #2563eb;
}

.turn-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.turn-btn:last-of-type :deep(svg) {
  transform: scaleX(-1);
}

.turn-label {
  font-size: 11px;
  color: var(--muted);
}

.joystick-hint {
  font-size: 11px;
  color: var(--muted);
  margin: 0;
}
</style>
