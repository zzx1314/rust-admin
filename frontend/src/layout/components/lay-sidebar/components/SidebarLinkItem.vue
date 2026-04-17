<script setup lang="ts">
import { computed } from "vue";
import { isUrl } from "@pureadmin/utils";

const props = defineProps<{
  to: string | { path: string };
}>();

const isExternalLink = computed(() =>
  typeof props.to === "string" ? isUrl(props.to) : false
);

const getLinkProps = computed(() => {
  if (isExternalLink.value) {
    return {
      href: props.to,
      target: "_blank",
      rel: "noopener"
    };
  }
  return {
    to: typeof props.to === "string" ? { path: props.to } : props.to
  };
});
</script>

<template>
  <component :is="isExternalLink ? 'a' : 'router-link'" v-bind="getLinkProps">
    <slot />
  </component>
</template>
