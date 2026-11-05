<script setup lang="ts">
import { Handle, Position, useVueFlow } from "@vue-flow/core";
import { nextTick, reactive, ref, watch } from "vue";
import SendPlane2Line from "~icons/ri/send-plane-2-line";
import { dogDeviceGetAll } from "@/api/dogDevice";
import { dogRouteGetAll } from "@/api/dogRoute";

const { updateNodeData } = useVueFlow();

const props = defineProps({
  id: { type: String, required: true },
  deviceId: { type: [String, Number], default: null },
  routeId: { type: [String, Number], default: null },
  data: { type: Object, default: () => ({}) },
  selected: { type: Boolean, default: false },
  status: { type: String, default: "default" }
});

const emit = defineEmits(["select", "update:data"]);

const showForm = ref(false);
const taskForm = reactive({
  deviceId: null,
  routeId: null,
  fromWpId: null,
  toWpId: null,
  event: ""
});
const deviceOptions = ref([]);
const routeOptions = ref([]);

watch(() => props.selected, val => { showForm.value = val; });

function initForm() {
  if (props.data?.taskInfo) {
    taskForm.event = props.data.taskInfo.event || "";
  }
  if (props.data?.deviceId) taskForm.deviceId = props.data.deviceId;
  if (props.data?.routeId) taskForm.routeId = props.data.routeId;
}

function onSelect() {
  emit("select", props.id);
  initForm();
  loadOptions();
}

async function loadOptions() {
  const devRes = await dogDeviceGetAll();
  deviceOptions.value = (devRes.data || []).map(d => ({ label: `${d.deviceId} (${d.model || ""})`, value: d.id }));
  const routeRes = await dogRouteGetAll();
  routeOptions.value = (routeRes.data || []).map(r => ({ label: r.routeName, value: r.id }));
}

function saveTask() {
  updateNodeData(props.id, {
    deviceId: taskForm.deviceId,
    routeId: taskForm.routeId,
    taskInfo: {
      fromWp: { wpId: taskForm.fromWpId },
      toWp: { wpId: taskForm.toWpId },
      event: taskForm.event || `event_${Date.now()}`
    },
    label: "任务节点"
  });
  showForm.value = false;
}
</script>
<template>
  <div class="task-node" :class="{ selected, 'status-success': status === 'success', 'status-error': status === 'error' }" @click.stop="onSelect">
    <SendPlane2Line style="font-size: 2em; color: #303" />
    <div class="label">任务节点</div>
    <Handle type="target" :position="Position.Top" style="width: 10px; height: 3px; background: #409eff" />
    <Handle type="source" :position="Position.Bottom" style="width: 10px; height: 3px; background: #409eff" />
    <Handle type="source" :position="Position.Right" style="width: 3px; height: 10px; background: #409eff" />

    <div v-if="showForm" class="node-form" @click.stop>
      <el-form size="small" label-width="60px">
        <el-form-item label="设备">
          <el-select v-model="taskForm.deviceId" style="width: 140px">
            <el-option v-for="opt in deviceOptions" :key="opt.value" :label="opt.label" :value="opt.value" />
          </el-select>
        </el-form-item>
        <el-form-item label="航线">
          <el-select v-model="taskForm.routeId" style="width: 140px">
            <el-option v-for="opt in routeOptions" :key="opt.value" :label="opt.label" :value="opt.value" />
          </el-select>
        </el-form-item>
        <el-form-item label="事件">
          <el-input v-model="taskForm.event" size="small" />
        </el-form-item>
        <el-button type="primary" size="small" @click="saveTask">保存</el-button>
      </el-form>
    </div>
  </div>
</template>
<style scoped>
.task-node {
  display: flex; flex-direction: column; align-items: center;
  padding: 8px; border: 1px solid #303; border-radius: 4px;
  background: #f3e5f5; min-width: 80px; position: relative;
}
.selected { border-color: #1890ff; box-shadow: 0 0 4px #1890ff; }
.status-success { background: #d4edda; }
.status-error { background: #f8d7da; }
.label { font-size: 12px; color: #333; margin-top: 4px; }
.node-form {
  position: absolute; top: 100%; left: 0; z-index: 100;
  background: white; border: 1px solid #ddd; border-radius: 4px;
  padding: 8px; box-shadow: 0 2px 8px rgba(0,0,0,.15);
  min-width: 200px;
}
</style>
