<script setup lang="ts">
import { Handle, Position } from "@vue-flow/core";
</script>
<template>
  <div class="node-container">
    <div class="circle-node" />
    <Handle id="right-output" type="source" :position="Position.Right" style="width: 3px; height: 10px; background-color: #409eff" />
    <Handle id="bottom-output" type="source" :position="Position.Bottom" style="width: 10px; height: 3px; background-color: #409eff" />
  </div>
</template>
<style scoped>
.circle-node {
  width: 40px; height: 40px;
  background-color: transparent;
  border: 2px solid #5370ff;
  border-radius: 50%;
}
</style>
