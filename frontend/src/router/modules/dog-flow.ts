export default [
  {
    path: "/dog/task/flow",
    name: "dronesFlowDesign",
    component: () => import("@/layout/index.vue"),
    redirect: "/dog/task/flow/index",
    meta: {
      title: "流程设计",
      showLink: false,
      rank: 99
    },
    children: [
      {
        path: "/dog/task/flow/index",
        name: "dronesFlowDesignIndex",
        component: () => import("@/views/dog/task/flowApp.vue"),
        meta: {
          title: "流程设计",
          showLink: false
        }
      }
    ]
  }
] satisfies Array<RouteConfigsTable>;
