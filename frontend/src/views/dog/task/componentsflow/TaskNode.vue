<script setup lang="ts">
import { Handle, Position, useVueFlow } from "@vue-flow/core";
import { computed, onMounted, reactive, ref, watch } from "vue";
import SendPlane2Line from "~icons/ri/send-plane-2-line";
import type { FormInstance, FormRules } from "element-plus";
import { dogDeviceGetAll } from "@/api/dogDevice";
import { dogRouteGetAll, dogRouteGetById } from "@/api/dogRoute";
import { SUCCESS } from "@/api/base";
import { useRoute } from "vue-router";

const { updateNodeData } = useVueFlow();
const route = useRoute();

type Option = { label: string; value: number };

const props = defineProps({
  id: { type: String, required: true },
  deviceId: { type: [String, Number], default: null },
  routeId: { type: [String, Number], default: null },
  data: { type: Object, required: true },
  isSubmit: { type: Boolean, default: false },
  selected: { type: Boolean, default: false },
  status: { type: String, default: "default" },
  getNodeDataById: { type: Function, default: null }
});

const emit = defineEmits(["select", "update-route", "show-custom-point"]);

const isActive = ref(false);
const isSuccess = computed(() => props.status === "success");
const isError = computed(() => props.status === "error");
const taskFormRef = ref<FormInstance>();

const deviceListOptions = ref<Option[]>([]);
const routeListOptions = ref<Option[]>([]);
const routePointListOptions = ref<Option[]>([]);
const isCustomeRoute = ref(false);

const rules = reactive<FormRules>({
  deviceId: [{ required: true, message: "设备必须选择", trigger: "change" }],
  routeId: [
    {
      required: true,
      message: "必须选择航线，或选择自定义航线",
      trigger: "change"
    }
  ],
  pathString: [
    { required: true, message: "航线段落必须选择", trigger: "change" }
  ]
});

const taskForm = reactive({
  taskId: props.data?.taskId || props.id,
  routeId: Number(route.query.routeId || props.routeId || 0) || null,
  deviceId: Number(route.query.deviceId || props.deviceId || 0) || null,
  nodeType: "task",
  taskInfo: {
    fromWp: { wpId: null },
    toWp: { wpId: null },
    event: `init_${Math.random().toString(36).slice(2, 10)}`
  },
  pathString: [] as number[],
  routePointListOptions: [] as Option[]
});

/** 按路线的 waypoints 生成航点下拉数据 */
async function loadRoutePoints(routeIdValue: number | null): Promise<Option[]> {
  if (!routeIdValue || Number(routeIdValue) === 0) return [];
  const res = await dogRouteGetById(routeIdValue);
  if (res.code !== SUCCESS || !res.data?.routeData) return [];
  try {
    const parsed = JSON.parse(res.data.routeData);
    if (!Array.isArray(parsed)) return [];
    return parsed.map((wp: any, index: number) => ({
      label: Array.isArray(wp)
        ? `航点 ${index + 1} (${Number(wp[0]).toFixed(2)}, ${Number(wp[1]).toFixed(2)})`
        : `航点 ${index + 1}`,
      value: index + 1
    }));
  } catch {
    return [];
  }
}

// 回显
const originalData = computed(() => props.getNodeDataById?.(props.id));
watch(
  originalData,
  val => {
    if (!val) return;
    Object.assign(taskForm, val);
    isCustomeRoute.value = !taskForm.routeId || Number(taskForm.routeId) === 0;
    emit("show-custom-point", isCustomeRoute.value);
    if (!isCustomeRoute.value) {
      loadRoutePoints(taskForm.routeId).then(options => {
        routePointListOptions.value = options;
        taskForm.routePointListOptions = options;
        emit("update-route", options);
      });
    }
  },
  { immediate: true }
);

function toggleActive() {
  emit("select", props.id);
}

