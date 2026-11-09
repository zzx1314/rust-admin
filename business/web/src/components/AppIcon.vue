<script setup lang="ts">
import { computed } from "vue";

const props = withDefaults(
  defineProps<{ name: string; size?: number | string }>(),
  { size: 16 }
);

const dimension = computed(() =>
  typeof props.size === "number" ? `${props.size}px` : props.size
);
</script>

<template>
  <svg class="ic" :style="{ width: dimension, height: dimension }" aria-hidden="true">
    <use :href="`#i-${name}`" />
  </svg>
</template>
