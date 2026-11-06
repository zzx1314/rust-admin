/**
 * 流程设计（隐藏路由）
 *
 * 与 `mgmt_platform` 的 `src/router/modules/workFlow.ts` 保持一致：
 * 父级只做重定向、不挂 `Layout` 组件（否则会在根 `Layout` 里再套一层 `Layout`），
 * 真正渲染页面的子路由是 `/dog/task/flow`。
 *
 * 该路由不出现在菜单中（`showLink: false`），所以不会自动生成标签页，
 * 标签页由 `src/views/dog/task/flowHook.ts` 手动 `push`，刷新时再按地址栏参数还原。
 */
export default [
  {
    path: "/dog/task/flowDesign",
    name: "dronesFlowDesign",
    redirect: "/dog/task/flow",
    meta: {
      title: "流程设计",
      showLink: false,
      rank: 99
    },
    children: [
      {
        path: "/dog/task/flow",
        name: "dronesFlowDesignIndex",
        component: () => import("@/views/dog/task/flowApp.vue"),
        meta: {
          title: "流程设计",
          activePath: "/dog/task/index",
          showLink: false,
          showParent: false
        }
      }
    ]
  }
] satisfies Array<RouteConfigsTable>;
