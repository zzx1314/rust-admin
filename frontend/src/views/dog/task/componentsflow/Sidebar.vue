<script setup>
import useDragAndDrop from "./useDnD.js";
import BankCardLine from "~icons/ri/bank-card-line";
import Settings3Line from "~icons/ri/settings-3-line";
import SendPlane2Line from "~icons/ri/send-plane-2-line";

const { onDragStart } = useDragAndDrop();
</script>
<template>
  <aside>
    <div class="nodes">
      <div class="node-item">
        <div
          class="circle-node-start"
          :draggable="true"
          @dragstart="onDragStart($event, 'start')"
        />
        <p style="font-size: 14px">开始节点</p>
      </div>
      <div class="node-item">
        <div
          class="circle-node-end"
          :draggable="true"
          @dragstart="onDragStart($event, 'end')"
        />
        <p style="font-size: 14px">结束节点</p>
      </div>
      <div class="node-item">
        <div :draggable="true" @dragstart="onDragStart($event, 'task')">
          <SendPlane2Line style="font-size: 3em; color: #303" />
        </div>
        <span class="node-text">任务节点</span>
      </div>
      <div class="node-item">
        <div :draggable="true" @dragstart="onDragStart($event, 'action')">
          <BankCardLine style="font-size: 3em; color: #00c" />
        </div>
        <p style="font-size: 14px">动作节点</p>
      </div>
      <div class="node-item">
        <div :draggable="true" @dragstart="onDragStart($event, 'error')">
          <Settings3Line style="font-size: 3em; color: #096" />
        </div>
        <p style="font-size: 14px">错误处理</p>
      </div>
    </div>
  </aside>
</template>
<style scoped>
.nodes {
  display: flex;
  flex-direction: column;
  align-items: center;
}
.node-item {
  display: flex;
  gap: 8px;
  align-items: center;
  width: 120px;
  margin-bottom: 10px;
  color: black;
}
.circle-node-start {
  width: 40px;
  height: 40px;
  background-color: transparent;
  border: 2px solid #5370ff;
  border-radius: 50%;
}
.circle-node-end {
  width: 40px;
  height: 40px;
  background-color: transparent;
  border: 2px solid #9f0429;
  border-radius: 50%;
}
.node-text {
  width: 60px;
  font-size: 14px;
  text-align: center;
}
</style>
