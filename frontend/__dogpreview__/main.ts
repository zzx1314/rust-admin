import { createApp } from "vue";
import ElementPlus from "element-plus";
import Dashboard from "@/views/welcome/index.vue";
import Orig from "@/views/welcome/__orig.vue";
import "@/style/reset.scss";
import "@/style/index.scss";
import "@/style/tailwind.css";
import "element-plus/dist/index.css";
import { useEcharts } from "@/plugins/echarts";

const which = new URLSearchParams(location.search).get("v");
const app = createApp(which === "orig" ? Orig : Dashboard);
useEcharts(app);
app.use(ElementPlus);
app.mount("#app");
if (new URLSearchParams(location.search).get("theme") === "dark") {
  document.documentElement.classList.add("dark");
  document.documentElement.setAttribute("data-theme", "dark");
}
