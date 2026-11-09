import { defineStore } from "pinia";
import { ref } from "vue";
import { alertApi } from "@/api/ops";

export const useAppStore = defineStore("app", () => {
  const sidebarCollapsed = ref(false);
  const pendingAlerts = ref(0);

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value;
  }

  async function refreshPendingAlerts() {
    try {
      const stats = await alertApi.stats();
      pendingAlerts.value = stats.pending;
    } catch {
      pendingAlerts.value = 0;
    }
  }

  return {
    sidebarCollapsed,
    pendingAlerts,
    toggleSidebar,
    refreshPendingAlerts
  };
});
