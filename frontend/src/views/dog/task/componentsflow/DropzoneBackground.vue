<script lang="ts" setup>
import { Background } from "@vue-flow/background";
</script>
<template>
  <div class="dropzone-background">
    <Background :size="2" :gap="20" pattern-color="#BDBDBD" />
    <div class="overlay">
      <slot />
    </div>
  </div>
</template>
<style scoped>
.dropzone-background {
  position: relative;
  width: 100%;
  height: 100%;
}

.dropzone-background .overlay {
  position: absolute;
  top: 0;
  left: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  pointer-events: none;
}
</style>
