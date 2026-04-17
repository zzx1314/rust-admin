interface FormItemProps {
  id?: number;
  /** 用于判断是`新增`还是`修改` */
  title: string;
  label: string;
  value: string;
  color: string;
  sort: number;
  status: number;
  remark: string;
}
interface FormProps {
  formInline: FormItemProps;
}

interface DictFormItemProps {
  title: string;
  name: string;
  code: string;
  remark: string;
}
interface DictFormProps {
  formInline: DictFormItemProps;
}

export type { FormItemProps, FormProps, DictFormItemProps, DictFormProps };
