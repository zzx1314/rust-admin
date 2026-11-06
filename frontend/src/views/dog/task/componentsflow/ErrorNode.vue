<script setup lang="ts">
import { Handle, Position, useVueFlow } from "@vue-flow/core";
import {
  computed,
  inject,
  onBeforeUnmount,
  onMounted,
  reactive,
  ref,
  watch
} from "vue";
import type { FormInstance, FormRules } from "element-plus";
import SettingsIcon from "~icons/ri/settings-3-line";

const { updateNodeData } = useVueFlow();

const errorActionForm = ref({
  nodeType: "action",
  action: {
    type: "",
    params: {
      reason: "",
      event: {
        onComplete: ""
      }
    }
  }
});

const props = defineProps({
  id: {
    type: String,
    required: true
  },
  data: {
    type: Object,
    required: true
  },
  isSubmit: {
    type: Boolean,
    default: false
  },
  selected: {
    type: Boolean,
    default: false
  },
  status: {
    type: String,
    default: "default"
  },
  getNodeDataById: { type: Function, default: null }
});

const emit = defineEmits(["validate-fail", "select"]);

const isActive = ref(false);
const isSuccess = computed(() => props.status === "success");
const isError = computed(() => props.status === "error");
const errorActionFormRef = ref<FormInstance>();
const validateOnRuleChange = ref(true);

// 必填规则（与 mgmt_platform 保持一致）
const rules = reactive<FormRules>({
  configId: [{ required: true, message: "配置必填", trigger: "change" }]
});

// 回显
const originalData = computed(() => props.getNodeDataById?.(props.id));
watch(
  originalData,
  val => {
    if (!val) return;
    errorActionForm.value = {
      nodeType: val.nodeType || "action",
      action: {
        type: val.action?.type || "",
        params: {
          reason: val.action?.params?.reason || "",
          event: {
            onComplete: val.action?.params?.event?.onComplete || ""
          }
        }
      }
    };
  },
  { immediate: true }
);

function toggleActive() {
  emit("select", props.id);
}

function handleChange() {
  updateNodeData(props.id, errorActionForm.value);
}

function submitForm(formEl: FormInstance | undefined): Promise<boolean> {
  if (!formEl) return Promise.resolve(false);
  return new Promise(resolve => {
    formEl.validate((valid, fields) => {
      if (valid) {
        resolve(true);
      } else {
        emit("validate-fail", fields);
        resolve(false);
      }
    });
  });
}

async function validate() {
  return await submitForm(errorActionFormRef.value);
}

defineExpose({ validate });

// 注册到父组件
const registerNodeRef = inject("registerNodeRef", null) as
  | ((id: string, nodeRef: { validate: () => Promise<boolean> }) => void)
  | null;
const unregisterNodeRef = inject("unregisterNodeRef", null) as
  | ((id: string) => void)
  | null;

watch(
  () => props.selected,
  val => {
    isActive.value = val;
  },
  { immediate: true }
);

onMounted(() => {
  registerNodeRef?.(props.id, { validate });
});

onBeforeUnmount(() => {
  unregisterNodeRef?.(props.id);
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
            <SettingsIcon style="font-size: 25px; color: #096" />
            错误处理
          </span>
        </div>
      </template>
      <el-form
        ref="errorActionFormRef"
        :model="errorActionForm"
        label-width="auto"
        style="max-width: 600px"
        :rules="rules"
        :validate-on-rule-change="validateOnRuleChange"
      >
        <el-form-item label="错误类型" prop="type">
          <el-input
            v-model="errorActionForm.action.type"
            placeholder="请输入错误类型"
            @blur="handleChange"
          />
        </el-form-item>
        <el-form-item label="错误原因" prop="type">
          <el-input
            v-model="errorActionForm.action.params.reason"
            placeholder="请输入错误原因"
            @blur="handleChange"
          />
        </el-form-item>
        <el-form-item label="成功事件" prop="onComplete">
          <el-input
            v-model="errorActionForm.action.params.event.onComplete"
            placeholder="请输入成功事件"
            @blur="handleChange"
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
:deep(.el-card__header) {
  height: 40px;
  padding-top: 10px;
}

:deep(.el-form-item) {
  margin-bottom: 9px;
}

.label-container {
  display: inline-flex;
  gap: 4px;
  align-items: center;
}

:deep(.el-card) {
  border: 2px solid transparent !important;
  border-radius: 6px;
  transition: border-color 0.2s ease;
}

:deep(.el-card.is-success) {
  background-color: #67c23a !important;
}

:deep(.el-card.is-error) {
  background-color: #f56c6c !important;
}

:deep(.el-card.active-border) {
  border-color: #409eff !important;
}
</style>
