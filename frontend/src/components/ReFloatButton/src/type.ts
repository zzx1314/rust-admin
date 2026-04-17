import type { Component } from "vue";

export interface floatBtnsType {
  /** 提示信息 */
  tip?: string;
  /** 图标 */
  icon: Component;
  /** 链接 */
  link?: string;
  /** 是否显示 */
  show: boolean;
}
