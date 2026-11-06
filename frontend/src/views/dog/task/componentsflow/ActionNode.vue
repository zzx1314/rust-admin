<script setup lang="ts">
import { Handle, Position, useVueFlow } from "@vue-flow/core";
import {
  computed,
  onBeforeUnmount,
  onMounted,
  reactive,
  ref,
  watch
} from "vue";
import type { FormInstance, FormRules } from "element-plus";
import BankCardLine from "~icons/ri/bank-card-line";
import More from "~icons/ep/more-filled";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";

const { updateNodeData } = useVueFlow();

const actionForm = reactive({
  nodeType: "action",
  action: {
    type: "",
    timeoutSec: null,
    params: {
      timeSec: null,
      targetAlt: null,
      targetWpStr: null,
      targetWp: null,
      num: null,
      intervalSec: null,
      type: "",
      reason: "",
      event: {
        onComplete: `complete_${Math.random().toString(36).slice(2, 10)}`,
        onFail: `fail_${Math.random().toString(36).slice(2, 10)}`
      }
    }
  },
  xpoint: null,
  ypoint: null,
  zpoint: null
});

const rules = reactive<FormRules>({
  "action.type": [
    { required: true, message: "动作类型必填", trigger: "change" }
  ],
  "action.timeoutSec": [
    { required: true, message: "超时时间必填", trigger: "blur" }
  ],
  // TAKEOFF
  "action.params.targetAlt": [
    {
      validator: (_, value, callback) => {
        if (
          actionForm.action.type === "TAKEOFF" &&
          (value === null || value === "")
        ) {
          callback(new Error("起飞高度必填"));
        } else callback();
      },
      trigger: "blur"
    }
  ],
  // GOTO (非自定义点)
  "action.params.targetWpStr": [
    {
      validator: (_, value, callback) => {
        if (
          actionForm.action.type === "GOTO" &&
          !props.isShowCustomPoint &&
          !value
        ) {
          callback(new Error("请选择航点"));
        } else callback();
      },
      trigger: "blur"
    }
  ],
  xpoint: [
    {
      validator: (_, value, callback) => {
        if (
          actionForm.action.type === "GOTO" &&
          props.isShowCustomPoint &&
          (value === null || value === "")
        ) {
          callback(new Error("X坐标必填"));
        } else callback();
      },
      trigger: "blur"
    }
  ],
  ypoint: [
    {
      validator: (_, value, callback) => {
        if (
          actionForm.action.type === "GOTO" &&
          props.isShowCustomPoint &&
          (value === null || value === "")
        ) {
          callback(new Error("Y坐标必填"));
        } else callback();
      },
      trigger: "blur"
    }
  ],
  zpoint: [
    {
      validator: (_, value, callback) => {
        if (
          actionForm.action.type === "GOTO" &&
          props.isShowCustomPoint &&
          (value === null || value === "")
        ) {
          callback(new Error("Z坐标必填"));
        } else callback();
      },
      trigger: "blur"
    }
  ],
  "action.params.num": [
    {
      validator: (_, value, callback) => {
        if (
          actionForm.action.type === "PHOTO" &&
          (value === null || value === "")
        ) {
          callback(new Error("拍照数量必填"));
        } else callback();
      },
      trigger: "blur"
    }
  ],
  "action.params.intervalSec": [
    {
      validator: (_, value, callback) => {
        if (
          actionForm.action.type === "PHOTO" &&
          (value === null || value === "")
        ) {
          callback(new Error("拍照间隔必填"));
        } else callback();
      },
      trigger: "blur"
    }
  ],
  "action.params.timeSec": [
    {
      validator: (_, value, callback) => {
        if (
          actionForm.action.type === "HOVER" &&
          (value === null || value === "")
        ) {
          callback(new Error("悬停时间必填"));
        } else callback();
      },
      trigger: "blur"
    }
  ]
});

const actionListOptions = [
  { label: "起飞动作", value: "TAKEOFF" },
  { label: "悬停动作", value: "HOVER" },
  { label: "GOTO动作", value: "GOTO" },
  { label: "拍照动作", value: "PHOTO" },
  { label: "启动YOLO", value: "SERVICE_START_YOLO" },
  { label: "启动RTSP", value: "SERVICE_START_RTSP" },
  { label: "停止YOLO", value: "SERVICE_STOP_YOLO" },
  { label: "停止RTSP", value: "SERVICE_STOP_RTSP" },
  { label: "降落动作", value: "LAND" }
];

