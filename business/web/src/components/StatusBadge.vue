<script setup lang="ts">
import { computed } from "vue";
import { statusTone } from "@/utils/format";

const props = defineProps<{ status?: string | null }>();
const tone = computed(() => statusTone(props.status));
</script>

<template>
  <span class="badge" :class="tone"><i />{{ status || "—" }}</span>
</template>
