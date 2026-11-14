<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAuthStore } from "@/stores/auth";
import { useAppStore } from "@/stores/app";
import { alertApi, deviceApi } from "@/api/ops";
import type { Alert } from "@/api/types";
import { ElNotification, type NotificationHandle } from "element-plus";
import AppIcon from "@/components/AppIcon.vue";
import { initials, timeOnly } from "@/utils/format";

const route = useRoute();
const router = useRouter();
const auth = useAuthStore();
const app = useAppStore();

const search = ref("");
const clock = ref("");
const syncTime = ref("");
const bellOpen = ref(false);
const userOpen = ref(false);
const bellAlerts = ref<Alert[]>([]);
const deviceTotal = ref(0);
const onlineDevices = ref(0);
let clockTimer = 0;
let pollTimer = 0;
let warmTimer = 0;
let controlTimer = 0;

const currentTitle = computed(() => (route.meta.title as string) || "总览驾驶舱");
const displayName = computed(() => auth.user?.displayName || "管理员");
const roleName = computed(() =>
  auth.user?.role === "admin" ? "平台运维组 · 管理员" : auth.user?.role || "运维人员"
);

const navGroups = [
  {
    title: "工作台",
    items: [{ path: "/dashboard", label: "总览驾驶舱", icon: "grid" }]
  },
  {
    title: "业务管理",
    items: [
      { path: "/parks", label: "园区管理", icon: "park" },
      { path: "/routes", label: "巡检任务", icon: "route" },
      { path: "/devices", label: "设备管理", icon: "dog" },
      { path: "/tasks", label: "计划管理", icon: "task" }
    ]
  },
  {
    title: "监控运维",
    items: [
      { path: "/monitor", label: "实时视频", icon: "video" },
      { path: "/alerts", label: "告警中心", icon: "bell" },
      { path: "/settings", label: "系统设置", icon: "settings" }
    ]
  }
];

function tick() {
  const now = new Date();
  const weekdays = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
  const pad = (value: number) => String(value).padStart(2, "0");
  clock.value = `${now.getFullYear()}年${now.getMonth() + 1}月${now.getDate()}日 ${weekdays[now.getDay()]} ${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
  syncTime.value = `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
}

function go(path: string) {
  if (route.path !== path) router.push(path);
}

function prefetch(path: string) {
  if (route.path === path) return;
  for (const record of router.resolve(path).matched) {
    for (const component of Object.values(record.components ?? {})) {
      if (typeof component !== "function") continue;
      const result = (component as () => unknown)();
      if (result && typeof (result as Promise<unknown>).catch === "function") {
        (result as Promise<unknown>).catch(() => undefined);
      }
    }
  }
}

function warmNavRoutes() {
  for (const group of navGroups) {
    for (const item of group.items) prefetch(item.path);
  }
}

function globalSearch() {
  const keyword = search.value.trim();
  if (!keyword) return;
  router.push({ path: "/devices", query: { q: keyword } });
  search.value = "";
}

async function loadBell() {
  try {
    bellAlerts.value = await alertApi.list({ status: "未处理" });
  } catch {
    bellAlerts.value = [];
  }
}

async function toggleBell() {
  userOpen.value = false;
  bellOpen.value = !bellOpen.value;
  if (bellOpen.value) await loadBell();
}

async function refreshStatus() {
  await app.refreshPendingAlerts();
  try {
    const stats = await deviceApi.stats();
    deviceTotal.value = stats.total;
    onlineDevices.value = stats.online;
  } catch {
    /* ignore transient polling errors */
  }
}

function logout() {
  auth.logout();
  router.push("/login");
}

/** 控制源监控：本体控制权跑出平台（1315 `ctrl_source=3`）时全局弹提示。
 *
 * 现场最难受的一种状态：控制台显示「在线待命、电量正常」，摇杆推不动、任务也下发不了。
 * 服务端在 `ctrl_source` 变成 3 时落一条 `AL-CTRL-*` 告警（见
 * `business/server/src/gateway.rs`），这里轮询设备列表做两件事：
 * - 某台设备从非 3 变成 3 → 弹一条常驻警告（点一下跳到设备列表）；
 * - 它又回到 1/2 → 自动关掉那条警告，避免运维以为还被占着。
 * 首轮轮询也会提示「当前正处于 3」的设备：新开控制台就该知道谁占着控制权；
 * 之后只在跳变时提示，不会每 10s 刷一遍。
 *
 * 文案不写死「遥控器」：`3` 是「其他控制源」，本体内部残留任务占着导航能力时
 * 固件同样报 3（现场更常见，伴随 13897 capability busy）。
 */
