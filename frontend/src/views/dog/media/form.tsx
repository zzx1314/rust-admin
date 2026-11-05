import type { PlusColumn } from "plus-pro-components";

export function useCollectorMediaForm() {
  const columnsForm: PlusColumn[] = [
    { label: "媒体名称", prop: "mediaName", valueType: "copy" },
    { label: "媒体类型", prop: "mediaType", valueType: "copy" },
    { label: "文件路径", prop: "mediaPath", valueType: "copy" },
    { label: "文件大小(字节)", prop: "mediaSize", valueType: "input-number" },
    { label: "备注", prop: "remarks", valueType: "textarea" }
  ];
  return { columnsForm };
}
