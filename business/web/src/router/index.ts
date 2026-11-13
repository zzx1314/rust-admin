import { createRouter, createWebHistory } from "vue-router";
import { useAuthStore } from "@/stores/auth";

const router = createRouter({
  history: createWebHistory(),
  scrollBehavior(_to, _from, savedPosition) {
    return savedPosition ?? { top: 0 };
  },
  routes: [
    {
      path: "/login",
      name: "login",
      component: () => import("@/views/login/index.vue"),
      meta: { public: true, title: "登录" }
    },
    {
      path: "/",
      component: () => import("@/layout/OpsLayout.vue"),
      redirect: "/dashboard",
      children: [
        {
          path: "dashboard",
          name: "dashboard",
          component: () => import("@/views/dashboard/index.vue"),
          meta: { title: "总览驾驶舱" }
        },
        {
          path: "parks",
          name: "parks",
          component: () => import("@/views/parks/index.vue"),
          meta: { title: "园区管理" }
        },
        {
          path: "routes",
          name: "routes",
          component: () => import("@/views/routes/index.vue"),
          meta: { title: "巡检任务" }
        },
        {
          path: "devices",
          name: "devices",
          component: () => import("@/views/devices/index.vue"),
          meta: { title: "设备管理" }
        },
        {
          path: "tasks",
          name: "tasks",
          component: () => import("@/views/tasks/index.vue"),
          meta: { title: "计划管理" }
        },
        {
          path: "alerts",
          name: "alerts",
          component: () => import("@/views/alerts/index.vue"),
          meta: { title: "告警中心" }
        },
        {
          path: "settings",
          name: "settings",
          component: () => import("@/views/settings/index.vue"),
          meta: { title: "系统设置" }
        }
      ]
    },
    { path: "/:pathMatch(.*)*", redirect: "/dashboard" }
  ]
});

router.beforeEach(to => {
  const auth = useAuthStore();
  if (!to.meta.public && !auth.isLoggedIn) {
    return { path: "/login", query: { redirect: to.fullPath } };
  }
  if (to.path === "/login" && auth.isLoggedIn) {
    return { path: "/dashboard" };
  }
  return true;
});

router.afterEach(to => {
  document.title = `${to.meta.title || "RoboDog OPS"} · 机器狗巡检业务平台`;
});

export default router;
