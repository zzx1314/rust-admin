<script setup lang="ts">
import { Handle, Position } from "@vue-flow/core";
import Settings3Line from "~icons/ri/settings-3-line";

defineProps({
  id: { type: String, required: true },
  data: { type: Object, default: () => ({}) },
  selected: { type: Boolean, default: false },
  status: { type: String, default: "default" }
});
</script>
<template>
  <div class="error-node" :class="{ selected, 'status-success': status === 'success', 'status-error': status === 'error' }">
    <Settings3Line style="font-size: 2em; color: #096" />
    <div class="label">{{ data?.action?.type || "错误处理" }}</div>
    <Handle type="target" :position="Position.Top" style="width: 10px; height: 3px; background: #409eff" />
    <Handle type="source" :position="Position.Bottom" style="width: 10px; height: 3px; background: #409eff" />
    <Handle type="source" :position="Position.Right" style="width: 3px; height: 10px; background: #409eff" />
  </div>
</template>
<style scoped>
.error-node {
  display: flex; flex-direction: column; align-items: center;
  padding: 8px; border: 1px solid #096; border-radius: 4px;
  background: #e8f5e9; min-width: 80px;
}
.selected { border-color: #1890ff; box-shadow: 0 0 4px #1890ff; }
.status-success { background: #d4edda; }
.status-error { background: #f8d7da; }
.label { font-size: 12px; color: #333; margin-top: 4px; }
</style>
