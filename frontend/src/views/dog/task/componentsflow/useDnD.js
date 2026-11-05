import { useVueFlow } from "@vue-flow/core";
import { ref, watch } from "vue";

function getId() {
  return `node_${Math.random().toString(36).slice(2, 10)}`;
}

const state = {
  draggedType: ref(null),
  isDragOver: ref(false),
  isDragging: ref(false)
};

export default function useDragAndDrop() {
  const { draggedType, isDragOver, isDragging } = state;

  const { addNodes, screenToFlowCoordinate, onNodesInitialized, updateNode } =
    useVueFlow();

  watch(isDragging, dragging => {
    document.body.style.userSelect = dragging ? "none" : "";
  });

  function onDragStart(event, type) {
    if (event.dataTransfer) {
      event.dataTransfer.setData("application/vueflow", type);
      event.dataTransfer.effectAllowed = "move";
    }
    draggedType.value = type;
    isDragging.value = true;
    document.addEventListener("drop", onDragEnd);
  }

  function onDragOver(event) {
    event.preventDefault();
    if (draggedType.value) {
      isDragOver.value = true;
      if (event.dataTransfer) {
        event.dataTransfer.dropEffect = "move";
      }
    }
  }

  function onDragLeave() {
    isDragOver.value = false;
  }

  function onDragEnd() {
    isDragging.value = false;
    isDragOver.value = false;
    draggedType.value = null;
    document.removeEventListener("drop", onDragEnd);
  }

  function onDrop(event) {
    const position = screenToFlowCoordinate({
      x: event.clientX,
      y: event.clientY
    });
    const nodeId = getId();

    const defaultData = {
      taskId: nodeId,
      label: "节点",
      deviceId: null,
      routeId: null,
      taskInfo: {
        fromWp: { wpId: null },
        toWp: { wpId: null },
        event: "event_" + Math.random().toString(36).slice(2, 8)
      },
      action: {
        type: "",
        params: {
          timeSec: null,
          targetAlt: null,
          targetWp: null,
          num: null,
          intervalSec: null,
          type: "",
          reason: "",
          event: {
            onComplete:
              "complete_" + Math.random().toString(36).slice(2, 10),
            onFail: "fail_" + Math.random().toString(36).slice(2, 10)
          }
        }
      }
    };

    const newNode = {
      id: nodeId,
      type: draggedType.value,
      position,
      data: { ...defaultData, label: draggedType.value + "节点" }
    };

    const { off } = onNodesInitialized(() => {
      updateNode(nodeId, node => ({
        position: {
          x: node.position.x - node.dimensions.width / 2,
          y: node.position.y - node.dimensions.height / 2
        }
      }));
      off();
    });

    addNodes(newNode);
  }

  return {
    draggedType,
    isDragOver,
    isDragging,
    onDragStart,
    onDragLeave,
    onDragOver,
    onDrop
  };
}
