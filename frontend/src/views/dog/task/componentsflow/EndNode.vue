<script setup lang="ts">
import { Handle, Position } from "@vue-flow/core";
</script>
<template>
  <div class="node-container">
    <div class="circle-node" />
    <Handle id="left-output" type="target" :position="Position.Left" style="width: 3px; height: 10px; background-color: #409eff" />
    <Handle id="top-output" type="target" :position="Position.Top" style="width: 10px; height: 3px; background-color: #409eff" />
  </div>
</template>
<style scoped>
.circle-node {
  width: 40px; height: 40px;
  background-color: transparent;
  border: 2px solid #9f0429;
  border-radius: 50%;
}
</style>
