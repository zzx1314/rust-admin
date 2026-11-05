import type { PlusColumn } from "plus-pro-components";

export function useCollectorServicesForm() {
  const columnsForm: PlusColumn[] = [
    { label: "类型", prop: "type", valueType: "copy" },
    { label: "参数", prop: "params", valueType: "textarea" }
  ];
  return { columnsForm };
}
