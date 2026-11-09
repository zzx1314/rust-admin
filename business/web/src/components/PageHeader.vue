<script setup lang="ts">
defineProps<{ title: string; subtitle?: string }>();
</script>

<template>
  <div class="page-head">
    <div>
      <div class="page-title">{{ title }}</div>
      <div v-if="subtitle" class="page-sub">{{ subtitle }}</div>
    </div>
    <div class="head-actions">
      <slot name="actions" />
    </div>
  </div>
</template>
