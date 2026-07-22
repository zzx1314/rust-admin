import { defineStore } from "pinia";
import { store } from "@/store";
import { getHarborInfo } from "@/api/harbor";

interface HarborState {
  registryUrl: string;
  loading: boolean;
  loaded: boolean;
}

export const useHarborStore = defineStore("pure-harbor", {
  state: (): HarborState => ({
    registryUrl: "",
    loading: false,
    loaded: false
  }),
  actions: {
    async fetchRegistryUrl() {
      if (this.loaded) return this.registryUrl;
      this.loading = true;
      try {
        const res = await getHarborInfo();
        if (res.code === 10200 && res.data) {
          this.registryUrl = res.data.registry_url || "";
        }
      } catch {
        // Keep empty string as fallback
      } finally {
        this.loading = false;
        this.loaded = true;
      }
      return this.registryUrl;
    }
  }
});

export function useHarborStoreHook() {
  return useHarborStore(store);
}
