<script setup lang="ts">
import { Handle, Position, useVueFlow } from "@vue-flow/core";
import { nextTick, reactive, ref, watch } from "vue";
import BankCardLine from "~icons/ri/bank-card-line";

const { updateNodeData } = useVueFlow();

const props = defineProps({
  id: { type: String, required: true },
  data: { type: Object, default: () => ({}) },
  selected: { type: Boolean, default: false },
  status: { type: String, default: "default" }
});

const emit = defineEmits(["select"]);

const showForm = ref(false);
const actionForm = reactive({
  type: "",
  timeoutSec: null,
  targetAlt: null,
  num: null,
  intervalSec: null,
  reason: "",
  onComplete: "",
  onFail: ""
});

const actionTypes = [
  { label: "TAKEOFF", value: "TAKEOFF" },
  { label: "LAND", value: "LAND" },
  { label: "GOTO", value: "GOTO" },
  { label: "HOVER", value: "HOVER" },
  { label: "SERVICE_START_YOLO", value: "SERVICE_START_YOLO" },
  { label: "SERVICE_STOP_YOLO", value: "SERVICE_STOP_YOLO" },
  { label: "SERVICE_START_DEEPSORT", value: "SERVICE_START_DEEPSORT" },
  { label: "SERVICE_STOP_DEEPSORT", value: "SERVICE_STOP_DEEPSORT" },
  { label: "SERVICE_START_RTSP", value: "SERVICE_START_RTSP" },
  { label: "SERVICE_STOP_RTSP", value: "SERVICE_STOP_RTSP" },
  { label: "WAIT", value: "WAIT" },
  { label: "RETURN", value: "RETURN" }
];

watch(() => props.selected, val => { showForm.value = val; });

function initForm() {
  const a = props.data?.action;
  if (a) {
    actionForm.type = a.type || "";
    actionForm.timeoutSec = a.timeoutSec || null;
    actionForm.targetAlt = a.params?.targetAlt || null;
    actionForm.num = a.params?.num || null;
    actionForm.intervalSec = a.params?.intervalSec || null;
    actionForm.reason = a.params?.reason || "";
    actionForm.onComplete = a.params?.event?.onComplete || "";
    actionForm.onFail = a.params?.event?.onFail || "";
  }
}

function onSelect() {
  emit("select", props.id);
  initForm();
}

function saveAction() {
  const rand = () => Math.random().toString(36).slice(2, 10);
  updateNodeData(props.id, {
    action: {
      type: actionForm.type,
      timeoutSec: actionForm.timeoutSec,
      params: {
        targetAlt: actionForm.targetAlt,
        num: actionForm.num,
        intervalSec: actionForm.intervalSec,
        type: actionForm.type,
        reason: actionForm.reason,
        targetWp: null,
        event: {
          onComplete: actionForm.onComplete || `complete_${rand()}`,
          onFail: actionForm.onFail || `fail_${rand()}`
        }
      }
    }
  });
  showForm.value = false;
}
</script>
<template>
  <div class="action-node" :class="{ selected, 'status-success': status === 'success', 'status-error': status === 'error' }" @click.stop="onSelect">
    <BankCardLine style="font-size: 2em; color: #00c" />
    <div class="label">{{ actionForm.type || "动作节点" }}</div>
    <Handle type="target" :position="Position.Top" style="width: 10px; height: 3px; background: #409eff" />
    <Handle type="source" :position="Position.Bottom" style="width: 10px; height: 3px; background: #409eff" />
    <Handle type="source" :position="Position.Right" style="width: 3px; height: 10px; background: #409eff" />

    <div v-if="showForm" class="node-form" @click.stop>
      <el-form size="small" label-width="80px">
        <el-form-item label="动作类型">
          <el-select v-model="actionForm.type" style="width: 160px">
            <el-option v-for="opt in actionTypes" :key="opt.value" :label="opt.label" :value="opt.value" />
          </el-select>
        </el-form-item>
        <el-form-item label="超时(秒)">
          <el-input-number v-model="actionForm.timeoutSec" :min="0" size="small" style="width: 160px" />
        </el-form-item>
        <el-form-item v-if="actionForm.type === 'TAKEOFF'" label="目标高度">
          <el-input-number v-model="actionForm.targetAlt" :min="0" :step="0.5" size="small" style="width: 160px" />
        </el-form-item>
        <el-form-item v-if="actionForm.type === 'WAIT'" label="等待次数">
          <el-input-number v-model="actionForm.num" :min="1" size="small" style="width: 160px" />
        </el-form-item>
        <el-form-item v-if="actionForm.type === 'WAIT'" label="间隔(秒)">
          <el-input-number v-model="actionForm.intervalSec" :min="0" :step="0.5" size="small" style="width: 160px" />
        </el-form-item>
        <el-form-item label="成功事件">
          <el-input v-model="actionForm.onComplete" size="small" />
        </el-form-item>
        <el-form-item label="失败事件">
          <el-input v-model="actionForm.onFail" size="small" />
        </el-form-item>
        <el-button type="primary" size="small" @click="saveAction">保存</el-button>
      </el-form>
    </div>
  </div>
</template>
<style scoped>
.action-node {
  display: flex; flex-direction: column; align-items: center;
  padding: 8px; border: 1px solid #00c; border-radius: 4px;
  background: #e3f2fd; min-width: 80px; position: relative;
}
.selected { border-color: #1890ff; box-shadow: 0 0 4px #1890ff; }
.status-success { background: #d4edda; }
.status-error { background: #f8d7da; }
.label { font-size: 12px; color: #333; margin-top: 4px; }
.node-form {
  position: absolute; top: 100%; left: 0; z-index: 100;
  background: white; border: 1px solid #ddd; border-radius: 4px;
  padding: 8px; box-shadow: 0 2px 8px rgba(0,0,0,.15);
  min-width: 260px;
}
</style>
