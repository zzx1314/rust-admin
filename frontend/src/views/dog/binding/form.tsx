import type { PlusColumn } from "plus-pro-components";

export function useDogBindingForm() {
  const columnsForm: PlusColumn[] = [
    { label: "用户ID", prop: "userId", valueType: "input-number" },
    { label: "设备ID", prop: "deviceId", valueType: "input-number" }
  ];

  const columnsQueryForm: PlusColumn[] = [
    {
      label: "用户ID",
      prop: "userId",
      valueType: "input-number",
      colProps: { span: 5 }
    },
    {
      label: "设备ID",
      prop: "deviceId",
      valueType: "input-number",
      colProps: { span: 5 }
    }
  ];
  return { columnsForm, columnsQueryForm };
}
