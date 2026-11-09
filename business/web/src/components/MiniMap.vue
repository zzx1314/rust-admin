<script setup lang="ts">
withDefaults(defineProps<{ color?: string }>(), { color: "#0284c7" });
</script>

<template>
  <svg viewBox="0 0 300 118" preserveAspectRatio="xMidYMid slice">
    <rect width="300" height="118" fill="#eef4f3" />
    <rect
      x="8"
      y="8"
      width="284"
      height="102"
      rx="8"
      fill="none"
      :stroke="color"
      stroke-width="1.4"
      stroke-dasharray="6 5"
      opacity=".55"
    />
    <rect x="20" y="20" width="82" height="44" rx="4" fill="#dbe7e4" stroke="#c3d6d1" />
    <rect
      x="116"
      y="16"
      width="66"
      height="52"
      rx="4"
      :fill="color"
      opacity=".22"
      :stroke="color"
      stroke-width="1"
    />
    <rect x="196" y="24" width="80" height="38" rx="4" fill="#dbe7e4" stroke="#c3d6d1" />
    <rect x="28" y="76" width="72" height="30" rx="4" fill="#dbe7e4" stroke="#c3d6d1" />
    <rect x="120" y="80" width="54" height="26" rx="4" fill="#dbe7e4" stroke="#c3d6d1" />
    <path
      d="M20 70 H280"
      stroke="#fff"
      stroke-width="8"
      stroke-dasharray="4 6"
      opacity=".9"
    />
    <circle cx="150" cy="42" r="4" :fill="color" />
    <circle cx="248" cy="92" r="3.5" :fill="color" opacity=".7" />
  </svg>
</template>
