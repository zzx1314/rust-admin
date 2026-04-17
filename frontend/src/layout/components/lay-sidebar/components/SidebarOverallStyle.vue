<script setup lang="ts">
import { useGlobal } from "@pureadmin/utils";
import { computed, watch, shallowRef } from "vue";
import { useDataThemeChange } from "@/layout/hooks/useDataThemeChange";

import DayIcon from "~icons/ri/sun-fill";
import DarkIcon from "~icons/ri/moon-fill";

const styleIcon = shallowRef();
const { $storage } = useGlobal<GlobalPropertiesApi>();
const { dataTheme, dataThemeChange } = useDataThemeChange();
const overallStyle = computed(() => $storage?.layout?.overallStyle);

function onToggle() {
  if (overallStyle.value === "light") {
    dataTheme.value = true;
    dataThemeChange("dark");
  } else {
    dataTheme.value = false;
    dataThemeChange("light");
  }
}

watch(
  overallStyle,
  style => {
    styleIcon.value = style === "light" ? DarkIcon : DayIcon;
  },
  {
    immediate: true
  }
);
</script>

<template>
  <span class="overall-style-icon navbar-bg-hover" @click="onToggle">
    <IconifyIconOffline :icon="styleIcon" />
  </span>
</template>
