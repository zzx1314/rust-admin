import { computed } from "vue";
import type { Ref } from "vue";
import type { PlusColumn } from "plus-pro-components";

type Option = { label: string; value: number };

export function useDogBindingForm(
  userOptions: Ref<Option[]>,
  deviceOptions: Ref<Option[]>
) {
  const selectProps = { filterable: true, clearable: true };

  const columnsForm = computed<PlusColumn[]>(() => [
    {
      label: "用户",
      prop: "userId",
      valueType: "select",
      fieldProps: selectProps,
      options: userOptions.value
    },
    {
      label: "设备",
      prop: "deviceId",
      valueType: "select",
      fieldProps: selectProps,
      options: deviceOptions.value
    }
  ]);

  const columnsQueryForm = computed<PlusColumn[]>(() => [
    {
      label: "用户",
      prop: "userId",
      valueType: "select",
      fieldProps: selectProps,
      options: userOptions.value,
      colProps: { span: 5 }
    },
    {
      label: "设备",
      prop: "deviceId",
      valueType: "select",
      fieldProps: selectProps,
      options: deviceOptions.value,
      colProps: { span: 5 }
    }
  ]);
  return { columnsForm, columnsQueryForm };
}
