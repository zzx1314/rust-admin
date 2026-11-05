import type { PlusColumn } from "plus-pro-components";

export function useCollectorModelForm() {
  const columnsForm: PlusColumn[] = [
    { label: "模型名称", prop: "modelName", valueType: "copy" },
    { label: "模型类型", prop: "modelType", valueType: "copy" },
    { label: "文件名称", prop: "fileName", valueType: "copy" },
    { label: "文件路径", prop: "filePath", valueType: "copy" },
    { label: "文件格式", prop: "fileFormat", valueType: "copy" },
    { label: "文件大小(字节)", prop: "fileSize", valueType: "input-number" },
    { label: "备注", prop: "remarks", valueType: "textarea" }
  ];
  return { columnsForm };
}
