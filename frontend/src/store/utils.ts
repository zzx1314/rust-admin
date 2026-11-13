export { store } from "@/store";
export { routerArrays } from "@/layout/types";
export { router, resetRouter, constantMenus } from "@/router";
// 注意：这里**不要**再 re-export `@/config` 里的 `getConfig` /
// `responsiveStorageNameSpace`。`config` 需要 `layout/types` 与 `@pureadmin/utils`，
// 两边互相 re-export 会被 Rollup 拆进不同 chunk，打包时报
// 「Export ... was reexported through module ... while both modules are dependencies
// of each other」。store 模块请直接从 `@/config` 引入。
export {
  ascending,
  filterTree,
  filterNoPermissionTree,
  formatFlatteningRoutes
} from "@/router/utils";
export {
  isUrl,
  isEqual,
  isNumber,
  debounce,
  isBoolean,
  getKeyList,
  storageSession,
  deviceDetection
} from "@pureadmin/utils";
export type {
  setType,
  appType,
  userType,
  multiType,
  cacheType,
  positionType
} from "./types";
