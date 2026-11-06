import { createApp, h } from "vue";
import { RouterView } from "vue-router";
import ElementPlus from "element-plus";
import { MotionPlugin } from "@vueuse/motion";
import router from "@/router";
import { getPlatformConfig } from "@/config";
import { setupStore } from "@/store";
import { useI18n } from "@/plugins/i18n";
import { injectResponsiveStorage } from "@/utils/responsive";
import "@/style/reset.scss";
import "@/style/index.scss";
import "@/style/tailwind.css";
import "element-plus/dist/index.css";
const app = createApp({ render: () => h(RouterView) });
setupStore(app);
useI18n(app);
app.use(ElementPlus);
app.use(MotionPlugin);
app.use(router);
async function boot(config: any) {
  injectResponsiveStorage(app, config);
  app.mount("#app");
  await router.isReady();
  if (!location.hash) router.push("/login");
}
getPlatformConfig(app).then(c => boot(c)).catch(() => boot({ Locale: "zh", Layout: "vertical", Theme: "default" }));
