/** 机器狗业务页面统一查询栏使用的字段配置。 */
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
}

/** 设备状态：网关心跳只会在 online / offline 之间切换。 */
export const DEVICE_STATUS_OPTIONS: DogSearchOption[] = [
  { label: "在线", value: "online" },
  { label: "离线", value: "offline" }
];

/** 指令状态：新建默认 pending，下发后 sent，设备回报 success / error。 */
export const COMMAND_STATUS_OPTIONS: DogSearchOption[] = [
  { label: "待下发", value: "pending" },
  { label: "已下发", value: "sent" },
  { label: "成功", value: "success" },
  { label: "失败", value: "error" }
];

/** 各页面「更多条件」里统一的创建时间范围。 */
export const CREATE_TIME_FIELDS: DogSearchField[] = [
  { label: "开始时间", prop: "beginTime", type: "date" },
  { label: "结束时间", prop: "endTime", type: "date" }
];
