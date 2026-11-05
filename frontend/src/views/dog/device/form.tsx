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

  return { columnsForm };
}
