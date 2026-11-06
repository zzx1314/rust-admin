import { isString, isEmpty } from "@pureadmin/utils";
import { useMultiTagsStoreHook } from "@/store/modules/multiTags";
import {
  useRouter,
  useRoute,
  type LocationQueryRaw,
  type RouteParamsRaw
} from "vue-router";

/** 流程设计页面路由，需与 `src/router/modules/dog-flow.ts` 中的子路由保持一致 */
export const FLOW_DESIGN_PATH = "/dog/task/flow";
/** 流程设计页面路由名称，需与 `src/router/modules/dog-flow.ts` 中的子路由保持一致 */
export const FLOW_DESIGN_NAME = "dronesFlowDesignIndex";

/**
 * 流程设计页面的路由跳转与标签页处理（对齐 mgmt_platform 的 `flowHook.tsx`）
 *
 * 流程设计是 `showLink: false` 的隐藏路由，`handleTags("push")` 对这类路由会自动跳过，
 * 因此标签页必须手动维护：
 * 1. 跳转前先把标签页 push 进 `multiTags`（带上 `query`，从标签页切回时才能还原参数）
 * 2. 页面刷新后（未开启标签页缓存时标签页会丢失）用地址栏参数重新 push 标签页
 */
export function useProcess() {
  const route = useRoute();
  const router = useRouter();
  const getParameter = (
    isEmpty(route.params) ? route.query : route.params
  ) as LocationQueryRaw;

  function toDetail(parameter: LocationQueryRaw | RouteParamsRaw) {
    if (!parameter) return;
    // query 只保留字符串，避免数字/数组在标签页比较时失效
    Object.keys(parameter).forEach(param => {
      if (parameter[param] != null && !isString(parameter[param])) {
        parameter[param] = parameter[param].toString();
      }
    });
    const query = parameter as LocationQueryRaw;
    // 保存信息到标签页
    useMultiTagsStoreHook().handleTags("push", {
      path: FLOW_DESIGN_PATH,
      name: FLOW_DESIGN_NAME,
      query,
      meta: {
        title: query.name ? `${query.name} - 流程设计` : "流程设计",
        dynamicLevel: 3
      }
    });
    // 路由跳转
    router.push({ name: FLOW_DESIGN_NAME, query });
  }

  /** 用于页面刷新，重新获取浏览器地址栏参数并保存到标签页 */
  const initToDetail = () => {
    if (!isEmpty(getParameter)) toDetail(getParameter);
  };

  return { toDetail, initToDetail, getParameter, router };
}
