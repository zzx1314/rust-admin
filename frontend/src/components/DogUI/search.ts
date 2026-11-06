/** 业务页面统一查询栏（DogSearchForm）使用的字段配置。 */
export interface DogSearchOption {
  label: string;
  value: string | number;
}

export interface DogSearchField {
  label: string;
  prop: string;
  type?: "input" | "number" | "select" | "date";
  placeholder?: string;
  options?: DogSearchOption[];
  /** 日期字段的取值格式，默认 YYYY-MM-DD */
  valueFormat?: string;
}

/** 各页面「更多条件」里统一的时间范围（后端同时接受日期与日期时间）。 */
export const DATE_RANGE_FIELDS: DogSearchField[] = [
  { label: "开始时间", prop: "beginTime", type: "date" },
  { label: "结束时间", prop: "endTime", type: "date" }
];
