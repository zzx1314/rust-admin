import type { PlusColumn } from "plus-pro-components";

export function useCollectorRouteForm() {
  const columnsForm: PlusColumn[] = [
    { label: "所属园区", prop: "parkName", valueType: "copy" },
    { label: "路线名称", prop: "routeName", valueType: "copy" },
    {
      label: "路线类型",
      prop: "routeType",
      valueType: "select",
      options: [
        { label: "在线地图", value: "在线地图" },
        { label: "离线地图", value: "离线地图" }
      ]
    },
    {
      label: "路线状态",
      prop: "routeStatus",
      valueType: "select",
      options: [
        { label: "启用", value: "启用" },
        { label: "停用", value: "停用" }
      ]
    },
    { label: "路线描述", prop: "routeDescription", valueType: "textarea" },
    { label: "模型ID", prop: "modelId", valueType: "input-number" },
    { label: "路线数据", prop: "routeData", valueType: "textarea" }
  ];
  return { columnsForm };
}
