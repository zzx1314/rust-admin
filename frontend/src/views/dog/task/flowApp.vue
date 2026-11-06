<script setup lang="ts">
import {
  ref,
  reactive,
  onMounted,
  nextTick,
  onBeforeUnmount,
  provide
} from "vue";
import { VueFlow, useVueFlow, MarkerType } from "@vue-flow/core";
import { useRouter } from "vue-router";
import "vue-json-pretty/lib/styles.css";
import VueJsonPretty from "vue-json-pretty";
import { useMultiTagsStoreHook } from "@/store/modules/multiTags";
import { message } from "@/utils/message";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import Back from "~icons/ep/back";
import Position from "~icons/ep/position";
import Search from "~icons/ep/search";
import DropzoneBackground from "./componentsflow/DropzoneBackground.vue";
import Sidebar from "./componentsflow/Sidebar.vue";
import StartNode from "./componentsflow/StartNode.vue";
import EndNode from "./componentsflow/EndNode.vue";
import ActionNode from "./componentsflow/ActionNode.vue";
import ErrorNode from "./componentsflow/ErrorNode.vue";
import TaskNode from "./componentsflow/TaskNode.vue";
import useDragAndDrop from "./componentsflow/useDnD.js";

import {
  dogWorkflowGetByUuid,
  dogWorkflowGetByTask,
  dogWorkflowCreate,
  dogWorkflowUpdate
} from "@/api/dogWorkflow";
import { dogTaskGetTaskStatus } from "@/api/dogTask";
import { SUCCESS } from "@/api/base";
import { useProcess, FLOW_DESIGN_PATH } from "./flowHook";

const router = useRouter();
// 与 mgmt_platform 一致：刷新后地址栏参数还在，用它把标签页重新 push 回来
const { initToDetail, getParameter } = useProcess();
initToDetail();

const {
  onConnect,
  addEdges,
  removeNodes,
  getSelectedNodes,
  removeEdges,
  getSelectedEdges,
  getEdges,
  getNodes,
  setNodes,
  setEdges
} = useVueFlow();
const { onDragOver, onDrop, onDragLeave, isDragOver } = useDragAndDrop();

const taskId = getParameter.taskId as string;
const taskType = getParameter.taskType as string;
const workflowId = getParameter.workflowId as string;
const deviceId = getParameter.deviceId as string;
const routeId = getParameter.routeId as string;
const name = getParameter.name as string;

const isShowSave = taskType === "taskDesign";
const isSuccessNodeId = ref<string[]>([]);
const isErrorNodeId = ref<string[]>([]);
const errorInfoMap = ref<Record<string, string>>({});
const selectedNodeId = ref("");
const isSubmit = ref(false);
const rawNodes = ref<any[]>([]);
const pathInfo = ref<any[]>([]);
const isShowCustomPoint = ref(false);
const dialogFormVisible = ref(false);
const errorInfoMsg = ref("");
const dialogJsonVisible = ref(false);

// JSON 查看器配置（与 mgmt_platform 的 vue-json-pretty 用法一致）
const jsonState = reactive({
  data: {} as any,
  showLine: true,
  showLineNumber: true,
  showDoubleQuotes: true,
  showLength: true,
  showIcon: true,
  deep: 3
});

// 当前画布对应的已保存流程（用于再次进入时回显与覆盖保存）
const workflowRowId = ref<number | null>(null);
const workflowUuid = ref<string>(
  workflowId || (taskId ? `task_${taskId}` : "")
);

// 节点校验注册（与 mgmt_platform 一致）
const nodeRefs = ref<Record<string, { validate?: () => Promise<boolean> }>>({});
provide("registerNodeRef", (id: string, nodeRef: any) => {
  nodeRefs.value[id] = nodeRef;
});
provide("unregisterNodeRef", (id: string) => {
  delete nodeRefs.value[id];
});

/** 画布上任务节点里选中的航点，供动作节点 GOTO 使用 */
function nodeRoute() {
  for (const node of getNodes.value) {
    if (node.type === "task") {
      const taskData = node.data as any;
      if (taskData?.routePointListOptions) {
        return taskData.routePointListOptions;
      }
    }
  }
  return [];
}

function updateRoute(newRoutePointListOptions: any[]) {
  pathInfo.value = newRoutePointListOptions || [];
}

function handleShowCustomPoint(param: boolean) {
  isShowCustomPoint.value = param;
}

/** 已保存流程的快照，节点回显用 */
function getNodeDataById(id: string) {
  return rawNodes.value.find(n => n.id === id)?.data;
}

function showErrorInfo(id: string) {
  if (errorInfoMap.value && errorInfoMap.value[id]) {
    errorInfoMsg.value = errorInfoMap.value[id];
  } else {
    errorInfoMsg.value = "未找到对应错误信息";
  }
  dialogFormVisible.value = true;
}

