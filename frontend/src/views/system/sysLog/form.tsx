// form表单
import type { PlusColumn } from "plus-pro-components";

export function useLogForm() {
  const columnsForm: PlusColumn[] = [
    {
      label: "名称",
      prop: "name",
      valueType: "copy",
      formItemProps: {
        style: {
          width: "50%"
        }
      }
    },
    {
      label: "备注",
      prop: "remark",
      width: "10px",
      valueType: "textarea"
    }
  ];

  return {
    columnsForm
  };
}