function handleChangeRoute(value: number) {
  if (value && Number(value) !== 0) {
    isCustomeRoute.value = false;
    emit("show-custom-point", false);
    taskForm.pathString = [];
    loadRoutePoints(value).then(options => {
      routePointListOptions.value = options;
      taskForm.routePointListOptions = options;
      emit("update-route", options);
      updateNodeData(props.id, { ...taskForm });
    });
  } else {
    // 自定义航线
    isCustomeRoute.value = true;
    emit("show-custom-point", true);
    routePointListOptions.value = [];
    taskForm.routePointListOptions = [];
    emit("update-route", []);
    updateNodeData(props.id, { ...taskForm });
  }
}

function handleChangePath(value: number[]) {
  if (value && value.length > 0) {
    taskForm.taskInfo.fromWp = { wpId: value[0] };
    taskForm.taskInfo.toWp = { wpId: value[value.length - 1] };
  } else {
    taskForm.taskInfo.fromWp = { wpId: null };
    taskForm.taskInfo.toWp = { wpId: null };
  }
  updateNodeData(props.id, { ...taskForm });
}

function handleChangeBase() {
  updateNodeData(props.id, { ...taskForm });
}

watch(
  () => props.selected,
  val => {
    isActive.value = val;
  },
  { immediate: true }
);

onMounted(async () => {
  // 设备列表
  const deviceRes = await dogDeviceGetAll();
  if (deviceRes.code === SUCCESS && deviceRes.data) {
    deviceListOptions.value = (deviceRes.data as any[]).map(d => ({
      label: d.model ? `${d.deviceId} (${d.model})` : `${d.deviceId}`,
      value: d.id
    }));
  }
  // 航线列表
  const routeRes = await dogRouteGetAll();
  if (routeRes.code === SUCCESS && routeRes.data) {
    routeListOptions.value = (routeRes.data as any[]).map(r => ({
      label: r.routeName,
      value: r.id
    }));
  }
  // 航点
  if (taskForm.routeId && Number(taskForm.routeId) !== 0) {
    routePointListOptions.value = await loadRoutePoints(taskForm.routeId);
    taskForm.routePointListOptions = routePointListOptions.value;
    emit("update-route", routePointListOptions.value);
  }
  updateNodeData(props.id, { ...taskForm });
});
</script>

<template>
  <div @click="toggleActive">
    <el-card
      style="max-width: 1000px"
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
            <SendPlane2Line style="font-size: 25px; color: #303" />
            任务节点
          </span>
        </div>
      </template>
      <el-form
        ref="taskFormRef"
        :model="taskForm"
        label-width="auto"
        style="max-width: 650px"
        :rules="rules"
      >
        <el-form-item label="机器狗设备" prop="deviceId">
          <el-select
            v-model="taskForm.deviceId"
            clearable
            disabled
            placeholder="请选择机器狗设备"
            style="width: 200px"
            @change="handleChangeBase"
          >
            <el-option
              v-for="item in deviceListOptions"
              :key="item.value"
              :value="item.value"
              :label="item.label"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="全局航线" prop="routeId">
          <el-select
            v-model="taskForm.routeId"
            clearable
            placeholder="请选择机器狗航线"
            style="width: 200px"
            @change="handleChangeRoute"
          >
            <el-option label="自定义航线" :value="0" />
            <el-option
              v-for="item in routeListOptions"
              :key="item.value"
              :value="item.value"
              :label="item.label"
            />
          </el-select>
        </el-form-item>
        <el-form-item v-if="!isCustomeRoute" label="航线段落" prop="pathString">
          <el-select
            v-model="taskForm.pathString"
            clearable
            multiple
            placeholder="请选择航线节点"
            style="width: 200px"
            @change="handleChangePath"
          >
            <el-option
              v-for="item in routePointListOptions"
              :key="item.value"
              :value="item.value"
              :label="item.label"
            />
          </el-select>
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

:deep(.el-card.active-border) {
  border-color: #409eff !important;
}

:deep(.el-card.is-success) {
  background-color: #67c23a !important;
}

:deep(.el-card.is-error) {
  background-color: #f56c6c !important;
}
</style>
