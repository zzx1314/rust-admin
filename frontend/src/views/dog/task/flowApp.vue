<script setup lang="ts">
import { ref, onMounted, nextTick, onBeforeUnmount, provide } from "vue";
import { VueFlow, useVueFlow, MarkerType } from "@vue-flow/core";
import { useRoute, useRouter } from "vue-router";
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
  dogWorkflowCreate,
  dogWorkflowUpdate
} from "@/api/dogWorkflow";
import { dogTaskGetTaskStatus } from "@/api/dogTask";
import { SUCCESS } from "@/api/base";

const route = useRoute();
const router = useRouter();

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

const taskId = route.query.taskId as string;
const taskType = route.query.taskType as string;
const workflowId = route.query.workflowId as string;
const deviceId = route.query.deviceId as string;
const routeId = route.query.routeId as string;
const name = route.query.name as string;

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

// 加载已有流程
async function loadFlow() {
  if (!taskId) return;
  try {
    const res = await dogWorkflowGetByUuid(workflowId);
    if (res.code === SUCCESS && res.data) {
      const data = res.data;
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
    // data 为 null 表示 workflow 不存在，使用空白画布
  } catch (e) {
    // No existing workflow - start fresh
  }
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
    uuid: workflowId,
    nodes: JSON.stringify(getNodes.value),
    edges: JSON.stringify(getEdges.value)
  };

  try {
    // Try to check if workflow exists
    const checkRes = await dogWorkflowGetByUuid(workflowId);
    let res;
    if (checkRes.code === SUCCESS && checkRes.data && checkRes.data.id) {
      // Update existing
      res = await dogWorkflowUpdate(checkRes.data.id, params);
    } else {
      // Create new
      res = await dogWorkflowCreate(params);
    }
    if (res.code === SUCCESS) {
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
    tagsStore.handleTags("splice", "/dog/task/flow");
  });
}

// 查看JSON
function showJsonString() {
  const data = {
    nodes: getNodes.value,
    edges: getEdges.value
  };
  alert(JSON.stringify(data, null, 2));
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
