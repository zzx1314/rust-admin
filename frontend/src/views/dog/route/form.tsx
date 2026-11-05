import type { PlusColumn } from "plus-pro-components";

export function useCollectorRouteForm() {
  const columnsForm: PlusColumn[] = [
    { label: "路线名称", prop: "routeName", valueType: "copy" },
    { label: "路线类型", prop: "routeType", valueType: "copy" },
    { label: "路线状态", prop: "routeStatus", valueType: "copy" },
    { label: "路线描述", prop: "routeDescription", valueType: "textarea" },
    { label: "模型ID", prop: "modelId", valueType: "input-number" },
    { label: "路线数据", prop: "routeData", valueType: "textarea" }
  ];
  return { columnsForm };
}
