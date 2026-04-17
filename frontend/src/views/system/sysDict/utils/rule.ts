import { reactive } from "vue";
import type { FormRules } from "element-plus";

/** 自定义表单规则校验 */
export const formRules = reactive(<FormRules>{
  label: [{ required: true, message: "字典标签为必填项", trigger: "blur" }],
  value: [{ required: true, message: "字典值为必填项", trigger: "blur" }]
});

export const dictFormRules = reactive(<FormRules>{
  name: [{ required: true, message: "名称为必填项", trigger: "blur" }],
  code: [{ required: true, message: "编码为必填项", trigger: "blur" }]
});