const ctrlSourceOf = new Map<number, number>();
const ctrlNotices = new Map<number, NotificationHandle>();

async function watchControlSource() {
  try {
    const devices = await deviceApi.list();
    for (const device of devices) {
      const previous = ctrlSourceOf.get(device.id);
      ctrlSourceOf.set(device.id, device.ctrlSource);
      if (previous !== undefined && previous === device.ctrlSource) continue;

      // 本体在跑我们下发的任务时也会报 ctrl_source=3（导航能力在本体手里），
      // 这不算抢占：任务执行中不弹提示，避免每次下发任务都被吓一次。
      if (device.status === "执行任务") {
        ctrlNotices.get(device.id)?.close();
        ctrlNotices.delete(device.id);
        continue;
      }

      if (device.ctrlSource === 3) {
        ctrlNotices.get(device.id)?.close();
        ctrlNotices.set(
          device.id,
          ElNotification({
            title: "控制权不在平台（ctrl_source=3）",
            message:
              `设备 ${device.deviceId} 上报 ctrl_source=3：可能是物理遥控器 / 第三方 App，` +
              "也可能是本体内残留任务占着导航能力（本体常回 13897 capability busy）。" +
              "摇杆与任务下发已暂停：请关闭遥控器电源，并在本体取消残留任务或重启本体算法服务；" +
              "确认遥控器已断电可直接「强制接管」。点此查看设备。",
            type: "warning",
            // 常驻：控制权收回时自动关闭，防止一闪而过被忽略
            duration: 0,
            position: "top-right",
            onClick: () => {
              ctrlNotices.get(device.id)?.close();
              router.push({ path: "/devices", query: { q: device.deviceId } });
            }
          })
        );
      } else if (ctrlNotices.has(device.id)) {
        ctrlNotices.get(device.id)?.close();
        ctrlNotices.delete(device.id);
      }
    }
  } catch {
    /* 轮询失败忽略：一次网络抖动不该弹提示 */
  }
}

function onDocumentClick(event: MouseEvent) {
  const target = event.target as HTMLElement;
  if (!target.closest("#bellPop") && !target.closest("#btnBell")) {
    bellOpen.value = false;
  }
  if (!target.closest(".tb-user") && !target.closest(".user-pop")) {
    userOpen.value = false;
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    bellOpen.value = false;
    userOpen.value = false;
  }
}

onMounted(async () => {
  tick();
  clockTimer = window.setInterval(tick, 1000);
  await auth.loadMe();
  await refreshStatus();
  pollTimer = window.setInterval(refreshStatus, 15000);
  // 控制权抢占随时会发生：10s 一轮够及时，也不会给后端添压力
  await watchControlSource();
  controlTimer = window.setInterval(watchControlSource, 10000);
  document.addEventListener("click", onDocumentClick);
  document.addEventListener("keydown", onKeydown);
  warmTimer = window.setTimeout(warmNavRoutes, 600);
});

onBeforeUnmount(() => {
  window.clearInterval(clockTimer);
  window.clearInterval(pollTimer);
  window.clearInterval(controlTimer);
  window.clearTimeout(warmTimer);
  for (const notice of ctrlNotices.values()) notice.close();
  ctrlNotices.clear();
  document.removeEventListener("click", onDocumentClick);
  document.removeEventListener("keydown", onKeydown);
});

watch(
  () => route.path,
  () => {
    bellOpen.value = false;
    userOpen.value = false;
  }
);
</script>