// 连线规则
onConnect(params => {
  const sourceType = getNodes.value.find(n => n.id === params.source)?.type;
  const targetType = getNodes.value.find(n => n.id === params.target)?.type;
  const isActionEdge = sourceType === "action" || targetType === "action";
  addEdges([
    {
      ...params,
      style: isActionEdge
        ? { stroke: "#409EFF", strokeWidth: 2, strokeDasharray: "5,5" }
        : { stroke: "#409EFF", strokeWidth: 2 },
      animated: false,
      markerEnd: { type: MarkerType.ArrowClosed, color: "#ff00a6" }
    }
  ]);
});

// 键盘删除
function handleKeydown(event: KeyboardEvent) {
  if (event.key === "Delete") {
    if (getSelectedNodes.value.length) removeNodes(getSelectedNodes.value);
    if (getSelectedEdges.value.length) removeEdges(getSelectedEdges.value);
  }
}

// 查找已保存的流程：先按 uuid，再按 taskId 兜底
async function resolveWorkflow(): Promise<any | null> {
  try {
    if (workflowUuid.value) {
      const res = await dogWorkflowGetByUuid(workflowUuid.value);
      if (res.code === SUCCESS && res.data) return res.data;
    }
    if (taskId) {
      const res = await dogWorkflowGetByTask(Number(taskId));
      if (res.code === SUCCESS && res.data) return res.data;
    }
  } catch (e) {
    // 查询失败按无流程处理
  }
  return null;
}

// 加载已有流程
async function loadFlow() {
  if (!taskId) return;
  const data = await resolveWorkflow();
  if (!data) return;
  if (data.id) workflowRowId.value = data.id;
  if (data.uuid) workflowUuid.value = data.uuid;
  if (data.nodes) {
    const nodeData = JSON.parse(data.nodes);
    setNodes(nodeData);
    rawNodes.value = JSON.parse(JSON.stringify(nodeData));
  }
  if (data.edges) {
    setEdges(JSON.parse(data.edges));
  }
  nextTick(() => {
    pathInfo.value = nodeRoute();
  });
}

// 保存
async function saveActivity() {
  if (getNodes.value.length === 0) {
    message("请先拖拽流程节点到画布", { type: "error" });
    return;
  }
  isSubmit.value = true;
  // 按节点注册的校验规则依次校验必填项（与 mgmt_platform 一致）
  let isPassed = true;
  for (const id in nodeRefs.value) {
    const node = nodeRefs.value[id];
    if (node?.validate && !(await node.validate())) {
      isPassed = false;
    }
  }
  if (!isPassed) {
    message("请完善节点配置后再保存", { type: "error" });
    isSubmit.value = false;
    return;
  }
  const params = {
    taskId: Number(taskId),
    uuid: workflowUuid.value,
    nodes: JSON.stringify(getNodes.value),
    edges: JSON.stringify(getEdges.value)
  };

  try {
    // 已有流程则覆盖保存，避免同一任务产生多份流程
    let targetId = workflowRowId.value;
    if (!targetId) {
      const existing = await resolveWorkflow();
      if (existing?.id) {
        targetId = existing.id;
        workflowRowId.value = existing.id;
        if (existing.uuid) workflowUuid.value = existing.uuid;
      }
    }
    const res = targetId
      ? await dogWorkflowUpdate(targetId, params)
      : await dogWorkflowCreate(params);
    if (res.code === SUCCESS) {
      if (res.data?.id) workflowRowId.value = res.data.id;
      if (res.data?.uuid) workflowUuid.value = res.data.uuid;
      message("保存成功", { type: "success" });
    } else {
      message(res.msg || "保存失败", { type: "error" });
    }
  } catch (e) {
    message("保存失败", { type: "error" });
  }
  isSubmit.value = false;
}

// 查看执行状态
async function getTaskStatus() {
  if (!taskId) return;
  const res = await dogTaskGetTaskStatus(Number(taskId));
  if (res.code === SUCCESS && res.data) {
    isSuccessNodeId.value = res.data.successIds || [];
    isErrorNodeId.value = res.data.failIds || [];
    errorInfoMap.value = res.data.errorInfoMap || {};
  }
}

function nodeStatus(id: string): string {
  if (isShowSave) return "default";
  if (isSuccessNodeId.value?.includes(id)) return "success";
  if (isErrorNodeId.value?.includes(id)) return "error";
  return "default";
}

// 返回
async function goBack() {
  const tagsStore = useMultiTagsStoreHook();
  await router.replace({ path: "/dog/task/index" });
  nextTick(() => {
    tagsStore.handleTags("splice", FLOW_DESIGN_PATH);
  });
}

