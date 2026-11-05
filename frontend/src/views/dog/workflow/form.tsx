import type { PlusColumn } from "plus-pro-components";

export function useCollectorWorkflowForm() {
  const columnsForm: PlusColumn[] = [
    { label: "UUID", prop: "uuid", valueType: "copy" },
    { label: "节点数据", prop: "nodes", valueType: "textarea" },
    { label: "边数据", prop: "edges", valueType: "textarea" },
    { label: "任务ID", prop: "taskId", valueType: "input-number" },
    { label: "指令JSON", prop: "commandJsonString", valueType: "textarea" }
  ];
  return { columnsForm };
}
