import type { PlusColumn } from "plus-pro-components";

export function useCollectorBusDevForm() {
  const columnsForm: PlusColumn[] = [
    { label: "设备ID", prop: "deviceId", valueType: "copy" },
    { label: "设备类型", prop: "deviceType", valueType: "copy" },
    { label: "设备状态", prop: "status", valueType: "copy" },
    { label: "IP地址", prop: "deviceIp", valueType: "copy" },
    { label: "设备型号", prop: "model", valueType: "copy" },
    { label: "速度", prop: "speed", valueType: "copy" },
    { label: "高度", prop: "height", valueType: "copy" },
    { label: "剩余电量", prop: "battery", valueType: "copy" },
    { label: "航向", prop: "course", valueType: "copy" },
    { label: "位置", prop: "location", valueType: "copy" }
  ];

  const columnsQueryForm: PlusColumn[] = [
    {
      label: "设备ID",
      prop: "deviceId",
      valueType: "copy",
      colProps: { span: 5 }
    },
    {
      label: "设备类型",
      prop: "deviceType",
      valueType: "copy",
      colProps: { span: 5 }
    },
    {
      label: "状态",
      prop: "status",
      valueType: "copy",
      colProps: { span: 5 }
    },
    {
      label: "开始时间",
      prop: "beginTime",
      valueType: "date-picker",
      fieldProps: { type: "date", valueFormat: "YYYY-MM-DD HH:mm:ss" },
      colProps: { span: 5 }
    },
    {
      label: "结束时间",
      prop: "endTime",
      valueType: "date-picker",
      fieldProps: { type: "date", valueFormat: "YYYY-MM-DD" },
      colProps: { span: 5 }
    }
  ];
  return { columnsForm, columnsQueryForm };
}