// 查看JSON：与 mgmt_platform 一致，展示保存流程时生成的指令 JSON
async function showJsonString() {
  const data = await resolveWorkflow();
  const raw = data?.commandJsonString;
  if (!raw) {
    message("获取指令集JSON字符串失败", { type: "error" });
    return;
  }
  try {
    jsonState.data = JSON.parse(raw);
  } catch (e) {
    message("指令集JSON字符串解析失败", { type: "error" });
    return;
  }
  dialogJsonVisible.value = true;
}

onMounted(() => {
  nextTick(() => {
    window.addEventListener("keydown", handleKeydown);
    loadFlow();
    getTaskStatus();
  });
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", handleKeydown);
});
</script>
<template>
  <div class="dnd-flow" @drop="onDrop">
    <div class="menuClass">
      <el-button
        type="primary"
        link
        :icon="useRenderIcon(Back)"
        @click="goBack()"
        >返回</el-button
      >
      <el-button
        v-if="isShowSave"
        :icon="useRenderIcon(Position)"
        type="primary"
        size="small"
        @click="saveActivity"
        >保存</el-button
      >
      <el-button
        v-if="isShowSave"
        :icon="useRenderIcon(Search)"
        type="success"
        size="small"
        @click="showJsonString"
        >查看JSON</el-button
      >
    </div>
    <Sidebar v-if="isShowSave" />
    <VueFlow
      @dragover="onDragOver"
      @dragleave="onDragLeave"
      @pane-click="selectedNodeId = ''"
    >
      <template #node-start><StartNode /></template>
      <template #node-end><EndNode /></template>
      <template #node-action="props">
        <ActionNode
          :id="props.id"
          :data="props.data"
          :selected="selectedNodeId === props.id"
          :status="nodeStatus(props.id)"
          :path-info="pathInfo"
          :is-show-custom-point="isShowCustomPoint"
          :get-node-data-by-id="getNodeDataById"
          @select="selectedNodeId = $event"
          @show-error-info="showErrorInfo(props.id)"
        />
      </template>
      <template #node-error="props">
        <ErrorNode
          :id="props.id"
          :is-submit="isSubmit"
          :data="props.data"
          :selected="selectedNodeId === props.id"
          :status="nodeStatus(props.id)"
          :get-node-data-by-id="getNodeDataById"
          @select="selectedNodeId = $event"
        />
      </template>
      <template #node-task="props">
        <TaskNode
          :id="props.id"
          :data="props.data"
          :device-id="deviceId"
          :route-id="routeId"
          :selected="selectedNodeId === props.id"
          :status="nodeStatus(props.id)"
          :get-node-data-by-id="getNodeDataById"
          @select="selectedNodeId = $event"
          @update-route="updateRoute"
          @show-custom-point="handleShowCustomPoint"
        />
      </template>
      <DropzoneBackground
        :style="{
          backgroundColor: isDragOver ? '#e7f3ff' : 'transparent',
          transition: 'background-color 0.2s ease'
        }"
      >
        <p v-if="isDragOver">拖拽到画布</p>
      </DropzoneBackground>
    </VueFlow>

    <el-dialog
      v-model="dialogFormVisible"
      title="错误信息"
      width="500px"
      @close="dialogFormVisible = false"
    >
      <div>{{ errorInfoMsg }}</div>
    </el-dialog>

    <el-dialog
      v-model="dialogJsonVisible"
      title="下发命令字符串"
      width="60%"
      @close="dialogJsonVisible = false"
    >
      <vue-json-pretty
        v-model:data="jsonState.data"
        :deep="jsonState.deep"
        :show-double-quotes="jsonState.showDoubleQuotes"
        :show-line="jsonState.showLine"
        :show-length="jsonState.showLength"
        :show-icon="jsonState.showIcon"
        :show-line-number="jsonState.showLineNumber"
      />
    </el-dialog>
  </div>
</template>
<style lang="scss">
@import url("@vue-flow/core/dist/style.css");
@import url("@vue-flow/core/dist/theme-default.css");
@import url("@vue-flow/controls/dist/style.css");
@import url("@vue-flow/minimap/dist/style.css");
@import url("@vue-flow/node-resizer/dist/style.css");

.menuClass {
  position: absolute;
  top: 10px;
  left: 10px;
  z-index: 10;
}
.dnd-flow {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  color: #2c3e50;
  -webkit-font-smoothing: antialiased;
}
.dnd-flow aside {
  padding: 15px 10px;
  margin-top: 44px;
  font-size: 12px;
  font-weight: 700;
  color: #fff;
  background: #10b981bf;
  box-shadow: 0 5px 10px #0000004d;
}
.dnd-flow .vue-flow-wrapper {
  flex-grow: 1;
  height: 100%;
}
@media screen and (width >= 200px) {
  .dnd-flow {
    flex-direction: row;
  }
  .dnd-flow aside {
    min-width: 8%;
  }
}
</style>
