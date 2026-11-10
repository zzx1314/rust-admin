<script setup lang="ts">
import { computed } from "vue";
import AppIcon from "@/components/AppIcon.vue";

const props = withDefaults(
  defineProps<{
    mapId?: string | null;
    status?: string | null;
    fallback?: string | null;
  }>(),
  { mapId: null, status: null, fallback: null }
);

const tone = computed(() => {
  if (!props.mapId) return "none";
  const status = props.status ?? props.fallback ?? "";
  if (status === "已发布" || status === "已标定") return "ok";
  if (status === "部分标定") return "warn";
  return "gray";
});

const label = computed(() => {
  if (!props.mapId) {
    return `${props.fallback || "未绑定"} · 仅台账`;
  }
  return `${props.mapId} · ${props.status ?? props.fallback ?? "已归档"}`;
});
</script>

<template>
  <span class="map-chip-baseline" :class="tone">
    <AppIcon name="layers" />{{ label }}
  </span>
</template>
