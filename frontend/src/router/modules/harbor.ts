import { system } from "@/router/enums";

export default {
  path: "/harbor",
  meta: {
    icon: "ri:store-2-line",
    title: "应用商店",
    rank: system
  },
  children: [
    {
      path: "/harbor/project",
      name: "HarborProject",
      meta: {
        title: "项目概要",
        roles: ["admin", "common"],
        keepAlive: true
      }
    },
    {
      path: "/harbor/repository",
      name: "HarborRepository",
      meta: {
        title: "镜像仓库",
        roles: ["admin", "common"],
        keepAlive: true
      }
    },
    {
      path: "/harbor/member",
      name: "HarborMember",
      meta: {
        title: "项目成员",
        roles: ["admin", "common"],
        keepAlive: true
      }
    }
  ]
};
