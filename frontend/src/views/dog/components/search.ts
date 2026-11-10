import { DATE_RANGE_FIELDS } from "@/components/DogUI/search";
import type {
  DogSearchField,
  DogSearchOption
} from "@/components/DogUI/search";

/** 查询栏字段配置：类型定义已下沉到共享组件，这里只保留业务选项。 */
export type { DogSearchField, DogSearchOption };

/** 地图类型（地图包分为在线 / 离线两种）。 */
export const MAP_TYPE_OPTIONS: DogSearchOption[] = [
  { label: "离线地图", value: "离线地图" },
  { label: "在线地图", value: "在线地图" }
];

/** 地图基线状态：草稿 → 部分标定 / 已发布 → 已归档。 */
export const MAP_STATUS_OPTIONS: DogSearchOption[] = [
  { label: "草稿", value: "草稿" },
  { label: "部分标定", value: "部分标定" },
  { label: "已发布", value: "已发布" },
  { label: "已归档", value: "已归档" }
];

/** 地图基线徽章配色。 */
export const MAP_STATUS_TAG_TYPE: Record<
  string,
  "success" | "warning" | "info" | "danger"
> = {
  已发布: "success",
  部分标定: "warning",
  草稿: "info",
  已归档: "info"
};

/** 各页面「更多条件」里统一的创建时间范围。 */
export const CREATE_TIME_FIELDS: DogSearchField[] = [...DATE_RANGE_FIELDS];