<template>
  <div class="app">
    <aside class="sidebar" :class="{ collapsed: app.sidebarCollapsed }">
      <div class="brand">
        <div class="brand-logo"><AppIcon name="dog" :size="23" /></div>
        <div class="brand-text">
          <b>机器狗巡检业务平台</b>
          <span>ROBODOG OPS</span>
        </div>
      </div>
      <div class="sb-scroll">
        <template v-for="group in navGroups" :key="group.title">
          <div class="nav-group-title">{{ group.title }}</div>
          <div
            v-for="item in group.items"
            :key="item.path"
            class="nav-item"
            :class="{ active: route.path === item.path }"
            @click="go(item.path)"
            @mouseenter="prefetch(item.path)"
            @focus="prefetch(item.path)"
          >
            <AppIcon :name="item.icon" />
            <span class="hide-c">{{ item.label }}</span>
            <span
              v-if="item.path === '/alerts' && app.pendingAlerts > 0"
              class="nav-badge hide-c"
            >
              {{ app.pendingAlerts }}
            </span>
          </div>
        </template>
      </div>
      <div class="sb-status">
        <div class="st-row"><i class="breath" />系统运行正常</div>
        <p>已接入设备 {{ deviceTotal }} 台 · 在线 {{ onlineDevices }} 台</p>
        <p>数据同步于 <span>{{ syncTime }}</span></p>
      </div>
    </aside>

    <div class="main">
      <header class="topbar">
        <button class="tb-toggle" title="收起/展开侧栏" @click="app.toggleSidebar()">
          <AppIcon name="menu" />
        </button>
        <div class="crumb">
          <span>智慧巡检平台</span>
          <AppIcon name="chevron-right" :size="13" />
          <b>{{ currentTitle }}</b>
        </div>
        <div class="tb-spacer" />
        <div class="tb-search">
          <AppIcon name="search" />
          <input
            v-model="search"
            class="input"
            placeholder="搜索设备编号 / 园区 / 任务…"
            @keydown.enter="globalSearch"
          />
        </div>
        <div class="tb-time">{{ clock }}</div>
        <button id="btnBell" class="tb-icon" title="告警通知" @click.stop="toggleBell">
          <AppIcon name="bell" />
          <span v-if="app.pendingAlerts > 0" class="red-dot" />
        </button>
        <div class="tb-user" @click.stop="userOpen = !userOpen">
          <div class="avatar">{{ initials(displayName) }}</div>
          <div>
            <div class="u-name">{{ displayName }}</div>
            <div class="u-role">{{ roleName }}</div>
          </div>
          <AppIcon name="chevron-down" :size="13" style="color: #94a3b8" />
        </div>

        <div v-if="bellOpen" id="bellPop" class="popover">
          <div class="popover-head">
            <span>告警通知</span>
            <span class="tag orange">{{ bellAlerts.length }} 条未处理</span>
          </div>
          <template v-if="bellAlerts.length">
            <div
              v-for="alert in bellAlerts.slice(0, 4)"
              :key="alert.id"
              class="alert-item"
              style="padding: 11px 15px; border-bottom: 1px solid var(--line-2)"
            >
              <div class="alert-ico danger"><AppIcon name="bell" /></div>
              <div class="alert-txt">
                <b>{{ alert.content }}</b>
                <span>{{ alert.parkName }} · {{ timeOnly(alert.occurredAt) }}</span>
              </div>
            </div>
          </template>
          <div v-else class="empty-box" style="padding: 26px 16px">暂无未处理告警</div>
          <div class="popover-foot">
            <button class="btn sm ghost" @click="go('/alerts')">
              查看全部告警 <AppIcon name="chevron-right" />
            </button>
          </div>
        </div>

        <div v-if="userOpen" class="user-pop">
          <div class="user-pop-head">
            <b>{{ displayName }}</b>
            <span>{{ auth.user?.username }} · {{ roleName }}</span>
          </div>
          <button @click="logout"><AppIcon name="close" />退出登录</button>
        </div>
      </header>

      <div class="content">
        <RouterView v-slot="{ Component, route: currentRoute }">
          <Transition name="page" mode="out-in">
            <component :is="Component" :key="currentRoute.path" />
          </Transition>
        </RouterView>
      </div>
    </div>
  </div>
</template>
