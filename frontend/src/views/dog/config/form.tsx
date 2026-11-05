import type { PlusColumn } from "plus-pro-components";

export function useCollectorConfigForm() {
  const columnsForm: PlusColumn[] = [
    { label: "配置名称", prop: "configName", valueType: "copy" },
    { label: "配置值", prop: "configValue", valueType: "copy" },
    { label: "描述", prop: "description", valueType: "textarea" }
  ];
  return { columnsForm };
}
