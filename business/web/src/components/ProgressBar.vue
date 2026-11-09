<script setup lang="ts">
import { computed } from "vue";
import { batteryTone } from "@/utils/format";

const props = withDefaults(
  defineProps<{ value: number; tone?: string; showValue?: boolean }>(),
  { tone: "", showValue: true }
);
const toneClass = computed(() => props.tone || batteryTone(props.value));
</script>

<template>
  <div class="bar-row">
    <div class="bar" :class="toneClass">
      <i :style="{ width: `${Math.max(0, Math.min(100, value))}%` }" />
    </div>
    <span v-if="showValue" class="num">{{ value }}%</span>
  </div>
</template>
