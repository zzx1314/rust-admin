import type { PlusColumn } from "plus-pro-components";

export function useCollectorTaskForm() {
  const columnsForm: PlusColumn[] = [
    { label: "任务名称", prop: "taskName", valueType: "copy" },
    { label: "任务描述", prop: "taskDescription", valueType: "textarea" },
    { label: "任务状态", prop: "taskStatus", valueType: "copy" },
    { label: "工作流ID", prop: "workflowId", valueType: "input-number" },
    { label: "设备ID", prop: "deviceId", valueType: "input-number" },
    { label: "路线ID", prop: "routeId", valueType: "input-number" }
  ];

  const columnsQueryForm: PlusColumn[] = [
    {
      label: "任务名称",
      prop: "taskName",
      valueType: "copy",
      colProps: { span: 5 }
    },
    {
      label: "任务状态",
      prop: "taskStatus",
      valueType: "copy",
      colProps: { span: 5 }
    }
  ];
  return { columnsForm, columnsQueryForm };
}
