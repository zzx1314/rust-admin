import type { PlusColumn } from "plus-pro-components";

export function useDogMapForm() {
  const columnsForm: PlusColumn[] = [
    { label: "地图名称", prop: "mapName", valueType: "copy" },
    { label: "地图类型", prop: "mapType", valueType: "copy" },
    { label: "所属园区ID", prop: "parkId", valueType: "input-number" },
    { label: "地图数据", prop: "mapData", valueType: "textarea" }
  ];

  return { columnsForm };
}