const props = defineProps({
  id: { type: String, required: true },
  data: { type: Object, required: true },
  selected: { type: Boolean, default: false },
  pathInfo: { type: Array, default: () => [] },
  status: { type: String, default: "default" },
  isShowCustomPoint: { type: Boolean, default: false },
  getNodeDataById: { type: Function, default: null }
});

const emit = defineEmits(["select", "show-error-info"]);

const isActive = ref(false);
const isSuccess = computed(() => props.status === "success");
const isError = computed(() => props.status === "error");
const actionFormRef = ref<FormInstance>();
const pathStringFromTask = ref<Array<any>>([]);

// 回显
const originalData = computed(() => props.getNodeDataById?.(props.id));
watch(
  originalData,
  val => {
    if (val) Object.assign(actionForm, JSON.parse(JSON.stringify(val)));
  },
  { immediate: true }
);

watch(
  () => props.pathInfo,
  val => {
    if (val) pathStringFromTask.value = val;
    else pathStringFromTask.value = [];
  },
  { immediate: true, deep: true }
);

function toggleActive() {
  emit("select", props.id);
}

function handleChange() {
  if (
    actionForm.action.type === "SERVICE_START_YOLO" ||
    actionForm.action.type === "SERVICE_STOP_YOLO"
  ) {
    actionForm.action.params.type = "YOLO";
  } else if (
    actionForm.action.type === "SERVICE_START_DEEPSORT" ||
    actionForm.action.type === "SERVICE_STOP_DEEPSORT"
  ) {
    actionForm.action.params.type = "DEEPSORT";
  } else if (
    actionForm.action.type === "SERVICE_START_RTSP" ||
    actionForm.action.type === "SERVICE_STOP_RTSP"
  ) {
    actionForm.action.params.type = "RTSP";
  }
  updateNodeData(props.id, { ...actionForm });
}

function handleChangeGoto(value: number) {
  actionForm.action.params.targetWp = { wpId: value, alt: 0.5 };
  updateNodeData(props.id, { ...actionForm });
}

function handleChangePoint() {
  actionForm.action.params.targetWp = {
    wpId: 0,
    lat: actionForm.xpoint ? Number(actionForm.xpoint) : 0,
    lon: actionForm.ypoint ? Number(actionForm.ypoint) : 0,
    alt: actionForm.zpoint ? Number(actionForm.zpoint) : 0.5
  };
  updateNodeData(props.id, { ...actionForm });
}

function showErrorInfo() {
  emit("show-error-info");
}

watch(
  () => props.selected,
  val => {
    isActive.value = val;
  },
  { immediate: true }
);

