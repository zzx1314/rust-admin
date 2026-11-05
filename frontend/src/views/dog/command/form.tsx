import type { PlusColumn } from "plus-pro-components";

export function useCollectorCommandForm() {
  const columnsForm: PlusColumn[] = [
    { label: "指令名称", prop: "commandName", valueType: "copy" },
    { label: "指令类型", prop: "commandType", valueType: "copy" },
    { label: "指令参数", prop: "commandParams", valueType: "textarea" },
    { label: "目标设备", prop: "deviceId", valueType: "copy" },
    { label: "指令状态", prop: "status", valueType: "copy" },
    { label: "返回值", prop: "returnValue", valueType: "textarea" },
    { label: "任务ID", prop: "taskId", valueType: "input-number" }
  ];
  const columnsQueryForm: PlusColumn[] = [
    {
      label: "指令名称",
      prop: "commandName",
      valueType: "copy",
      colProps: { span: 5 }
    },
    {
      label: "指令类型",
      prop: "commandType",
      valueType: "copy",
      colProps: { span: 5 }
    },
    { label: "状态", prop: "status", valueType: "copy", colProps: { span: 5 } }
  ];
  return { columnsForm, columnsQueryForm };
}
