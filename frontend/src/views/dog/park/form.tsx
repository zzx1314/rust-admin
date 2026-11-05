import type { PlusColumn } from "plus-pro-components";

export function useDogParkForm() {
  const columnsForm: PlusColumn[] = [
    { label: "园区名称", prop: "parkName", valueType: "copy" },
    { label: "园区描述", prop: "parkDescription", valueType: "textarea" },
    { label: "园区地址", prop: "parkAddress", valueType: "copy" }
  ];

  return { columnsForm };
}