onMounted(() => {
  updateNodeData(props.id, { ...actionForm });
});
</script>
<template>
  <div @click="toggleActive">
    <el-card
      style="max-width: 480px"
      shadow="hover"
      :class="{
        'active-border': isActive,
        'is-success': isSuccess,
        'is-error': isError
      }"
    >
      <template #header>
        <div class="card-header">
          <span class="label-container">
            <span class="left">
              <BankCardLine style="font-size: 25px; color: #00c" />
              动作信息
            </span>
            <el-button
              link
              type="primary"
              :icon="useRenderIcon(More)"
              @click.stop="showErrorInfo"
            />
          </span>
        </div>
      </template>
      <el-form
        ref="actionFormRef"
        :model="actionForm"
        label-width="auto"
        style="max-width: 600px"
        :rules="rules"
        @mousedown.stop
      >
        <el-form-item label="动作类型" prop="action.type">
          <el-select
            v-model="actionForm.action.type"
            placeholder="请选择动作类型"
            style="width: 200px"
            @change="handleChange"
          >
            <el-option
              v-for="item in actionListOptions"
              :key="item.value"
              :label="item.label"
              :value="item.value"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="超时时间" prop="action.timeoutSec">
          <el-input-number
            v-model="actionForm.action.timeoutSec"
            style="width: 200px"
            :min="0"
            :max="999999"
            @change="handleChange"
          />
        </el-form-item>
        <!-- TAKEOFF -->
        <el-form-item
          v-if="actionForm.action.type === 'TAKEOFF'"
          label="起飞高度"
          prop="action.params.targetAlt"
          required
        >
          <el-input-number
            v-model="actionForm.action.params.targetAlt"
            style="width: 200px"
            :min="0"
            :max="999999"
            @change="handleChange"
          />
        </el-form-item>
        <!-- GOTO (非自定义) -->
        <el-form-item
          v-if="actionForm.action.type === 'GOTO' && !isShowCustomPoint"
          label="GOTO动作"
          required
          prop="action.params.targetWpStr"
        >
          <el-select
            v-model="actionForm.action.params.targetWpStr"
            @change="handleChangeGoto"
          >
            <el-option
              v-for="(item, index) in pathStringFromTask"
              :key="index"
              :value="item.value"
              :label="item.label"
            />
          </el-select>
        </el-form-item>
        <!-- GOTO (自定义) -->
        <template v-if="actionForm.action.type === 'GOTO' && isShowCustomPoint">
          <el-form-item prop="xpoint" label="目标点X坐标" required>
            <el-input-number
              v-model="actionForm.xpoint"
              style="width: 200px"
              :min="0"
              :max="999999"
              @change="handleChangePoint"
            />
          </el-form-item>
          <el-form-item prop="ypoint" label="目标点Y坐标" required>
            <el-input-number
              v-model="actionForm.ypoint"
              style="width: 200px"
              :min="0"
              :max="999999"
              @change="handleChangePoint"
            />
          </el-form-item>
          <el-form-item prop="zpoint" label="目标点Z坐标" required>
            <el-input-number
              v-model="actionForm.zpoint"
              style="width: 200px"
              :min="0"
              :max="999999"
              @change="handleChangePoint"
            />
          </el-form-item>
        </template>
        <!-- PHOTO -->
        <template v-if="actionForm.action.type === 'PHOTO'">
          <el-form-item label="拍照数量" prop="action.params.num" required>
            <el-input-number
              v-model="actionForm.action.params.num"
              style="width: 200px"
              :min="0"
              :max="999999"
              @change="handleChange"
            />
          </el-form-item>
          <el-form-item
            label="拍照间隔"
            prop="action.params.intervalSec"
            required
          >
            <el-input-number
              v-model="actionForm.action.params.intervalSec"
              style="width: 200px"
              :min="0"
              :max="999999"
              @change="handleChange"
            />
          </el-form-item>
        </template>
        <!-- HOVER -->
        <el-form-item
          v-if="actionForm.action.type === 'HOVER'"
          label="悬停时间"
          prop="action.params.timeSec"
          required
        >
          <el-input-number
            v-model="actionForm.action.params.timeSec"
            style="width: 200px"
            :min="0"
            :max="999999"
            @change="handleChange"
          />
        </el-form-item>
      </el-form>
    </el-card>
    <Handle
      id="right-output"
      type="source"
      :position="Position.Right"
      style="width: 3px; height: 10px; background-color: #409eff"
    />
    <Handle
      id="left-output"
      type="source"
      :position="Position.Left"
      style="width: 3px; height: 10px; background-color: #409eff"
    />
    <Handle
      id="bottom-output"
      type="source"
      :position="Position.Bottom"
      style="width: 10px; height: 3px; background-color: #409eff"
    />
    <Handle
      id="top-output"
      type="source"
      :position="Position.Top"
      style="width: 10px; height: 3px; background-color: #409eff"
    />
  </div>
</template>
<style scoped lang="scss">
.left {
  display: inline-flex;
  gap: 4px;
  align-items: center;
}
:deep(.el-card__header) {
  height: 40px;
  padding-top: 10px;
}
.label-container {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
}
:deep(.el-card) {
  border: 2px solid transparent !important;
  border-radius: 6px;
  transition: border-color 0.2s ease;
}
:deep(.el-card.active-border) {
  border-color: #409eff !important;
}
:deep(.el-card.is-success) {
  background-color: #67c23a !important;
}
:deep(.el-card.is-error) {
  background-color: #f56c6c !important;
}
:deep(.el-form-item) {
  margin-bottom: 9px;
}
</style>
